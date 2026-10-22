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
fn content_baselines_detect_equal_size_edits_and_reject_stale_plans() {
    let w = World::new();
    let m = w.machine("fingerprints");
    let folder = m.home.join("work");
    std::fs::create_dir_all(&folder).unwrap();
    let file = claude_session(
        &m,
        &folder,
        &[("user", "hello"), ("assistant", "first")],
        Some("Hashes"),
    );
    let key = format!("claude:{}", file.file_stem().unwrap().to_string_lossy());
    let req = json!({"action":"add","ids":[key],"general":true});
    let p = api(&m, req.clone());
    assert_eq!(api(&m, confirm(&req, &p))["status"], "ok");
    let original = std::fs::read_to_string(&file).unwrap();
    let edited = original.replace("first", "other");
    assert_eq!(original.len(), edited.len());
    std::fs::write(&file, edited).unwrap();
    let listing = api(&m, json!({"action":"list"}));
    assert_eq!(
        listing["data"]["conversations"][0]["copies"][0]["state"], "push",
        "{listing}"
    );
    let request = json!({"action":"sync","ids":[key]});
    let plan = api(&m, request.clone());
    assert_eq!(plan["status"], "ok", "{plan}");
    std::fs::write(&file, original.replace("first", "third")).unwrap();
    let changed = api(&m, confirmed(&request, &plan));
    assert_eq!(changed["status"], "decision_required", "{changed}");
}

#[test]
fn a_codex_compaction_rebuilds_an_untouched_mirror_and_preserves_its_original() {
    let line = |v: Value| format!("{v}\n");
    let w = World::new();
    let m = w.machine("compact");
    let folder = m.home.join("work");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(m.claude_dir()).unwrap();
    m.actx(&[
        "init",
        m.store.to_str().unwrap(),
        "--harnesses",
        "codex,claude",
        "--retention",
        "never",
    ]);
    let sid = uuid::Uuid::new_v4().to_string();
    let key = format!("codex:{sid}");
    let path = m.codex_sessions().join(format!("2026/01/01/rollout-{sid}.jsonl"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut raw = line(json!({"type":"session_meta","payload":{"id":sid,"cwd":folder,"source":"cli"}}));
    let message = |role: &str, text: &str| json!({"type":"message","role":role,"content":[{"type":if role=="user"{"input_text"}else{"output_text"},"text":text}]});
    for (role, text) in [("user", "hello"), ("assistant", "previous response")] {
        raw += &line(json!({"type":"response_item","timestamp":"2026-01-01T00:00:00Z","payload":message(role,text)}));
    }
    std::fs::write(&path, &raw).unwrap();
    let closed = |p: &std::path::Path| {
        std::fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(86400))
            .unwrap();
    };
    closed(&path);
    let req = json!({"action":"add","ids":[key],"general":true});
    let p = api(&m, req.clone());
    assert_eq!(api(&m, confirm(&req, &p))["status"], "ok");
    let sync = json!({"action":"sync","ids":[key]});
    let p = api(&m, sync.clone());
    let r = api(&m, confirmed(&sync, &p));
    assert_eq!(r["status"], "ok", "{r}");
    let links = m.home.join(".local/state/actx/harness-links.json");
    let before: Value = serde_json::from_slice(&std::fs::read(&links).unwrap()).unwrap();
    let old = before["copies"][format!("{key}/claude")]["target"].as_str().unwrap();
    let old_path = find(&m.claude_dir(), &format!("{}.jsonl", old.split_once(':').unwrap().1)).unwrap();
    closed(&old_path);
    let original = std::fs::read(&old_path).unwrap();
    append(
        &path,
        &line(
            json!({"type":"compacted","timestamp":"2026-01-02T00:00:00Z","payload":{"replacement_history":[message("user","summary"),message("assistant","current context")]}}),
        ),
    );
    closed(&path);
    let p = api(&m, sync.clone());
    assert_eq!(p["status"], "ok", "{p}");
    let r = api(&m, confirmed(&sync, &p));
    assert_eq!(r["status"], "ok", "{r}");
    let after: Value = serde_json::from_slice(&std::fs::read(&links).unwrap()).unwrap();
    assert_ne!(after["copies"][format!("{key}/claude")]["target"], old);
    assert_eq!(std::fs::read(&old_path).unwrap(), original);
    assert!(after["retired"].as_array().unwrap().contains(&json!(old)));
}

#[test]
fn conflicts_are_compared_resolved_and_recovered_with_revision_checks() {
    let w = World::new();
    let a = w.machine("left");
    let b = w.machine("right");
    let folder = a.home.join("work");
    std::fs::create_dir_all(&folder).unwrap();
    let file = claude_session(
        &a,
        &folder,
        &[("user", "base question"), ("assistant", "base answer")],
        Some("Conflict"),
    );
    let sid = file.file_stem().unwrap().to_string_lossy().to_string();
    let key = format!("claude:{sid}");
    let add = json!({"action":"add","ids":[key],"general":true});
    let p = api(&a, add.clone());
    assert_eq!(api(&a, confirm(&add, &p))["status"], "ok");
    let dest = b.home.join("work");
    std::fs::create_dir_all(&dest).unwrap();
    let sync = json!({"action":"sync","ids":[key],"folder":dest});
    let p = api(&b, sync.clone());
    let r = api(&b, confirmed(&sync, &p));
    assert_eq!(r["status"], "ok", "{r}");
    let local = claude_transcript(&b, &sid).0;
    claude_continue(&file, &[("user", "from left"), ("assistant", "left answer")], None);
    let p = api(&a, add.clone());
    assert_eq!(api(&a, confirm(&add, &p))["status"], "ok");
    claude_continue(&local, &[("user", "from right"), ("assistant", "right answer")], None);
    let comparison = api(&b, json!({"action":"compare","ids":[key]}));
    assert_eq!(comparison["status"], "ok", "{comparison}");
    assert!(comparison["data"]["comparison"]["common_messages"].as_u64().unwrap() >= 2);
    let defer = api(
        &b,
        json!({"action":"resolve","ids":[key],"choice":"defer","apply":true}),
    );
    assert_eq!(defer["status"], "cancelled");
    let resolve = json!({"action":"resolve","ids":[key],"choice":"remote"});
    let p = api(&b, resolve.clone());
    let r = api(&b, confirmed(&resolve, &p));
    assert_eq!(r["status"], "ok", "{r}");
    assert!(claude_transcript(&b, &sid).1.contains(&"left answer".into()));
    let backups = api(&b, json!({"action":"recoveries","ids":[key]}));
    let entry = backups["data"]["entries"].as_array().unwrap().first().unwrap();
    let restore = json!({"action":"restore","backup":entry["id"]});
    let p = api(&b, restore.clone());
    claude_continue(&local, &[("user", "late edit"), ("assistant", "late reply")], None);
    assert_eq!(api(&b, confirmed(&restore, &p))["status"], "decision_required");
    let p = api(&b, restore.clone());
    let r = api(&b, confirmed(&restore, &p));
    assert_eq!(r["status"], "ok", "{r}");
    assert!(claude_transcript(&b, &sid).1.contains(&"right answer".into()));
}

#[test]
fn desktop_project_deletion_and_automatic_previews_are_reviewed() {
    let w = World::new();
    let m = w.machine("desktop");
    let folder = m.home.join("project");
    std::fs::create_dir_all(&folder).unwrap();
    let create = json!({"action":"create_project","project":"gui","folder":folder});
    let p = api(&m, create.clone());
    assert_eq!(api(&m, confirmed(&create, &p))["status"], "ok");
    let first = claude_session(
        &m,
        &folder,
        &[("user", "first"), ("assistant", "first reply")],
        Some("First"),
    );
    let add = |file: &std::path::Path| json!({"action":"add","project":"gui","ids":[format!("claude:{}",file.file_stem().unwrap().to_string_lossy())]});
    let req = add(&first);
    let p = api(&m, req.clone());
    assert_eq!(api(&m, confirm(&req, &p))["status"], "ok");
    let delete = json!({"action":"delete_project","project":"gui"});
    let stale = api(&m, delete.clone());
    assert_eq!(stale["status"], "ok");
    let second = claude_session(
        &m,
        &folder,
        &[("user", "second"), ("assistant", "second reply")],
        Some("Second"),
    );
    let req = add(&second);
    let p = api(&m, req.clone());
    assert_eq!(api(&m, confirm(&req, &p))["status"], "ok");
    assert_eq!(api(&m, confirmed(&delete, &stale))["status"], "decision_required");
    let p = api(&m, delete.clone());
    let r = api(&m, confirmed(&delete, &p));
    assert_eq!(r["status"], "ok", "{r}");
    assert!(!m.store.join("gui").exists());
    assert!(first.exists() && second.exists() && folder.exists());
    let auto = json!({"action":"automatic","choice":"on"});
    let p = api(&m, auto.clone());
    assert_eq!(p["status"], "ok", "{p}");
    assert!(!m.home.join(".config/systemd/user/actx-sync.service").exists());
    let mut cancelled = confirmed(&auto, &p);
    cancelled["cancel"] = json!(true);
    assert_eq!(api(&m, cancelled)["status"], "cancelled");
    assert!(!m.home.join(".config/systemd/user/actx-sync.service").exists());
}

#[test]
fn services_sync_two_machines_and_convert_without_duplicates() {
    let w = World::new();
    let a = w.machine("desk");
    let mut b = w.machine("laptop");
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
    let agy = b.home.join("bin").join(if cfg!(windows) { "agy.cmd" } else { "agy" });
    std::fs::write(&agy, "not executed").unwrap();
    let unmaintained = api(&b, json!({"action":"resume","ids":[id],"target":"antigravity"}));
    assert_eq!(unmaintained["status"], "decision_required");
    assert!(
        unmaintained["data"]["operations"][0]["requirements"]
            .as_array()
            .unwrap()
            .contains(&json!("enable_target_harness"))
    );
    let online = b.store.clone();
    b.store = w.dir.path().join("offline");
    assert_eq!(
        api(&b, resume.clone())["diagnostics"][0]["code"],
        "shared_folder_unavailable"
    );
    b.store = online;
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

#[test]
fn desktop_setup_discovers_project_conversations_without_adding_them() {
    let w = World::new();
    let m = w.machine("pc");
    let repo = git_repo(&m.home.join("project"));
    let prompt = format!("Work in {}", repo.display());
    let source = claude_session(
        &m,
        &m.home,
        &[("user", &prompt), ("assistant", "yes")],
        Some("Project from home"),
    );
    claude_session(
        &m,
        &m.home,
        &[("user", "unrelated"), ("assistant", "no")],
        Some("Unrelated"),
    );
    let config = m.home.join(".config/actx/config.json");
    let req = json!({"action":"configure","root":m.store,"lang":"zh-CN","harnesses":["claude"]});
    let plan = api(&m, req.clone());
    assert!(!config.exists());
    assert!(plan["data"]["expected"]["configure"].is_string());
    assert_eq!(api(&m, confirmed(&req, &plan))["status"], "ok");
    assert!(config.exists());
    let req = json!({"action":"create_project","project":"Work","folder":repo});
    let plan = api(&m, req.clone());
    assert!(!w.store().join("Work").exists());
    assert_eq!(api(&m, confirmed(&req, &plan))["status"], "ok");
    let candidates = api(&m, json!({"action":"candidates","project":"Work"}));
    assert_eq!(candidates["status"], "ok", "{candidates}");
    assert_eq!(candidates["data"]["conversations"].as_array().unwrap().len(), 1);
    assert_eq!(candidates["data"]["conversations"][0]["title"], "Project from home");
    assert!(
        api(&m, json!({"action":"list"}))["data"]["conversations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let id = format!("claude:{}", source.file_stem().unwrap().to_string_lossy());
    let before = std::fs::read(&source).unwrap();
    let messages = api(&m, json!({"action":"messages","ids":[id]}));
    assert_eq!(messages["data"]["messages"][0]["text"], prompt);
    assert_eq!(std::fs::read(&source).unwrap(), before);
    let add = json!({"action":"add","project":"Work","ids":[id]});
    let plan = api(&m, add);
    assert_eq!(
        plan["data"]["operations"][0]["destination"],
        repo.to_string_lossy().as_ref()
    );
    let wrong = git_repo(&m.home.join("wrong"));
    assert_eq!(
        api(&m, json!({"action":"add","project":"Work","ids":[id],"folder":wrong}))["diagnostics"][0]["code"],
        "project_binding_failed"
    );
}

#[test]
fn automatic_additions_are_opt_in_reviewed_scoped_and_respect_explicit_deletion() {
    let w = World::new();
    let m = w.machine("adoption");
    let folder = m.home.join("work");
    let scratch = folder.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let create = json!({"action":"create_project","project":"work","folder":folder});
    let p = api(&m, create.clone());
    assert_eq!(api(&m, confirmed(&create, &p))["status"], "ok");
    let source = claude_session(&m, &folder, &[("user", "hello"), ("assistant", "yes")], Some("Main"));
    let key = format!("claude:{}", source.file_stem().unwrap().to_string_lossy());
    let original = std::fs::read(&source).unwrap();
    claude_session(
        &m,
        &scratch,
        &[("user", "scratch"), ("assistant", "yes")],
        Some("Scratch"),
    );
    claude_session(
        &m,
        &folder,
        &[("user", "private"), ("assistant", "yes")],
        Some("Private conversation"),
    );
    claude_session(
        &m,
        &m.home,
        &[("user", "unrelated"), ("assistant", "yes")],
        Some("Unrelated"),
    );
    let sync = json!({"action":"sync","project":"work"});
    assert!(
        api(&m, sync.clone())["data"]["operations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let request = json!({"action":"adoption","project":"work","adoption":{"enabled":true,"harnesses":["claude"],"exclude":["scratch/**"],"exclude_titles":["*private*"]}});
    let p = api(&m, request.clone());
    assert_eq!(p["status"], "ok", "{p}");
    assert_eq!(p["data"]["candidate_operations"].as_array().unwrap().len(), 1, "{p}");
    assert!(!serde_json::from_slice::<Value>(&std::fs::read(m.store.join("work/.project.json")).unwrap()).unwrap()["auto_add"].is_object());
    assert_eq!(api(&m, confirmed(&request, &p))["status"], "ok");
    let p = api(&m, sync.clone());
    assert_eq!(p["data"]["operations"][0]["kind"], "adopt", "{p}");
    let dry = api(&m, json!({"action":"preview","project":"work"}));
    assert_eq!(dry["data"]["operations"][0]["kind"], "adopt", "{dry}");
    assert!(!m.store.join("work/claude").exists());
    let r = api(&m, confirmed(&sync, &p));
    assert_eq!(r["status"], "ok", "{r}");
    assert_eq!(
        api(&m, json!({"action":"list"}))["data"]["conversations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let delete = json!({"action":"delete","ids":[key]});
    let p = api(&m, delete.clone());
    assert_eq!(api(&m, confirm(&delete, &p))["status"], "ok");
    assert!(
        api(&m, sync.clone())["data"]["operations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let manual = json!({"action":"add","project":"work","ids":[key]});
    let p = api(&m, manual.clone());
    assert_eq!(api(&m, confirm(&manual, &p))["status"], "ok");
    assert_eq!(std::fs::read(&source).unwrap(), original);
    let off = json!({"action":"adoption","project":"work","adoption":{"enabled":false}});
    let p = api(&m, off.clone());
    assert_eq!(api(&m, confirmed(&off, &p))["status"], "ok");
    claude_session(&m, &folder, &[("user", "new"), ("assistant", "yes")], Some("Later"));
    assert!(
        !api(&m, sync)["data"]["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["kind"] == "adopt")
    );
}

#[test]
fn automatic_additions_defer_ambiguous_project_membership() {
    let w = World::new();
    let m = w.machine("ambiguous");
    let parent = m.home.join("parent");
    let child = parent.join("child");
    std::fs::create_dir_all(&child).unwrap();
    for (project, folder) in [("Parent", &parent), ("Child", &child)] {
        let create = json!({"action":"create_project","project":project,"folder":folder});
        let p = api(&m, create.clone());
        assert_eq!(api(&m, confirmed(&create, &p))["status"], "ok");
        let enable = json!({"action":"adoption","project":project,"adoption":{"enabled":true,"harnesses":["claude"]}});
        let p = api(&m, enable.clone());
        assert_eq!(api(&m, confirmed(&enable, &p))["status"], "ok");
    }
    claude_session(
        &m,
        &child,
        &[("user", "hello"), ("assistant", "yes")],
        Some("Ambiguous"),
    );
    let p = api(&m, json!({"action":"sync","project":"Child"}));
    assert_eq!(p["data"]["operations"][0]["kind"], "deferred", "{p}");
    assert_eq!(p["data"]["operations"][0]["reason"], "ambiguous_project");
    assert!(
        api(&m, json!({"action":"list"}))["data"]["conversations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn shared_adoption_rules_follow_the_other_computers_bound_project_folder() {
    let w = World::new();
    let a = w.machine("rule-desktop");
    let b = w.machine("rule-laptop");
    let repo = git_repo(&a.home.join("source"));
    let clone = git_clone(&repo, &b.home.join("workspace/different-name"));
    let clone2 = git_clone(&repo, &b.home.join("workspace/another-clone"));
    let create = json!({"action":"create_project","project":"Portable","folder":repo});
    let p = api(&a, create.clone());
    assert_eq!(api(&a, confirmed(&create, &p))["status"], "ok");
    // Bind explicitly on laptop; metadata/rules remain shared.
    b.actx_with(
        &["mk", "Portable"],
        Opts {
            cwd: Some(&clone),
            ..Default::default()
        },
    );
    let enable = json!({"action":"adoption","project":"Portable","adoption":{"enabled":true,"subfolders":false,"harnesses":["claude"]}});
    let p = api(&a, enable.clone());
    assert_eq!(api(&a, confirmed(&enable, &p))["status"], "ok");
    claude_session(&b, &clone, &[("user", "local"), ("assistant", "yes")], Some("Laptop"));
    claude_session(
        &b,
        &clone2,
        &[("user", "clone"), ("assistant", "yes")],
        Some("Another clone"),
    );
    let child = clone2.join("child");
    std::fs::create_dir_all(&child).unwrap();
    claude_session(
        &b,
        &child,
        &[("user", "child"), ("assistant", "yes")],
        Some("Clone child"),
    );
    let plan = api(&b, json!({"action":"adoption","project":"Portable"}));
    assert_eq!(
        plan["data"]["candidate_operations"].as_array().unwrap().len(),
        1,
        "{plan}"
    );
    assert_eq!(plan["data"]["candidate_operations"][0]["title"], "Laptop");
    let clones = json!({"action":"adoption","project":"Portable","adoption":{"enabled":true,"subfolders":false,"include_clones":true,"harnesses":["claude"]}});
    let p = api(&b, clones);
    assert_eq!(p["data"]["candidate_operations"].as_array().unwrap().len(), 2, "{p}");
    assert!(
        !p["data"]["candidate_operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["title"] == "Clone child")
    );
}
