//! Claude Code: one .jsonl per conversation in ~/.claude/projects/<folder name>/, a chain of
//! messages linked by parentUuid.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

use regex::bytes::Regex;
use serde_json::{Value, json};

use crate::ctx::{Result, ctx, jstr, now_ms};
use crate::paths::{Fix, apply, encode_claude, resolve_encoded};
use crate::session::Session;
use crate::state::{Copy, backup};
use crate::store::{COMP, Pushed, complete_lines, open_read, stored_lines, tar_dir, untar, write_compressed};
use crate::tr;
use crate::turns::{OUT_MAX, Turn, add_turn, as_text, iso, iso_ms, tool_call, tool_text};
use crate::ui::render;

static CWD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""cwd":"((?:[^"\\]|\\.)*)""#).unwrap());

/// One line the way Claude Code writes them: compact JSON (what local() and last_entry look for).
pub fn jsonl(v: &Value) -> String {
    format!("{}\n", serde_json::to_string(v).unwrap_or_default())
}

/// Start and end of a .jsonl: sessions can be close to 1 GB, no need to read them whole.
pub fn chunks(f: &Path) -> io::Result<(Vec<u8>, Vec<u8>)> {
    const HEAD: u64 = 256 << 10;
    const TAIL: u64 = 2 << 20;
    let mut file = File::open(f)?;
    let size = file.metadata()?.len();
    let mut head = Vec::new();
    (&mut file).take(HEAD).read_to_end(&mut head)?;
    if size <= HEAD {
        return Ok((head.clone(), head));
    }
    file.seek(SeekFrom::Start(HEAD.max(size.saturating_sub(TAIL))))?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail)?;
    Ok((head, tail))
}

/// How much conversation a file has: up to the end of its last message, said or answered
/// ("type": "user" or "assistant"). Opening and closing a conversation appends lines of its own
/// (mode, costs, the last prompt, the output of SessionStart hooks, notices…): they don't count,
/// or merely looking at one would change it.
pub fn content_weight(f: &Path) -> io::Result<u64> {
    #[derive(serde::Deserialize)]
    struct Line {
        #[serde(rename = "type")]
        kind: Option<String>,
    }
    let mut file = File::open(f)?;
    let mut end = file.metadata()?.len(); // what's before `end` is still to look at
    let mut chunk: u64 = 2 << 20;
    while end > 0 {
        let start = end.saturating_sub(chunk);
        let mut buf = Vec::new();
        file.seek(SeekFrom::Start(start))?;
        (&mut file).take(end - start).read_to_end(&mut buf)?;
        let complete = memchr::memrchr(b'\n', &buf).map_or(0, |i| i + 1); // a line being written doesn't count yet
        // the first line may have begun before this chunk: it's looked at with the next one
        let first = if start == 0 {
            0
        } else {
            memchr::memchr(b'\n', &buf[..complete]).map_or(complete, |i| i + 1)
        };
        if first == complete && start > 0 {
            chunk *= 2; // no whole line in it: a long one
            continue;
        }
        let mut line_end = complete;
        for line in buf[first..complete.saturating_sub(1)].rsplit(|b| *b == b'\n') {
            if memchr::memmem::find(line, b"\"uuid\":\"").is_some() {
                let kind = serde_json::from_slice::<Line>(line).ok().and_then(|l| l.kind);
                if matches!(kind.as_deref(), Some("user" | "assistant")) {
                    return Ok(start + line_end as u64);
                }
            }
            line_end = line_end.saturating_sub(line.len() + 1);
        }
        if start == 0 {
            break;
        }
        end = start + first as u64;
        chunk = 2 << 20;
    }
    Ok(0)
}

/// The JSON lines of a buffer, skipping any cut by a chunk boundary.
pub fn lines(buf: &[u8]) -> impl Iterator<Item = Value> + '_ {
    buf.split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_slice(l).ok())
}

fn last_entry(buf: &[u8], kind: &str) -> Option<Value> {
    let needle = format!("\"type\":\"{kind}\"");
    let i = memchr::memmem::rfind(buf, needle.as_bytes())?;
    let start = buf[..i].iter().rposition(|b| *b == b'\n').map_or(0, |p| p + 1);
    let end = memchr::memchr(b'\n', &buf[i..]).map_or(buf.len(), |p| i + p);
    serde_json::from_slice(&buf[start..end]).ok()
}

fn flag(e: &Value, key: &str) -> bool {
    e.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn user_text(e: &Value) -> Option<String> {
    if jstr(e, "type") != "user" || flag(e, "isMeta") || flag(e, "isSidechain") {
        return None;
    }
    let blocks: Vec<&str> = match e.get("message").and_then(|m| m.get("content")) {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(items)) => items
            .iter()
            .filter(|b| jstr(b, "type") == "text")
            .map(|b| jstr(b, "text"))
            .collect(),
        _ => return None,
    };
    // leave out system reminders, local command output, etc.
    let text = blocks
        .into_iter()
        .filter(|b| !b.trim_start().starts_with('<'))
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn assistant_text(e: &Value) -> Option<String> {
    if jstr(e, "type") != "assistant" || flag(e, "isSidechain") {
        return None;
    }
    let items = e.get("message")?.get("content")?.as_array()?;
    let text = items
        .iter()
        .filter(|b| jstr(b, "type") == "text")
        .map(|b| jstr(b, "text"))
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

pub fn messages(buf: &[u8]) -> Vec<(String, String)> {
    let mut msgs = Vec::new();
    for e in lines(buf) {
        if let Some(t) = user_text(&e) {
            msgs.push(("user".to_string(), t));
        } else if let Some(t) = assistant_text(&e) {
            msgs.push(("assistant".to_string(), t));
        }
    }
    msgs
}

fn title(head: &[u8], tail: &[u8]) -> String {
    for (kind, field) in [
        ("custom-title", "customTitle"),
        ("ai-title", "aiTitle"),
        ("summary", "summary"),
    ] {
        for buf in [tail, head] {
            if let Some(e) = last_entry(buf, kind) {
                let t = jstr(&e, field);
                if !t.is_empty() {
                    return t.to_string();
                }
            }
        }
    }
    lines(head)
        .find_map(|e| user_text(&e))
        .map(|t| crate::ui::take_chars(t.lines().next().unwrap_or(""), 80).to_string())
        .unwrap_or_else(|| tr!("(untitled)"))
}

/// The project's real path. True if it comes from a cwd that matches the folder name.
fn project_path(dirname: &str, head: &[u8], tail: &[u8]) -> (String, bool) {
    let mut any_cwd = None;
    for buf in [tail, head] {
        for cap in CWD_RE.captures_iter(buf) {
            let quoted = [b"\"".as_slice(), &cap[1], b"\""].concat();
            if let Ok(cwd) = serde_json::from_slice::<String>(&quoted)
                && !cwd.is_empty()
            {
                if encode_claude(&cwd) == dirname {
                    return (cwd, true);
                }
                if any_cwd.is_none() {
                    any_cwd = Some(cwd);
                }
            }
        }
    }
    if let Some(resolved) = resolve_encoded(dirname).or(any_cwd) {
        return (resolved, false);
    }
    let fallback = if dirname.starts_with('-') {
        crate::paths::native(&dirname.replace('-', "/"))
    } else if let [drive, b'-', b'-', ..] = dirname.as_bytes()
        && drive.is_ascii_alphabetic()
    {
        crate::paths::native(&format!("{}:/{}", *drive as char, dirname[3..].replace('-', "/")))
    } else {
        String::new()
    };
    (fallback, false)
}

fn epoch(t: std::time::SystemTime) -> f64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

pub fn local() -> Vec<Session> {
    local_keys(None)
}
pub fn local_keys(keys: Option<&std::collections::HashSet<String>>) -> Vec<Session> {
    let mut out = Vec::new();
    let Ok(dirs) = std::fs::read_dir(&ctx().claude_projects) else {
        return out;
    };
    for pdir in dirs.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()) {
        let dirname = pdir.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let (mut path, mut exact) = (String::new(), false);
        let mut found = Vec::new();
        let Ok(files) = std::fs::read_dir(&pdir) else { continue };
        for f in files.filter_map(|e| e.ok()).map(|e| e.path()) {
            if f.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let id = f.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            if keys.is_some_and(|keys| !keys.contains(&format!("claude:{id}"))) {
                continue;
            }
            if crate::work::cancelled() {
                break;
            }
            let row: Result<Option<(crate::discovery::Record, bool)>> =
                crate::discovery::memo("claude-session", &f, &Value::Null, || {
                    let meta = f.metadata()?;
                    let (head, tail) = chunks(&f)?;
                    let has = |b: &[u8]| memchr::memmem::find(b, b"\"type\":\"assistant\"").is_some();
                    if !has(&head) && !has(&tail) {
                        return Ok(None);
                    }
                    let (folder, verified) = project_path(&dirname, &head, &tail);
                    let mut s = Session::local(
                        "claude",
                        id.clone(),
                        title(&head, &tail),
                        folder,
                        meta.modified().map(epoch).unwrap_or(0.),
                        content_weight(&f).unwrap_or(meta.len()),
                    );
                    s.size = Some(meta.len());
                    Ok(Some((crate::discovery::Record::from(&s), verified)))
                });
            match row {
                Ok(Some((row, verified))) => {
                    let s = row.session("claude", Some(&f));
                    if !exact {
                        path = s.path.clone();
                        exact = verified;
                    }
                    found.push(s);
                }
                Ok(None) => {}
                Err(e) => {
                    if crate::inspection::enabled() {
                        crate::inspection::record_error(e.to_string());
                    }
                }
            }
        }
        for mut s in found {
            s.path = path.clone();
            out.push(s);
        }
    }
    out
}

/// Include native child transcripts for the explicit cleanup workflow only.
pub fn clean_local() -> Vec<Session> {
    let mut sessions = local();
    for project in std::fs::read_dir(&ctx().claude_projects)
        .into_iter()
        .flatten()
        .flatten()
    {
        let dirname = project.file_name().to_string_lossy().into_owned();
        for parent in std::fs::read_dir(project.path()).into_iter().flatten().flatten() {
            let children = parent.path().join("subagents");
            for entry in std::fs::read_dir(children).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                if crate::work::cancelled() {
                    break;
                }
                let row = crate::discovery::session("claude", &path, &json!({"child":true}), || {
                    let (head, tail) = chunks(&path)?;
                    let meta = path.metadata()?;
                    let id = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                    let mut s = Session::local(
                        "claude",
                        id,
                        title(&head, &tail),
                        project_path(&dirname, &head, &tail).0,
                        meta.modified().map(epoch).unwrap_or(0.),
                        meta.len(),
                    );
                    s.size = Some(meta.len());
                    Ok(Some(s))
                });
                match row {
                    Ok(Some(s)) => sessions.push(s),
                    Ok(None) => {}
                    Err(e) => {
                        if crate::inspection::enabled() {
                            crate::inspection::record_error(e.to_string());
                        }
                    }
                }
            }
        }
    }
    sessions
}

pub fn preview(s: &Session) -> Vec<(String, String)> {
    s.src
        .as_ref()
        .and_then(|f| chunks(f).ok())
        .map(|(_, tail)| messages(&tail))
        .unwrap_or_default()
}

pub fn push(s: &Session, tmp: &Path) -> Result<Pushed> {
    let src = s.src.as_ref().expect("a local Claude conversation");
    let updated = epoch(src.metadata()?.modified()?);
    let session = format!("session.jsonl.{COMP}");
    // an agent may be writing to it right now: up to its last complete line
    let weight = content_weight(src)?;
    let (file, _) = complete_lines(src)?;
    write_compressed(&tmp.join(&session), file)?;
    let mut files = json!({"session": session});
    let extra = src.with_extension(""); // <id>/ with subagents and tool results
    if extra.is_dir() {
        let name = format!("extra.tar.{COMP}");
        tar_dir(&tmp.join(&name), &extra, &s.id)?;
        files["extra"] = json!(name);
    }
    let preview = render(&messages(&chunks(src)?.1), "claude");
    Ok(Pushed {
        files,
        updated,
        weight,
        seen: weight,
        preview,
    })
}

fn set_mtime(path: &Path, ts: f64) -> Result<()> {
    let when = UNIX_EPOCH + Duration::from_secs_f64(ts.max(0.0));
    File::options().write(true).open(path)?.set_modified(when)?;
    Ok(())
}

fn private(path: &Path) -> Result<()> {
    crate::paths::make_private(path)?;
    Ok(())
}

/// Write a stored conversation file to `dest` (atomically), with this machine's paths.
pub fn install_file(r: &Session, dest: &Path, fix: Fix, work_folder: Option<&str>) -> Result<u64> {
    let before = std::fs::read(dest).ok().map(|data| crate::engine::digest(&data));
    let tmp = dest.with_file_name(format!(
        ".{}.actx-tmp",
        dest.file_name().unwrap_or_default().to_string_lossy()
    ));
    let result = (|| -> Result<()> {
        let mut out = io::BufWriter::new(File::create(&tmp)?);
        let mut reader = open_read(&r.stored_file(None))?;
        let origin = serde_json::to_vec(&r.meta()["path"])?;
        let origin: Value = serde_json::from_slice(&apply(fix, &origin))?;
        match (fix, work_folder) {
            (None, None) => {
                io::copy(&mut reader, &mut out)?;
            }
            _ => {
                let mut line = Vec::new();
                while io::BufRead::read_until(&mut reader, b'\n', &mut line)? > 0 {
                    let mut data = apply(fix, &line);
                    if let Some(folder) = work_folder
                        && memchr::memmem::find(&data, b"\"cwd\"").is_some()
                        && let Ok(mut entry) = serde_json::from_slice::<Value>(&data)
                    {
                        if r.harness == "codex"
                            && matches!(crate::ctx::jstr(&entry, "type"), "session_meta" | "turn_context")
                        {
                            entry["payload"]["cwd"] = serde_json::json!(folder);
                            data = jsonl(&entry).into_bytes();
                        } else if r.harness == "claude" && entry.get("cwd").is_some_and(|cwd| cwd == &origin) {
                            entry["cwd"] = serde_json::json!(folder);
                            data = jsonl(&entry).into_bytes();
                        }
                    }
                    out.write_all(&data)?;
                    line.clear();
                }
            }
        }
        out.flush()?;
        drop(out);
        private(&tmp)?;
        set_mtime(&tmp, r.meta().get("updated").and_then(Value::as_f64).unwrap_or(0.0))?;
        if std::fs::read(dest).ok().map(|data| crate::engine::digest(&data)) != before {
            return Err(crate::ctx::Error(
                "Local history changed during download; the existing file was kept".into(),
            ));
        }
        std::fs::rename(&tmp, dest)?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&tmp);
    result?;
    Ok(dest.metadata()?.len())
}

pub fn install(r: &Session, local: Option<&Session>, folder: &str, fix: Fix) -> Result<u64> {
    let _loading = crate::ui::loading(&crate::tr!("Installing {title}", title = r.title));
    let tdir = ctx().claude_projects.join(encode_claude(folder));
    std::fs::create_dir_all(&tdir)?;
    let dest = tdir.join(format!("{}.jsonl", r.id));
    let local_src = local.and_then(|l| l.src.clone());
    if let (Some(l), Some(src)) = (local, &local_src) {
        backup(l, Copy::File(src), "pull")?;
    }
    let project = r.meta().get("project_folder");
    let outside = project.is_some_and(|project| {
        let source = crate::paths::slashed(crate::ctx::jstr(r.meta(), "path"));
        let project = crate::paths::slashed(crate::ctx::jstr(project, "path"));
        source != project && !source.starts_with(&format!("{project}/"))
    });
    install_file(r, &dest, fix, outside.then_some(folder))?;
    if let Some(src) = local_src.filter(|s| *s != dest) {
        let _ = std::fs::remove_file(src); // it lived under another folder; it has a backup
    }
    if let Some(extra) = r.meta()["files"].get("extra").and_then(Value::as_str) {
        untar(&r.store_dir().join(extra), &tdir)?;
    }
    Ok(content_weight(&dest)?)
}

enum Block {
    Text(String),
    Call(String, String),
    Result(String, String, usize),
}

struct Slim {
    parent: Option<String>,
    kind: String,
    is_meta: bool,
    ts: String,
    blocks: Vec<Block>,
}

/// Turns (who, text, ms) of the active branch of a Claude Code conversation.
///
/// parentUuid is followed back from the last message, so /rewind and /compact are respected
/// (the chain stops at the compaction, and the summary becomes the first message).
pub fn turns<L: AsRef<[u8]>>(raw_lines: impl IntoIterator<Item = L>) -> Vec<Turn> {
    let (mut entries, mut last, mut loose) = (HashMap::new(), None::<String>, Vec::new());
    for raw in raw_lines {
        let Ok(e) = serde_json::from_slice::<Value>(raw.as_ref()) else {
            continue;
        };
        let kind = jstr(&e, "type").to_string();
        let uuid = e
            .get("uuid")
            .and_then(Value::as_str)
            .filter(|u| !u.is_empty())
            .map(String::from);
        if kind != "user" && kind != "assistant" && uuid.is_none() {
            continue; // anything with a uuid is part of the chain (attachments, notices...), even if not converted
        }
        let content = e
            .get("message")
            .and_then(|m| m.get("content"))
            .cloned()
            .unwrap_or(Value::Null);
        let content = match content {
            Value::String(s) => vec![json!({"type": "text", "text": s})],
            Value::Array(items) => items,
            _ => Vec::new(),
        };
        let mut blocks = Vec::new();
        for b in &content {
            match jstr(b, "type") {
                "text" => blocks.push(Block::Text(jstr(b, "text").to_string())),
                "tool_use" => blocks.push(Block::Call(
                    jstr(b, "id").to_string(),
                    tool_call(
                        b.get("name").and_then(Value::as_str).unwrap_or("?"),
                        b.get("input").unwrap_or(&Value::Null),
                    ),
                )),
                "tool_result" => {
                    let out = as_text(b.get("content").unwrap_or(&Value::Null));
                    let total = out.chars().count();
                    blocks.push(Block::Result(
                        jstr(b, "tool_use_id").to_string(),
                        crate::ui::take_chars(&out, OUT_MAX).to_string(),
                        total,
                    ));
                }
                _ => {}
            }
        }
        let slim = Slim {
            parent: e.get("parentUuid").and_then(Value::as_str).map(String::from),
            is_meta: flag(&e, "isMeta"),
            ts: jstr(&e, "timestamp").to_string(),
            blocks,
            kind: kind.clone(),
        };
        let Some(uuid) = uuid else {
            loose.push(slim); // no chain: file order it is
            continue;
        };
        if (kind == "user" || kind == "assistant") && !flag(&e, "isSidechain") {
            last = Some(uuid.clone());
        }
        entries.insert(uuid, slim);
    }
    let (mut chain, mut seen) = (Vec::new(), HashSet::new());
    while let Some(id) = last.take() {
        let Some(e) = entries.get(&id) else { break };
        if !seen.insert(id.clone()) {
            break;
        }
        chain.push(e);
        last = e.parent.clone();
    }
    let ordered: Vec<&Slim> = if chain.is_empty() {
        loose.iter().collect()
    } else {
        chain.into_iter().rev().collect()
    };
    let (mut turns, mut calls) = (Vec::new(), HashMap::new());
    for e in ordered {
        let ts = iso_ms(&e.ts);
        if e.kind == "assistant" {
            for b in &e.blocks {
                match b {
                    Block::Text(t) => add_turn(&mut turns, "assistant", t, ts),
                    Block::Call(id, call) => {
                        calls.insert(id.clone(), call.clone());
                    }
                    Block::Result(..) => {}
                }
            }
        } else if e.kind == "user" && !e.is_meta {
            let mut said = Vec::new();
            for b in &e.blocks {
                match b {
                    Block::Result(id, out, total) => {
                        let call = calls.get(id).map(String::as_str).unwrap_or("‹tool›");
                        add_turn(&mut turns, "assistant", &tool_text(call, out, Some(*total)), ts);
                    }
                    Block::Text(t) if !t.trim_start().starts_with('<') => said.push(t.as_str()), // leave out system reminders
                    _ => {}
                }
            }
            add_turn(&mut turns, "user", &said.join("\n"), ts);
        }
    }
    turns
}

pub fn version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        crate::ctx::command("claude")
            .arg("--version")
            .env_remove("CLAUDECODE")
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split_whitespace()
                    .next()
                    .map(String::from)
            })
            .unwrap_or_else(|| "2.1.0".to_string())
    })
}

fn new_file(folder: &str) -> Result<(String, PathBuf)> {
    let sid = uuid::Uuid::new_v4().to_string();
    let tdir = ctx().claude_projects.join(encode_claude(folder));
    std::fs::create_dir_all(&tdir)?;
    let path = tdir.join(format!("{sid}.jsonl"));
    Ok((sid, path))
}

fn finished(sid: String, title: &str, folder: &str, path: PathBuf) -> Result<Session> {
    private(&path)?;
    let meta = path.metadata()?;
    let mut s = Session::local(
        "claude",
        sid,
        title.to_string(),
        folder.to_string(),
        epoch(meta.modified()?),
        meta.len(),
    );
    s.src = Some(path);
    Ok(s)
}

pub fn write(turns: &[Turn], folder: &str, title: &str, model: Option<&str>) -> Result<Session> {
    let (sid, path) = new_file(folder)?;
    let mut out = io::BufWriter::new(File::create(&path)?);
    let mut parent: Option<String> = None;
    for (n, t) in turns.iter().enumerate() {
        let message = if t.who == "user" {
            json!({"role": "user", "content": t.text})
        } else {
            json!({
                "model": model.unwrap_or("imported"), "id": format!("msg_actx_{n:05}"), "type": "message",
                "role": "assistant", "content": [{"type": "text", "text": t.text}], "stop_reason": "end_turn",
                "stop_sequence": null, "usage": {"input_tokens": 0, "output_tokens": 0},
            })
        };
        let u = uuid::Uuid::new_v4().to_string();
        out.write_all(
            jsonl(&json!({
                "parentUuid": parent, "isSidechain": false, "type": t.who, "message": message, "uuid": u,
                "timestamp": iso(t.ts), "userType": "external", "entrypoint": "cli",
                "cwd": folder, "sessionId": sid, "version": version(),
            }))
            .as_bytes(),
        )?;
        parent = Some(u);
    }
    out.write_all(jsonl(&json!({"type": "ai-title", "aiTitle": title, "sessionId": sid})).as_bytes())?;
    out.flush()?;
    drop(out);
    finished(sid, title, folder, path)
}

/// What was done on the other machine since both versions split.
pub fn other_branch(l: &Session, r: &Session, fix: Fix) -> Result<Vec<Turn>> {
    let mine: HashSet<String> = lines(&std::fs::read(l.src.as_ref().expect("local"))?)
        .filter_map(|e| e.get("uuid").and_then(Value::as_str).map(String::from))
        .collect();
    let theirs: Vec<Vec<u8>> = stored_lines(&r.stored_file(None), fix)?
        .into_iter()
        .filter(|line| {
            serde_json::from_slice::<Value>(line)
                .ok()
                .is_some_and(|e| !e.get("uuid").and_then(Value::as_str).is_some_and(|u| mine.contains(u)))
        })
        .collect();
    Ok(turns(theirs)) // the chain stops by itself where the shared part begins
}

pub fn append(l: &mut Session, text: &str, ack: &str) -> Result<()> {
    let src = l.src.clone().expect("a local Claude conversation");
    // the last message of the conversation, to hang the new ones from, and the cwd it was using
    let scan = |buf: &[u8], cwd: &mut String| {
        let mut leaf = None;
        for e in lines(buf) {
            if let Some(u) = e.get("uuid").and_then(Value::as_str)
                && matches!(jstr(&e, "type"), "user" | "assistant")
                && !flag(&e, "isSidechain")
            {
                leaf = Some(u.to_string());
                if !jstr(&e, "cwd").is_empty() {
                    *cwd = jstr(&e, "cwd").to_string();
                }
            }
        }
        leaf
    };
    let mut cwd = l.path.clone();
    let mut leaf = scan(&chunks(&src)?.1, &mut cwd);
    if leaf.is_none() {
        leaf = scan(&std::fs::read(&src)?, &mut cwd);
    }
    let now = iso(now_ms());
    let (u1, u2) = (uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string());
    let common = |parent: Option<&str>, kind: &str, uuid: &str, message: Value| {
        json!({
            "isSidechain": false, "userType": "external", "entrypoint": "cli", "cwd": cwd,
            "sessionId": l.id, "version": version(), "timestamp": now,
            "parentUuid": parent, "type": kind, "uuid": uuid, "message": message,
        })
    };
    let reply = json!({
        "model": "actx", "id": format!("msg_actx_{}", &u2[..8]), "type": "message", "role": "assistant",
        "content": [{"type": "text", "text": ack}], "stop_reason": "end_turn", "stop_sequence": null,
        "usage": {"input_tokens": 0, "output_tokens": 0},
    });
    let mut f = File::options().append(true).open(&src)?;
    f.write_all(
        jsonl(&common(
            leaf.as_deref(),
            "user",
            &u1,
            json!({"role": "user", "content": text}),
        ))
        .as_bytes(),
    )?;
    f.write_all(jsonl(&common(Some(&u1), "assistant", &u2, reply)).as_bytes())?;
    let meta = src.metadata()?;
    l.weight = meta.len();
    l.updated = epoch(meta.modified()?);
    Ok(())
}

/// The stored version, whole, as a new conversation (new ids so it doesn't clash with the original).
pub fn branch_copy(r: &Session, folder: &str, fix: Fix, title: &str) -> Result<Session> {
    let (sid, path) = new_file(folder)?;
    let mut renamed: HashMap<String, String> = HashMap::new();
    let mut out = io::BufWriter::new(File::create(&path)?);
    for line in stored_lines(&r.stored_file(None), fix)? {
        let Ok(mut e) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        if matches!(jstr(&e, "type"), "ai-title" | "custom-title" | "summary") {
            continue;
        }
        for key in ["uuid", "parentUuid", "logicalParentUuid", "leafUuid"] {
            if let Some(old) = e
                .get(key)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(String::from)
            {
                let new = renamed
                    .entry(old)
                    .or_insert_with(|| uuid::Uuid::new_v4().to_string())
                    .clone();
                e[key] = json!(new);
            }
        }
        if e.get("sessionId").is_some() {
            e["sessionId"] = json!(sid);
        }
        out.write_all(jsonl(&e).as_bytes())?;
    }
    out.write_all(jsonl(&json!({"type": "ai-title", "aiTitle": title, "sessionId": sid})).as_bytes())?;
    out.flush()?;
    drop(out);
    finished(sid, title, folder, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookkeeping_lines_dont_count() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("s.jsonl");
        let msg = |n: u32| {
            json!({"type": "user", "uuid": format!("u{n}"), "message": {"content": "x".repeat(1000)}}).to_string()
                + "\n"
        };
        let messages: String = (0..3000).map(msg).collect(); // over one 2MB read
        // resuming one adds these too, with a uuid: SessionStart hooks' output, notices
        let book = concat!(
            "{\"type\":\"cost-state\",\"sessionId\":\"s\"}\n{\"type\":\"last-prompt\",\"leafUuid\":\"u2\"}\n",
            "{\"parentUuid\":\"u2\",\"type\":\"attachment\",\"attachment\":{\"type\":\"hook_success\",\"content\":\"{\\\"type\\\":\\\"user\\\"}\"},\"uuid\":\"a1\"}\n",
            "{\"parentUuid\":\"a1\",\"type\":\"system\",\"subtype\":\"informational\",\"uuid\":\"a2\"}\n",
        );
        std::fs::write(&f, format!("{messages}{book}{{\"type\":\"user\",\"uuid\":\"half")).unwrap();
        assert_eq!(content_weight(&f).unwrap(), messages.len() as u64);
        std::fs::write(&f, book).unwrap();
        assert_eq!(content_weight(&f).unwrap(), 0);
        // a message longer than one read, with a notice after it
        let long = msg(1).replace(&"x".repeat(1000), &"y".repeat(5 << 20));
        std::fs::write(&f, format!("{}{long}{book}", msg(0))).unwrap();
        assert_eq!(content_weight(&f).unwrap(), (msg(0).len() + long.len()) as u64);
    }

    #[test]
    fn the_title_is_found_whatever_the_size() {
        let dir = tempfile::tempdir().unwrap();
        for size in [10_000usize, 1 << 20, 3 << 20] {
            let f = dir.path().join(format!("{size}.jsonl"));
            let filler = json!({"type": "user", "message": {"content": "x".repeat(1000)}}).to_string() + "\n";
            let mut text = filler.repeat(size / filler.len());
            text += &(json!({"type": "ai-title", "aiTitle": "Late title"}).to_string() + "\n");
            std::fs::write(&f, text).unwrap();
            let (head, tail) = chunks(&f).unwrap();
            assert_eq!(title(&head, &tail), "Late title", "{size} bytes");
        }
    }

    fn entry(kind: &str, content: Value, parent: Option<&str>, extra: Value) -> (String, String) {
        let u = uuid::Uuid::new_v4().to_string();
        let mut e = json!({"type": kind, "uuid": u, "parentUuid": parent, "message": {"role": kind, "content": content},
                           "timestamp": "2026-01-01T10:00:00.000Z"});
        for (k, v) in extra.as_object().unwrap() {
            e[k] = v.clone();
        }
        (u, e.to_string())
    }

    #[test]
    fn turns_follow_the_active_branch() {
        let mut ls = Vec::new();
        let mut add = |kind: &str, content: Value, parent: Option<&str>, extra: Value| {
            let (u, line) = entry(kind, content, parent, extra);
            ls.push(line);
            u
        };
        let a = add("user", json!("old question before compacting"), None, json!({}));
        add(
            "assistant",
            json!([{"type": "text", "text": "old answer"}]),
            Some(&a),
            json!({}),
        );
        let b = "b".to_string();
        let s = add(
            "user",
            json!("Summary of the earlier conversation"),
            Some(&b),
            json!({}),
        );
        let q = add(
            "user",
            json!([{"type": "text", "text": "<system-reminder>ignore me</system-reminder>"},
                                   {"type": "text", "text": "fix the cart"}]),
            Some(&s),
            json!({}),
        );
        let call = add(
            "assistant",
            json!([{"type": "thinking", "thinking": "hmm"},
                                           {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}}]),
            Some(&q),
            json!({}),
        );
        let res = add(
            "user",
            json!([{"type": "tool_result", "tool_use_id": "t1", "content": "x".repeat(2000)}]),
            Some(&call),
            json!({}),
        );
        add(
            "assistant",
            json!([{"type": "text", "text": "rewound answer (dropped)"}]),
            Some(&res),
            json!({}),
        );
        add(
            "assistant",
            json!([{"type": "text", "text": "subagent work"}]),
            Some(&res),
            json!({"isSidechain": true}),
        );
        let fin = add(
            "assistant",
            json!([{"type": "text", "text": "cart fixed"}]),
            Some(&res),
            json!({}),
        );
        add(
            "user",
            json!("<local-command-stdout>noise</local-command-stdout>"),
            Some(&fin),
            json!({"isMeta": true}),
        );
        ls.insert(
            2,
            json!({"type": "system", "subtype": "compact_boundary", "uuid": "b", "parentUuid": null}).to_string(),
        );

        let t = turns(&ls);
        assert_eq!(t[0].who, "user");
        assert_eq!(t[0].text, "Summary of the earlier conversation\n\nfix the cart");
        assert!(t[1].text.starts_with(&format!("‹Bash› $ ls\n{}", "x".repeat(1500))));
        assert!(t[1].text.contains("500 more characters"));
        assert!(t[1].text.ends_with("cart fixed"));
        for turn in &t {
            for word in ["old", "rewound", "subagent", "noise"] {
                assert!(!turn.text.contains(word), "{word} leaked");
            }
        }
    }

    #[test]
    fn turns_without_uuids_use_file_order() {
        let ls = [
            r#"{"type":"user","message":{"content":"hi"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello"}]}}"#,
        ];
        let t = turns(ls);
        assert_eq!(
            t.iter().map(|t| (t.who, t.text.as_str())).collect::<Vec<_>>(),
            [("user", "hi"), ("assistant", "hello")]
        );
    }
}
