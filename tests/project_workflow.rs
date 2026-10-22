//! The ordinary workflow: create here, choose local sessions, sync a differently located clone.
mod common;
use common::*;
use serde_json::Value;
#[cfg(unix)]
use std::process::Stdio;
use std::time::{Duration, SystemTime};

#[test]
fn create_select_sync_and_convert_in_the_matching_clone() {
    needs!("codex");
    needs!("opencode");
    let w = World::new();
    let a = w.machine("desk");
    let b = w.machine("laptop");
    let repo = git_repo(&a.home.join("code/pneuma"));
    let other = git_repo(&a.home.join("code/unrelated"));
    #[cfg(unix)]
    let clone = {
        let clone = git_clone(&repo, &w.dir.path().join("external/a-different-name"));
        std::fs::create_dir_all(b.home.join("workspace")).unwrap();
        std::os::unix::fs::symlink(&clone, b.home.join("workspace/project-link")).unwrap();
        clone
    };
    #[cfg(not(unix))]
    let clone = git_clone(&repo, &b.home.join("workspace/a-different-name"));
    let decoy = git_repo(&b.home.join("workspace/unrelated-repository"));
    std::fs::rename(decoy, b.home.join("workspace/pneuma")).unwrap(); // same name, different Git history
    let source = claude_session(
        &a,
        &a.home,
        &[
            ("user", &format!("work on {}", repo.display())),
            ("assistant", "saved answer"),
        ],
        Some("Project discussion"),
    );
    let unrelated = claude_session(
        &a,
        &other,
        &[("user", "unrelated"), ("assistant", "keep out")],
        Some("Project discussion"),
    );
    std::fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
    let id = source.file_stem().unwrap().to_string_lossy();
    let other_id = unrelated.file_stem().unwrap().to_string_lossy();
    a.actx(&["profile", "--harnesses", "claude"]);
    let out = a.actx_with(
        &["mk", "pneuma"],
        Opts {
            cwd: Some(&repo),
            pick: Some(&[&id, &other_id]),
            ..Default::default()
        },
    );
    assert!(out.contains("Work folder:"), "{out}");
    assert!(w.store().join("pneuma/claude").join(&*id).is_dir());
    assert!(!w.store().join("pneuma/claude").join(&*other_id).exists());
    let meta: Value = serde_json::from_slice(&std::fs::read(w.store().join("pneuma/.project.json")).unwrap()).unwrap();
    assert_eq!(meta["path"].as_str(), repo.to_str());
    assert!(meta["git"]["root"].is_string());
    // Seed a native model choice, without sending any prompt or performing inference.
    let model_folder = b.home.join("workspace/pneuma");
    opencode_import(
        &b,
        &opencode_fixture(&model_folder, opencode_major().unwrap()),
        &model_folder,
    );
    b.actx(&["profile", "--harnesses", "claude,codex,opencode"]);
    let out = b.actx_with(
        &["sync"],
        Opts {
            cwd: Some(&clone),
            ..Default::default()
        },
    );
    assert!(!out.contains("Where does"), "{out}");
    assert_eq!(claude_transcript(&b, &id).2, [clone.to_string_lossy()]);
    let links: Value =
        serde_json::from_slice(&std::fs::read(b.home.join(".local/state/actx/harness-links.json")).unwrap()).unwrap();
    for tool in ["codex", "opencode"] {
        let target = links["copies"][format!("claude:{id}/{tool}")]["target"]
            .as_str()
            .unwrap_or_else(|| panic!("missing {tool}: {out}"));
        let sid = target.split_once(':').unwrap().1;
        if tool == "codex" {
            assert_eq!(codex_threads_from_state(&b)[sid].2, clone.to_string_lossy());
        } else {
            assert_eq!(native(std::path::Path::new(&opencode_sessions(&b)[sid].1)), clone);
        }
    }
    let view = b.actx(&["ls"]);
    assert_eq!(view.matches("Project discussion").count(), 1, "{view}");
    assert!(
        !view.contains("Fix the login form"),
        "unmanaged sessions stay out of the normal view: {view}"
    );
}

#[cfg(unix)]
#[test]
fn normal_view_never_opens_unmanaged_transcripts() {
    let w = World::new();
    let m = w.machine("desk");
    let work = m.home.join("notes");
    std::fs::create_dir_all(&work).unwrap();
    let src = claude_session(
        &m,
        &work,
        &[("user", "tracked"), ("assistant", "reply")],
        Some("Tracked"),
    );
    m.pick(&["add"], &[&src.file_stem().unwrap().to_string_lossy()]);
    let fifo = src.parent().unwrap().join("unmanaged.jsonl");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let mut child = m.command(ACTX).arg("ls").stdout(Stdio::piped()).spawn().unwrap();
    let until = std::time::Instant::now() + Duration::from_secs(3);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() > until {
            let _ = child.kill();
            let _ = child.wait();
            panic!("normal view opened an unmanaged transcript");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("Tracked"));
}
