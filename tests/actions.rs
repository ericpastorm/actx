mod common;
use common::*;
use serde_json::{Value, json};
use std::io::Write;
use std::process::Stdio;

fn api(m: &Machine, req: Value) -> Value {
    let path = std::env::join_paths(
        std::iter::once(m.home.join("bin")).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())),
    )
    .unwrap();
    let mut c = m
        .command(ACTX)
        .env("PATH", path)
        .arg("api")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.take().unwrap().write_all(req.to_string().as_bytes()).unwrap();
    let out = c.wait_with_output().unwrap();
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))
}
fn confirm(req: &Value, plan: &Value) -> Value {
    let mut req = req.clone();
    req["apply"] = json!(true);
    req["expected"] = plan["data"]["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| (o["session_id"].as_str().unwrap().to_string(), o["revision"].clone()))
        .collect::<serde_json::Map<_, _>>()
        .into();
    req
}

fn confirmed(req: &Value, plan: &Value) -> Value {
    let mut r = req.clone();
    r["apply"] = json!(true);
    r["expected"] = plan["data"]["expected"].clone();
    r
}

#[test]
fn services_sync_two_machines_and_convert_without_duplicates() {
    let w = World::new();
    let a = w.machine("desk");
    let b = w.machine("laptop");
    let folder = a.home.join("notes");
    std::fs::create_dir_all(&folder).unwrap();
    let file = claude_session(
        &a,
        &folder,
        &[("user", "hello"), ("assistant", "world")],
        Some("Service notes"),
    );
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(86400))
        .unwrap();
    let id = format!("claude:{}", file.file_stem().unwrap().to_string_lossy());
    let add = json!({"action":"add","ids":[id],"general":true});
    let plan = api(&a, add.clone());
    assert_eq!(api(&a, confirm(&add, &plan))["status"], "ok");
    let dest = b.home.join("new folder");
    std::fs::create_dir_all(&dest).unwrap();
    let req = json!({"action":"sync","ids":[id]});
    assert_eq!(api(&b, req.clone())["status"], "decision_required");
    let req = json!({"action":"sync","ids":[id],"folder":dest});
    let plan = api(&b, req.clone());
    assert_eq!(plan["status"], "ok", "{plan}");
    let synced = api(&b, confirmed(&req, &plan));
    assert_eq!(synced["status"], "ok", "{synced}");
    let (_, text, cwds) = claude_transcript(&b, file.file_stem().unwrap().to_str().unwrap());
    assert_eq!(text, vec!["hello", "world"]);
    assert_eq!(cwds, vec![dest.to_string_lossy().into_owned()]);
    std::fs::create_dir_all(b.home.join(".gemini/antigravity-cli")).unwrap();
    let req = json!({"action":"convert","ids":[id],"target":"antigravity"});
    let plan = api(&b, req.clone());
    assert_eq!(plan["status"], "ok", "{plan}");
    let converted = api(&b, confirmed(&req, &plan));
    assert_eq!(converted["status"], "ok", "{converted}");
    let target = converted["data"]["target_id"].clone();
    std::fs::create_dir_all(b.home.join("bin")).unwrap();
    let shim = b
        .home
        .join("bin")
        .join(if cfg!(windows) { "claude.cmd" } else { "claude" });
    std::fs::write(&shim, "should never be executed").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let resume = json!({"action":"resume","ids":[id],"target":"claude","launch":false});
    let p = api(&b, resume.clone());
    assert_eq!(p["status"], "ok", "{p}");
    let r = api(&b, confirmed(&resume, &p));
    assert_eq!(r["status"], "ok", "{r}");
    assert_eq!(r["data"]["launch"]["args"][0], "--resume");
    assert!(r["data"]["terminal_pid"].is_null());
    let plan = api(&b, req.clone());
    assert_eq!(plan["data"]["operations"][0]["kind"], "reuse");
    assert_eq!(api(&b, confirmed(&req, &plan))["data"]["target_id"], target);
    let req = json!({"action":"sync","ids":[id]});
    let stale = api(&b, req.clone());
    let (local, _, _) = claude_transcript(&b, file.file_stem().unwrap().to_str().unwrap());
    claude_continue(&local, &[("user", "changed"), ("assistant", "again")], None);
    assert_eq!(api(&b, confirmed(&req, &stale))["status"], "decision_required");
}
#[test]
fn explicit_add_delete_recheck_revisions_and_preserve_originals() {
    let w = World::new();
    let m = w.machine("pc");
    let folder = m.home.join("notes");
    std::fs::create_dir_all(&folder).unwrap();
    let file = claude_session(&m, &folder, &[("user", "hello"), ("assistant", "world")], Some("Notes"));
    let id = format!("claude:{}", file.file_stem().unwrap().to_string_lossy());
    let req = json!({"action":"add","ids":[id],"general":true});
    let before = std::fs::read(&file).unwrap();
    let plan = api(&m, req.clone());
    assert_eq!(plan["status"], "ok");
    assert_eq!(plan["data"]["applied"], false);
    assert!(!w.store().join("_general").exists());
    let mut no_preview = req.clone();
    no_preview["apply"] = json!(true);
    assert_eq!(api(&m, no_preview)["status"], "decision_required");
    let done = api(&m, confirm(&req, &plan));
    assert_eq!(done["status"], "ok");
    assert_eq!(done["data"]["results"][0]["success"], true);
    let delete = json!({"action":"delete","ids":[id]});
    let plan = api(&m, delete.clone());
    let stored = w
        .store()
        .join("_general/claude")
        .join(file.file_stem().unwrap())
        .join("meta.json");
    let mut meta: Value = serde_json::from_slice(&std::fs::read(&stored).unwrap()).unwrap();
    meta["pushed_at"] = json!(99);
    std::fs::write(&stored, meta.to_string()).unwrap();
    assert_eq!(api(&m, confirm(&delete, &plan))["status"], "decision_required");
    assert!(stored.exists());
    let fresh = api(&m, delete.clone());
    let mut cancelled = confirm(&delete, &fresh);
    cancelled["cancel"] = json!(true);
    assert_eq!(api(&m, cancelled)["status"], "cancelled");
    assert!(stored.exists());
    assert_eq!(api(&m, confirm(&delete, &fresh))["status"], "ok");
    assert!(!stored.exists());
    assert_eq!(std::fs::read(file).unwrap(), before);
}
#[test]
fn rejects_unknown_fields_and_scope_mismatch_without_changes() {
    let w = World::new();
    let m = w.machine("pc");
    assert_eq!(
        api(&m, json!({"action":"delete","ids":[],"apply":true}))["diagnostics"][0]["code"],
        "selection_required"
    );
    assert_eq!(
        api(&m, json!({"action":"delete","force":true}))["diagnostics"][0]["code"],
        "invalid_request"
    );
    assert!(!m.home.join(".local/state/actx").exists());
}

#[test]
fn chinese_locale_help_and_json_work_without_changing_machine_codes() {
    let w = World::new();
    let mut m = w.machine("中文设备");
    m.lang = "zh_CN.UTF-8".into();
    let help = m.actx(&["--help"]);
    assert!(help.contains("简体中文"), "{help}");
    assert!(help.contains("actx sync"));
    let init = m.actx(&["init", m.store.to_str().unwrap(), "--lang", "zh-CN"]);
    assert!(init.contains("共享文件夹"), "{init}");
    let config: Value =
        serde_json::from_slice(&std::fs::read(m.home.join(".config/actx/config.json")).unwrap()).unwrap();
    assert_eq!(config["lang"], "zh-CN");
    let req = json!({"action":"list"});
    let zh = api(&m, req.clone());
    m.lang = "en".into();
    assert_eq!(api(&m, req), zh);
}
