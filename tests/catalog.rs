//! Focused UI smoke checks: explicit identity, duplicate titles, physical harness selection.
mod common;
use common::*;
use serde_json::{Value, json};

#[test]
fn browser_hides_local_conversations_until_all() {
    let w = World::new();
    let m = w.machine("pc");
    let work = m.home.join("notes");
    std::fs::create_dir_all(&work).unwrap();
    claude_session(
        &m,
        &work,
        &[("user", "just local"), ("assistant", "answer")],
        Some("Only local"),
    );
    let out = m.actx(&["ls"]);
    assert!(!out.contains("Only local"), "{out}");
    assert!(out.contains("No conversations in this view"), "{out}");
    let out = m.actx(&["ls", "--all"]);
    assert!(out.contains("Only local"), "{out}");
}

#[test]
fn compact_view_groups_converted_copies_but_never_merges_matching_titles() {
    let w = World::new();
    let m = w.machine("pc");
    let work = m.home.join("notes");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(m.home.join(".gemini/antigravity-cli")).unwrap();
    let source = claude_session(
        &m,
        &work,
        &[("user", "one"), ("assistant", "answer")],
        Some("Matching title"),
    );
    let unrelated = claude_session(
        &m,
        &work,
        &[("user", "another"), ("assistant", "different")],
        Some("Matching title"),
    );
    let id = source.file_stem().unwrap().to_string_lossy();
    let out = m.pick(&["cv"], &[&id, "antigravity"]);
    assert!(out.contains("agy --conversation"), "{out}");
    let view = m.actx(&["ls", "--all"]);
    assert_eq!(
        view.lines().filter(|l| l.contains("Matching title")).count(),
        2,
        "{view}"
    );
    assert_eq!(view.matches("(agy, claude)").count(), 1, "{view}");
    assert!(
        view.contains("(claude)"),
        "unrelated conversation is still separate: {view}"
    );
    let out = m.pick(&["resume", "--all", "--print"], &[&id, "antigravity"]);
    assert!(out.contains("agy --conversation"), "{out}");
    let before = files_under(&m.home.join(".gemini/antigravity-cli/conversations")).len();
    m.actx(&["profile", "--harnesses", "claude,antigravity"]);
    std::fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(86400))
        .unwrap();
    m.pick(&["add"], &[&id]);
    assert_eq!(
        files_under(&m.home.join(".gemini/antigravity-cli/conversations")).len(),
        before,
        "enabling a profile adopts the known existing conversion"
    );
    m.pick(&["cv"], &[&id, "antigravity"]);
    assert_eq!(
        files_under(&m.home.join(".gemini/antigravity-cli/conversations")).len(),
        before,
        "cv reuses an existing harness copy"
    );
    assert!(unrelated.exists());
}

#[test]
fn shared_identity_groups_copies_on_a_second_machine_without_local_links() {
    let w = World::new();
    let a = w.machine("desk");
    let b = w.machine("laptop");
    let work = a.home.join("notes");
    std::fs::create_dir_all(&work).unwrap();
    let source = claude_session(
        &a,
        &work,
        &[("user", "hi"), ("assistant", "hello")],
        Some("Portable family"),
    );
    let id = source.file_stem().unwrap().to_string_lossy();
    a.pick(&["add"], &[&id]);
    let root_key = format!("claude:{id}");
    let path = w.store().join("_general/opencode/ses_other_copy");
    std::fs::create_dir_all(&path).unwrap();
    let meta: Value = json!({"harness":"opencode","id":"ses_other_copy","title":"Different imported title", "path":work.to_string_lossy(),"host":"desk","weight":2,"updated":1_792_000_000.0,"pushed_at":1_792_000_000.0,"files":{},"conversation_id":root_key});
    std::fs::write(path.join("meta.json"), meta.to_string()).unwrap();
    let view = b.actx(&["ls", "--general"]);
    assert_eq!(view.matches("Portable family").count(), 1, "{view}");
    assert!(view.contains("(claude, opencode)"), "{view}");
    assert!(
        !view.contains("Different imported title"),
        "same explicit family, despite different titles: {view}"
    );
    assert!(
        !b.home.join(".local/state/actx/conversation-groups.json").exists(),
        "listing must not write identity state"
    );
}

#[test]
fn profile_has_a_readable_summary_and_separate_and_full_setup_paths() {
    let w = World::new();
    let m = w.machine("pc");
    let view = m.actx(&["profile"]);
    assert!(
        view.contains("Your actx profile") && view.contains("actx profile setup") && view.contains("actx setup"),
        "{view}"
    );
    assert!(!view.trim_start().starts_with('{'));
    let raw: Value = serde_json::from_str(&m.actx(&["profile", "--json"])).unwrap();
    assert_eq!(raw["retention"], "never");
    m.actx_with(
        &["profile", "setup"],
        Opts {
            pick: Some(&["claude", "3m", "::ask"]),
            answer: Some("y"),
            ..Default::default()
        },
    );
    let raw: Value = serde_json::from_str(&m.actx(&["profile", "--json"])).unwrap();
    assert_eq!(raw["harnesses"], json!(["claude"]));
    assert_eq!(raw["retention"], "3m");
    assert!(
        !w.store().join(".actx").exists(),
        "profile setup does not initialize shared storage"
    );
    m.actx(&[
        "setup",
        w.store().to_str().unwrap(),
        "--harnesses",
        "claude,codex",
        "--retention",
        "1y",
    ]);
    let raw: Value = serde_json::from_str(&m.actx(&["profile", "--json"])).unwrap();
    assert!(w.store().join(".actx").exists());
    assert_eq!(raw["harnesses"], json!(["claude", "codex"]));
    assert_eq!(raw["retention"], "1y");
}
