//! opencode, against the real opencode CLI installed here (1.x or 2.x; skipped when there's none).
//! These double as an early warning: if opencode changes its format, they break here first.

mod common;

use common::*;

fn setup(w: &World) -> Option<(Machine, Machine, std::path::PathBuf, std::path::PathBuf, u8)> {
    let major = opencode_major()?;
    let (a, b) = (w.machine("desk"), w.machine("laptop"));
    let repo = git_repo(&a.home.join("code/web"));
    let clone = git_clone(&repo, &b.home.join("src/web-laptop"));
    opencode_import(&a, &opencode_fixture(&repo, major), &repo);
    a.actx(&["mk", "web"]);
    let out = a.pick(&["add", "web"], &[OC_SID]);
    assert!(out.contains("Fix the login form"), "{out}");
    Some((a, b, repo, clone, major))
}

#[test]
fn opencode_sync_and_convert() {
    needs!("opencode");
    let w = World::new();
    let (_a, b, _repo, clone, _) = setup(&w).unwrap();
    b.actx(&["sync"]);
    let (title, dir, n) = opencode_sessions(&b)[OC_SID].clone();
    assert_eq!(
        (title.as_str(), dir.as_str(), n),
        ("Fix the login form", slash(&clone).as_str(), 3)
    );

    let claude = claude_session(&b, &clone, &[("user", "hi"), ("assistant", "hello")], None); // so "convert to" has a choice
    let out = b.pick(&["cv"], &[OC_SID, "claude"]); // opencode → Claude Code
    assert!(
        out.contains("Fix the login form (from opencode)") && out.contains("claude --resume"),
        "{out}"
    );
    let copy = files_under(&b.claude_dir())
        .into_iter()
        .find(|p| *p != claude && p.extension().is_some_and(|e| e == "jsonl"))
        .unwrap();
    let first: serde_json::Value =
        serde_json::from_str(std::fs::read_to_string(&copy).unwrap().lines().next().unwrap()).unwrap();
    assert_eq!(first["cwd"], clone.to_string_lossy().as_ref());
    assert!(
        first["message"]["content"]
            .as_str()
            .unwrap()
            .contains("Conversation imported from opencode (test-model)")
    );
    assert!(
        std::fs::read_to_string(&copy)
            .unwrap()
            .contains("$ grep -n submit src/login.js")
    );

    let stem = copy.file_stem().unwrap().to_string_lossy().into_owned();
    let out = b.pick(&["cv"], &[&stem, "opencode"]); // and back: Claude Code → opencode
    assert!(out.contains("opencode ") && out.contains(" -s ses_"), "{out}");
    assert!(
        opencode_sessions(&b)
            .values()
            .any(|(t, _, _)| t == "Fix the login form (from Claude Code)")
    );
    let listing = b.actx(&["ls"]);
    assert!(listing.contains("web") && !listing.contains("pending"), "{listing}"); // the copies travel with the project
}

#[test]
fn opencode_conflict_merge() {
    needs!("opencode");
    let w = World::new();
    let (a, b, repo, clone, _) = setup(&w).unwrap();
    b.actx(&["sync"]);
    opencode_continue(&a, &repo, "and on Windows?", "Use the Task Manager (desk)");
    opencode_continue(&b, &clone, "how do I prevent it?", "Disable the daemon (laptop)");
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
    let found = opencode_sessions(&a);
    assert_eq!(found[OC_SID].2, 3 + 2 + 2); // fixture + desk's turn + the merge note and its reply
    assert!(
        found
            .values()
            .any(|(t, _, _)| t == "Fix the login form (branch from laptop)")
    );
    b.actx(&["sync"]);
    assert!(!a.actx(&["ls"]).contains("pending") && !b.actx(&["ls"]).contains("pending"));
}

#[test]
fn a_conversation_from_the_other_opencode_version_is_converted() {
    needs!("opencode");
    let w = World::new();
    let (_a, b, repo, clone, major) = setup(&w).unwrap();
    let other = if major == 2 { 1 } else { 2 };
    // make the stored copy look like it came from the other version
    let stored = w.store().join("web/opencode").join(OC_SID);
    let mut meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(stored.join("meta.json")).unwrap()).unwrap();
    meta["files"]["format"] = serde_json::json!(format!("opencode-v{other}"));
    std::fs::write(stored.join("meta.json"), meta.to_string()).unwrap();
    let file = std::fs::File::create(stored.join(meta["files"]["session"].as_str().unwrap())).unwrap();
    let mut enc = zstd::Encoder::new(file, 3).unwrap();
    std::io::Write::write_all(&mut enc, opencode_fixture(&repo, other).to_string().as_bytes()).unwrap();
    enc.finish().unwrap();

    let out = b.actx(&["sync"]);
    assert!(out.contains("converted"), "{out}");
    let (title, dir, n) = opencode_sessions(&b)[OC_SID].clone();
    assert_eq!(
        (title.as_str(), dir.as_str(), n),
        ("Fix the login form", slash(&clone).as_str(), 2)
    ); // question + flattened answer
}

#[test]
fn opencode_syncs_automatically() {
    needs!("opencode");
    let w = World::new();
    let (a, b, repo, clone, _) = setup(&w).unwrap();
    let out = b.actx(&["sync", "--auto"]);
    assert!(out.contains("↓") && out.contains("Fix the login form"), "{out}");
    assert_eq!(opencode_sessions(&b)[OC_SID].1, slash(&clone));

    opencode_continue(&a, &repo, "and on Windows?", "Use the Task Manager (desk)");
    assert!(a.actx(&["sync", "--auto"]).contains("↑"));
    // opencode 2 dates an import now, so it would look open: make it an hour old
    let table = if opencode_major() == Some(2) {
        "session_v2"
    } else {
        "session"
    };
    let con = rusqlite::Connection::open(b.opencode_db()).unwrap();
    con.execute(&format!("update {table} set time_updated = time_updated - 3600000"), [])
        .unwrap();
    drop(con);
    let out = b.actx(&["sync", "--auto"]);
    assert!(out.contains("↓"), "{out}");
    assert_eq!(opencode_sessions(&b)[OC_SID].2, 3 + 2);
    assert_eq!(b.actx(&["sync", "--auto"]), "");
}
