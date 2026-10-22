//! Talking to the user: colors, messages, questions, previews and the fzf pickers.

use std::collections::HashMap;
use std::io::{IsTerminal, Read, Write};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::{Mutex, mpsc};

use chrono::{Local, TimeZone};

use crate::ctx::ctx;
use crate::tr;

pub const N_PREVIEW: usize = 14;

pub fn paint(code: &str, s: &str) -> String {
    if ctx().no_color || machine_mode() {
        s.to_string()
    } else {
        format!("\x1b[{code}m{s}\x1b[0m")
    }
}

pub fn dim(s: &str) -> String {
    paint("2", s)
}

pub fn bold(s: &str) -> String {
    paint("1", s)
}

pub fn harness_tag(h: &str) -> String {
    let (color, name) = match h {
        "claude" => ("38;5;173", h),
        "opencode" => ("36", h),
        "antigravity" => ("38;5;141", "agy"),
        _ => ("38;5;75", h),
    };
    paint(color, &format!("{name:<8}"))
}

static AUTO: AtomicBool = AtomicBool::new(false);
static MACHINE: AtomicBool = AtomicBool::new(false);
static SILENT: AtomicUsize = AtomicUsize::new(0);
static REPORTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
pub(crate) fn take_reports() -> Vec<String> {
    std::mem::take(&mut *REPORTS.lock().unwrap())
}
/// Services return diagnostics to their caller instead of writing to a terminal.
pub struct Silence;
impl Silence {
    pub fn new() -> Self {
        SILENT.fetch_add(1, Ordering::Relaxed);
        Self
    }
}
impl Drop for Silence {
    fn drop(&mut self) {
        SILENT.fetch_sub(1, Ordering::Relaxed);
    }
}
pub fn set_machine_mode() {
    progress_clear();
    MACHINE.store(true, Ordering::Relaxed);
}
pub fn machine_mode() -> bool {
    MACHINE.load(Ordering::Relaxed)
}
static PROGRESS: AtomicBool = AtomicBool::new(false);
static LOADING_ID: AtomicU64 = AtomicU64::new(0);
static PROGRESS_OUTPUT: Mutex<()> = Mutex::new(());
static LOADING_STACK: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());
static LOOP_LOADING: Mutex<Option<Loading>> = Mutex::new(None);

/// Animated while an operation blocks, including a single slow file or child process. A new
/// phase supersedes the old one; prompts and pickers cancel it before taking over the terminal.
pub struct Loading {
    id: u64,
    worker: Option<(mpsc::Sender<()>, std::thread::JoinHandle<()>)>,
}
pub fn loading(label: &str) -> Loading {
    if !progress_allowed() {
        return Loading { id: 0, worker: None };
    }
    let _output = PROGRESS_OUTPUT.lock().unwrap();
    let id = LOADING_ID.fetch_add(1, Ordering::Relaxed) + 1;
    LOADING_STACK.lock().unwrap().push((id, clip(label, 68)));
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        // Fast operations do not flash a loading line.
        if rx.recv_timeout(std::time::Duration::from_millis(150)).is_ok() {
            return;
        }
        let frames = ["|", "/", "-", "\\"];
        let mut i = 0;
        loop {
            {
                let _output = PROGRESS_OUTPUT.lock().unwrap();
                let stack = LOADING_STACK.lock().unwrap();
                if !stack.iter().any(|(entry, _)| *entry == id) {
                    return;
                }
                if let Some((top, label)) = stack.last()
                    && *top == id
                {
                    print!("\r\x1b[2K{} {}", frames[i % frames.len()], label);
                    let _ = std::io::stdout().flush();
                    PROGRESS.store(true, Ordering::Relaxed);
                }
            }
            i += 1;
            if rx.recv_timeout(std::time::Duration::from_millis(80)).is_ok() {
                return;
            }
        }
    });
    Loading {
        id,
        worker: Some((tx, worker)),
    }
}
impl Drop for Loading {
    fn drop(&mut self) {
        if let Some((stop, worker)) = self.worker.take() {
            let _ = stop.send(());
            let _ = worker.join();
            let _output = PROGRESS_OUTPUT.lock().unwrap();
            let mut stack = LOADING_STACK.lock().unwrap();
            let active = stack.last().is_some_and(|(id, _)| *id == self.id);
            stack.retain(|(id, _)| *id != self.id);
            if active && PROGRESS.swap(false, Ordering::Relaxed) {
                print!("\r\x1b[2K");
                let _ = std::io::stdout().flush();
            }
        }
    }
}
fn clear_progress() {
    LOADING_STACK.lock().unwrap().clear();
    if PROGRESS.swap(false, Ordering::Relaxed) {
        print!("\r\x1b[2K");
        let _ = std::io::stdout().flush();
    }
}

/// A one-line spinner for slow loops. Only in a real terminal, never in scripts or auto runs.
pub fn progress(index: usize, total: usize, label: &str) {
    if !progress_allowed() {
        return;
    }
    let previous = LOOP_LOADING.lock().unwrap().take();
    drop(previous);
    *LOOP_LOADING.lock().unwrap() = Some(loading(&format!("{label} {index}/{total}")));
}

/// Remove the spinner line before printing anything else.
pub fn progress_clear() {
    let previous = LOOP_LOADING.lock().unwrap().take();
    drop(previous);
    let _output = PROGRESS_OUTPUT.lock().unwrap();
    clear_progress();
}

fn progress_allowed() -> bool {
    SILENT.load(Ordering::Relaxed) == 0
        && !machine_mode()
        && !is_auto()
        && ctx().pick.is_none()
        && std::io::stdout().is_terminal()
        && std::io::stdin().is_terminal()
}

/// Automatic runs: nobody is watching, output goes to a log.
pub fn set_auto() {
    AUTO.store(true, Ordering::Relaxed)
}

pub fn is_auto() -> bool {
    AUTO.load(Ordering::Relaxed)
}

/// Print a line (in automatic runs, with the time: it's a log).
pub fn say(msg: &str) {
    if SILENT.load(Ordering::Relaxed) > 0 {
        return;
    }
    progress_clear();
    if machine_mode() {
        eprintln!("{msg}");
    } else if is_auto() {
        println!("{} {msg}", local_time(crate::ctx::now(), "%Y-%m-%d %H:%M:%S"));
    } else {
        println!("{msg}");
    }
}

pub fn die(msg: &str) -> ! {
    progress_clear();
    eprintln!("{}{msg}", paint("31", "✗ "));
    std::process::exit(1)
}

pub fn warn(msg: &str) {
    if SILENT.load(Ordering::Relaxed) > 0 {
        REPORTS.lock().unwrap().push(msg.to_string());
        return;
    }
    say(&format!("{}{msg}", paint("33", "⚠ ")));
}

pub fn ok(msg: &str) {
    say(&format!("{}{msg}", paint("32", "✓ ")));
}

pub fn fail(msg: &str) {
    if SILENT.load(Ordering::Relaxed) > 0 {
        REPORTS.lock().unwrap().push(msg.to_string());
        return;
    }
    say(&format!("{}{msg}", paint("31", "✗ ")));
}

/// Is there someone to ask? (a terminal, or a scripted answer in tests)
pub fn interactive() -> bool {
    ctx().answer.is_some() || std::io::stdin().is_terminal()
}

pub fn ask(question: &str, lower: bool) -> String {
    progress_clear();
    if let Some(answer) = &ctx().answer {
        println!("{} {question} {answer}", paint("33", "?"));
        return answer.clone();
    }
    if !std::io::stdin().is_terminal() {
        return String::new();
    }
    print!("{} {question} ", paint("33", "?"));
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return String::new();
    }
    let answer = line.trim();
    if lower {
        answer.to_lowercase()
    } else {
        answer.to_string()
    }
}

pub fn confirm(question: &str) -> bool {
    let answer = ask(&format!("{question} {}", tr!("[y/N]")), true);
    matches!(answer.as_str(), "y" | "yes" | "s" | "si" | "sí" | "是" | "好" | "确认")
}

pub fn ago(ts: f64) -> String {
    let d = crate::ctx::now() - ts;
    if d < 90.0 {
        return tr!("now");
    }
    if d < 3600.0 {
        return tr!("{n} min ago", n = (d / 60.0) as i64);
    }
    if d < 86400.0 {
        return tr!("{n} h ago", n = (d / 3600.0) as i64);
    }
    if d < 7.0 * 86400.0 {
        return tr!("{n} d ago", n = (d / 86400.0) as i64);
    }
    local_time(ts, "%d %b %Y")
}

pub fn local_time(ts: f64, format: &str) -> String {
    Local
        .timestamp_opt(ts as i64, 0)
        .single()
        .map(|t| t.format(format).to_string())
        .unwrap_or_default()
}

pub fn human(n: u64) -> String {
    let mut n = n as f64;
    for unit in ["B", "KB", "MB"] {
        if n < 1024.0 {
            return format!("{n:.0}{unit}");
        }
        n /= 1024.0;
    }
    format!("{n:.1}GB")
}

/// The first `n` characters (not bytes) of `s`.
pub fn take_chars(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or(s, |(i, _)| &s[..i])
}

/// The last `n` characters of `s`.
pub fn last_chars(s: &str, n: usize) -> &str {
    let len = s.chars().count();
    if len <= n {
        s
    } else {
        &s[s.char_indices().nth(len - n).map_or(0, |(i, _)| i)..]
    }
}

pub fn visible_width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
}

pub fn clip(title: &str, width: usize) -> String {
    if visible_width(title) <= width {
        return title.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let budget = width - 1;
    let mut end = 0;
    for (i, c) in title.char_indices() {
        let next = i + c.len_utf8();
        if visible_width(&title[..next]) > budget {
            break;
        }
        end = next;
    }
    format!("{}…", &title[..end])
}
/// Pad by terminal columns, including wide CJK characters and combining marks.
pub fn pad(s: &str, width: usize) -> String {
    format!("{s}{}", " ".repeat(width.saturating_sub(visible_width(s))))
}

/// Last messages as plain text, for previews. msgs = (role, text).
pub fn render(msgs: &[(String, String)], agent: &str) -> String {
    // from the agent, only the last message of each turn: otherwise it buries your questions
    let kept: Vec<&(String, String)> = msgs
        .iter()
        .enumerate()
        .filter(|(i, m)| m.0 == "user" || i + 1 == msgs.len() || msgs[i + 1].0 == "user")
        .map(|(_, m)| m)
        .collect();
    if kept.is_empty() {
        return tr!("(no text messages)");
    }
    let you = tr!("you");
    kept[kept.len().saturating_sub(N_PREVIEW)..]
        .iter()
        .map(|(role, text)| {
            let text = text.trim();
            let text = if text.chars().count() > 600 {
                format!("{} …", take_chars(text, 600).trim_end())
            } else {
                text.to_string()
            };
            format!("▌ {}\n{text}\n", if role == "user" { you.as_str() } else { agent })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A quoted path for a shell command.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_./~+-".contains(c)) {
        s.to_string()
    } else if cfg!(windows) {
        // double quotes work in Git Bash, cmd and PowerShell alike (callers pass `/` paths)
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// A path to put in a command line: on Windows with `/`, which every shell understands
/// (`C:\Users\me\actx.exe` is mangled by bash).
pub fn command_path(path: &str) -> String {
    if !cfg!(windows) {
        return shell_quote(path);
    }
    let path = path.replace('\\', "/");
    // `C:/Users/me/actx.exe` needs no quotes, and unquoted it also runs from PowerShell
    if path.chars().all(|c| c.is_ascii_alphanumeric() || "_./~+-:".contains(c)) {
        path
    } else {
        shell_quote(&path)
    }
}

pub struct Row {
    pub key: String,
    pub line: String,
    pub preview: String,
}

/// Show `rows` in fzf and return the chosen keys.
pub struct Selection {
    pub action: String,
    pub keys: Vec<String>,
}
pub fn fzf(rows: &[Row], header: &str, multi: bool, preview: bool) -> Vec<String> {
    picker(rows, header, multi, preview, &[], &[])
        .map(|s| s.keys)
        .unwrap_or_default()
}
pub fn checkbox_picker(
    rows: &[Row],
    header: &str,
    actions: &[&str],
    selected: &[String],
    preview: bool,
) -> Option<Selection> {
    if ctx().pick.is_some() {
        return picker(rows, header, true, preview, actions, selected);
    }
    let mut checked = selected.to_vec();
    let all_keys: Vec<String> = rows.iter().map(|row| row.key.clone()).collect();
    let mut focus = Vec::new();
    loop {
        let all_checked = !all_keys.is_empty() && all_keys.iter().all(|key| checked.contains(key));
        let mut choices: Vec<Row> = Vec::with_capacity(rows.len() + 2);
        choices.push(Row {
            key: "::all".into(),
            line: format!("{} {}", if all_checked { "☑" } else { "☐" }, tr!("Select all")),
            preview: String::new(),
        });
        choices.push(Row {
            key: "::next".into(),
            line: tr!("Next → ({n})", n = checked.len()),
            preview: String::new(),
        });
        choices.extend(rows.iter().map(|row| Row {
            key: row.key.clone(),
            line: format!("{} {}", if checked.contains(&row.key) { "☑" } else { "☐" }, row.line),
            preview: row.preview.clone(),
        }));
        let pick = picker(&choices, header, false, preview, actions, &focus)?;
        if pick.action == "ctrl-d" {
            return Some(Selection {
                action: pick.action,
                keys: Vec::new(),
            });
        }
        let key = pick.keys.first()?;
        if key == "::next" {
            return Some(Selection {
                action: "enter".into(),
                keys: checked,
            });
        }
        if key == "::all" {
            if all_checked {
                checked.clear();
            } else {
                checked = all_keys.clone();
            }
        } else if checked.contains(key) {
            checked.retain(|item| item != key);
        } else {
            checked.push(key.clone());
        }
        focus = vec![key.clone()];
    }
}

pub fn picker(
    rows: &[Row],
    header: &str,
    multi: bool,
    preview: bool,
    actions: &[&str],
    selected: &[String],
) -> Option<Selection> {
    progress_clear();
    if let Some(pick) = &ctx().pick {
        let keys = rows
            .iter()
            .filter(|r| pick.iter().any(|p| r.key.contains(p.as_str())))
            .map(|r| r.key.clone())
            .collect::<Vec<_>>();
        return (!keys.is_empty()).then_some(Selection {
            action: "enter".into(),
            keys,
        });
    }
    if !crate::ctx::which("fzf") {
        die(&if cfg!(windows) {
            tr!("actx needs fzf (e.g. winget install fzf)")
        } else {
            tr!("actx needs fzf (e.g. sudo pacman -S fzf)")
        });
    }
    let mut cmd = crate::ctx::console_command("fzf");
    cmd.args([
        "--ansi",
        "--delimiter=\t",
        "--with-nth=2..",
        "--layout=reverse",
        "--height=90%",
        "--border=rounded",
    ])
    .args(["--header", header, "--header-first"]);
    if !actions.is_empty() {
        cmd.arg(format!("--expect={}", actions.join(",")));
    }
    if multi {
        cmd.args(["--multi", "--bind", "ctrl-a:select-all"]);
    }
    let mut start = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        if selected.contains(&row.key) {
            start.push(if multi {
                format!("pos({})+select", i + 1)
            } else {
                format!("pos({})", i + 1)
            });
        }
    }
    if !start.is_empty() {
        if multi {
            start.push("first".into());
        }
        cmd.args(["--bind", &format!("load:{}", start.join("+"))]);
    }
    let cache = if preview {
        let map: HashMap<&str, &str> = rows.iter().map(|r| (r.key.as_str(), r.preview.as_str())).collect();
        let file = tempfile::Builder::new().prefix("actx-").suffix(".json").tempfile().ok();
        if let Some(f) = &file {
            let _ = std::fs::write(f.path(), serde_json::to_string(&map).unwrap_or_default());
            let show = format!(
                "{} _show {} {{1}}",
                command_path(&ctx().self_path.to_string_lossy()),
                command_path(&f.path().to_string_lossy())
            );
            cmd.args([
                "--preview",
                &show,
                "--preview-window",
                "right,50%,wrap,<60(down,50%,wrap)",
            ]);
        }
        file
    } else {
        None
    };
    let Ok(mut child) = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn() else {
        return None;
    };
    let input: String = rows.iter().map(|r| format!("{}\t{}\n", r.key, r.line)).collect();
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    let status = child.wait();
    drop(cache);
    if !status.map(|s| s.success()).unwrap_or(false) {
        return None;
    }
    let mut lines = out.lines();
    let action = if actions.is_empty() {
        "enter".to_string()
    } else {
        lines.next().unwrap_or("enter").to_string()
    };
    let keys = lines
        .filter(|l| !l.is_empty())
        .map(|l| l.split('\t').next().unwrap_or("").to_string())
        .collect::<Vec<_>>();
    (!keys.is_empty()).then_some(Selection { action, keys })
}

/// `actx _show <cache> <key>`: what fzf runs to show a preview.
pub fn show_preview(cache: &str, key: &str) {
    if let Ok(text) = std::fs::read_to_string(cache)
        && let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&text)
    {
        println!("{}", map.get(key).map(String::as_str).unwrap_or(""));
    }
}

#[cfg(test)]
mod width_tests {
    use super::*;
    #[test]
    fn aligns_and_clips_cjk_by_visible_columns() {
        assert_eq!(visible_width("中文abc"), 7);
        assert_eq!(pad("中文", 6), "中文  ");
        assert_eq!(clip("中文abc", 5), "中文…");
        assert_eq!(clip("中文", 0), "");
        assert_eq!(clip("中文", 1), "…");
        assert_eq!(clip("e\u{301}cole", 3), "e\u{301}c…");
        assert_eq!(visible_width(&clip("🙂中文", 4)), 3);
    }
}
