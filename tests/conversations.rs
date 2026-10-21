//! Project commands, explicit conversation membership, and per-machine General destinations.
mod common;

use common::*;
use serde_json::{Value, json};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

fn conversation(m: &Machine, folder: &Path, title: &str) -> (PathBuf, String) {
    std::fs::create_dir_all(folder).unwrap();
    let path = claude_session(
        m,
        folder,
        &[
            ("user", title),
            ("assistant", &format!("Saved in {}/notes.txt", folder.display())),
        ],
        Some(title),
    );
    let id = path.file_stem().unwrap().to_string_lossy().into_owned();
    (path, id)
}

fn save(m: &Machine, project: &str, id: &str) {
    m.actx(&["mk", project]);
    let out = m.pick(&["add", project], &[id]);
    assert!(out.contains('✓'), "{out}");
}

fn remove(m: &Machine, args: &[&str], ids: &[&str], answer: &str) -> String {
    m.actx_with(
        args,
        Opts {
            pick: Some(ids),
            answer: Some(answer),
            ..Default::default()
        },
    )
}

fn place(m: &Machine, folder: &Path) -> String {
    std::fs::create_dir_all(folder).unwrap();
    m.actx_with(
        &["sync", "--general"],
        Opts {
            pick: Some(&["::other"]),
            answer: Some(folder.to_str().unwrap()),
            ..Default::default()
        },
    )
}

#[test]
fn mk_creates_only_a_project_and_add_requires_an_existing_project() {
    let w = World::new();
    let a = w.machine("desk");
    let (_file, id) = conversation(&a, &a.home.join("notes"), "Planning");
    let out = a.pick(&["mk", "ideas"], &[&id]);
    assert!(out.contains("Project ideas created"), "{out}");
    assert!(files_under(&w.store().join("ideas")).is_empty());
    let out = a.pick(&["add", "missing"], &[&id]);
    assert!(out.contains("doesn't exist"), "{out}");
    assert!(!w.store().join("missing").exists());
    assert!(a.pick(&["add", "ideas"], &[&id]).contains("Planning"));
    assert!(w.store().join("ideas/claude").join(&id).join("meta.json").exists());
    assert!(a.actx(&["mk", "_general"]).contains("Invalid name"));
}

#[test]
fn general_destinations_are_explicit_independent_and_remembered() {
    let w = World::new();
    let a = w.machine("desk");
    let b = w.machine("laptop");
    let folder = a.home.join("notes");
    let (first_source, first) = conversation(&a, &folder, "First notes");
    a.pick(&["add"], &[&first]);
    let meta: Value = serde_json::from_slice(
        &std::fs::read(w.store().join("_general/claude").join(&first).join("meta.json")).unwrap(),
    )
    .unwrap();
    assert!(meta["git"].is_null());
    assert!(a.actx(&["ls", "--general"]).contains("General (no project)"));
    // Even a matching source-relative folder must not silently become the destination.
    std::fs::create_dir_all(b.home.join("notes")).unwrap();
    assert!(b.actx(&["sync", "--auto"]).contains("needs a destination folder"));
    assert!(find(&b.claude_dir(), &format!("{first}.jsonl")).is_none());
    assert!(b.actx(&["sync", "--general"]).contains("skipping"));
    let one = b.home.join("personal").join("first notes");
    let out = place(&b, &one);
    assert!(out.contains('↓'), "{out}");
    assert_eq!(claude_transcript(&b, &first).2, [one.to_string_lossy()]);
    assert!(
        claude_transcript(&b, &first)
            .1
            .contains(&format!("Saved in {}/notes.txt", one.display()))
    );
    let (_source, second) = conversation(&a, &folder, "Second notes");
    a.pick(&["add", "--general"], &[&second]);
    assert!(b.actx(&["sync", "--auto"]).contains("needs a destination folder"));
    assert!(find(&b.claude_dir(), &format!("{second}.jsonl")).is_none());
    let two = b.home.join("other").join("second notes");
    let out = place(&b, &two);
    assert!(out.contains('↓'), "{out}");
    assert_eq!(claude_transcript(&b, &second).2, [two.to_string_lossy()]);
    claude_continue(
        &claude_transcript(&b, &first).0,
        &[("user", "continue"), ("assistant", "continued on laptop")],
        None,
    );
    assert!(b.actx(&["sync", "--auto"]).contains('↑'));
    // The fixture has just been written; simulate its agent having closed an hour ago.
    std::fs::File::options()
        .write(true)
        .open(&first_source)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
    assert!(a.actx(&["sync", "--auto"]).contains('↓'));
    assert!(
        claude_transcript(&a, &first)
            .1
            .contains(&"continued on laptop".to_string())
    );
    let state: Value =
        serde_json::from_slice(&std::fs::read(b.home.join(".local/state/actx/state.json")).unwrap()).unwrap();
    assert_eq!(
        state["folders"][format!("session:claude:{first}")],
        one.to_string_lossy().as_ref()
    );
    assert_eq!(
        state["folders"][format!("session:claude:{second}")],
        two.to_string_lossy().as_ref()
    );
    std::fs::remove_file(claude_transcript(&b, &first).0).unwrap();
    assert!(b.actx(&["sync", "--auto"]).contains('↓'));
    assert_eq!(claude_transcript(&b, &first).2, [one.to_string_lossy()]);
}

#[test]
fn delete_preserves_projects_and_originals_and_can_be_cancelled() {
    let w = World::new();
    let a = w.machine("desk");
    let (file, id) = conversation(&a, &a.home.join("notes"), "Keep original");
    let before = std::fs::read(&file).unwrap();
    save(&a, "ideas", &id);
    let stored = w.store().join("ideas/claude").join(&id);
    remove(&a, &["delete", "ideas"], &[&id], "n");
    assert!(stored.exists());
    let out = remove(&a, &["delete", "ideas"], &[&id], "y");
    assert!(out.contains("Removed 1"), "{out}");
    assert!(!stored.exists());
    assert!(w.store().join("ideas").is_dir());
    assert_eq!(std::fs::read(&file).unwrap(), before);
    remove(&a, &["rm", "ideas"], &[], "n");
    assert!(w.store().join("ideas").exists());
    remove(&a, &["rm", "ideas"], &[], "y");
    assert!(!w.store().join("ideas").exists());
    assert_eq!(std::fs::read(&file).unwrap(), before);
}

#[test]
fn membership_can_move_and_delete_filters_do_not_remove_other_scopes() {
    let w = World::new();
    let a = w.machine("desk");
    let (file, id) = conversation(&a, &a.home.join("notes"), "Move me");
    save(&a, "ideas", &id);
    let out = a.pick(&["add"], &[&id]);
    assert!(out.contains("moved from ideas"), "{out}");
    assert!(!w.store().join("ideas/claude").join(&id).exists());
    assert!(w.store().join("_general/claude").join(&id).exists());
    let (_, other) = conversation(&a, &a.home.join("notes"), "Project notes");
    save(&a, "ideas", &other);
    remove(&a, &["delete", "--general"], &[&other], "y");
    assert!(w.store().join("ideas/claude").join(&other).exists());
    remove(&a, &["delete", "ideas"], &[&id], "y");
    assert!(w.store().join("_general/claude").join(&id).exists());
    let out = a.pick(&["add", "ideas"], &[&id]);
    assert!(out.contains("moved from General"), "{out}");
    assert!(!w.store().join("_general/claude").join(&id).exists());
    assert!(w.store().join("ideas/claude").join(&id).exists());
    a.pick(&["add"], &[&id]);
    let out = remove(&a, &["delete"], &[&id, &other], "y");
    assert!(out.contains("Removed 2"), "{out}");
    assert!(file.exists());
    assert!(w.store().join("ideas").is_dir());
}

/// Let the command reach the lock with its original snapshot, change the store, then release it.
fn while_waiting(m: &Machine, args: &[&str], ids: &[&str], mutate: impl FnOnce()) -> String {
    let lock = m.store.join(".lock");
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    std::fs::write(
        &lock,
        json!({"host":"elsewhere","pid":1,"since":now,"at":now,"token":"test"}).to_string(),
    )
    .unwrap();
    let mut child = m
        .command(ACTX)
        .args(args)
        .env("ACTX_PICK", ids.join(","))
        .env("ACTX_ANSWER", "y")
        .env("ACTX_LOCK_WAIT", "20")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let mut out = String::new();
    loop {
        let line = match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(line) => line,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("didn't reach lock: {e}: {out}");
            }
        };
        out.push_str(&line);
        out.push('\n');
        if line.contains("waiting") {
            break;
        }
    }
    mutate();
    std::fs::remove_file(lock).unwrap();
    for line in rx {
        out.push_str(&line);
        out.push('\n');
    }
    assert!(child.wait().unwrap().success(), "{out}");
    out
}

#[test]
fn delete_and_rm_recheck_changes_after_waiting_for_the_lock() {
    for cmd in ["delete", "rm"] {
        let w = World::new();
        let a = w.machine("desk");
        let (_, id) = conversation(&a, &a.home.join("notes"), "Changed elsewhere");
        save(&a, "ideas", &id);
        let meta = w.store().join("ideas/claude").join(&id).join("meta.json");
        let out = while_waiting(&a, &[cmd, "ideas"], &[&id], || {
            let mut v: Value = serde_json::from_slice(&std::fs::read(&meta).unwrap()).unwrap();
            v["pushed_at"] = json!(v["pushed_at"].as_f64().unwrap() + 1.0);
            std::fs::write(&meta, serde_json::to_vec(&v).unwrap()).unwrap();
        });
        assert!(out.contains("changed"), "{out}");
        assert!(meta.exists());
    }
}

#[test]
fn sync_does_not_resurrect_a_conversation_deleted_while_it_waited() {
    let w = World::new();
    let a = w.machine("desk");
    let b = w.machine("laptop");
    let repo = git_repo(&a.home.join("code/ideas"));
    git_clone(&repo, &b.home.join("code/ideas"));
    let (_, id) = conversation(&a, &repo, "Deleted elsewhere");
    save(&a, "ideas", &id);
    b.actx(&["sync"]);
    claude_continue(
        &claude_transcript(&b, &id).0,
        &[("user", "new message"), ("assistant", "local reply")],
        None,
    );
    let stored = w.store().join("ideas/claude").join(&id);
    let out = while_waiting(&b, &["sync", "--all"], &[], || {
        std::fs::remove_dir_all(&stored).unwrap()
    });
    assert!(out.contains("changed in the shared folder meanwhile"), "{out}");
    assert!(!stored.exists());
    assert!(claude_transcript(&b, &id).1.contains(&"local reply".to_string()));
}

#[test]
fn rm_removes_the_whole_project_but_leaves_general_and_all_local_originals() {
    let w = World::new();
    let a = w.machine("desk");
    let (one, id_one) = conversation(&a, &a.home.join("notes"), "One");
    let (two, id_two) = conversation(&a, &a.home.join("notes"), "Two");
    let (general, id_general) = conversation(&a, &a.home.join("notes"), "General note");
    save(&a, "ideas", &id_one);
    a.pick(&["add", "ideas"], &[&id_two]);
    a.pick(&["add"], &[&id_general]);
    // No conversations are selected: rm must remove the entire project anyway.
    let out = remove(&a, &["rm", "ideas"], &[], "y");
    assert!(out.contains("Project ideas deleted"), "{out}");
    assert!(!w.store().join("ideas").exists());
    assert!(w.store().join("_general/claude").join(id_general).exists());
    assert!(one.exists() && two.exists() && general.exists());
}

#[test]
fn add_does_not_recreate_a_project_deleted_while_it_waited() {
    let w = World::new();
    let a = w.machine("desk");
    let (source, id) = conversation(&a, &a.home.join("notes"), "Pending add");
    a.actx(&["mk", "ideas"]);
    let project = w.store().join("ideas");
    let out = while_waiting(&a, &["add", "ideas"], &[&id], || std::fs::remove_dir(&project).unwrap());
    assert!(out.contains("doesn't exist"), "{out}");
    assert!(!project.exists());
    assert!(source.exists());
}

#[test]
fn cv_converts_without_init_git_or_original_folder_and_rewrites_paths() {
    let w = World::new();
    let mut m = w.machine("solo");
    // No shared folder configured at all (`actx init` was never run).
    m.store = m.home.join("unconfigured-store");

    let work = m.home.join("work").join("notes");
    std::fs::create_dir_all(&work).unwrap();
    let agy_home = m.home.join(".gemini/antigravity-cli");
    std::fs::create_dir_all(&agy_home).unwrap();

    // 1. A Claude conversation whose original folder is from another OS and doesn't exist here.
    let foreign = if cfg!(windows) {
        "/home/alice/old_notes"
    } else {
        r"C:\Users\alice\old_notes"
    };
    let foreign_sep = if cfg!(windows) { '/' } else { '\\' };
    let claude_dir = m.claude_dir().join("default");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let claude_id = "11111111-1111-1111-1111-111111111111";
    let claude_file = claude_dir.join(format!("{claude_id}.jsonl"));
    let u1 = json!({
        "type": "user", "uuid": "u1", "parentUuid": null, "cwd": foreign,
        "timestamp": "2026-01-01T10:00:00.000Z",
        "message": {"role": "user", "content": "where is the draft?"}
    });
    let a1 = json!({
        "type": "assistant", "uuid": "a1", "parentUuid": "u1", "cwd": foreign,
        "timestamp": "2026-01-01T10:00:05.000Z",
        "message": {"role": "assistant", "content": [{"type": "text", "text": format!("In {foreign}{foreign_sep}draft.md")}]}
    });
    std::fs::write(&claude_file, format!("{u1}\n{a1}\n")).unwrap();

    // Cancelling or picking a non-existent folder fails cleanly and keeps the original.
    let bad = m.actx_with(
        &["cv"],
        Opts {
            pick: Some(&[claude_id, "antigravity", "::other"]),
            answer: Some(m.home.join("no-such-folder").to_str().unwrap()),
            ..Default::default()
        },
    );
    assert!(bad.contains("doesn't exist"), "{bad}");
    assert!(claude_file.exists());

    // Picking an existing non-git working folder succeeds, rewrites the foreign path, and never creates a store.
    let out = m.actx_with(
        &["cv"],
        Opts {
            pick: Some(&[claude_id, "antigravity", "::other"]),
            answer: Some(work.to_str().unwrap()),
            ..Default::default()
        },
    );
    assert!(out.contains("agy --conversation"), "{out}");
    assert!(out.contains("Note: tool calls become text"), "{out}");
    assert!(!m.store.exists(), "cv must not create or touch a shared store");
    assert!(claude_file.exists(), "original conversation must remain intact");

    // 2. A Codex rollout with an empty cwd (`""`), converted to Claude Code in `work`.
    let codex_id = "01999999-1111-7111-8111-111111111111";
    let codex_dir = m.codex_sessions().join("2026/01/01");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let rollout = codex_dir.join(format!("rollout-2026-01-01T10-00-00-{codex_id}.jsonl"));
    let meta = json!({"timestamp": "2026-01-01T10:00:00.000Z", "type": "session_meta", "payload": {"id": codex_id, "cwd": ""}});
    let user = json!({"timestamp": "2026-01-01T10:00:01.000Z", "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "folderless codex"}]}});
    let reply = json!({"timestamp": "2026-01-01T10:00:02.000Z", "type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "codex answer"}]}});
    std::fs::write(&rollout, format!("{meta}\n{user}\n{reply}\n")).unwrap();

    let out = m.actx_with(
        &["cv"],
        Opts {
            pick: Some(&[codex_id, "claude", "::other"]),
            answer: Some(work.to_str().unwrap()),
            ..Default::default()
        },
    );
    assert!(out.contains("folderless codex (from Codex)"), "{out}");
    let converted = files_under(&m.claude_dir())
        .into_iter()
        .find(|p| p != &claude_file && p.extension().is_some_and(|e| e == "jsonl"))
        .expect("converted Claude session");
    let stem = converted.file_stem().unwrap().to_string_lossy();
    let (_, texts, cwds) = claude_transcript(&m, &stem);
    assert_eq!(cwds, [work.to_string_lossy()]);
    assert_eq!(texts.last().unwrap(), "codex answer");

    // And the Antigravity copy created in step 1 has the rewritten path when converted back to Claude.
    let agy_id = std::fs::read_dir(agy_home.join("conversations"))
        .unwrap()
        .find_map(|e| {
            let p = e.unwrap().path();
            (p.extension().and_then(|x| x.to_str()) == Some("db"))
                .then(|| p.file_stem().unwrap().to_string_lossy().into_owned())
        })
        .unwrap();
    let out = m.pick(&["cv"], &[&agy_id, "claude"]);
    assert!(out.contains("(from Antigravity)"), "{out}");
    let expected_draft = native(&work.join("draft.md"));
    let all_claude: Vec<String> = files_under(&m.claude_dir())
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();
    assert!(
        all_claude
            .iter()
            .any(|c| c.contains(&expected_draft.to_string_lossy().replace('\\', "\\\\"))),
        "{all_claude:?}"
    );
}

#[test]
fn folderless_claude_converts_without_configuration_and_cancel_keeps_original() {
    let w = World::new();
    let mut m = w.machine("solo");
    m.store = PathBuf::new(); // ACTX_ROOT is empty, with no config file or init.
    let dir = m.claude_dir().join("default");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(m.home.join(".gemini/antigravity-cli")).unwrap();
    let id = "22222222-2222-4222-8222-222222222222";
    let file = dir.join(format!("{id}.jsonl"));
    let original = format!(
        "{}\n{}\n",
        json!({"type":"user", "uuid":"question", "parentUuid":null,
        "message":{"role":"user", "content":"No working directory"}}),
        json!({"type":"assistant", "uuid":"answer", "parentUuid":"question",
        "message":{"role":"assistant", "content":[{"type":"text", "text":"Hello"}]}})
    );
    std::fs::write(&file, &original).unwrap();
    let out = m.pick(&["cv"], &[id, "antigravity"]);
    assert!(out.contains("I don't know which folder"), "{out}");
    assert!(!m.home.join(".gemini/antigravity-cli/conversations").exists());
    let out = m.actx_with(
        &["cv"],
        Opts {
            pick: Some(&[id, "antigravity", "::other"]),
            answer: Some(m.home.to_str().unwrap()),
            ..Default::default()
        },
    );
    assert!(out.contains("agy --conversation"), "{out}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
    assert!(!m.home.join(".config/actx/config.json").exists());
    assert!(std::fs::read_dir(w.store()).unwrap().next().is_none());
}
