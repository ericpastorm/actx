//! Codex, checked against the real Codex: its app-server lists and reads what actx writes.

mod common;

use common::*;

fn setup(w: &World) -> (Machine, Machine, std::path::PathBuf, std::path::PathBuf, String) {
    let (a, b) = (w.machine("desk"), w.machine("laptop"));
    let repo = git_repo(&a.home.join("code/shop"));
    let clone = git_clone(&repo, &b.home.join("work/shop-laptop"));
    let rollout = codex_rollout(
        &a,
        &repo,
        &[
            ("user", "fix the cart"),
            ("assistant", &format!("Done in {}/cart.ts", repo.display())),
        ],
        Some("Cart fix"),
    );
    let sid = rollout_id(&rollout);
    let out = a.pick(&["mk", "shop"], &[&sid]);
    assert!(out.contains("Cart fix"), "{out}");
    (a, b, repo, clone, sid)
}

fn rollout_of(m: &Machine, sid: &str) -> std::path::PathBuf {
    find(&m.codex_sessions(), &format!("{sid}.jsonl")).expect("the rollout is there")
}

/// Compare message text after decoding JSON: Windows paths contain escaped backslashes on disk.
fn message_texts(rollout: &str) -> Vec<String> {
    rollout
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|entry| entry["type"] == "response_item" && entry["payload"]["type"] == "message")
        .flat_map(|entry| entry["payload"]["content"].as_array().cloned().unwrap_or_default())
        .filter_map(|content| content["text"].as_str().map(String::from))
        .collect()
}

#[test]
fn codex_sync_between_different_paths() {
    needs!("codex");
    let w = World::new();
    let (a, b, repo, clone, sid) = setup(&w);
    b.actx(&["sync"]);
    let path = rollout_of(&b, &sid);
    let text = std::fs::read_to_string(&path).unwrap();
    let first: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(first["payload"]["cwd"], clone.to_string_lossy().as_ref());
    assert!(message_texts(&text).contains(&format!("Done in {}/cart.ts", clone.display())));
    assert_eq!(
        codex_threads(&b)[&sid],
        ("fix the cart".into(), 1, clone.to_string_lossy().into_owned())
    ); // Codex itself sees it
    assert!(
        std::fs::read_to_string(b.home.join(".codex/session_index.jsonl"))
            .unwrap()
            .contains("Cart fix")
    );

    codex_continue(
        &path,
        &[
            ("user", "add coupons"),
            ("assistant", &format!("Coupons in {}/coupons.ts", clone.display())),
        ],
        None,
    );
    assert!(b.actx(&["sync"]).contains('↑'));
    a.actx(&["sync"]);
    let text = std::fs::read_to_string(rollout_of(&a, &sid)).unwrap();
    let messages = message_texts(&text);
    assert!(
        messages.contains(&format!("Coupons in {}/coupons.ts", repo.display()))
            && !messages.iter().any(|message| message.contains(clone.to_str().unwrap()))
    );
    assert!(!a.actx(&["ls"]).contains("pending") && !b.actx(&["ls"]).contains("pending"));
}

#[test]
fn codex_conversions() {
    needs!("codex");
    let w = World::new();
    let (_a, b, _repo, clone, sid) = setup(&w);
    b.actx(&["sync"]);
    claude_session(&b, &clone, &[("user", "hi"), ("assistant", "hello")], None);
    let out = b.pick(&["cv"], &[&sid, "claude"]); // Codex → Claude Code
    assert!(
        out.contains("Cart fix (from Codex)") && out.contains("claude --resume"),
        "{out}"
    );

    let copy = files_under(&b.claude_dir())
        .into_iter()
        .find(|p| std::fs::read_to_string(p).unwrap_or_default().contains("from Codex"))
        .unwrap();
    let out = b.pick(&["cv"], &[&copy.file_stem().unwrap().to_string_lossy(), "codex"]); // Claude Code → Codex
    let new_id = out
        .split("codex resume ")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .expect("a resume command")
        .to_string();
    let (preview, turns, cwd) = codex_threads(&b)[&new_id].clone();
    assert!(
        preview.contains("Conversation imported from Claude Code") && turns == 1 && cwd == clone.to_string_lossy(),
        "{preview}"
    );
}

#[test]
fn codex_conflict_merge() {
    needs!("codex");
    let w = World::new();
    let (a, b, _repo, _clone, sid) = setup(&w);
    b.actx(&["sync"]);
    codex_continue(
        &rollout_of(&a, &sid),
        &[("user", "coupons"), ("assistant", "Coupons done on desk")],
        None,
    );
    codex_continue(
        &rollout_of(&b, &sid),
        &[("user", "green button"), ("assistant", "Button green on laptop")],
        None,
    );
    b.actx(&["sync"]);
    assert!(a.actx(&["ls"]).contains("⚠ conflict"));

    let out = a.actx_with(
        &["sync", "--all"],
        Opts {
            answer: Some("m"),
            ..Default::default()
        },
    );
    assert!(out.contains("branch from laptop"), "{out}");
    let text = std::fs::read_to_string(rollout_of(&a, &sid)).unwrap();
    assert!(
        text.contains("Coupons done on desk") && text.contains("[Meanwhile, this conversation continued on laptop")
    );
    assert!(text.contains("Button green on laptop"));
    let threads = codex_threads(&a);
    assert_eq!(threads[&sid].1, 3); // the original turn, desk's, and the merge note with its reply
    assert_eq!(threads.values().filter(|(p, _, _)| p == "fix the cart").count(), 2); // plus the branch
    b.actx(&["sync"]);
    assert!(!a.actx(&["ls"]).contains("pending") && !b.actx(&["ls"]).contains("pending"));
}
