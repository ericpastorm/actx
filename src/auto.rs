//! Automatic sync. Claude Code and Codex call actx when a conversation starts and ends (hooks),
//! and a timer runs it every few minutes for everything else (opencode included). Each run is
//! `actx sync --auto`: it pushes what changed, pulls what's new where that's safe, and leaves
//! anything that needs a decision for `actx sync`.

use std::fs::File;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

use crate::ctx::{Error, Result, ctx, jstr};
use crate::paths::short;
use crate::session::{Session, status};
use crate::store::{find_stored, reachable_root};
use crate::tr;
use crate::ui::{ago, bold, dim, fail, ok, paint, shell_quote, warn};

pub const EVERY_MIN: u64 = 10;
const UNIT: &str = "actx-sync";
const LAUNCHD_LABEL: &str = "io.github.ericpastorm.actx";
/// (event in the tool's hooks file, what actx calls it)
const EVENTS: [(&str, &str); 2] = [("SessionStart", "start"), ("SessionEnd", "end")];

pub fn log_file() -> PathBuf {
    ctx().state_dir.join("auto.log")
}

/// Run `actx sync --auto` in the background, detached from the agent that called us.
fn spawn_sync(delay: u64, busy: Option<&str>, after: Option<u32>) {
    let _ = std::fs::create_dir_all(&ctx().state_dir);
    let Ok(log) = File::options().create(true).append(true).open(log_file()) else {
        return;
    };
    let mut c = Command::new(&ctx().self_path);
    c.args(["sync", "--auto"]);
    if delay > 0 {
        c.args(["--delay", &delay.to_string()]);
    }
    if let Some(key) = busy {
        c.args(["--busy", key]);
    }
    if let Some(pid) = after {
        c.args(["--after", &pid.to_string()]);
    }
    let err = log.try_clone().map(Stdio::from).unwrap_or_else(|_| Stdio::null());
    let child = c.stdin(Stdio::null()).stdout(log).stderr(err).process_group(0).spawn();
    if ctx().hook_wait
        && let Ok(mut child) = child
    {
        let _ = child.wait();
    }
}

// ── the hooks themselves ────────────────────────────────────────────────────

/// `actx hook <tool> start|end`, called by Claude Code or Codex with the event as JSON on stdin.
/// Whatever it prints at start is shown in the agent, so it prints nothing but that message.
pub fn hook(tool: &str, event: &str) {
    let mut input = String::new();
    let _ = std::io::stdin().take(1 << 20).read_to_string(&mut input);
    let payload: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let harness = if tool == "codex" { "codex" } else { "claude" };
    let sid = jstr(&payload, "session_id");
    if event != "start" {
        // once the agent has finished writing and exited (or, if we can't tell which it is, in a moment)
        let agent = crate::agents::calling_agent();
        spawn_sync(if agent.is_some() { 1 } else { 3 }, None, agent);
        return;
    }
    if matches!(jstr(&payload, "source"), "" | "startup" | "resume") {
        // this one is opening right now: never pull over it
        spawn_sync(
            0,
            (!sid.is_empty()).then(|| format!("{harness}:{sid}")).as_deref(),
            None,
        );
    }
    if let Some(message) = outdated(harness, sid, jstr(&payload, "transcript_path")) {
        println!("{}", json!({"systemMessage": message}));
    }
}

/// If the conversation being opened has news in the shared folder, what to tell the user.
fn outdated(harness: &'static str, sid: &str, transcript: &str) -> Option<String> {
    let size = Path::new(transcript).metadata().ok()?.len();
    let weight = if harness == "claude" {
        crate::claude::content_weight(Path::new(transcript)).ok()?
    } else {
        size
    };
    let root = reachable_root(Duration::from_secs(3))?;
    let r = find_stored(&root, harness, sid)?;
    let mut here = Session::local(harness, sid.to_string(), r.title.clone(), String::new(), 0.0, weight);
    here.size = (harness == "claude").then_some(size);
    let base = crate::state::load().base.get(&r.key()).copied();
    let (host, when) = (jstr(r.meta(), "host"), ago(r.updated));
    match status(Some(&here), Some(&r), base) {
        "pull" => Some(tr!(
            "actx: «{title}» has newer messages from {host} ({ago}). To get them, exit, run actx sync and resume it again.",
            title = r.title,
            host = host,
            ago = when
        )),
        "conflict" => Some(tr!(
            "actx: «{title}» also changed on {host} ({ago}). Exit and run actx sync to merge both versions.",
            title = r.title,
            host = host,
            ago = when
        )),
        _ => None,
    }
}

// ── hooks files ─────────────────────────────────────────────────────────────

fn claude_settings() -> PathBuf {
    ctx()
        .claude_projects
        .parent()
        .map(|d| d.join("settings.json"))
        .unwrap_or_default()
}

fn codex_hooks() -> PathBuf {
    ctx().codex_home.join("hooks.json")
}

fn our_command(tool: &str, event: &str) -> String {
    format!(
        "{} hook {tool} {event}",
        shell_quote(&ctx().self_path.to_string_lossy())
    )
}

fn is_ours(handler: &Value, tool: &str, event: &str) -> bool {
    let cmd = jstr(handler, "command");
    cmd.contains("actx")
        && cmd
            .trim_end_matches(['\'', '"', ' '])
            .ends_with(&format!(" hook {tool} {event}"))
}

fn read_json(file: &Path) -> Result<Value> {
    match std::fs::read_to_string(file) {
        Ok(text) if text.trim().is_empty() => Ok(json!({})),
        Ok(text) => {
            let v: Value = serde_json::from_str(&text).map_err(|e| {
                Error(tr!(
                    "{path} isn't valid JSON ({err}): not touching it",
                    path = short(&file.to_string_lossy()),
                    err = e
                ))
            })?;
            if v.is_object() {
                Ok(v)
            } else {
                Err(Error(tr!(
                    "{path} isn't a JSON object: not touching it",
                    path = short(&file.to_string_lossy())
                )))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(e.into()),
    }
}

fn write_json(file: &Path, v: &Value) -> Result<()> {
    if file.exists() {
        std::fs::copy(file, file.with_extension("json.actx-backup"))?;
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("json.actx-tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(v)? + "\n")?;
    if let Ok(meta) = file.metadata() {
        std::fs::set_permissions(&tmp, meta.permissions())?;
    }
    std::fs::rename(&tmp, file)?;
    Ok(())
}

/// The handlers of `event` in a hooks file: {"hooks": {"SessionStart": [{"hooks": [handler…]}]}}.
fn handlers<'a>(v: &'a Value, event: &str) -> impl Iterator<Item = &'a Value> {
    v.get("hooks")
        .and_then(|h| h.get(event))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
}

fn installed_in(file: &Path, tool: &str) -> bool {
    let Ok(v) = read_json(file) else { return false };
    EVENTS
        .iter()
        .all(|(event, ours)| handlers(&v, event).any(|h| is_ours(h, tool, ours)))
}

/// Add actx's hooks to a hooks file, keeping everything else (and their order: Codex trusts each
/// hook by its position). Returns whether anything changed.
fn add_hooks(file: &Path, tool: &str) -> Result<bool> {
    let mut v = read_json(file)?;
    let before = v.clone();
    if !v.get("hooks").is_some_and(Value::is_object) {
        v["hooks"] = json!({});
    }
    for (event, ours) in EVENTS {
        let command = our_command(tool, ours);
        let groups = &mut v["hooks"][event];
        if !groups.is_array() {
            *groups = json!([]);
        }
        let mut found = false;
        for group in groups.as_array_mut().into_iter().flatten() {
            for h in group
                .get_mut("hooks")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                if is_ours(h, tool, ours) {
                    h["command"] = json!(command); // actx may have moved
                    found = true;
                }
            }
        }
        if !found && let Some(groups) = groups.as_array_mut() {
            groups.push(json!({"hooks": [{"type": "command", "command": command, "timeout": 10}]}));
        }
    }
    if v == before {
        return Ok(false);
    }
    write_json(file, &v)?;
    Ok(true)
}

fn remove_hooks(file: &Path, tool: &str) -> Result<bool> {
    if !file.exists() {
        return Ok(false);
    }
    let mut v = read_json(file)?;
    let before = v.clone();
    for (event, ours) in EVENTS {
        let Some(groups) = v
            .get_mut("hooks")
            .and_then(|h| h.get_mut(event))
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        for group in groups.iter_mut() {
            if let Some(hs) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                hs.retain(|h| !is_ours(h, tool, ours));
            }
        }
        groups.retain(|g| g.get("hooks").and_then(Value::as_array).is_none_or(|hs| !hs.is_empty()));
        if groups.is_empty()
            && let Some(hooks) = v["hooks"].as_object_mut()
        {
            hooks.remove(event);
        }
    }
    if v["hooks"].as_object().is_some_and(|h| h.is_empty())
        && let Some(top) = v.as_object_mut()
    {
        top.remove("hooks");
    }
    if v == before {
        return Ok(false);
    }
    write_json(file, &v)?;
    Ok(true)
}

// ── the timer ───────────────────────────────────────────────────────────────

/// What the timer's runs need from this environment (systemd and launchd start with almost nothing).
fn carried_env() -> Vec<(String, String)> {
    const NAMES: [&str; 14] = [
        "PATH",
        "LANG",
        "LC_ALL",
        "ACTX_ROOT",
        "ACTX_LANG",
        "ACTX_SCAN",
        "ACTX_OPENCODE_ARGS",
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "HOME",
    ];
    NAMES
        .iter()
        .filter_map(|n| Some((n.to_string(), std::env::var(n).ok().filter(|v| !v.is_empty())?)))
        .collect()
}

fn systemd_dir() -> PathBuf {
    let config_home = ctx()
        .config_file
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| ctx().home.join(".config"));
    config_home.join("systemd").join("user")
}

fn launchd_plist() -> PathBuf {
    ctx()
        .home
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"))
}

fn timer_file() -> PathBuf {
    if cfg!(target_os = "macos") {
        launchd_plist()
    } else {
        systemd_dir().join(format!("{UNIT}.timer"))
    }
}

fn run(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn systemd_quote(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%")
    )
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Install and start the timer. Ok(false): files written, but there's no scheduler to start it.
fn add_timer() -> Result<bool> {
    let me = ctx().self_path.to_string_lossy().into_owned();
    let log = log_file().to_string_lossy().into_owned();
    std::fs::create_dir_all(&ctx().state_dir)?;
    if cfg!(target_os = "macos") {
        let env: String = carried_env()
            .iter()
            .map(|(k, v)| format!("    <key>{}</key><string>{}</string>\n", xml(k), xml(v)))
            .collect();
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>{LAUNCHD_LABEL}</string>\n  <key>ProgramArguments</key>\n  <array><string>{}</string><string>sync</string><string>--auto</string></array>\n  <key>StartInterval</key><integer>{}</integer>\n  <key>RunAtLoad</key><true/>\n  <key>ProcessType</key><string>Background</string>\n  <key>StandardOutPath</key><string>{}</string>\n  <key>StandardErrorPath</key><string>{}</string>\n  <key>EnvironmentVariables</key>\n  <dict>\n{env}  </dict>\n</dict>\n</plist>\n",
            xml(&me),
            EVERY_MIN * 60,
            xml(&log),
            xml(&log)
        );
        let file = launchd_plist();
        std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))?;
        std::fs::write(&file, plist)?;
        let domain = format!("gui/{}", user_id());
        let _ = run("launchctl", &["bootout", &format!("{domain}/{LAUNCHD_LABEL}")]);
        return Ok(run("launchctl", &["bootstrap", &domain, &file.to_string_lossy()]));
    }
    let dir = systemd_dir();
    std::fs::create_dir_all(&dir)?;
    let env: String = carried_env()
        .iter()
        .map(|(k, v)| format!("Environment={}\n", systemd_quote(&format!("{k}={v}"))))
        .collect();
    std::fs::write(
        dir.join(format!("{UNIT}.service")),
        format!(
            "[Unit]\nDescription=actx: sync agent conversations (actx auto)\n\n[Service]\nType=oneshot\nExecStart={} sync --auto\n{env}StandardOutput=append:{log}\nStandardError=append:{log}\nNice=10\n",
            systemd_quote(&me)
        ),
    )?;
    std::fs::write(
        dir.join(format!("{UNIT}.timer")),
        format!(
            "[Unit]\nDescription=actx: sync agent conversations every {EVERY_MIN} minutes\n\n[Timer]\nOnStartupSec=2min\nOnUnitActiveSec={EVERY_MIN}min\nRandomizedDelaySec=30s\n\n[Install]\nWantedBy=timers.target\n"
        ),
    )?;
    Ok(run("systemctl", &["--user", "daemon-reload"])
        && run("systemctl", &["--user", "enable", "--now", &format!("{UNIT}.timer")]))
}

fn remove_timer() -> bool {
    let file = timer_file();
    if !file.exists() {
        return false;
    }
    if cfg!(target_os = "macos") {
        let _ = run("launchctl", &["bootout", &format!("gui/{}/{LAUNCHD_LABEL}", user_id())]);
    } else {
        let _ = run("systemctl", &["--user", "disable", "--now", &format!("{UNIT}.timer")]);
        let _ = std::fs::remove_file(systemd_dir().join(format!("{UNIT}.service")));
    }
    let _ = std::fs::remove_file(&file);
    if !cfg!(target_os = "macos") {
        let _ = run("systemctl", &["--user", "daemon-reload"]);
    }
    true
}

fn user_id() -> String {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn timer_name() -> String {
    if cfg!(target_os = "macos") {
        format!("launchd: {LAUNCHD_LABEL}")
    } else {
        format!("systemd: {UNIT}.timer")
    }
}

// ── actx auto ───────────────────────────────────────────────────────────────

fn tools() -> Vec<(&'static str, &'static str, PathBuf)> {
    let mut out = Vec::new();
    if crate::harness::get("claude").installed() {
        out.push(("claude", "Claude Code", claude_settings()));
    }
    if crate::harness::get("codex").installed() {
        out.push(("codex", "Codex", codex_hooks()));
    }
    out
}

pub fn auto(action: Option<&str>) {
    match action {
        Some("on") => on(),
        Some("off") => off(),
        _ => show(),
    }
}

fn on() {
    if crate::store::configured_root().is_none() {
        crate::ui::die(&tr!(
            "No shared folder configured yet. Run: {cmd}",
            cmd = bold("actx init")
        ));
    }
    if crate::store::install_kind() == "dev" {
        warn(&tr!(
            "This is a development build: the hooks will run {path} until you turn this on again from an installed actx.",
            path = short(&ctx().self_path.to_string_lossy())
        ));
    }
    for (tool, label, file) in tools() {
        match add_hooks(&file, tool) {
            Ok(_) => {
                ok(&tr!(
                    "{tool}: hooks in {path}",
                    tool = label,
                    path = short(&file.to_string_lossy())
                ));
                if tool == "codex" {
                    println!(
                        "  {}",
                        dim(&tr!("Codex asks you to review new hooks once: accept them in /hooks"))
                    );
                }
            }
            Err(e) => fail(&format!("{label}: {e}")),
        }
    }
    match add_timer() {
        Ok(true) => ok(&tr!(
            "Every {n} min ({name}): this covers opencode too",
            n = EVERY_MIN,
            name = timer_name()
        )),
        Ok(false) => warn(&tr!(
            "Couldn't start the timer ({name}). Without it, opencode only syncs when you run actx sync. With cron: {cmd}",
            name = timer_name(),
            cmd = format!(
                "*/{EVERY_MIN} * * * * {} sync --auto >> {} 2>&1",
                shell_quote(&ctx().self_path.to_string_lossy()),
                shell_quote(&log_file().to_string_lossy())
            )
        )),
        Err(e) => fail(&tr!("The timer: {err}", err = e)),
    }
    println!(
        "\n{}",
        tr!(
            "Automatic sync is on: when a conversation starts or ends, and every {n} minutes, actx pushes what changed and pulls what's new. It never pulls over a conversation that's open in an agent, and leaves conflicts and new folders for {cmd}.",
            n = EVERY_MIN,
            cmd = bold("actx sync")
        )
    );
    println!(
        "{}",
        dim(&tr!("Log: {path}", path = short(&log_file().to_string_lossy())))
    );
}

fn off() {
    let mut any = false;
    for (tool, label, file) in tools() {
        match remove_hooks(&file, tool) {
            Ok(true) => {
                any = true;
                ok(&tr!("{tool}: hooks removed", tool = label));
            }
            Ok(false) => {}
            Err(e) => fail(&format!("{label}: {e}")),
        }
    }
    if remove_timer() {
        any = true;
        ok(&tr!("Timer removed ({name})", name = timer_name()));
    }
    if !any {
        println!("{}", tr!("Automatic sync wasn't on"));
    }
}

/// (what, on?) for each part of automatic sync.
pub fn parts() -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = tools()
        .into_iter()
        .map(|(tool, label, file)| (tr!("{tool} hooks", tool = label), installed_in(&file, tool)))
        .collect();
    out.push((tr!("timer ({name})", name = timer_name()), timer_file().exists()));
    out
}

/// The last lines of the log.
pub fn last_log(n: usize) -> Vec<String> {
    let text = std::fs::read_to_string(log_file()).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| l.to_string())
        .collect()
}

fn show() {
    let parts = parts();
    let on = parts.iter().any(|(_, on)| *on);
    println!(
        "{}",
        if on {
            tr!("Automatic sync: {state}", state = paint("32", &tr!("on")))
        } else {
            tr!("Automatic sync: {state}", state = dim(&tr!("off")))
        }
    );
    for (what, on) in &parts {
        println!("  {} {what}", if *on { paint("32", "✓") } else { dim("·") });
    }
    let log = last_log(8);
    if !log.is_empty() {
        println!(
            "\n{}",
            dim(&tr!("Last runs ({path}):", path = short(&log_file().to_string_lossy())))
        );
        for line in log {
            println!("  {line}");
        }
    }
    if !on {
        println!("\n{}", tr!("Turn it on with {cmd}", cmd = bold("actx auto on")));
    }
}

/// Keep the log small: start a new one when it grows too much.
pub fn rotate_log() {
    let log = log_file();
    if log.metadata().is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::rename(&log, log.with_extension("log.old"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_are_added_once_and_removed_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        let theirs = json!({
            "model": "x",
            "hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "other-tool start"}]}]},
        });
        std::fs::write(&file, serde_json::to_string_pretty(&theirs).unwrap()).unwrap();
        assert!(add_hooks(&file, "claude").unwrap());
        assert!(!add_hooks(&file, "claude").unwrap(), "already there");
        let v = read_json(&file).unwrap();
        let start: Vec<&Value> = handlers(&v, "SessionStart").collect();
        assert_eq!(start.len(), 2);
        assert_eq!(start[0]["command"], "other-tool start", "theirs stay first");
        assert!(is_ours(start[1], "claude", "start"));
        assert!(installed_in(&file, "claude"));
        assert!(file.with_extension("json.actx-backup").exists());
        assert!(remove_hooks(&file, "claude").unwrap());
        assert_eq!(read_json(&file).unwrap(), theirs);
        assert!(!installed_in(&file, "claude"));
    }

    #[test]
    fn a_broken_settings_file_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        std::fs::write(&file, "{ not json").unwrap();
        assert!(add_hooks(&file, "claude").is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ not json");
    }
}
