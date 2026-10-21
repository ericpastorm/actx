//! Focused dry-run regressions: no writes, native scope/destinations, linked copies and retention.
mod common;
use common::*;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

fn snapshot(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    fn visit(path: &Path, out: &mut BTreeMap<PathBuf, (Vec<u8>, SystemTime)>) {
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
fn assert_unchanged(root: &Path, before: &BTreeMap<PathBuf, (Vec<u8>, SystemTime)>) {
    let after = snapshot(root);
    assert_eq!(after.keys().collect::<Vec<_>>(), before.keys().collect::<Vec<_>>());
    for (path, (bytes, modified)) in &after {
        let (old_bytes, old_modified) = &before[path];
        assert!(bytes == old_bytes, "preview changed contents: {}", path.display());
        assert_eq!(modified, old_modified, "preview changed mtime: {}", path.display());
    }
}
fn closed(path: &Path, days: u64) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(days * 86400))
        .unwrap();
}
fn source(m: &Machine, title: &str) -> (PathBuf, String) {
    let folder = m.home.join("notes");
    std::fs::create_dir_all(&folder).unwrap();
    let path = claude_session(m, &folder, &[("user", title), ("assistant", "answer")], Some(title));
    closed(&path, 100);
    let id = path.file_stem().unwrap().to_string_lossy().into_owned();
    (path, id)
}

#[test]
fn preview_preserves_legacy_state_and_every_local_and_shared_file() {
    let w = World::new();
    let a = w.machine("desk");
    let b = w.machine("laptop");
    let (path, id) = source(&a, "Local continuation");
    a.pick(&["add"], &[&id]);
    claude_continue(&path, &[("user", "changed"), ("assistant", "new answer")], None);
    closed(&path, 1);
    let state = a.home.join(".local/state/actx/state.json");
    let legacy = a.home.join(".local/state/ctx/state.json");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::rename(&state, &legacy).unwrap();
    let before = snapshot(w.dir.path());
    let out = a.actx(&["sync", "--dry-run", "--all"]);
    assert!(out.contains("Upload"), "{out}");
    assert!(out.contains("Local changes since the last sync"), "{out}");
    let out = b.actx(&["sync", "--dry-run", "--general", "--auto"]);
    assert!(out.contains("New local conversation"), "{out}");
    assert!(out.contains("Destination folder needs a decision"), "{out}");
    assert_unchanged(w.dir.path(), &before);
    assert!(!state.exists());
}

#[test]
fn preview_filters_projects_and_classifies_downloads_and_conflicts() {
    let w = World::new();
    let a = w.machine("desk");
    let b = w.machine("laptop");
    let (first, id) = source(&a, "Project conversation");
    a.actx(&["mk", "Work"]);
    a.pick(&["add", "Work"], &[&id]);
    let (_, general) = source(&a, "General only");
    a.pick(&["add"], &[&general]);
    std::fs::create_dir_all(b.home.join("notes")).unwrap();
    let out = b.actx(&["sync", "--dry-run", "work"]);
    assert!(out.contains("Project conversation"), "{out}");
    assert!(!out.contains("General only"), "{out}");
    assert!(out.contains("Destination:"), "{out}");
    // Seed the second machine with the same baseline, then continue independently.
    b.actx(&["sync", "--all"]);
    let (other, _, _) = claude_transcript(&b, &id);
    claude_continue(&other, &[("user", "on laptop"), ("assistant", "laptop reply")], None);
    closed(&other, 1);
    b.actx(&["sync", "Work"]);
    let before = snapshot(w.dir.path());
    let out = a.actx(&["sync", "--dry-run", "Work"]);
    assert!(out.contains("Download"), "{out}");
    assert_unchanged(w.dir.path(), &before);
    claude_continue(&first, &[("user", "on desk"), ("assistant", "desk reply")], None);
    closed(&first, 1);
    let out = a.actx(&["sync", "--dry-run", "Work"]);
    assert!(out.contains("Both copies changed"), "{out}");
    let out = a.actx(&["sync", "--dry-run", "--general"]);
    assert!(out.contains("General only"), "{out}");
    assert!(!out.contains("Project conversation"), "{out}");
}

#[test]
fn preview_inspects_linked_wal_databases_and_retention_without_writing() {
    let w = World::new();
    let a = w.machine("desk");
    let (path, id) = source(&a, "Linked old notes");
    std::fs::create_dir_all(a.home.join(".gemini/antigravity-cli")).unwrap();
    a.actx(&[
        "init",
        a.store.to_str().unwrap(),
        "--harnesses",
        "claude,antigravity",
        "--retention",
        "never",
    ]);
    a.pick(&["add"], &[&id]);
    let links: Value =
        serde_json::from_slice(&std::fs::read(a.home.join(".local/state/actx/harness-links.json")).unwrap()).unwrap();
    let target = links["copies"].as_object().unwrap().values().next().unwrap()["target"]
        .as_str()
        .unwrap()
        .split_once(':')
        .unwrap()
        .1;
    let db = a
        .home
        .join(format!(".gemini/antigravity-cli/conversations/{target}.db"));
    let con = rusqlite::Connection::open(&db).unwrap();
    con.execute_batch("pragma journal_mode=WAL; pragma wal_autocheckpoint=0; create table preview_probe(value text); insert into preview_probe values ('keep the WAL live');").unwrap();
    a.actx(&["profile", "--retention", "1m"]);
    let before = snapshot(w.dir.path());
    let out = a.actx(&["sync", "--dry-run", "--all"]);
    assert!(out.contains("Linked histories match their saved baselines"), "{out}");
    assert!(out.contains("Delete expired shared copy"), "{out}");
    assert_unchanged(w.dir.path(), &before);
    // A continuation must be shown but not appended to the mirror in dry-run.
    claude_continue(&path, &[("user", "forward"), ("assistant", "new reply")], None);
    closed(&path, 1);
    let before = snapshot(w.dir.path());
    let out = a.actx(&["sync", "--dry-run", "--all"]);
    assert!(out.contains("Update linked copy"), "{out}");
    assert!(!out.contains("Delete expired shared copy · Linked old notes"), "{out}");
    assert_unchanged(w.dir.path(), &before);
    // Reconfigure a fresh target: show conversion without generating a conversation.
    a.actx(&["profile", "--harnesses", "claude,codex"]);
    std::fs::create_dir_all(a.home.join(".codex")).unwrap();
    let before = snapshot(w.dir.path());
    let out = a.actx(&["sync", "--dry-run", "--all"]);
    assert!(out.contains("Create linked copy"), "{out}");
    assert_unchanged(w.dir.path(), &before);
}

#[test]
fn preview_does_not_create_an_unreachable_store() {
    let w = World::new();
    let a = w.machine("desk");
    let missing = w.dir.path().join("not-created");
    let out = a
        .command(ACTX)
        .args(["sync", "--dry-run"])
        .env("ACTX_ROOT", &missing)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!missing.exists());
}

#[test]
fn preview_reads_both_opencode_schemas_without_starting_the_cli() {
    for version in [1, 2] {
        let w = World::new();
        let a = w.machine("desk");
        let (_, id) = source(&a, "SQLite inspection");
        a.pick(&["add"], &[&id]);
        let path = a.opencode_db();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let con = rusqlite::Connection::open(&path).unwrap();
        con.execute_batch("pragma journal_mode=WAL; pragma wal_autocheckpoint=0;")
            .unwrap();
        let session = if version == 1 { "session" } else { "session_v2" };
        con.execute_batch(&format!("create table {session}(id text, title text, directory text, time_updated integer, parent_id text, time_archived integer);")).unwrap();
        con.execute(
            &format!("insert into {session} values ('mirror','SQLite mirror',?1,1700000000000,null,null)"),
            [a.home.join("notes").to_string_lossy()],
        )
        .unwrap();
        if version == 1 {
            con.execute_batch("create table message(id text, session_id text, data text, time_created integer); create table part(id text, message_id text, data text, time_created integer);").unwrap();
            for (n, role, text) in [(1, "user", "SQLite inspection"), (2, "assistant", "answer")] {
                let mid = format!("msg{n}");
                con.execute(
                    "insert into message values (?1,'mirror',?2,?3)",
                    rusqlite::params![mid, serde_json::json!({"id":mid,"role":role}).to_string(), n],
                )
                .unwrap();
                con.execute(
                    "insert into part values (?1,?2,?3,?4)",
                    rusqlite::params![
                        format!("part{n}"),
                        mid,
                        serde_json::json!({"type":"text","text":text}).to_string(),
                        n
                    ],
                )
                .unwrap();
            }
        } else {
            con.execute_batch("create table session_message(id text, session_id text, type text, data text, seq integer, time_created integer);").unwrap();
            for (n, role, text) in [(1, "user", "SQLite inspection"), (2, "assistant", "answer")] {
                let data = if role == "user" {
                    serde_json::json!({"text":text})
                } else {
                    serde_json::json!({"content":[{"type":"text","text":text}]})
                };
                con.execute(
                    "insert into session_message values (?1,'mirror',?2,?3,?4,?4)",
                    rusqlite::params![format!("msg{n}"), role, data.to_string(), n],
                )
                .unwrap();
            }
        }
        let hashes: Vec<u64> = [("user", "SQLite inspection"), ("assistant", "answer")]
            .iter()
            .map(|(role, text)| {
                role.bytes()
                    .chain([0])
                    .chain(text.bytes())
                    .fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
            })
            .collect();
        let key = format!("claude:{id}");
        let links = serde_json::json!({"copies":{format!("{key}/opencode"):{"source":key,"target":"opencode:mirror","a":hashes,"b":hashes}}});
        std::fs::write(a.home.join(".local/state/actx/harness-links.json"), links.to_string()).unwrap();
        a.actx(&["profile", "--harnesses", "claude,opencode"]);
        let before = snapshot(w.dir.path());
        let output = a
            .command(ACTX)
            .args(["sync", "--dry-run", "--all"])
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            text.contains("Linked histories match their saved baselines"),
            "schema {version}: {text}"
        );
        assert_unchanged(w.dir.path(), &before);
    }
}
