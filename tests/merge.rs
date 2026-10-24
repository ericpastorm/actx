//! A merge must not modify a conversation held by an agent, including one opened while waiting.

mod common;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

use common::*;
use serde_json::{Value, json};

fn conflict(w: &World) -> (Machine, Machine, PathBuf, PathBuf, String) {
    let (a, b) = (w.machine("desk"), w.machine("laptop"));
    let repo = git_repo(&a.home.join("code/shop"));
    let clone = git_clone(&repo, &b.home.join("work/shop"));
    let path = claude_session(
        &a,
        &repo,
        &[("user", "fix cart"), ("assistant", "Cart fixed")],
        Some("Cart"),
    );
    let sid = path.file_stem().unwrap().to_str().unwrap().to_string();
    a.actx(&["mk", "shop"]);
    a.pick(&["add", "shop"], &[&sid]);
    b.actx(&["sync"]);
    claude_continue(&path, &[("user", "coupons"), ("assistant", "Coupons on desk")], None);
    claude_continue(
        &claude_transcript(&b, &sid).0,
        &[("user", "green button"), ("assistant", "Green on laptop")],
        None,
    );
    b.actx(&["sync"]);
    (a, b, repo, clone, sid)
}

fn base(machine: &Machine, sid: &str) -> Value {
    let state: Value =
        serde_json::from_slice(&std::fs::read(machine.home.join(".local/state/actx/state.json")).unwrap()).unwrap();
    state["base"][format!("claude:{sid}")].clone()
}

#[test]
fn an_open_conversation_is_not_merged_until_its_agent_closes() {
    let w = World::new();
    let (a, _b, repo, _clone, sid) = conflict(&w);
    let path = claude_transcript(&a, &sid).0;
    let stored = w.store().join("shop/claude").join(&sid);
    let before = std::fs::read(&path).unwrap();
    let remote = std::fs::read(stored.join("session.jsonl.zst")).unwrap();
    let meta = std::fs::read(stored.join("meta.json")).unwrap();
    let last_sync = base(&a, &sid);
    let agent = OpenClaude::start(&a, &repo, &sid);

    let out = a.actx_with(
        &["sync", "--all"],
        Opts {
            answer: Some("m"),
            ..Default::default()
        },
    );
    assert!(
        out.contains("it's open in Claude Code right now: close it first"),
        "{out}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read(stored.join("session.jsonl.zst")).unwrap(), remote);
    assert_eq!(std::fs::read(stored.join("meta.json")).unwrap(), meta);
    assert_eq!(base(&a, &sid), last_sync);
    assert_eq!(std::fs::read_dir(w.store().join("shop/claude")).unwrap().count(), 1);
    assert_eq!(files_under(&a.claude_dir()).len(), 1, "no local branch is created");
    assert!(
        !a.home.join(".local/state/actx/backups").exists(),
        "no backup is needed for a blocked merge"
    );
    assert!(a.actx(&["ls"]).contains("⚠ conflict"));

    drop(agent);
    let out = a.actx_with(
        &["sync", "--all"],
        Opts {
            answer: Some("m"),
            ..Default::default()
        },
    );
    assert!(out.contains("branch from laptop"), "{out}");
    let messages = claude_transcript(&a, &sid).1;
    assert!(messages.iter().any(|text| text.contains("Coupons on desk")));
    assert!(messages.iter().any(|text| text.contains("Green on laptop")));
    assert_eq!(std::fs::read_dir(w.store().join("shop/claude")).unwrap().count(), 2);
    assert!(a.actx(&["ls"]).contains("✓ synced"));
}

#[test]
fn an_agent_opened_while_waiting_for_the_lock_prevents_the_merge() {
    let w = World::new();
    let (a, b, repo, _clone, sid) = conflict(&w);

    // A second conversation needs a pull. Checking it before taking the lock caches the list of
    // running agents, so a merge must refresh that list after the wait rather than reuse it.
    let other = claude_session(
        &a,
        &repo,
        &[("user", "docs"), ("assistant", "Initial docs")],
        Some("Docs"),
    );
    let other_id = other.file_stem().unwrap().to_str().unwrap();
    a.actx(&["mk", "shop"]);
    a.pick(&["add", "shop"], &[other_id]);
    b.actx(&["sync"]);
    claude_continue(
        &claude_transcript(&b, other_id).0,
        &[("user", "more"), ("assistant", "More docs")],
        None,
    );
    b.actx(&["sync"]);
    let path = claude_transcript(&a, &sid).0;
    let before = std::fs::read(&path).unwrap();
    let stored = w.store().join("shop/claude").join(&sid);
    let remote = std::fs::read(stored.join("session.jsonl.zst")).unwrap();
    let last_sync = base(&a, &sid);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64();
    let lock = w.store().join(".lock");
    std::fs::write(
        &lock,
        json!({"host": "elsewhere", "pid": 1, "since": now, "at": now, "token": "held"}).to_string(),
    )
    .unwrap();

    let mut child = a
        .command(ACTX)
        .args(["sync", "--all"])
        .env("ACTX_ANSWER", "m")
        .env("ACTX_LOCK_WAIT", "30")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let waiting = lines
        .by_ref()
        .map_while(Result::ok)
        .find(|line| line.contains("waiting"));
    assert!(waiting.is_some(), "sync waits for the shared lock");

    let _agent = OpenClaude::start(&a, &repo, &sid);
    std::fs::remove_file(lock).unwrap();
    let out = lines.map_while(Result::ok).collect::<Vec<_>>().join("\n");
    assert!(child.wait().unwrap().success(), "{out}");
    assert!(
        out.contains("it's open in Claude Code right now: close it first"),
        "{out}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read(stored.join("session.jsonl.zst")).unwrap(), remote);
    assert_eq!(base(&a, &sid), last_sync);
    assert_eq!(std::fs::read_dir(w.store().join("shop/claude")).unwrap().count(), 2);
    assert!(claude_transcript(&a, other_id).1.contains(&"More docs".to_string()));
}
