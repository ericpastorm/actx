//! Reorganizing saved conversations preserves snapshots, linked copies and native histories.
mod common;

use common::*;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

fn source(m: &Machine, folder: &Path, title: &str) -> String {
    std::fs::create_dir_all(folder).unwrap();
    claude_session(m, folder, &[("user", title), ("assistant", "Answer")], Some(title))
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}
fn bytes(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    files_under(dir)
        .into_iter()
        .map(|p| {
            (
                p.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/"),
                std::fs::read(&p).unwrap(),
            )
        })
        .collect()
}
fn run(m: &Machine, args: &[&str], cwd: &Path, picks: &[&str], answer: &str) -> String {
    m.actx_with(
        args,
        Opts {
            cwd: Some(cwd),
            pick: Some(picks),
            answer: Some(answer),
        },
    )
}
fn state(m: &Machine) -> Value {
    serde_json::from_slice(&std::fs::read(m.home.join(".local/state/actx/state.json")).unwrap()).unwrap()
}

#[test]
fn moving_a_family_preserves_shared_snapshots_and_unsynced_native_edits() {
    let w = World::new();
    let a = w.machine("desk");
    let id = source(&a, &a.home.join("notes"), "Linked conversation");
    a.pick(&["add", "--general"], &[&id]);
    a.actx(&["mk", "pneuma"]);
    let old = w.store().join("_general/claude").join(&id);
    let linked = w.store().join("_general/opencode/ses_linked");
    std::fs::create_dir_all(&linked).unwrap();
    std::fs::write(
        linked.join("meta.json"),
        json!({
            "harness":"opencode", "id":"ses_linked", "title":"Imported title", "path":"", "host":"desk",
            "weight":2, "updated":1_792_000_000.0, "pushed_at":1_792_000_000.0, "files":{},
            "conversation_id":format!("claude:{id}")
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(linked.join("extra.bin"), b"Keep unknown saved files too").unwrap();
    let original = claude_transcript(&a, &id).0;
    claude_continue(
        &original,
        &[("user", "Unsynced edit"), ("assistant", "Local only")],
        None,
    );
    let native = std::fs::read(&original).unwrap();
    let modified = std::fs::metadata(&original).unwrap().modified().unwrap();
    let stored = bytes(&old);
    let linked_stored = bytes(&linked);
    let baseline = state(&a);

    run(&a, &["mv", "pneuma"], &a.home, &[&id], "n");
    assert!(old.exists() && linked.exists());
    assert!(!w.store().join("pneuma/claude").exists());
    let out = run(&a, &["mv", "pneuma"], &a.home, &[&id], "y");
    assert!(out.contains("Moved 1 conversation(s) to pneuma"), "{out}");
    assert!(!old.exists() && !linked.exists());
    assert_eq!(bytes(&w.store().join("pneuma/claude").join(&id)), stored);
    assert_eq!(bytes(&w.store().join("pneuma/opencode/ses_linked")), linked_stored);
    assert_eq!(std::fs::read(&original).unwrap(), native);
    assert_eq!(std::fs::metadata(&original).unwrap().modified().unwrap(), modified);
    assert_eq!(state(&a)["base"], baseline["base"]);
    assert_eq!(state(&a)["content"], baseline["content"]);
    let out = run(&a, &["mv", "pneuma"], &a.home, &[&id], "y");
    assert!(out.contains("already in pneuma"), "{out}");
}

#[test]
fn move_scopes_to_the_current_project_and_can_choose_or_create_a_destination() {
    let w = World::new();
    let a = w.machine("desk");
    let one = a.home.join("one");
    let two = a.home.join("two");
    let id_one = source(&a, &one, "One");
    let id_two = source(&a, &two, "Two");
    run(&a, &["mk", "first"], &one, &[&id_one], "");
    run(&a, &["mk", "second"], &two, &[&id_two], "");
    let out = run(&a, &["mv", "--general"], &one, &[&id_one, &id_two], "y");
    assert!(out.contains("Moved 1 conversation(s)"), "{out}");
    assert!(w.store().join("_general/claude").join(&id_one).exists());
    assert!(w.store().join("second/claude").join(&id_two).exists());
    assert_eq!(
        state(&a)["folders"][format!("session:claude:{id_one}")],
        one.to_string_lossy().as_ref()
    );

    // The current first project is empty. --all deliberately reaches other saved collections.
    let out = run(&a, &["mv", "second"], &one, &[&id_one], "y");
    assert!(out.contains("use actx mv --all"), "{out}");
    let out = run(&a, &["mv", "--all"], &one, &[&id_one, "second"], "y");
    assert!(out.contains("Moved 1 conversation(s) to second"), "{out}");

    // New-project creation is deferred until confirmation, including cancellation.
    let out = run(&a, &["mv", "--all"], &a.home, &[&id_one, "::new"], "cancelled-project");
    assert!(out.contains("cancelled-project"), "{out}");
    assert!(!w.store().join("cancelled-project").exists());
    // The scripted answer supplies both the valid project name and the confirmation.
    let out = run(&a, &["mv", "--all"], &a.home, &[&id_one, "::new"], "yes");
    assert!(out.contains("Moved 1 conversation(s) to yes"), "{out}");
    assert!(w.store().join("yes/claude").join(&id_one).exists());
    let out = run(&a, &["mv", "missing"], &a.home, &[&id_two], "y");
    assert!(out.contains("doesn't exist"), "{out}");
    assert!(!w.store().join("missing").exists());
}

#[test]
fn project_renames_preserve_non_git_folder_bindings_on_both_computers() {
    let w = World::new();
    let a = w.machine("desk");
    let b = w.machine("laptop");
    let work = a.home.join("original-notes");
    let local = b.home.join("different-notes");
    std::fs::create_dir_all(&local).unwrap();
    let id = source(&a, &work, "Portable notes");
    run(&a, &["mk", "old"], &work, &[&id], "");
    b.actx_with(
        &["mk", "old"],
        Opts {
            cwd: Some(&local),
            ..Default::default()
        },
    );
    let stored = bytes(&w.store().join("old"));
    run(&a, &["mv", "--project", "old", "new"], &work, &[], "n");
    assert_eq!(bytes(&w.store().join("old")), stored);
    let out = run(&a, &["mv", "--project", "old", "new"], &work, &[], "y");
    assert!(out.contains("Project renamed: old → new"), "{out}");
    assert!(!w.store().join("old").exists());
    let mut renamed = bytes(&w.store().join("new"));
    renamed.remove(".renamed-from.json");
    assert_eq!(renamed, stored);
    assert_eq!(state(&a)["folders"]["project:new"], work.to_string_lossy().as_ref());
    assert!(state(&a)["folders"].get("project:old").is_none());
    let listing: Value = serde_json::from_str(&b.actx(&["ls", "new", "--json"])).unwrap();
    assert_eq!(
        listing["data"]["projects"][0]["local_folder"],
        local.to_string_lossy().as_ref()
    );
    let out = b.actx(&["sync", "new", "--auto"]);
    assert!(out.contains('↓'), "{out}");
    assert_eq!(claude_transcript(&b, &id).2, [local.to_string_lossy()]);

    // A second rename remains discoverable from a locally bound subfolder without Git.
    run(&a, &["mv", "--project", "new", "final"], &work, &[], "y");
    let subdir = local.join("drafts");
    let new_id = source(&b, &subdir, "Continued on laptop");
    let out = run(&b, &["add"], &subdir, &[&new_id], "");
    assert!(out.contains("Continued on laptop"), "{out}");
    assert!(w.store().join("final/claude").join(new_id).exists());
    assert!(!w.store().join("old").exists() && !w.store().join("new").exists());
    let out = run(&a, &["mv", "--project", "final", "../escape"], &work, &[], "y");
    assert!(out.contains("Invalid name"), "{out}");
    let out = run(&a, &["mv", "--project", "final", "FINAL"], &work, &[], "y");
    assert!(out.contains("already exists"), "{out}");
    assert!(w.store().join("final").is_dir());
}
