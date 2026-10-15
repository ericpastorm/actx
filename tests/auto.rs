//! Automatic sync (`sync --auto`, the hooks, `actx auto`), the shared folder's lock, backups and
//! restore, doctor.

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use common::*;
use serde_json::{Value, json};

/// Pretend nobody touched `path` for an hour (a conversation written seconds ago may still be open).
fn age(path: &Path) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
}

/// desk has a conversation in project shop; laptop has a clone of its repo at another path.
fn setup(w: &World) -> (Machine, Machine, PathBuf, String) {
    let (a, b) = (w.machine("desk"), w.machine("laptop"));
    let repo = git_repo(&a.home.join("code/shop"));
    let clone = git_clone(&repo, &b.home.join("work/shop-laptop"));
    let session = claude_session(
        &a,
        &repo,
        &[("user", "fix the cart"), ("assistant", "Cart fixed on desk")],
        Some("Cart fix"),
    );
    let sid = session.file_stem().unwrap().to_string_lossy().into_owned();
    a.pick(&["mk", "shop"], &[&sid]);
    (a, b, clone, sid)
}

fn texts(m: &Machine, sid: &str) -> Vec<String> {
    claude_transcript(m, sid).1
}

fn run(m: &Machine, args: &[&str], env: &[(&str, &str)], stdin: &str) -> (bool, String, String) {
    let mut c = m.command(ACTX);
    c.args(args)
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn log(m: &Machine) -> String {
    std::fs::read_to_string(m.home.join(".local/state/actx/auto.log")).unwrap_or_default()
}

#[test]
fn auto_sync_pulls_new_conversations_and_pushes_changes() {
    let w = World::new();
    let (a, b, clone, sid) = setup(&w);

    let out = b.actx(&["sync", "--auto"]);
    assert!(out.contains("↓") && out.contains("Cart fix"), "{out}");
    assert!(out.starts_with("20"), "log lines start with the time: {out}");
    let (path, _, cwds) = claude_transcript(&b, &sid);
    assert_eq!(cwds, [clone.to_string_lossy()]);

    claude_continue(&path, &[("user", "coupons"), ("assistant", "Coupons on laptop")], None);
    assert!(b.actx(&["sync", "--auto"]).contains("↑"));
    assert_eq!(b.actx(&["sync", "--auto"]), "", "nothing to do, nothing to say");

    age(&claude_transcript(&a, &sid).0);
    assert!(a.actx(&["sync", "--auto"]).contains("↓"));
    assert!(texts(&a, &sid).contains(&"Coupons on laptop".to_string()));
    assert!(a.actx(&["ls"]).contains("✓ synced"));
}

#[test]
fn auto_sync_leaves_open_conversations_and_conflicts_alone() {
    let w = World::new();
    let (a, b, _clone, sid) = setup(&w);
    b.actx(&["sync", "--auto"]);
    let b_path = claude_transcript(&b, &sid).0;
    claude_continue(
        &b_path,
        &[("user", "coupons"), ("assistant", "Coupons on laptop")],
        None,
    );
    b.actx(&["sync", "--auto"]);

    // desk's copy was written moments ago: it may be open
    let out = a.actx(&["sync", "--auto"]);
    assert!(out.contains("is open in Claude Code"), "{out}");
    assert!(!texts(&a, &sid).contains(&"Coupons on laptop".to_string()));
    // and the conversation a hook says is opening right now
    let a_path = claude_transcript(&a, &sid).0;
    age(&a_path);
    let busy = format!("claude:{sid}");
    assert!(
        a.actx(&["sync", "--auto", "--busy", &busy])
            .contains("is open in Claude Code")
    );
    assert!(a.actx(&["sync", "--auto"]).contains("↓"));

    // both sides changed: nothing is touched, `actx sync` decides
    claude_continue(&a_path, &[("user", "green"), ("assistant", "Green on desk")], None);
    claude_continue(&b_path, &[("user", "blue"), ("assistant", "Blue on laptop")], None);
    b.actx(&["sync", "--auto"]);
    let before = std::fs::read(&a_path).unwrap();
    let out = a.actx(&["sync", "--auto"]);
    assert!(out.contains("run actx sync to decide"), "{out}");
    assert_eq!(std::fs::read(&a_path).unwrap(), before);
    assert!(a.actx(&["ls"]).contains("⚠ conflict"));
}

#[test]
fn the_lock_makes_others_wait() {
    let w = World::new();
    let (_a, b, _clone, sid) = setup(&w);
    let lock = w.store().join(".lock");
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let held = |at: f64| json!({"host": "elsewhere", "pid": 1, "since": at, "at": at, "token": "t"}).to_string();
    std::fs::write(&lock, held(now)).unwrap();

    let (ok, out, err) = run(&b, &["sync"], &[("ACTX_LOCK_WAIT", "1")], "");
    assert!(!ok && err.contains("still busy (elsewhere"), "{out}{err}");
    assert!(find(&b.claude_dir(), &format!("{sid}.jsonl")).is_none());
    let (ok, out, _) = run(&b, &["sync", "--auto"], &[("ACTX_LOCK_WAIT", "1")], "");
    assert!(ok && out.contains("still busy"), "{out}");
    assert!(
        std::fs::read_to_string(&lock).unwrap().contains("elsewhere"),
        "not ours to remove"
    );

    // a lock nobody has refreshed for a long time was left behind by a run that died
    std::fs::write(&lock, held(now - 3600.0)).unwrap();
    assert!(b.actx(&["sync"]).contains("Cart fix"));
    assert!(!lock.exists());
}

#[test]
fn hooks_warn_about_news_and_sync_in_the_background() {
    let w = World::new();
    let (a, b, clone, sid) = setup(&w);
    b.actx(&["sync"]);
    let b_path = claude_transcript(&b, &sid).0;
    let event = |source: &str| {
        json!({"session_id": sid, "transcript_path": b_path, "cwd": clone, "source": source, "hook_event_name": "x"})
            .to_string()
    };
    let wait = [("ACTX_HOOK_WAIT", "1")];
    let (_, out, _) = run(&b, &["hook", "claude", "start"], &wait, &event("resume"));
    assert_eq!(out, "", "up to date: nothing to say");

    claude_continue(
        &claude_transcript(&a, &sid).0,
        &[("user", "coupons"), ("assistant", "Coupons on desk")],
        None,
    );
    a.actx(&["sync"]);
    age(&b_path);

    // opening it on the laptop: a warning in Claude Code, and it's not replaced under its feet
    let (ok, out, _) = run(&b, &["hook", "claude", "start"], &wait, &event("resume"));
    assert!(ok);
    let message: Value = serde_json::from_str(out.trim()).expect("only JSON on stdout");
    let text = message["systemMessage"].as_str().unwrap();
    assert!(text.contains("newer messages from desk"), "{text}");
    assert!(log(&b).contains("is open in Claude Code"), "{}", log(&b));
    assert!(!texts(&b, &sid).contains(&"Coupons on desk".to_string()));

    // closing it: the sync that follows pulls it
    let (_, out, _) = run(&b, &["hook", "claude", "end"], &wait, &event(""));
    assert_eq!(out, "");
    assert!(texts(&b, &sid).contains(&"Coupons on desk".to_string()), "{}", log(&b));
}

#[cfg(target_os = "linux")]
#[test]
fn auto_on_installs_hooks_and_a_timer_and_off_removes_them() {
    let w = World::new();
    let m = w.machine("pc");
    let theirs =
        json!({"model": "x", "hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "orca start"}]}]}});
    let settings = m.home.join(".claude/settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(&settings, serde_json::to_string_pretty(&theirs).unwrap()).unwrap();
    std::fs::create_dir_all(m.home.join(".codex")).unwrap();

    let out = m.actx(&["auto", "on"]);
    assert!(out.contains("Claude Code: hooks in ~/.claude/settings.json"), "{out}");
    assert!(
        out.contains("Codex: hooks in ~/.codex/hooks.json") && out.contains("/hooks"),
        "{out}"
    );
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(v["model"], "x");
    let start = v["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(start[0]["hooks"][0]["command"], "orca start");
    let ours = start[1]["hooks"][0]["command"].as_str().unwrap();
    assert!(ours.ends_with("hook claude start") && ours.contains("actx"), "{ours}");
    let codex: Value =
        serde_json::from_str(&std::fs::read_to_string(m.home.join(".codex/hooks.json")).unwrap()).unwrap();
    assert!(
        codex["hooks"]["SessionEnd"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with("hook codex end")
    );
    let units = m.home.join(".config/systemd/user");
    let service = std::fs::read_to_string(units.join("actx-sync.service")).unwrap();
    assert!(
        service.contains("sync --auto") && service.contains("auto.log"),
        "{service}"
    );
    assert!(units.join("actx-sync.timer").exists());
    assert!(m.actx(&["auto", "on"]).contains("hooks in"), "running it again is fine");
    assert!(m.actx(&["auto"]).contains("Automatic sync: on"));

    let out = m.actx(&["auto", "off"]);
    assert!(out.contains("hooks removed") && out.contains("Timer removed"), "{out}");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(v, theirs);
    assert!(!units.join("actx-sync.timer").exists());
    assert!(m.actx(&["auto"]).contains("Automatic sync: off"));
}

#[test]
fn restore_brings_back_what_a_pull_replaced() {
    let w = World::new();
    let (a, b, _clone, sid) = setup(&w);
    assert!(b.actx(&["restore"]).contains("No backups yet"));
    b.actx(&["sync"]);
    let before = std::fs::read(claude_transcript(&b, &sid).0).unwrap();
    claude_continue(
        &claude_transcript(&a, &sid).0,
        &[("user", "coupons"), ("assistant", "Coupons on desk")],
        None,
    );
    a.actx(&["sync"]);
    b.actx(&["sync"]);
    assert!(texts(&b, &sid).contains(&"Coupons on desk".to_string()));

    let out = b.actx_with(
        &["restore"],
        Opts {
            pick: Some(&[&sid]),
            answer: Some("y"),
            ..Default::default()
        },
    );
    assert!(out.contains("Restored «Cart fix»"), "{out}");
    assert_eq!(std::fs::read(claude_transcript(&b, &sid).0).unwrap(), before);
    // the version it replaced was backed up too, and the next sync puts it back on top
    let backups = files_under(&b.home.join(".local/state/actx/backups"));
    assert_eq!(
        backups
            .iter()
            .filter(|f| f.to_string_lossy().ends_with(".jsonl"))
            .count(),
        2
    );
    assert!(b.actx(&["ls"]).contains("↑ push"));
}

#[test]
fn doctor_and_completions() {
    needs!("fzf");
    let w = World::new();
    let (a, _b, _clone, _sid) = setup(&w);
    let out = a.actx(&["doctor"]);
    assert!(
        out.contains("shared folder") && out.contains("1 project(s), 1 conversation(s)"),
        "{out}"
    );
    assert!(out.contains("All good"), "{out}");
    let out = Command::new(ACTX).args(["completions", "bash"]).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("actx"));
}

#[test]
fn what_was_pushed_while_waiting_for_the_lock_is_not_overwritten() {
    use std::io::BufRead;
    let w = World::new();
    let (a, b, _clone, sid) = setup(&w);
    b.actx(&["sync"]);
    claude_continue(
        &claude_transcript(&a, &sid).0,
        &[("user", "coupons"), ("assistant", "Coupons on desk")],
        None,
    );
    a.actx(&["sync"]);
    let lock = w.store().join(".lock");
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    std::fs::write(
        &lock,
        json!({"host": "elsewhere", "pid": 1, "since": now, "at": now, "token": "t"}).to_string(),
    )
    .unwrap();

    // laptop decides to pull, then waits for the lock…
    let mut c = b.command(ACTX);
    c.args(["sync"]).env("ACTX_LOCK_WAIT", "20").stdout(Stdio::piped());
    let mut child = c.spawn().unwrap();
    let mut lines = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let waiting = lines.by_ref().map_while(Result::ok).find(|l| l.contains("waiting"));
    assert!(waiting.is_some());
    // …while elsewhere pushes a newer version
    let meta = files_under(&w.store())
        .into_iter()
        .find(|f| f.ends_with("meta.json"))
        .unwrap();
    let mut m: Value = serde_json::from_str(&std::fs::read_to_string(&meta).unwrap()).unwrap();
    m["pushed_at"] = json!(now + 1.0);
    std::fs::write(&meta, m.to_string()).unwrap();
    std::fs::remove_file(&lock).unwrap();

    let rest: Vec<String> = lines.map_while(Result::ok).collect();
    child.wait().unwrap();
    assert!(
        rest.iter()
            .any(|l| l.contains("changed in the shared folder meanwhile")),
        "{rest:?}"
    );
    assert!(!texts(&b, &sid).contains(&"Coupons on desk".to_string()));
}
