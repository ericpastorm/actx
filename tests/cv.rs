//! Conversion discovery follows the working folder without changing source histories.
mod common;

use common::*;
use std::path::{Path, PathBuf};

fn source(m: &Machine, folder: &Path, title: &str) -> (PathBuf, String, Vec<u8>) {
    std::fs::create_dir_all(folder).unwrap();
    let file = claude_session(m, folder, &[("user", title), ("assistant", "Answer")], Some(title));
    let id = file.file_stem().unwrap().to_string_lossy().into_owned();
    let bytes = std::fs::read(&file).unwrap();
    (file, id, bytes)
}

fn enable_target(m: &Machine) {
    std::fs::create_dir_all(m.home.join(".gemini/antigravity-cli")).unwrap();
}

#[test]
fn cv_defaults_to_this_folder_and_children_with_an_explicit_global_escape() {
    let w = World::new();
    let mut m = w.machine("solo");
    m.store = PathBuf::new();
    enable_target(&m);
    let folder = m.home.join("notes");
    let (file, id, bytes) = source(&m, &folder, "This folder");
    let (child, child_id, child_bytes) = source(&m, &folder.join("drafts"), "Subfolder");
    let (other, other_id, other_bytes) = source(&m, &m.home.join("notes-other"), "Other folder");
    let empty = m.home.join("empty");
    std::fs::create_dir_all(&empty).unwrap();

    let out = m.actx_with(
        &["cv"],
        Opts {
            cwd: Some(&empty),
            pick: Some(&[&other_id, "antigravity"]),
            ..Default::default()
        },
    );
    assert!(out.contains("No conversations found in"), "{out}");
    assert!(out.contains("actx cv --all"), "{out}");

    let out = m.actx_with(
        &["cv"],
        Opts {
            cwd: Some(&folder),
            pick: Some(&[&other_id, "antigravity"]),
            ..Default::default()
        },
    );
    assert!(
        !out.contains("agy --conversation"),
        "a sibling with the same path prefix must stay hidden: {out}"
    );
    assert!(!m.home.join(".gemini/antigravity-cli/conversations").exists());

    let out = m.actx_with(
        &["cv"],
        Opts {
            cwd: Some(&folder),
            pick: Some(&[&id, &child_id, "antigravity"]),
            ..Default::default()
        },
    );
    assert_eq!(out.matches("agy --conversation").count(), 2, "{out}");

    let out = m.actx_with(
        &["cv", "--all"],
        Opts {
            cwd: Some(&empty),
            pick: Some(&[&other_id, "antigravity"]),
            ..Default::default()
        },
    );
    assert!(out.contains("Other folder (from Claude Code)"), "{out}");
    for (file, bytes) in [(file, bytes), (child, child_bytes), (other, other_bytes)] {
        assert_eq!(std::fs::read(file).unwrap(), bytes);
    }
    assert!(std::fs::read_dir(w.store()).unwrap().next().is_none());
    assert!(!m.home.join(".config/actx/config.json").exists());
}

#[test]
fn cv_from_a_git_subdirectory_finds_the_repository_conversations() {
    let w = World::new();
    let mut m = w.machine("solo");
    m.store = PathBuf::new();
    enable_target(&m);
    let repo = git_repo(&m.home.join("repo"));
    let (file, id, bytes) = source(&m, &repo, "Repository root");
    let subdir = repo.join("src");
    std::fs::create_dir_all(&subdir).unwrap();
    let (_, unrelated, _) = source(&m, &m.home.join("another-repo"), "Unrelated");
    let out = m.actx_with(
        &["cv"],
        Opts {
            cwd: Some(&subdir),
            pick: Some(&[&id, &unrelated, "antigravity"]),
            ..Default::default()
        },
    );
    assert!(out.contains("Repository root (from Claude Code)"), "{out}");
    assert_eq!(out.matches("agy --conversation").count(), 1, "{out}");
    assert_eq!(std::fs::read(file).unwrap(), bytes);
}

#[test]
fn cv_finds_shared_only_conversations_through_the_local_project_binding() {
    let w = World::new();
    let a = w.machine("desk");
    let b = w.machine("laptop");
    enable_target(&b);
    let (file, id, bytes) = source(&a, &a.home.join("original-path"), "Shared notes");
    let out = a.actx_with(
        &["mk", "notes"],
        Opts {
            cwd: Some(&a.home.join("original-path")),
            pick: Some(&[&id]),
            ..Default::default()
        },
    );
    assert!(out.contains("Shared notes"), "{out}");
    assert!(w.store().join("notes/claude").join(&id).join("meta.json").is_file());
    let folder = b.home.join("different-path");
    let subdir = folder.join("drafts");
    std::fs::create_dir_all(&subdir).unwrap();
    b.actx_with(
        &["mk", "notes"],
        Opts {
            cwd: Some(&folder),
            ..Default::default()
        },
    );
    let out = b.actx_with(
        &["cv"],
        Opts {
            cwd: Some(&subdir),
            pick: Some(&[&id, "antigravity"]),
            ..Default::default()
        },
    );
    assert!(out.contains("Shared notes (from Claude Code)"), "{out}");
    assert!(
        out.contains("different-path"),
        "must use the local project folder: {out}"
    );
    assert!(out.contains("agy --conversation"), "{out}");
    assert_eq!(std::fs::read(file).unwrap(), bytes);
    assert!(find(&b.claude_dir(), &format!("{id}.jsonl")).is_none());
}
