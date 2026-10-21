//! Cleanup is always reviewed, local-only and recoverable. No test deletes real user data.
mod common;
use common::*;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
fn old(path: &Path) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(10 * 86400))
        .unwrap();
}
fn conversation(m: &Machine, title: &str, prompt: &str, reply: &str) -> (PathBuf, String) {
    let folder = m.home.join("notes");
    std::fs::create_dir_all(&folder).unwrap();
    let path = claude_session(m, &folder, &[("user", prompt), ("assistant", reply)], Some(title));
    old(&path);
    let id = path.file_stem().unwrap().to_string_lossy().into_owned();
    (path, id)
}
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    let mut out = BTreeMap::new();
    for path in files_under(root) {
        out.insert(
            path.clone(),
            (
                std::fs::read(&path).unwrap(),
                path.metadata().unwrap().modified().unwrap(),
            ),
        );
    }
    out
}
fn entry(m: &Machine) -> (PathBuf, Value) {
    let dir = m.home.join(".local/state/actx/quarantine");
    let path = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path().join("entry.json"))
        .find(|p| p.exists())
        .unwrap();
    let value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    (path, value)
}
fn native(m: &Machine, args: &[&str], pick: Option<&str>) -> String {
    let mut cmd = m.command(ACTX);
    cmd.args(args).env("ACTX_ANSWER", "yes");
    if let Some(pick) = pick {
        cmd.env("ACTX_PICK", pick);
    }
    if let Some(bin) = std::env::var_os("ACTX_TEST_CODEX_BIN") {
        let parent = PathBuf::from(bin).parent().unwrap().to_path_buf();
        let path = std::env::join_paths(
            std::iter::once(parent).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())),
        )
        .unwrap();
        cmd.env("PATH", path);
    }
    let output = cmd.output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}
#[test]
fn read_only_suggestions_explain_tests_repetition_and_native_children() {
    let w = World::new();
    let m = w.machine("desk");
    let (_, test) = conversation(&m, "Connection smoke", "Responde solo: OK", "OK");
    let (_, shared) = conversation(&m, "Shared important short", "Keep me", "Yes");
    m.pick(&["add"], &[&shared]);
    let (recent, _) = conversation(&m, "Recent smoke", "Responde solo: OK", "OK");
    std::fs::File::options()
        .write(true)
        .open(recent)
        .unwrap()
        .set_modified(SystemTime::now())
        .unwrap();
    let (parent, parent_id) = conversation(
        &m,
        "Meaningful parent",
        "A detailed question about my project",
        &"Important context. ".repeat(100),
    );
    let child_dir = parent.parent().unwrap().join(&parent_id).join("subagents");
    std::fs::create_dir_all(&child_dir).unwrap();
    let child = child_dir.join("agent-clean-test.jsonl");
    std::fs::copy(&parent, &child).unwrap();
    old(&child);
    for n in 0..3 {
        conversation(
            &m,
            &format!("Worker {n}"),
            &format!(
                "Eres un agente de programación que trabaja en este proyecto. {} task {n}",
                "Shared role instructions. ".repeat(12)
            ),
            &"Work done. ".repeat(60),
        );
    }
    let before = snapshot(w.dir.path());
    let out = m.actx(&["clean", "--dry-run"]);
    assert!(out.contains(&test), "{out}");
    assert!(out.contains("Short connection test"), "{out}");
    assert!(out.contains("Similar opening in 3"), "{out}");
    assert!(out.contains("Subagent confirmed"), "{out}");
    assert!(!out.contains("Recent smoke"), "{out}");
    assert!(!out.contains("Shared important short"), "{out}");
    assert_eq!(snapshot(w.dir.path()), before);
    let out = m.actx(&["clean", "--dry-run", "--only", "tests"]);
    assert!(!out.contains("Worker 0"), "{out}");
    // An active parent protects its old native children too.
    std::fs::File::options()
        .write(true)
        .open(&parent)
        .unwrap()
        .set_modified(SystemTime::now())
        .unwrap();
    let out = m.actx(&["clean", "--dry-run", "--only", "agents"]);
    assert!(!out.contains("agent-clean-test"), "{out}");
}
#[test]
fn selected_claude_conversations_are_compressed_and_recoverable_without_overwrites() {
    let w = World::new();
    let m = w.machine("desk");
    let (path, id) = conversation(&m, "Disposable test", "Responde solo OK", "OK");
    let original = std::fs::read(&path).unwrap();
    let out = m.actx_with(
        &["clean"],
        Opts {
            pick: Some(&[&id]),
            answer: Some("no"),
            ..Default::default()
        },
    );
    assert!(out.contains("nothing has been removed"));
    assert!(path.exists());
    let out = native(&m, &["clean"], Some(&id));
    assert!(out.contains("Quarantined:"), "{out}");
    assert!(!path.exists());
    let (manifest, record) = entry(&m);
    assert_eq!(record["status"], "quarantined");
    assert!(manifest.parent().unwrap().join("payload.zst").is_file());
    let key = record["id"].as_str().unwrap();
    std::fs::write(&path, "new data must survive").unwrap();
    let out = native(&m, &["clean", "restore", key], None);
    assert!(out.contains("nothing was overwritten"), "{out}");
    assert_eq!(std::fs::read(&path).unwrap(), b"new data must survive");
    std::fs::remove_file(&path).unwrap();
    let out = native(&m, &["clean", "restore", key], None);
    assert!(out.contains("Recovered:"), "{out}");
    assert_eq!(std::fs::read(path).unwrap(), original);
    assert!(m.actx(&["clean", "quarantine"]).contains("Quarantine is empty"));
}
#[test]
fn antigravity_round_trip_preserves_messages_index_and_cache() {
    let w = World::new();
    let m = w.machine("desk");
    let (_, id) = conversation(&m, "AGY disposable", "Responde solo OK", "OK");
    std::fs::create_dir_all(m.home.join(".gemini/antigravity-cli")).unwrap();
    m.pick(&["cv"], &[&id, "antigravity"]);
    let home = m.home.join(".gemini/antigravity-cli");
    let db = files_under(&home.join("conversations"))
        .into_iter()
        .find(|p| p.extension().is_some_and(|e| e == "db"))
        .unwrap();
    let agy_id = db.file_stem().unwrap().to_string_lossy().into_owned();
    let con = rusqlite::Connection::open(&db).unwrap();
    con.execute_batch("pragma journal_mode=WAL; pragma wal_autocheckpoint=0;")
        .unwrap();
    let n: i64 = con.query_row("select count(*) from steps", [], |r| r.get(0)).unwrap();
    drop(con);
    let out = native(&m, &["clean", "--harnesses", "antigravity"], Some(&agy_id));
    assert!(out.contains("Quarantined:"), "{out}");
    assert!(!db.exists());
    let index = rusqlite::Connection::open(home.join("conversation_summaries.db")).unwrap();
    let count: i64 = index
        .query_row(
            "select count(*) from conversation_summaries where conversation_id=?1",
            [&agy_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    drop(index);
    let (_, record) = entry(&m);
    let out = native(&m, &["clean", "restore", record["id"].as_str().unwrap()], None);
    assert!(out.contains("Recovered:"), "{out}");
    let con = rusqlite::Connection::open(db).unwrap();
    assert_eq!(
        con.query_row("select count(*) from steps", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        n
    );
    assert!(m.actx(&["ls"]).contains("AGY disposable"));
}
#[test]
fn codex_native_archive_and_recovery_preserve_identity_and_transcript() {
    if std::env::var_os("ACTX_TEST_CODEX_BIN").is_none() && !which("codex") {
        return;
    }
    let w = World::new();
    let m = w.machine("desk");
    let folder = m.home.join("notes");
    std::fs::create_dir_all(&folder).unwrap();
    let path = codex_rollout(
        &m,
        &folder,
        &[("user", "Responde solo OK"), ("assistant", "OK")],
        Some("Native disposable"),
    );
    old(&path);
    let id = rollout_id(&path);
    let bytes = std::fs::read(&path).unwrap();
    let out = native(&m, &["clean", "--harnesses", "codex"], Some(&id));
    assert!(out.contains("Quarantined:"), "{out}");
    assert!(!path.exists());
    let (_, record) = entry(&m);
    let out = native(&m, &["clean", "restore", record["id"].as_str().unwrap()], None);
    assert!(out.contains("Recovered:"), "{out}");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(m.actx(&["ls"]).contains("Native disposable"));
}

#[test]
fn opencode_multiple_sessions_can_be_quarantined_and_recovered_independently() {
    needs!("opencode");
    let w = World::new();
    let m = w.machine("desk");
    let folder = m.home.join("notes");
    std::fs::create_dir_all(&folder).unwrap();
    let major = opencode_major().unwrap();
    let mut exp = opencode_fixture(&folder, major);
    exp["messages"].as_array_mut().unwrap().truncate(2);
    if major == 1 {
        for (i, text) in [(0, "Responde solo OK"), (1, "OK")] {
            let mut part = exp["messages"][i]["parts"][0].clone();
            part["type"] = serde_json::json!("text");
            part["text"] = serde_json::json!(text);
            exp["messages"][i]["parts"] = serde_json::json!([part]);
        }
    } else {
        exp["messages"][0]["text"] = serde_json::json!("Responde solo OK");
        exp["messages"][1]["content"] = serde_json::json!([{"type":"text","text":"OK"}]);
    }
    exp["info"]["title"] = serde_json::json!("OC disposable A");
    opencode_import(&m, &exp, &folder);
    let other = format!("{}U", &OC_SID[..OC_SID.len() - 1]);
    let mut encoded = exp.to_string().replace(OC_SID, &other);
    // Message/part identifiers are global, not scoped to a session.
    for message in exp["messages"].as_array().unwrap() {
        let id = if major == 1 {
            message["info"]["id"].as_str().unwrap()
        } else {
            message["id"].as_str().unwrap()
        };
        encoded = encoded.replace(id, &format!("{}X", &id[..id.len() - 1]));
        if major == 1 {
            for part in message["parts"].as_array().unwrap() {
                if let Some(id) = part["id"].as_str() {
                    encoded = encoded.replace(id, &format!("{}X", &id[..id.len() - 1]));
                }
            }
        }
    }
    let mut second: Value = serde_json::from_str(&encoded).unwrap();
    second["info"]["title"] = serde_json::json!("OC disposable B");
    opencode_import(&m, &second, &folder);
    // Importing can touch activity: explicitly simulate both agents having closed ten days ago.
    let con = rusqlite::Connection::open(m.opencode_db()).unwrap();
    let table = if major == 1 { "session" } else { "session_v2" };
    con.execute(
        &format!("update {table} set time_updated=?1"),
        [(SystemTime::now() - Duration::from_secs(10 * 86400))
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64],
    )
    .unwrap();
    drop(con);
    let out = native(&m, &["clean", "--harnesses", "opencode"], Some("opencode:"));
    assert!(out.contains("2 local conversations quarantined"), "{out}");
    assert!(opencode_sessions(&m).is_empty());
    let entries = files_under(&m.home.join(".local/state/actx/quarantine"))
        .into_iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "entry.json"))
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 2);
    for p in entries {
        let entry: Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
        let out = native(&m, &["clean", "restore", entry["id"].as_str().unwrap()], None);
        assert!(out.contains("Recovered:"), "{out}");
    }
    let sessions = opencode_sessions(&m);
    assert!(sessions.contains_key(OC_SID));
    assert!(sessions.contains_key(&other));
}
