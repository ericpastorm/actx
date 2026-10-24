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
    a.actx(&["mk", "shop"]);
    a.pick(&["add", "shop"], &[&sid]);
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
fn automatic_runs_on_one_machine_take_turns() {
    let w = World::new();
    let (_a, b, _clone, sid) = setup(&w);
    let state = b.home.join(".local/state/actx");
    std::fs::create_dir_all(&state).unwrap();
    let hold = |name: &str| {
        let f = std::fs::File::create(state.join(name)).unwrap();
        f.lock().unwrap();
        f
    };
    // one run going and another waiting: a third trigger has nothing to add
    let (running, waiting) = (hold("auto-run.lock"), hold("auto-next.lock"));
    let (ok, out, err) = run(&b, &["sync", "--auto", "--busy", "claude:opening"], &[], "");
    assert!(ok, "{out}{err}");
    assert!(find(&b.claude_dir(), &format!("{sid}.jsonl")).is_none());
    let notes = state.join("auto-busy.txt");
    assert_eq!(std::fs::read_to_string(&notes).unwrap().trim(), "claude:opening");
    drop((running, waiting));
    // the next run does the work and takes over what that trigger was opening
    run(&b, &["sync", "--auto"], &[], "");
    assert!(find(&b.claude_dir(), &format!("{sid}.jsonl")).is_some());
    assert!(!notes.exists());
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
    let out = hidden(&mut Command::new(ACTX))
        .args(["completions", "bash"])
        .output()
        .unwrap();
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

#[test]
fn opening_a_conversation_doesnt_change_it() {
    let w = World::new();
    let (a, b, _clone, sid) = setup(&w);
    b.actx(&["sync"]);
    let b_path = claude_transcript(&b, &sid).0;
    // what Claude Code writes just by opening and closing a conversation: bookkeeping, and with a
    // uuid, the output of SessionStart hooks and notices
    let resumed = |n: u32| {
        format!(
            concat!(
                "{{\"type\":\"permission-mode\",\"permissionMode\":\"default\",\"sessionId\":\"{sid}\"}}\n",
                "{{\"parentUuid\":null,\"type\":\"attachment\",\"attachment\":{{\"type\":\"hook_success\",\"hookName\":\"SessionStart:resume\",\"content\":\"\"}},\"uuid\":\"hook-{n}\",\"sessionId\":\"{sid}\"}}\n",
                "{{\"parentUuid\":\"hook-{n}\",\"type\":\"system\",\"subtype\":\"informational\",\"content\":\"notice\",\"uuid\":\"notice-{n}\",\"sessionId\":\"{sid}\"}}\n",
                "{{\"type\":\"cost-state\",\"sessionId\":\"{sid}\",\"totalCostUSD\":0}}\n",
            ),
            sid = sid,
            n = n
        )
    };
    append(&b_path, &resumed(1));
    assert!(b.actx(&["ls"]).contains("✓ synced"));
    b.actx(&["sync"]);
    append(&b_path, &resumed(2));
    assert!(b.actx(&["ls"]).contains("✓ synced"));
    claude_continue(
        &claude_transcript(&a, &sid).0,
        &[("user", "coupons"), ("assistant", "Coupons on desk")],
        None,
    );
    a.actx(&["sync"]);
    assert!(b.actx(&["ls"]).contains("↓ pull"), "not a conflict");
}

#[test]
fn the_end_hook_waits_for_the_agent_to_exit() {
    let w = World::new();
    let (a, _b, _clone, sid) = setup(&w);
    let path = claude_transcript(&a, &sid).0;
    // the last message, which our stand-in for Claude Code writes after calling the hook
    let copy = w.dir.path().join("copy.jsonl");
    std::fs::copy(&path, &copy).unwrap();
    claude_continue(&copy, &[("user", "one more"), ("assistant", "Late message")], None);
    let late = w.dir.path().join("late.jsonl");
    std::fs::write(
        &late,
        &std::fs::read(&copy).unwrap()[std::fs::metadata(&path).unwrap().len() as usize..],
    )
    .unwrap();
    let payload = w.dir.path().join("payload.json");
    std::fs::write(
        &payload,
        json!({"session_id": sid, "transcript_path": path}).to_string(),
    )
    .unwrap();
    let bin = w.dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    #[cfg(unix)]
    let ok = {
        use std::os::unix::fs::PermissionsExt;
        let agent = bin.join("claude");
        std::fs::write(
            &agent,
            "#!/bin/sh\n\"$ACTX_BIN\" hook claude end < \"$1\"\nsleep 5\ncat \"$2\" >> \"$3\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();
        a.command(agent.to_str().unwrap())
            .args([&payload, &late, &path])
            .env("ACTX_BIN", ACTX)
            .status()
            .unwrap()
            .success()
    };
    // Windows: cmd.exe under the name claude.exe is the agent: the hook runs from it, it goes on
    // for a few seconds, and then writes the last message
    #[cfg(windows)]
    let ok = {
        use std::os::windows::process::CommandExt;
        let agent = bin.join("claude.exe");
        std::fs::copy(r"C:\Windows\System32\cmd.exe", &agent).unwrap();
        let script = format!(
            "\"\"{ACTX}\" hook claude end < \"{}\" & ping -n 6 127.0.0.1 > nul & type \"{}\" >> \"{}\"\"",
            payload.display(),
            late.display(),
            path.display()
        );
        a.command(agent.to_str().unwrap())
            .args(["/S", "/C"])
            .raw_arg(script)
            .status()
            .unwrap()
            .success()
    };
    assert!(ok);
    let stored = w.store().join("shop/claude").join(&sid).join("session.jsonl.zst");
    let pushed = (0..60).any(|_| {
        std::thread::sleep(Duration::from_millis(250));
        std::fs::File::open(&stored)
            .ok()
            .and_then(|f| zstd::decode_all(f).ok())
            .is_some_and(|d| String::from_utf8_lossy(&d).contains("Late message"))
    });
    assert!(pushed, "{}", log(&a));
}

/// A machine that keeps its projects outside its home: a new conversation of one of them is put in
/// its folder when it has another next to it that it knows: one it has conversations in, or one it
/// was told to put a conversation in.
fn found_next_to(told: bool) {
    let w = World::new();
    let (a, b) = (w.machine("desk"), w.machine("laptop"));
    let disk = w.dir.path().join("disk/projects");
    let shop = git_repo(&a.home.join("code/shop"));
    let blog = git_repo(&a.home.join("code/blog"));
    let shop_here = git_clone(&shop, &disk.join("shop"));
    git_clone(&blog, &disk.join("blog"));
    if told {
        let first = hidden(&mut Command::new("git"))
            .args(["rev-list", "--max-parents=0", "HEAD"])
            .current_dir(&shop)
            .output()
            .unwrap();
        let first = String::from_utf8_lossy(&first.stdout).trim().to_string();
        let state = b.home.join(".local/state/actx/state.json");
        std::fs::create_dir_all(state.parent().unwrap()).unwrap();
        std::fs::write(
            &state,
            json!({"base": {}, "folders": {format!("git:{first}"): shop_here}}).to_string(),
        )
        .unwrap();
    } else {
        claude_session(
            &b,
            &shop_here,
            &[("user", "hi"), ("assistant", "Hi from the laptop")],
            None,
        );
    }
    let session = claude_session(
        &a,
        &blog,
        &[("user", "a post"), ("assistant", "Posted on desk")],
        Some("Post"),
    );
    let sid = session.file_stem().unwrap().to_string_lossy().into_owned();
    a.actx(&["mk", "blog"]);
    a.pick(&["add", "blog"], &[&sid]);
    b.actx(&["sync", "--auto"]);
    let cwds = claude_transcript(&b, &sid).2;
    assert!(cwds.iter().all(|c| Path::new(c) == disk.join("blog")), "{cwds:?}");
}

#[test]
fn repos_next_to_folders_with_conversations_are_found() {
    found_next_to(false);
}

#[test]
fn repos_next_to_folders_picked_before_are_found() {
    found_next_to(true);
}
