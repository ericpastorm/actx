//! Talking to the user: colors, messages, questions, previews and the fzf pickers.

use std::collections::HashMap;
use std::io::{IsTerminal, Read, Write};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{Local, TimeZone};

use crate::ctx::ctx;
use crate::tr;

pub const N_PREVIEW: usize = 14;

pub fn paint(code: &str, s: &str) -> String {
    if ctx().no_color {
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

/// Automatic runs: nobody is watching, output goes to a log.
pub fn set_auto() {
    AUTO.store(true, Ordering::Relaxed)
}

pub fn is_auto() -> bool {
    AUTO.load(Ordering::Relaxed)
}

/// Print a line (in automatic runs, with the time: it's a log).
pub fn say(msg: &str) {
    if is_auto() {
        println!("{} {msg}", local_time(crate::ctx::now(), "%Y-%m-%d %H:%M:%S"));
    } else {
        println!("{msg}");
    }
}

pub fn die(msg: &str) -> ! {
    eprintln!("{}{msg}", paint("31", "✗ "));
    std::process::exit(1)
}

pub fn warn(msg: &str) {
    say(&format!("{}{msg}", paint("33", "⚠ ")));
}

pub fn ok(msg: &str) {
    say(&format!("{}{msg}", paint("32", "✓ ")));
}

pub fn fail(msg: &str) {
    say(&format!("{}{msg}", paint("31", "✗ ")));
}

/// Is there someone to ask? (a terminal, or a scripted answer in tests)
pub fn interactive() -> bool {
    ctx().answer.is_some() || std::io::stdin().is_terminal()
}

pub fn ask(question: &str, lower: bool) -> String {
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
    matches!(answer.as_str(), "y" | "yes" | "s" | "si" | "sí")
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

pub fn clip(title: &str, width: usize) -> String {
    if title.chars().count() <= width {
        title.to_string()
    } else {
        format!("{}…", take_chars(title, width - 1))
    }
}

/// Pad to `width` characters, counting characters rather than bytes.
pub fn pad(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - len))
    }
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
pub fn checkbox_picker(rows: &[Row], header: &str, actions: &[&str], selected: &[String]) -> Option<Selection> {
    if ctx().pick.is_some() {
        return picker(rows, header, true, false, actions, selected);
    }
    let mut checked = selected.to_vec();
    let mut focus = Vec::new();
    loop {
        let mut choices: Vec<Row> = rows
            .iter()
            .map(|row| Row {
                key: row.key.clone(),
                line: format!("{} {}", if checked.contains(&row.key) { "☑" } else { "☐" }, row.line),
                preview: row.preview.clone(),
            })
            .collect();
        choices.push(Row {
            key: "::next".into(),
            line: tr!("Next →"),
            preview: String::new(),
        });
        let pick = picker(&choices, header, false, false, actions, &focus)?;
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
        if checked.contains(key) {
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
