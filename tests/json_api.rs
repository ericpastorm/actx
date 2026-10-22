//! The machine interface must be parseable, scoped, diagnostic and read-only.
mod common;
use common::*;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

fn response(m: &Machine, args: &[&str], code: i32) -> Value {
    let out = m.command(ACTX).args(args).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: Value =
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)));
    assert_eq!(value["schema_version"], 1);
    assert!(!out.stdout.contains(&0x1b), "JSON contains terminal escapes");
    value
}
fn source(m: &Machine, title: &str) -> (PathBuf, String) {
    let folder = m.home.join("notes");
    std::fs::create_dir_all(&folder).unwrap();
    let path = claude_session(m, &folder, &[("user", title), ("assistant", "answer")], Some(title));
    closed(&path);
    let id = path.file_stem().unwrap().to_string_lossy().into_owned();
    (path, id)
}
fn closed(path: &Path) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(86400))
        .unwrap();
}
type Snapshot = BTreeMap<PathBuf, (Vec<u8>, SystemTime)>;
fn snapshot(root: &Path) -> Snapshot {
    fn visit(path: &Path, out: &mut Snapshot) {
        let meta = std::fs::symlink_metadata(path).unwrap();
        out.insert(
            path.to_path_buf(),
            (
                if meta.is_file() {
                    std::fs::read(path).unwrap()
                } else {
                    Vec::new()
                },
                meta.modified().unwrap(),
            ),
        );
        if meta.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                visit(&entry.unwrap().path(), out);
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, &mut out);
    out
}
fn unchanged(root: &Path, before: &Snapshot) {
    let after = snapshot(root);
    assert_eq!(before.keys().collect::<Vec<_>>(), after.keys().collect::<Vec<_>>());
    for (path, old) in before {
        assert!(after[path] == *old, "Changed {}", path.display());
    }
}

#[test]
fn list_has_stable_identities_collections_and_locale_without_writes() {
    let w = World::new();
    let mut m = w.machine("desk");
    let (_, id) = source(&m, "Shared notes");
    m.pick(&["add", "--general"], &[&id]);
    source(&m, "Only local");
    m.actx(&["mk", "Empty"]);
    // Inspection must read legacy state without migrating it or populating caches.
    let state = m.home.join(".local/state/actx/state.json");
    let legacy = m.home.join(".local/state/ctx/state.json");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::rename(state, legacy).unwrap();
    let before = snapshot(w.dir.path());
    let en = response(&m, &["ls", "--json"], 0);
    assert_eq!(en["status"], "ok");
    let data = &en["data"];
    assert_eq!(data["complete"], true);
    assert_eq!(data["conversations"].as_array().unwrap().len(), 1);
    let c = &data["conversations"][0];
    assert_eq!(c["id"], format!("claude:{id}"));
    assert_eq!(c["copies"][0]["id"], c["id"]);
    assert_eq!(c["copies"][0]["local"]["weight_unit"], "bytes");
    assert_eq!(c["collections"], json!([{"kind":"general", "id":null}]));
    assert!(
        data["projects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["collection"]["id"] == "Empty")
    );
    m.lang = "es".into();
    assert_eq!(response(&m, &["ls", "--json"], 0), en);
    let all = response(&m, &["ls", "--json", "--all"], 0);
    assert_eq!(all["data"]["conversations"].as_array().unwrap().len(), 2);
    let empty = response(&m, &["ls", "--json", "Empty"], 0);
    assert_eq!(empty["data"]["conversations"], json!([]));
    unchanged(w.dir.path(), &before);
}

#[test]
fn preview_is_structured_read_only_and_reports_destinations_and_conflicts() {
    let w = World::new();
    let mut a = w.machine("desk");
    let b = w.machine("laptop");
    let (first, id) = source(&a, "Project notes");
    a.actx(&["mk", "Work"]);
    a.pick(&["add", "Work"], &[&id]);
    let (_, general) = source(&a, "General notes");
    a.pick(&["add", "--general"], &[&general]);
    let before = snapshot(w.dir.path());
    let new = response(&b, &["sync", "--dry-run", "--json", "--general", "--auto"], 0);
    assert_eq!(new["data"]["mode"], "auto");
    assert_eq!(new["data"]["read_only"], true);
    assert_eq!(new["data"]["valid"], true);
    let op = &new["data"]["operations"][0];
    assert_eq!(op["session_id"], format!("claude:{general}"));
    assert_eq!(op["reason"], "local_copy_missing");
    assert_eq!(op["requirements"], json!(["destination_required"]));
    assert_eq!(op["destination"], Value::Null);
    let en = response(&a, &["sync", "--dry-run", "--json", "Work"], 0);
    a.lang = "es".into();
    assert_eq!(
        response(&a, &["sync", "--dry-run", "--json", "work"], 0)["data"]["operations"],
        en["data"]["operations"]
    );
    unchanged(w.dir.path(), &before);
    // Independently advance the same conversation from two machines.
    std::fs::create_dir_all(b.home.join("notes")).unwrap();
    b.actx(&["sync", "Work"]);
    let (other, _, _) = claude_transcript(&b, &id);
    claude_continue(&other, &[("user", "laptop"), ("assistant", "laptop reply")], None);
    closed(&other);
    b.actx(&["sync", "Work"]);
    claude_continue(&first, &[("user", "desk"), ("assistant", "desk reply")], None);
    closed(&first);
    let before = snapshot(w.dir.path());
    let conflict = response(&a, &["sync", "--dry-run", "--json", "Work"], 3);
    assert_eq!(conflict["status"], "conflict");
    assert_eq!(conflict["data"]["counts"]["conflict"], 1);
    let op = &conflict["data"]["operations"][0];
    assert_eq!(op["reason"], "both_changed");
    assert_eq!(op["requirements"], json!(["resolve_conflict"]));
    assert_eq!(response(&a, &["ls", "--json", "Work"], 3)["status"], "conflict");
    unchanged(w.dir.path(), &before);
}

#[test]
fn failures_are_json_and_incomplete_reads_never_look_successful() {
    let w = World::new();
    let m = w.machine("desk");
    let missing = response(&m, &["ls", "--json", "Missing"], 1);
    assert_eq!(missing["diagnostics"][0]["code"], "project_not_found");
    assert_eq!(missing["data"], Value::Null);
    let absent = w.dir.path().join("unmounted");
    let out = m
        .command(ACTX)
        .args(["sync", "--dry-run", "--json"])
        .env("ACTX_ROOT", &absent)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(error["diagnostics"][0]["code"], "shared_folder_unavailable");
    assert!(!absent.exists());
    let (_, id) = source(&m, "Known notes");
    m.pick(&["add"], &[&id]);
    let out = m
        .command(ACTX)
        .args(["ls", "--json"])
        .env("ACTX_ROOT", &absent)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    let offline: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(offline["data"]["conversations"][0]["state"], "unknown");
    // A corrupt existing SQLite database is an incomplete inspection, not an empty success.
    let db = m.opencode_db();
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    std::fs::write(&db, "broken SQLite").unwrap();
    let before = snapshot(w.dir.path());
    let broken = response(&m, &["ls", "--json", "--all"], 4);
    assert_eq!(broken["status"], "incomplete");
    assert_eq!(broken["data"]["complete"], false);
    assert_eq!(broken["diagnostics"][0]["code"], "inspection_failed");
    unchanged(w.dir.path(), &before);
    std::fs::remove_file(db).unwrap();
    let meta = m.store.join("_general/claude").join(id).join("meta.json");
    std::fs::write(meta, "invalid JSON").unwrap();
    assert_eq!(
        response(&m, &["sync", "--dry-run", "--json", "--all"], 4)["data"]["valid"],
        false
    );
    let config = m.home.join(".config/actx/config.json");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(config, "invalid JSON").unwrap();
    assert_eq!(
        response(&m, &["ls", "--json"], 1)["diagnostics"][0]["code"],
        "invalid_config"
    );
}

#[test]
fn sync_json_requires_explicit_dry_run() {
    let w = World::new();
    let m = w.machine("desk");
    let before = snapshot(w.dir.path());
    let out = m.command(ACTX).args(["sync", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    unchanged(w.dir.path(), &before);
}
