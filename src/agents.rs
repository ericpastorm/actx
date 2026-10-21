//! Is a conversation open in a running agent? Then it can't be replaced: the agent keeps its own
//! version in memory and would go on writing on top of it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::Value;

use crate::ctx::{ctx, jstr, now};
use crate::session::Session;

/// A conversation file touched this recently is probably still open.
pub const ACTIVE_SECS: f64 = 120.0;

struct Proc {
    tool: &'static str,
    pid: u32,
    cwd: String,
    /// files it has open (Linux only)
    files: Vec<PathBuf>,
}

struct Running {
    procs: Vec<Proc>,
    /// Claude Code says which conversation each of its processes has open
    claude_open: HashSet<String>,
    claude_registered: HashSet<u32>,
    /// this Claude Code keeps that list (~/.claude/sessions)
    claude_lists: bool,
}

fn tool_of(program: &str) -> Option<&'static str> {
    let name = crate::paths::base_name(program).to_lowercase();
    let name = name.trim_start_matches('.').trim_end_matches(".js");
    let name = name.trim_end_matches(".exe").trim_end_matches(".cmd");
    ["claude", "codex", "opencode"]
        .into_iter()
        .find(|t| name == *t || name.starts_with(&format!("{t}-")))
}

/// Which agent a command line runs, by the name it was started with: the same executable can
/// play other parts (Claude Code runs its built-in grep as `ugrep`). Through npm: node <…>/codex.js;
/// or a wrapper script: sh <…>/claude.
fn tool_of_command(args: &[&str]) -> Option<&'static str> {
    let first = *args.first()?;
    let n = crate::paths::base_name(first).to_lowercase();
    let n = n.trim_end_matches(".exe");
    let runtime = ["node", "bun", "sh", "bash", "dash", "zsh"].contains(&n);
    if runtime { tool_of(args.get(1)?) } else { tool_of(first) }
}

#[cfg(target_os = "linux")]
fn processes() -> Vec<Proc> {
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().filter_map(|e| e.ok()) {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let dir = e.path();
        let cmdline = std::fs::read(dir.join("cmdline")).unwrap_or_default();
        let cmdline = String::from_utf8_lossy(&cmdline);
        let args: Vec<&str> = cmdline.split('\0').take(2).collect();
        let Some(tool) = tool_of_command(&args) else { continue };
        let cwd = std::fs::read_link(dir.join("cwd")).unwrap_or_default();
        let files = std::fs::read_dir(dir.join("fd"))
            .into_iter()
            .flatten()
            .filter_map(|f| std::fs::read_link(f.ok()?.path()).ok())
            .collect();
        out.push(Proc {
            tool,
            pid,
            cwd: cwd.to_string_lossy().into_owned(),
            files,
        });
    }
    out
}

/// Windows: (pid, parent pid, command line) of the processes that might be agents (or hooks'
/// parents), from one PowerShell query. `only` is a pid to look up, or None for the agents.
#[cfg(windows)]
fn win_processes(only: Option<u32>) -> Vec<(u32, u32, String)> {
    let filter = match only {
        Some(pid) => format!("-Filter 'ProcessId={pid}'"),
        None => String::new(),
    };
    let script = format!(
        "Get-CimInstance Win32_Process {filter} | ForEach-Object {{ '{{0}}|{{1}}|{{2}}' -f $_.ProcessId, $_.ParentProcessId, $_.CommandLine }}"
    );
    let out = crate::ctx::quiet("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(std::process::Stdio::null())
        .output();
    String::from_utf8_lossy(&out.map(|o| o.stdout).unwrap_or_default())
        .lines()
        .filter_map(|l| {
            let mut parts = l.trim().splitn(3, '|');
            Some((
                parts.next()?.parse().ok()?,
                parts.next()?.parse().ok()?,
                parts.next().unwrap_or("").to_string(),
            ))
        })
        .collect()
}

/// The arguments of a Windows command line (`"C:\Program Files\x.exe" arg`).
#[cfg(any(windows, test))]
fn win_args(command: &str) -> Vec<String> {
    let (mut args, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    for c in command.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    args.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        args.push(cur);
    }
    args
}

/// Ignore known helper subcommands, but never mistake a prompt or option value for one.
/// Unknown options are treated conservatively: an unrecognized process may still be writing.
#[cfg(any(windows, test))]
fn interactive_tool(args: &[&str]) -> Option<&'static str> {
    let tool = tool_of_command(args)?;
    let start = if tool_of(args[0]).is_some() { 1 } else { 2 };
    let values: &[&str] = match tool {
        "opencode" => &[
            "--prompt",
            "--agent",
            "--model",
            "-m",
            "--session",
            "-s",
            "--server",
            "--directory",
            "--port",
            "--hostname",
            "--log-level",
            "--completions",
        ],
        "codex" => &[
            "--config",
            "-c",
            "--model",
            "-m",
            "--profile",
            "-p",
            "--sandbox",
            "-s",
            "--ask-for-approval",
            "-a",
            "--cd",
            "-C",
            "--image",
            "-i",
            "--add-dir",
            "--enable",
            "--disable",
        ],
        _ => &[
            "--model",
            "--agent",
            "--resume",
            "-r",
            "--session-id",
            "--permission-mode",
            "--mcp-config",
            "--system-prompt",
            "--append-system-prompt",
            "--output-format",
            "--input-format",
            "--settings",
        ],
    };
    let flags: &[&str] = match tool {
        "opencode" => &["--standalone", "--auto", "--continue", "-c", "--print-logs"],
        "codex" => &[
            "--full-auto",
            "--dangerously-bypass-approvals-and-sandbox",
            "--search",
            "--no-alt-screen",
            "--oss",
        ],
        _ => &["--continue", "-c", "--verbose", "--dangerously-skip-permissions"],
    };
    let helpers: &[&str] = match tool {
        "opencode" => &[
            "serve", "session", "export", "import", "run", "auth", "models", "stats", "mcp", "service", "api",
        ],
        "codex" => &[
            "exec",
            "e",
            "auth",
            "login",
            "logout",
            "mcp",
            "app-server",
            "completion",
        ],
        _ => &["auth", "mcp", "install", "update", "upgrade", "doctor", "setup-token"],
    };
    let mut i = start;
    while let Some(&arg) = args.get(i) {
        if arg == "--" {
            return Some(tool);
        }
        if ["--help", "-h", "--version", "-V", "-v"].contains(&arg) {
            return None;
        }
        if values.contains(&arg) {
            i += 2;
        } else if flags.contains(&arg) || (arg.starts_with("--") && arg.contains('=')) {
            i += 1;
        } else if arg.starts_with('-') {
            return Some(tool);
        } else {
            return (!helpers.contains(&arg)).then_some(tool);
        }
    }
    Some(tool)
}

#[cfg(windows)]
fn processes() -> Vec<Proc> {
    // a folder per process isn't available here: such an agent counts as open in any folder
    win_processes(None)
        .into_iter()
        .filter_map(|(pid, _, command)| {
            let all = win_args(&command);
            let args: Vec<&str> = all.iter().map(String::as_str).collect();
            Some(Proc {
                tool: interactive_tool(&args)?,
                pid,
                cwd: String::new(),
                files: Vec::new(),
            })
        })
        .collect()
}

#[cfg(all(not(target_os = "linux"), not(windows)))]
fn processes() -> Vec<Proc> {
    // macOS and the BSDs: ps for the programs, lsof for their folders
    let ps = std::process::Command::new("ps").args(["-axo", "pid=,args="]).output();
    let mut out: Vec<Proc> = String::from_utf8_lossy(&ps.map(|o| o.stdout).unwrap_or_default())
        .lines()
        .filter_map(|l| {
            let (pid, command) = l.trim().split_once(' ')?;
            let args: Vec<&str> = command.split_whitespace().take(2).collect();
            Some(Proc {
                tool: tool_of_command(&args)?,
                pid: pid.parse().ok()?,
                cwd: String::new(),
                files: Vec::new(),
            })
        })
        .collect();
    if out.is_empty() {
        return out;
    }
    let pids: Vec<String> = out.iter().map(|p| p.pid.to_string()).collect();
    let lsof = std::process::Command::new("lsof")
        .args(["-a", "-d", "cwd", "-Fpn", "-p", &pids.join(",")])
        .output();
    let mut pid = 0;
    let mut cwds = HashMap::new();
    for line in String::from_utf8_lossy(&lsof.map(|o| o.stdout).unwrap_or_default()).lines() {
        match line.split_at_checked(1) {
            Some(("p", n)) => pid = n.parse().unwrap_or(0),
            Some(("n", path)) => {
                cwds.insert(pid, path.to_string());
            }
            _ => {}
        }
    }
    for p in &mut out {
        p.cwd = cwds.remove(&p.pid).unwrap_or_default();
    }
    out
}

fn scan_running() -> Running {
    let procs = processes();
    let live: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let (mut claude_open, mut claude_registered) = (HashSet::new(), HashSet::new());
    let registry = ctx().claude_projects.parent().map(|d| d.join("sessions"));
    let claude_lists = registry.as_ref().is_some_and(|d| d.is_dir());
    for f in registry
        .and_then(|d| std::fs::read_dir(d).ok())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
    {
        let Some(entry) = std::fs::read_to_string(f.path())
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        else {
            continue;
        };
        let pid = entry.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
        if live.get(&pid).is_some_and(|p| p.tool == "claude") {
            claude_open.insert(jstr(&entry, "sessionId").to_string());
            claude_registered.insert(pid);
        }
    }
    Running {
        procs,
        claude_open,
        claude_registered,
        claude_lists,
    }
}

fn running() -> &'static Running {
    static RUNNING: OnceLock<Running> = OnceLock::new();
    RUNNING.get_or_init(scan_running)
}

/// Is `s` (a conversation on this machine) open in a running agent, or was it just being written
/// (by an agent actx doesn't recognize, say)?
pub fn in_use(s: &Session) -> bool {
    // Claude Code's own list is exact: then a conversation closed seconds ago is free
    let exact = s.harness == "claude" && running().claude_lists;
    (!exact && now() - s.updated < ACTIVE_SECS) || in_use_by_agent(s)
}

/// Is `s` open in a running Claude Code, Codex or opencode?
pub fn in_use_by_agent(s: &Session) -> bool {
    used_by(running(), s)
}

/// Refresh the process and session lists before modifying a conversation: an agent may have
/// opened it since the cached snapshot was taken, especially while waiting for the shared lock.
pub fn in_use_by_agent_now(s: &Session) -> bool {
    used_by(&scan_running(), s)
}

fn used_by(r: &Running, s: &Session) -> bool {
    // Windows can't tell which folder an agent runs in: Codex and opencode count as open in any
    let here = |p: &Proc| {
        p.tool == s.harness && (p.cwd == s.path || (cfg!(windows) && p.cwd.is_empty() && s.harness != "claude"))
    };
    match s.harness {
        // its own list says exactly which; older versions without one: any Claude in that folder
        "claude" => {
            r.claude_open.contains(&s.id) || r.procs.iter().any(|p| here(p) && !r.claude_registered.contains(&p.pid))
        }
        "codex" => r
            .procs
            .iter()
            .any(|p| here(p) || (p.tool == "codex" && s.src.as_ref().is_some_and(|f| p.files.contains(f)))),
        _ => r.procs.iter().any(here),
    }
}

/// The agent that started this process (a hook's caller): its pid. Hooks run directly or through a
/// shell, so it looks a couple of parents up, and no further: an agent beyond that isn't the caller.
pub fn calling_agent() -> Option<u32> {
    #[cfg(unix)]
    let mut pid = std::os::unix::process::parent_id();
    #[cfg(windows)]
    let mut pid = parent_and_args(std::process::id())?.0;
    for _ in 0..3 {
        let (ppid, args) = parent_and_args(pid)?;
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        if tool_of_command(&args).is_some() {
            return Some(pid);
        }
        if ppid <= 1 {
            return None;
        }
        pid = ppid;
    }
    None
}

#[cfg(target_os = "linux")]
fn parent_and_args(pid: u32) -> Option<(u32, Vec<String>)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // "pid (name) state ppid …", and the name may have spaces or parentheses
    let ppid = stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()?;
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args = String::from_utf8_lossy(&cmdline)
        .split('\0')
        .take(2)
        .map(String::from)
        .collect();
    Some((ppid, args))
}

#[cfg(windows)]
fn parent_and_args(pid: u32) -> Option<(u32, Vec<String>)> {
    let (_, ppid, command) = win_processes(Some(pid)).into_iter().next()?;
    Some((ppid, win_args(&command)))
}

#[cfg(all(not(target_os = "linux"), not(windows)))]
fn parent_and_args(pid: u32) -> Option<(u32, Vec<String>)> {
    let out = std::process::Command::new("ps")
        .args(["-o", "ppid=,args=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let (ppid, args) = text.trim().split_once(' ')?;
    let args = args.split_whitespace().take(2).map(String::from).collect();
    Some((ppid.trim().parse().ok()?, args))
}

/// Wait (up to a minute) for a process to end.
pub fn wait_for(pid: u32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while crate::lock::alive(pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// The agent processes running now: (tool, folder). For `actx doctor`.
pub fn summary() -> Vec<(&'static str, String)> {
    running().procs.iter().map(|p| (p.tool, p.cwd.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_the_agents() {
        assert_eq!(tool_of("claude"), Some("claude"));
        assert_eq!(tool_of("/usr/local/bin/codex"), Some("codex"));
        assert_eq!(tool_of(".opencode"), Some("opencode"));
        assert_eq!(tool_of("/opt/codex/codex-x86_64-unknown-linux-musl"), Some("codex"));
        assert_eq!(tool_of("bash"), None);
        assert_eq!(tool_of("claudette"), None);
        assert_eq!(
            tool_of_command(&["/usr/bin/node", "/usr/lib/node_modules/@openai/codex/bin/codex.js"]),
            Some("codex")
        );
        assert_eq!(tool_of_command(&["claude", "--resume"]), Some("claude"));
        // Claude Code's own grep: the claude executable, started as ugrep
        assert_eq!(tool_of_command(&["ugrep", "-G"]), None);
        assert_eq!(tool_of_command(&["node", "server.js"]), None);
    }

    #[test]
    fn windows_agent_prompts_and_option_values_are_not_subcommands() {
        for args in [
            vec!["opencode.exe", "--prompt", "run"],
            vec!["opencode.exe", "--prompt", "models", "C:\\work\\shop"],
            vec!["opencode.exe", "--model", "session"],
            vec!["opencode.exe", "--prompt=run"],
            vec!["opencode.exe", "--", "run"],
            vec!["opencode.exe", "--unknown-option", "session"],
            vec!["codex.exe", "--model", "mcp", "resume"],
            vec!["claude.exe", "run"],
            vec!["claude.exe", "--resume", "models"],
            vec![r"C:\Program Files\node.exe", r"C:\npm\opencode.js", "--prompt", "run"],
        ] {
            assert_eq!(interactive_tool(&args), tool_of_command(&args), "{args:?}");
        }
    }

    #[test]
    fn windows_agent_helpers_are_ignored_after_options_and_runtime() {
        for args in [
            vec!["opencode.exe", "session", "export", "id"],
            vec!["opencode.exe", "--standalone", "session", "import", "file.json"],
            vec!["opencode.exe", "--model", "run", "serve"],
            vec!["codex.exe", "--config", "model=example", "exec", "run"],
            vec!["codex.exe", "app-server"],
            vec!["claude.exe", "--model", "models", "mcp", "list"],
            vec!["node.exe", r"C:\npm\opencode.js", "session", "export", "id"],
            vec!["opencode.exe", "--version"],
        ] {
            assert_eq!(interactive_tool(&args), None, "{args:?}");
        }
        let args = win_args(r#""C:\Program Files\opencode.exe" --prompt "run""#);
        assert_eq!(
            interactive_tool(&args.iter().map(String::as_str).collect::<Vec<_>>()),
            Some("opencode")
        );
    }

    #[test]
    fn a_conversation_just_written_is_in_use() {
        let mut s = Session::local("opencode", "x".into(), "t".into(), "/nowhere/at/all".into(), now(), 1);
        assert!(in_use(&s));
        s.updated = now() - 3600.0;
        assert!(!in_use(&s));
    }
}
