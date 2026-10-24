//! Two focused smoke checks for the feature pass; broad matrix/debugging is deferred.
mod common;
use common::*;
use serde_json::Value;
use std::{
    path::Path,
    time::{Duration, SystemTime},
};
fn closed(path: &Path, days: u64) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(days * 86400))
        .unwrap();
}
fn field(n: u8, data: &[u8]) -> Vec<u8> {
    assert!(data.len() < 128);
    let key = (u16::from(n) << 3) | 2;
    let mut v = if key >= 128 {
        vec![(key as u8 & 127) | 128, (key >> 7) as u8]
    } else {
        vec![key as u8]
    };
    v.push(data.len() as u8);
    v.extend(data);
    v
}
#[test]
fn linked_harnesses_continue_both_ways_without_new_session_ids() {
    let w = World::new();
    let m = w.machine("desk");
    let folder = m.home.join("notes");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(m.home.join(".gemini/antigravity-cli")).unwrap();
    let source = claude_session(
        &m,
        &folder,
        &[("user", "hello"), ("assistant", "hi")],
        Some("Linked notes"),
    );
    closed(&source, 1);
    let sid = source.file_stem().unwrap().to_string_lossy();
    m.actx(&[
        "init",
        m.store.to_str().unwrap(),
        "--harnesses",
        "claude,antigravity",
        "--retention",
        "never",
    ]);
    m.pick(&["add"], &[&sid]);
    let link_file = m.home.join(".local/state/actx/harness-links.json");
    let first: Value = serde_json::from_slice(&std::fs::read(&link_file).unwrap()).unwrap();
    let link = first["copies"].as_object().unwrap().values().next().unwrap();
    let target = link["target"].as_str().unwrap().split_once(':').unwrap().1;
    let db = m
        .home
        .join(format!(".gemini/antigravity-cli/conversations/{target}.db"));
    claude_continue(&source, &[("user", "forward"), ("assistant", "forward answer")], None);
    closed(&source, 1);
    let out = m.actx(&["sync", "--all"]);
    assert!(out.contains('↔'), "{out}");
    let con = rusqlite::Connection::open(&db).unwrap();
    let n: i64 = con.query_row("select count(*) from steps", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 4);
    // Simulate a completed continuation written by Antigravity.
    let user = field(19, &field(2, b"reverse"));
    let reply = field(20, &field(1, b"reverse answer"));
    con.execute(
        "insert into steps(idx,step_type,status,step_payload) values(4,14,3,?1)",
        [user],
    )
    .unwrap();
    con.execute(
        "insert into steps(idx,step_type,status,step_payload) values(5,15,3,?1)",
        [reply],
    )
    .unwrap();
    drop(con);
    let out = m.actx(&["sync", "--all"]);
    assert!(out.contains('↔'), "{out}");
    let (_, texts, _) = claude_transcript(&m, &sid);
    assert_eq!(texts.last().unwrap(), "reverse answer");
    let stored = w.store().join("_general/claude").join(sid.as_ref());
    let meta: Value = serde_json::from_slice(&std::fs::read(stored.join("meta.json")).unwrap()).unwrap();
    let saved =
        zstd::stream::decode_all(std::fs::File::open(stored.join(meta["files"]["session"].as_str().unwrap())).unwrap())
            .unwrap();
    assert!(
        String::from_utf8(saved).unwrap().contains("reverse answer"),
        "linked continuation must be shared in the same sync"
    );
    m.actx(&["sync", "--all"]);
    let last: Value = serde_json::from_slice(&std::fs::read(&link_file).unwrap()).unwrap();
    assert_eq!(
        first["copies"].as_object().unwrap().keys().collect::<Vec<_>>(),
        last["copies"].as_object().unwrap().keys().collect::<Vec<_>>()
    );
    assert_eq!(
        last["copies"].as_object().unwrap().values().next().unwrap()["target"],
        link["target"]
    );
}
#[test]
fn listings_show_linked_copies_as_in_use_while_an_agent_writes_the_source() {
    use std::io::Write;
    let w = World::new();
    let m = w.machine("desk");
    let folder = m.home.join("notes");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(m.home.join(".gemini/antigravity-cli")).unwrap();
    let source = claude_session(
        &m,
        &folder,
        &[("user", "hello"), ("assistant", "hi")],
        Some("Busy notes"),
    );
    closed(&source, 1);
    let sid = source.file_stem().unwrap().to_string_lossy().into_owned();
    m.actx(&[
        "init",
        m.store.to_str().unwrap(),
        "--harnesses",
        "claude,antigravity",
        "--retention",
        "never",
    ]);
    m.pick(&["add"], &[&sid]);
    let listed = || {
        let mut child = m
            .command(ACTX)
            .arg("api")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(br#"{"action":"list"}"#).unwrap();
        let out: Value = serde_json::from_slice(&child.wait_with_output().unwrap().stdout).unwrap();
        let copies = out["data"]["conversations"][0]["copies"].as_array().unwrap().clone();
        let state = |harness: &str| {
            let copy = copies.iter().find(|c| c["harness"] == harness).unwrap();
            copy["state"].as_str().unwrap().to_string()
        };
        (state("claude"), state("antigravity"))
    };
    // The agent goes on writing and still has it open.
    claude_continue(&source, &[("user", "more"), ("assistant", "more answer")], None);
    let agent = OpenClaude::start(&m, &folder, &sid);
    assert_eq!(listed(), ("push".into(), "active".into()));
    // Closed: listings read it again and report the exact state.
    drop(agent);
    assert_eq!(listed(), ("push".into(), "pull".into()));
}
#[test]
fn desktop_jobs_hand_over_refreshed_caches_and_plain_api_calls_write_nothing() {
    use std::io::Write;
    let w = World::new();
    let m = w.machine("desk");
    let folder = m.home.join("notes");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(m.home.join(".gemini/antigravity-cli")).unwrap();
    let source = claude_session(
        &m,
        &folder,
        &[("user", "hello"), ("assistant", "hi")],
        Some("Cached notes"),
    );
    closed(&source, 1);
    let sid = source.file_stem().unwrap().to_string_lossy().into_owned();
    m.actx(&[
        "init",
        m.store.to_str().unwrap(),
        "--harnesses",
        "claude,antigravity",
        "--retention",
        "never",
    ]);
    m.pick(&["add"], &[&sid]);
    claude_continue(&source, &[("user", "more"), ("assistant", "more answer")], None);
    closed(&source, 1);
    let cache = m.home.join(".cache/actx");
    let listed = |job: bool| {
        let mut c = m.command(ACTX);
        if job {
            c.env("ACTX_PROGRESS", "1");
        }
        let mut child = c
            .arg("api")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(br#"{"action":"list"}"#).unwrap();
        let err = String::from_utf8(child.wait_with_output().unwrap().stderr).unwrap();
        err.lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|e| e["event"] == "cache_entries")
            .map(|e| e["file"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    let snapshot = || {
        files_under(&cache)
            .into_iter()
            .map(|p| (p.clone(), std::fs::read(p).unwrap()))
            .collect::<Vec<_>>()
    };
    let before = snapshot();
    assert!(listed(false).is_empty());
    assert_eq!(snapshot(), before, "a plain API call writes no cache");
    let files = listed(true);
    assert!(files.contains(&"content-v1.json".to_string()), "{files:?}");
    assert!(files.contains(&"linked-history-v1.json".to_string()), "{files:?}");
    assert_eq!(snapshot(), before, "the job's parent keeps them, not the job");
}
#[test]
fn a_claude_compaction_repairs_linked_copies_and_quarantines_the_replaced_one() {
    use std::io::Write;
    let w = World::new();
    let m = w.machine("desk");
    let folder = m.home.join("notes");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(m.home.join(".gemini/antigravity-cli")).unwrap();
    let source = claude_session(
        &m,
        &folder,
        &[("user", "hello"), ("assistant", "hi")],
        Some("Long notes"),
    );
    closed(&source, 1);
    let sid = source.file_stem().unwrap().to_string_lossy().into_owned();
    m.actx(&[
        "init",
        m.store.to_str().unwrap(),
        "--harnesses",
        "claude,antigravity",
        "--retention",
        "never",
    ]);
    m.pick(&["add"], &[&sid]);
    let link_file = m.home.join(".local/state/actx/harness-links.json");
    let target = |links: &Value| links["copies"].as_object().unwrap().values().next().unwrap()["target"].clone();
    let before: Value = serde_json::from_slice(&std::fs::read(&link_file).unwrap()).unwrap();
    // Claude Code compacts: the history starts again from a summary.
    let boundary = serde_json::json!({"type":"system","subtype":"compact_boundary","uuid":"boundary-1",
        "parentUuid":null,"isSidechain":false,"cwd":folder,"sessionId":sid});
    let mut file = std::fs::OpenOptions::new().append(true).open(&source).unwrap();
    writeln!(file, "{boundary}").unwrap();
    drop(file);
    claude_continue(
        &source,
        &[
            ("user", "Summary of the earlier conversation"),
            ("assistant", "after compaction"),
        ],
        None,
    );
    closed(&source, 1);
    let out = m.actx(&["sync", "--all"]);
    assert!(out.contains("Compacted history repaired"), "{out}");
    let after: Value = serde_json::from_slice(&std::fs::read(&link_file).unwrap()).unwrap();
    assert_ne!(target(&after), target(&before));
    assert!(!m.actx(&["ls"]).contains("conflict"));
    // The replaced copy is recoverable instead of piling up in Antigravity.
    let old = target(&before).as_str().unwrap().split_once(':').unwrap().1.to_string();
    let quarantined = m.actx(&["clean", "quarantine"]);
    assert!(quarantined.contains("Long notes"), "{quarantined}");
    assert!(
        !m.home
            .join(format!(".gemini/antigravity-cli/conversations/{old}.db"))
            .exists()
    );
}
#[test]
fn a_codex_conversation_closed_seconds_ago_updates_its_linked_copies_at_once() {
    let w = World::new();
    let m = w.machine("desk");
    let folder = m.home.join("notes");
    std::fs::create_dir_all(&folder).unwrap();
    // Claude Code's list of open sessions, as on a real machine: nothing holds the copy.
    std::fs::create_dir_all(m.home.join(".claude/sessions")).unwrap();
    let source = codex_rollout(
        &m,
        &folder,
        &[("user", "hello"), ("assistant", "hi")],
        Some("Codex notes"),
    );
    closed(&source, 1);
    let id = rollout_id(&source);
    m.actx(&[
        "init",
        m.store.to_str().unwrap(),
        "--harnesses",
        "codex,claude",
        "--retention",
        "never",
    ]);
    m.pick(&["add"], &[&id]);
    let link_file = m.home.join(".local/state/actx/harness-links.json");
    let links: Value = serde_json::from_slice(&std::fs::read(&link_file).unwrap()).unwrap();
    let mirror = links["copies"].as_object().unwrap().values().next().unwrap()["target"]
        .as_str()
        .unwrap()
        .split_once(':')
        .unwrap()
        .1
        .to_string();
    // Codex just wrote it and nothing has it open: switching harness doesn't wait two minutes.
    codex_continue(&source, &[("user", "next"), ("assistant", "codex answer")], None);
    m.actx(&["sync", "--all"]);
    let (_, texts, _) = claude_transcript(&m, &mirror);
    assert_eq!(texts.last().unwrap(), "codex answer", "{texts:?}");
}
#[test]
fn continuing_in_a_codex_copy_closed_seconds_ago_reaches_the_source_at_once() {
    let w = World::new();
    let m = w.machine("desk");
    let folder = m.home.join("notes");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(m.home.join(".claude/sessions")).unwrap();
    let source = claude_session(
        &m,
        &folder,
        &[("user", "hello"), ("assistant", "hi")],
        Some("Claude notes"),
    );
    closed(&source, 1);
    let sid = source.file_stem().unwrap().to_string_lossy().into_owned();
    m.actx(&[
        "init",
        m.store.to_str().unwrap(),
        "--harnesses",
        "claude,codex",
        "--retention",
        "never",
    ]);
    m.pick(&["add"], &[&sid]);
    let link_file = m.home.join(".local/state/actx/harness-links.json");
    let links: Value = serde_json::from_slice(&std::fs::read(&link_file).unwrap()).unwrap();
    let target = links["copies"].as_object().unwrap().values().next().unwrap()["target"]
        .as_str()
        .unwrap()
        .to_string();
    let copy = find(
        &m.codex_sessions(),
        &format!("{}.jsonl", target.split_once(':').unwrap().1),
    )
    .unwrap();
    // Continued in Codex and closed it just now: back in Claude Code without waiting.
    codex_continue(&copy, &[("user", "from codex"), ("assistant", "codex reply")], None);
    m.actx(&["sync", "--all"]);
    let (_, texts, _) = claude_transcript(&m, &sid);
    assert_eq!(texts.last().unwrap(), "codex reply", "{texts:?}");
}
#[test]
fn retention_is_opt_in_and_only_removes_shared_copies() {
    let w = World::new();
    let m = w.machine("desk");
    let folder = m.home.join("notes");
    std::fs::create_dir_all(&folder).unwrap();
    let source = claude_session(
        &m,
        &folder,
        &[("user", "old"), ("assistant", "answer")],
        Some("Old notes"),
    );
    closed(&source, 100);
    let id = source.file_stem().unwrap().to_string_lossy();
    m.pick(&["add"], &[&id]);
    let stored = w.store().join("_general/claude").join(id.as_ref());
    m.actx(&["sync", "--all"]);
    assert!(stored.exists());
    m.actx(&["profile", "--retention", "1m"]);
    let original = std::fs::read(&source).unwrap();
    let out = m.actx(&["sync", "--all"]);
    assert!(out.contains("Retention:"), "{out}");
    assert!(!stored.exists());
    assert_eq!(std::fs::read(&source).unwrap(), original);
    m.actx(&["sync", "--all"]);
    assert!(!stored.exists());
}
