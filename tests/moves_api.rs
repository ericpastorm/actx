//! The desktop's organization API reviews current snapshots before applying changes.
mod common;
use common::*;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    time::SystemTime,
};

fn api(m: &Machine, req: &Value) -> Value {
    api_with_cancel(m, req, None)
}
fn api_with_cancel(m: &Machine, req: &Value, cancel: Option<&Path>) -> Value {
    let mut child = m
        .command(ACTX)
        .arg("api")
        .envs(cancel.map(|p| ("ACTX_CANCEL_FILE", p)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(req.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stderr)))
}
fn confirmed(req: &Value, plan: &Value) -> Value {
    assert_eq!(plan["status"], "ok", "{plan}");
    let mut req = req.clone();
    req["apply"] = json!(true);
    req["expected"] = plan["data"]["expected"].clone();
    req
}
fn source(m: &Machine, title: &str) -> (PathBuf, String) {
    let work = m.home.join("notes");
    std::fs::create_dir_all(&work).unwrap();
    let file = claude_session(m, &work, &[("user", title), ("assistant", "Answer")], Some(title));
    let id = file.file_stem().unwrap().to_string_lossy().into_owned();
    m.pick(&["add", "--general"], &[&id]);
    (file, id)
}
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    files_under(dir)
        .into_iter()
        .map(|p| {
            let entry = (std::fs::read(&p).unwrap(), p.metadata().unwrap().modified().unwrap());
            (p, entry)
        })
        .collect()
}

#[test]
fn move_api_is_read_only_until_confirmed_and_moves_every_linked_copy() {
    let w = World::new();
    let m = w.machine("desktop");
    let (file, id) = source(&m, "Shared one");
    let (_, second) = source(&m, "Shared two");
    m.actx(&["mk", "Target"]);
    let key = format!("claude:{id}");
    let linked = w.store().join("_general/opencode/ses_linked");
    std::fs::create_dir_all(&linked).unwrap();
    std::fs::write(linked.join("meta.json"), json!({"harness":"opencode","id":"ses_linked","title":"Linked","path":"","weight":2,"files":{},"conversation_id":key}).to_string()).unwrap();
    claude_continue(
        &file,
        &[("user", "Not synchronized yet"), ("assistant", "Local edit")],
        None,
    );
    let native = std::fs::read(&file).unwrap();
    let saved_meta = std::fs::read(w.store().join("_general/claude").join(&id).join("meta.json")).unwrap();
    let before = snapshot(w.dir.path());
    let req = json!({"action":"move","ids":[key,format!("claude:{second}"),"opencode:ses_linked"],"project":"Target"});
    let plan = api(&m, &req);
    assert_eq!(plan["status"], "ok", "{plan}");
    assert_eq!(plan["data"]["conversation_count"], 2);
    assert_eq!(plan["data"]["operations"].as_array().unwrap().len(), 3);
    assert_eq!(
        api(&m, &req)["data"]["expected"],
        plan["data"]["expected"],
        "tokens must survive separate API processes"
    );
    let mut unreviewed = req.clone();
    unreviewed["apply"] = json!(true);
    assert_eq!(api(&m, &unreviewed)["status"], "decision_required");
    let mut cancelled = confirmed(&req, &plan);
    cancelled["cancel"] = json!(true);
    assert_eq!(api(&m, &cancelled)["status"], "cancelled");
    assert_eq!(
        snapshot(w.dir.path()),
        before,
        "previews and cancellation must not write"
    );
    let done = api(&m, &confirmed(&req, &plan));
    assert_eq!(done["status"], "ok", "{done}");
    assert_eq!(done["data"]["applied"], true);
    assert!(w.store().join("Target/opencode/ses_linked/meta.json").exists());
    assert!(w.store().join("Target/claude").join(&second).exists());
    assert!(!w.store().join("_general/claude").join(&id).exists());
    assert_eq!(
        std::fs::read(w.store().join("Target/claude").join(&id).join("meta.json")).unwrap(),
        saved_meta
    );
    assert_eq!(std::fs::read(file).unwrap(), native);
    assert!(api(&m, &req)["data"]["operations"].as_array().unwrap().is_empty());
}

#[test]
fn move_api_rejects_stale_sources_changed_destinations_and_unconfirmed_creation() {
    let w = World::new();
    let m = w.machine("desktop");
    let (_, id) = source(&m, "Review me");
    m.actx(&["mk", "First"]);
    m.actx(&["mk", "Second"]);
    let req = json!({"action":"move","ids":[format!("claude:{id}")],"project":"First"});
    let plan = api(&m, &req);
    let cancel = m.home.join("cancel-request");
    std::fs::write(&cancel, "cancel").unwrap();
    let before = snapshot(w.dir.path());
    assert_eq!(
        api_with_cancel(&m, &confirmed(&req, &plan), Some(&cancel))["status"],
        "cancelled"
    );
    assert_eq!(snapshot(w.dir.path()), before);
    std::fs::remove_file(cancel).unwrap();
    let mut changed = confirmed(&req, &plan);
    changed["project"] = json!("Second");
    assert_eq!(api(&m, &changed)["status"], "decision_required");
    let meta = w.store().join("_general/claude").join(&id).join("meta.json");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&meta).unwrap()).unwrap();
    value["title"] = json!("Changed elsewhere");
    std::fs::write(&meta, value.to_string()).unwrap();
    assert_eq!(api(&m, &confirmed(&req, &plan))["status"], "decision_required");
    assert!(meta.exists());
    let new = json!({"action":"move","ids":[format!("claude:{id}")],"project":"Created","choice":"create"});
    let plan = api(&m, &new);
    assert!(!w.store().join("Created").exists());
    assert_eq!(api(&m, &confirmed(&new, &plan))["status"], "ok");
    let general = json!({"action":"move","ids":[format!("claude:{id}")],"general":true});
    let plan = api(&m, &general);
    assert_eq!(api(&m, &confirmed(&general, &plan))["status"], "ok");
    assert!(meta.exists());
    let missing = json!({"action":"move","ids":["claude:unknown"],"general":true});
    assert_eq!(api(&m, &missing)["status"], "error");
}

#[test]
fn rename_api_requires_a_fresh_project_review_and_keeps_histories() {
    let w = World::new();
    let m = w.machine("desktop");
    let (file, id) = source(&m, "Rename me");
    m.actx(&["mk", "Old"]);
    let mv = json!({"action":"move","ids":[format!("claude:{id}")],"project":"Old"});
    let plan = api(&m, &mv);
    assert_eq!(api(&m, &confirmed(&mv, &plan))["status"], "ok");
    let original = std::fs::read(&file).unwrap();
    let req = json!({"action":"rename_project","project":"Old","target":"Renamed"});
    let before = snapshot(w.dir.path());
    let plan = api(&m, &req);
    assert_eq!(api(&m, &req)["data"]["expected"], plan["data"]["expected"]);
    assert_eq!(snapshot(w.dir.path()), before);
    std::fs::write(w.store().join("Old/settings.json"), "{}").unwrap();
    assert_eq!(api(&m, &confirmed(&req, &plan))["status"], "decision_required");
    let plan = api(&m, &req);
    let done = api(&m, &confirmed(&req, &plan));
    assert_eq!(done["status"], "ok", "{done}");
    assert_eq!(done["data"]["applied"], true);
    assert!(!w.store().join("Old").exists());
    assert!(w.store().join("Renamed/claude").join(&id).exists());
    assert_eq!(
        std::fs::read_to_string(w.store().join("Renamed/settings.json")).unwrap(),
        "{}"
    );
    assert_eq!(std::fs::read(file).unwrap(), original);
}

#[test]
fn completed_moves_are_reported_when_local_state_cannot_be_saved() {
    let w = World::new();
    let m = w.machine("desktop");
    let (file, id) = source(&m, "Keep me");
    let original = std::fs::read(&file).unwrap();
    m.actx(&["mk", "Target"]);
    let req = json!({"action":"move","ids":[format!("claude:{id}")],"project":"Target"});
    let plan = api(&m, &req);
    let state_lock = m.home.join(".local/state/actx/state.lock");
    std::fs::remove_file(&state_lock).unwrap();
    std::fs::create_dir(&state_lock).unwrap();
    let done = api(&m, &confirmed(&req, &plan));
    assert_eq!(done["status"], "incomplete", "{done}");
    assert_eq!(done["data"]["applied"], true);
    assert_eq!(done["diagnostics"][0]["code"], "state_save_failed");
    assert!(w.store().join("Target/claude").join(&id).exists());
    assert_eq!(std::fs::read(file).unwrap(), original);
}
