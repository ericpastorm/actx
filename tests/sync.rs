//! Syncing Claude Code conversations between fake machines with different homes and repo paths.

mod common;

use common::*;

fn setup(w: &World) -> (Machine, Machine, std::path::PathBuf, std::path::PathBuf, String) {
    let (a, b) = (w.machine("desk"), w.machine("laptop"));
    let repo = git_repo(&a.home.join("code/shop"));
    let clone = git_clone(&repo, &b.home.join("work/shop-laptop"));
    let session = claude_session(
        &a,
        &repo,
        &[
            ("user", "fix the cart"),
            ("assistant", &format!("Done in {}/cart.ts", repo.display())),
        ],
        Some("Cart fix"),
    );
    let sid = session.file_stem().unwrap().to_string_lossy().into_owned();
    let out = a.pick(&["mk", "shop"], &[&sid]);
    assert!(
        out.contains("Project shop created") && out.contains("Cart fix"),
        "{out}"
    );
    (a, b, repo, clone, sid)
}

#[test]
fn roundtrip_between_different_paths() {
    let w = World::new();
    let (a, b, repo, clone, sid) = setup(&w);

    let out = b.actx(&["sync"]); // no terminal: all projects
    assert!(out.contains("Cart fix") && out.contains("~/work/shop-laptop"), "{out}");
    let (path, texts, cwds) = claude_transcript(&b, &sid);
    assert_eq!(
        path.parent().unwrap().file_name().unwrap().to_string_lossy(),
        encode_claude(&clone)
    );
    assert_eq!(cwds, [clone.to_string_lossy()]);
    assert!(texts.contains(&format!("Done in {}/cart.ts", clone.display())));
    assert!(b.actx(&["ls"]).contains("✓ synced"));

    claude_continue(
        &path,
        &[
            ("user", "add coupons"),
            ("assistant", &format!("Coupons in {}/coupons.ts", clone.display())),
        ],
        None,
    );
    assert!(b.actx(&["ls"]).contains("↑ push"));
    assert!(b.actx(&["sync"]).contains('↑'));

    assert!(a.actx(&["ls"]).contains("↓ pull"));
    a.actx(&["sync"]);
    let (_, texts, cwds) = claude_transcript(&a, &sid);
    assert_eq!(cwds, [repo.to_string_lossy()]);
    assert!(texts.contains(&format!("Coupons in {}/coupons.ts", repo.display())));
    assert!(!a.actx(&["ls"]).contains("pending") && !b.actx(&["ls"]).contains("pending"));
}

#[test]
fn conflict_merge_keeps_both_sides() {
    let w = World::new();
    let (a, b, _repo, _clone, sid) = setup(&w);
    b.actx(&["sync"]);
    claude_continue(
        &claude_transcript(&a, &sid).0,
        &[("user", "coupons"), ("assistant", "Coupons done on desk")],
        None,
    );
    claude_continue(
        &claude_transcript(&b, &sid).0,
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
    let (_, texts, _) = claude_transcript(&a, &sid);
    assert!(texts.contains(&"Coupons done on desk".to_string()));
    let summary = texts
        .iter()
        .find(|t| t.starts_with("[Meanwhile"))
        .expect("the merge note");
    assert!(
        summary.contains("green button") && summary.contains("Button green on laptop") && !summary.contains("Coupons")
    );
    assert!(texts.last().unwrap().starts_with("Got it"));

    b.actx(&["sync"]); // the laptop gets the merged version and the branch
    let (_, texts, _) = claude_transcript(&b, &sid);
    assert!(texts.iter().any(|t| t.starts_with("[Meanwhile")));
    let listing = b.actx(&["ls"]);
    assert!(listing.contains("Cart fix (branch from laptop)"), "{listing}");
    assert!(!listing.contains("pending") && !a.actx(&["ls"]).contains("pending"));
}

#[test]
fn running_inside_the_project_places_it_there() {
    let w = World::new();
    let (_a, _b, repo, _clone, sid) = setup(&w);
    let c = w.machine("third");
    let one = git_clone(&repo, &c.home.join("old/shop-a"));
    let two = git_clone(&repo, &c.home.join("ide/workspaces/shop/main")); // e.g. an IDE worktree

    assert!(c.actx(&["sync"]).contains("skipping")); // two clones, same branch: ambiguous, and no terminal to ask
    c.actx_with(
        &["sync"],
        Opts {
            cwd: Some(&two),
            ..Default::default()
        },
    );
    let (path, _, cwds) = claude_transcript(&c, &sid);
    assert_eq!(cwds, [two.to_string_lossy()]);
    assert!(!path.to_string_lossy().contains(&encode_claude(&one)));
}

#[test]
fn sync_one_project_leaves_the_rest_pending() {
    let w = World::new();
    let (a, b, _repo, _clone, _sid) = setup(&w);
    let notes = git_repo(&a.home.join("code/notes"));
    let other = claude_session(
        &a,
        &notes,
        &[("user", "sort notes"), ("assistant", "sorted")],
        Some("Notes"),
    );
    a.pick(&["mk", "notes"], &[&other.file_stem().unwrap().to_string_lossy()]);
    let out = b.actx(&["sync", "shop"]);
    assert!(
        out.contains("Cart fix") && !out.contains("Notes") && out.contains("1 pending in projects you didn't pick"),
        "{out}"
    );
    assert!(b.actx(&["sync", "--all"]).contains("Notes"));
}

#[test]
fn rm_removes_from_the_store_only() {
    let w = World::new();
    let (a, _b, _repo, _clone, sid) = setup(&w);
    let out = a.actx_with(
        &["rm", "shop"],
        Opts {
            pick: Some(&[&sid]),
            answer: Some("y"),
            ..Default::default()
        },
    );
    assert!(out.contains("Project shop deleted"), "{out}");
    assert!(!w.store().join("shop").exists());
    assert!(find(&a.claude_dir(), &format!("{sid}.jsonl")).is_some()); // still on the machine
}

#[test]
fn spanish_interface() {
    let w = World::new();
    let mut a = w.machine("desk");
    a.lang = "es".into();
    assert!(a.actx(&[]).contains("Aún no hay proyectos"));
}

#[test]
fn reads_what_the_python_version_stored() {
    // a conversation stored by actx 0.2 (Python): same layout, gzip instead of zstd on Python < 3.14
    let w = World::new();
    let (a, b) = (w.machine("desk"), w.machine("laptop"));
    let repo = git_repo(&a.home.join("code/old"));
    let session = claude_session(
        &a,
        &repo,
        &[("user", "hello from python"), ("assistant", "hi")],
        Some("Old one"),
    );
    let sid = session.file_stem().unwrap().to_string_lossy().into_owned();
    let dir = w.store().join("old/claude").join(&sid);
    std::fs::create_dir_all(&dir).unwrap();
    let mut gz = flate2::write::GzEncoder::new(
        std::fs::File::create(dir.join("session.jsonl.gz")).unwrap(),
        flate2::Compression::default(),
    );
    std::io::copy(&mut std::fs::File::open(&session).unwrap(), &mut gz).unwrap();
    gz.finish().unwrap();
    let size = std::fs::metadata(&session).unwrap().len();
    let meta = serde_json::json!({
        "v": 2, "harness": "claude", "id": sid, "title": "Old one", "path": repo, "rel": "code/old",
        "home": a.home, "host": "desk", "pushed_at": 1790000000.5, "updated": 1790000000.25, "weight": size,
        "files": {"session": "session.jsonl.gz"}, "git": null,
    });
    std::fs::write(dir.join("meta.json"), serde_json::to_string_pretty(&meta).unwrap()).unwrap();
    std::fs::write(dir.join("preview.txt"), "▌ tú\nhello from python\n").unwrap();
    git_clone(&repo, &b.home.join("code/old"));
    let out = b.actx(&["sync"]);
    assert!(out.contains("Old one"), "{out}");
    assert!(claude_transcript(&b, &sid).1.contains(&"hello from python".to_string()));
}
