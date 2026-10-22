mod common;
use common::*;
use serde_json::{Value, json};
use std::io::Write;
use std::process::Stdio;

fn api(m: &Machine, req: Value) -> Value {
    let mut c = m
        .command(ACTX)
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
