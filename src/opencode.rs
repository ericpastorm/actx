//! opencode. 1.x and 2.x store sessions differently (tables session/message/part vs session_v2/
//! session_message) and export them differently (per message {info, parts} vs flat messages).
//! Both are handled; a stored conversation remembers which one it came from.

use std::io::{Read, Seek};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

use crate::ctx::{Error, Result, ctx, jstr, now, now_ms, which};
use crate::paths::{Fix, apply};
use crate::session::Session;
use crate::state::{Copy, backup};
use crate::store::{COMP, Pushed, read_all, write_compressed};
use crate::tr;
use crate::turns::{Turn, add_turn, as_text, tool_call, tool_text};
use crate::ui::{render, warn};

fn db() -> Option<Connection> {
    let path = &ctx().opencode_db;
    if !path.exists() {
        return None;
    }
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()
}

/// opencode's major version here (1 or 2): from its database, or its CLI when there's no database yet.
pub fn version() -> Option<u8> {
    static VERSION: OnceLock<Option<u8>> = OnceLock::new();
    *VERSION.get_or_init(|| {
        if let Some(con) = db() {
            let tables: Vec<String> = con
                .prepare("select name from sqlite_master where type = 'table'")
                .and_then(|mut st| st.query_map([], |r| r.get(0))?.collect())
                .unwrap_or_default();
            if tables.iter().any(|t| t == "session_v2") {
                return Some(2);
            }
            if tables.iter().any(|t| t == "session") {
                return Some(1);
            }
        }
        if !which("opencode") {
            return None;
        }
        let out = crate::ctx::command("opencode")
            .args(["session", "--help"])
            .output()
            .ok()?;
        let help = [out.stdout, out.stderr].concat();
        Some(if memchr::memmem::find(&help, b"import").is_some() {
            2
        } else {
            1
        })
    })
}

fn query<T>(
    sql: &str,
    params: impl rusqlite::Params,
    map: impl FnMut(&rusqlite::Row) -> rusqlite::Result<T>,
) -> Vec<T> {
    static WARNED: AtomicBool = AtomicBool::new(false);
    let Some(con) = db() else { return Vec::new() };
    let rows = con
        .prepare(sql)
        .and_then(|mut st| st.query_map(params, map)?.collect::<rusqlite::Result<Vec<T>>>());
    rows.unwrap_or_else(|e| {
        if !WARNED.swap(true, Ordering::Relaxed) {
            warn(&tr!(
                "Can't read the opencode database ({err}); skipping opencode",
                err = e
            ));
        }
        Vec::new()
    })
}

pub fn local() -> Vec<Session> {
    let (sessions, messages, user) = if version() == Some(2) {
        ("session_v2", "session_message", "m.type = 'user'")
    } else {
        ("session", "message", "json_extract(m.data, '$.role') = 'user'")
    };
    let sql = format!(
        "select s.id, s.title, s.directory, s.time_updated,
                (select count(*) from {messages} m where m.session_id = s.id),
                (select count(*) from {messages} m where m.session_id = s.id and {user})
         from {sessions} s where s.parent_id is null and s.time_archived is null"
    );
    query(&sql, [], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, i64>(5)?,
        ))
    })
    .into_iter()
    .filter(|row| row.5 > 0)
    .map(|(id, title, dir, updated, n, _)| {
        let title = title.filter(|t| !t.is_empty()).unwrap_or_else(|| tr!("(untitled)"));
        Session::local(
            "opencode",
            id,
            title,
            crate::paths::native(&dir),
            updated as f64 / 1000.0,
            n as u64,
        )
    })
    .collect()
}

fn children(sid: &str) -> Vec<String> {
    let table = if version() == Some(2) { "session_v2" } else { "session" };
    let (mut out, mut todo) = (Vec::new(), vec![sid.to_string()]);
    while let Some(id) = todo.pop() {
        for child in query(&format!("select id from {table} where parent_id = ?1"), [&id], |r| {
            r.get::<_, String>(0)
        }) {
            out.push(child.clone());
            todo.push(child);
        }
    }
    out
}

/// Text of an opencode 2.x message.
fn text2(kind: &str, d: &Value) -> Option<String> {
    let text = match kind {
        "user" => jstr(d, "text").to_string(),
        "assistant" => d
            .get("content")
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| jstr(b, "type") == "text")
                    .map(|b| jstr(b, "text"))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn hidden(p: &Value) -> bool {
    p.get("synthetic").and_then(Value::as_bool).unwrap_or(false)
        || p.get("ignored").and_then(Value::as_bool).unwrap_or(false)
}

/// Text of an opencode 1.x message ({info, parts}), without parts opencode added by itself.
fn text1(m: &Value) -> String {
    let parts = m.get("parts").and_then(Value::as_array).cloned().unwrap_or_default();
    parts
        .iter()
        .filter(|p| jstr(p, "type") == "text" && !hidden(p))
        .map(|p| jstr(p, "text"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn messages_of(exp: &Value) -> Vec<Value> {
    exp.get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn format_of(exp: &Value) -> u8 {
    match exp.get("messages").and_then(Value::as_array).and_then(|m| m.first()) {
        Some(first) if first.get("info").is_some() => 1,
        _ => 2,
    }
}

/// (role, text) of an export of either version, for previews.
/// The messages of an export file (a backup, say), for previews.
pub fn file_preview(path: &std::path::Path) -> Vec<(String, String)> {
    std::fs::read(path)
        .ok()
        .and_then(|data| serde_json::from_slice::<Value>(&data).ok())
        .map(|exp| export_messages(&exp))
        .unwrap_or_default()
}

fn export_messages(exp: &Value) -> Vec<(String, String)> {
    let one = format_of(exp) == 1;
    messages_of(exp)
        .iter()
        .filter_map(|m| {
            if one {
                let text = text1(m);
                (!text.is_empty()).then(|| (jstr(&m["info"], "role").to_string(), text))
            } else {
                let kind = jstr(m, "type");
                text2(kind, m).map(|t| (kind.to_string(), t))
            }
        })
        .collect()
}

pub fn preview(s: &Session) -> Vec<(String, String)> {
    const LIMIT: usize = crate::ui::N_PREVIEW * 3;
    if version() == Some(2) {
        let rows = query(
            "select type, data from session_message where session_id = ?1 and type in ('user', 'assistant') order by seq desc limit ?2",
            rusqlite::params![s.id, LIMIT as i64],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        );
        return rows
            .into_iter()
            .rev()
            .filter_map(|(kind, data)| {
                serde_json::from_str::<Value>(&data)
                    .ok()
                    .and_then(|d| text2(&kind, &d))
                    .map(|t| (kind, t))
            })
            .collect();
    }
    let rows = query(
        "select json_extract(m.data, '$.role'),
                (select group_concat(json_extract(p.data, '$.text'), char(10)) from part p
                 where p.message_id = m.id and json_extract(p.data, '$.type') = 'text'
                   and coalesce(json_extract(p.data, '$.synthetic'), 0) = 0)
         from message m where m.session_id = ?1 order by m.time_created desc, m.id desc limit ?2",
        rusqlite::params![s.id, LIMIT as i64],
        |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
    );
    rows.into_iter()
        .rev()
        .filter_map(|(role, text)| {
            let text = text?.trim().to_string();
            (!text.is_empty()).then(|| (role.unwrap_or_default(), text))
        })
        .collect()
}

fn cmd(sub: &[&str], rest: &[&str], cwd: &str) -> Result<Command> {
    if !which("opencode") {
        return Err(Error(tr!("opencode is not installed on this machine")));
    }
    let mut c = crate::ctx::command("opencode");
    c.args(sub).args(&ctx().opencode_args).args(rest);
    c.current_dir(if Path::new(cwd).is_dir() {
        Path::new(cwd)
    } else {
        ctx().home.as_path()
    });
    Ok(c)
}

fn tail(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    crate::ui::last_chars(s.trim(), 300).to_string()
}

pub fn export(sid: &str, cwd: &str) -> Result<Vec<u8>> {
    let sub: &[&str] = if version() == Some(2) {
        &["session", "export"]
    } else {
        &["export"]
    };
    // to a file, not a pipe: through a pipe opencode cuts its output (~320 KB)
    let mut out = tempfile::tempfile()?;
    let res = cmd(sub, &[sid], cwd)?
        .stdout(out.try_clone()?)
        .stderr(Stdio::piped())
        .output()?;
    out.rewind()?;
    let mut data = Vec::new();
    out.read_to_end(&mut data)?;
    if serde_json::from_slice::<Value>(&data).is_err() {
        return Err(Error(tr!(
            "opencode couldn't export {sid}: {err}",
            sid = sid,
            err = tail(&res.stderr)
        )));
    }
    Ok(data)
}

pub fn import(data: &[u8], folder: &str) -> Result<()> {
    let file = tempfile::Builder::new().prefix("actx-").suffix(".json").tempfile()?;
    std::fs::write(file.path(), data)?;
    let path = file.path().to_string_lossy().into_owned();
    let res = if version() == Some(2) {
        cmd(&["session", "import"], &["--directory", folder, &path], folder)?.output()?
    } else {
        cmd(&["import"], &[&path], folder)?.output()? // opencode 1.x imports into the folder it runs in
    };
    let output = [res.stdout, res.stderr].concat();
    if memchr::memmem::find(&output, b"Imported session").is_none() {
        return Err(Error(tr!(
            "opencode didn't accept the conversation: {err}",
            err = tail(&output)
        )));
    }
    Ok(())
}

pub fn delete(sid: &str, cwd: &str) -> Result<()> {
    let res = cmd(&["session", "delete"], &[sid], cwd)?.output()?;
    if !res.status.success() {
        return Err(Error(tr!(
            "opencode couldn't delete {sid}: {err}",
            sid = sid,
            err = tail(&res.stderr)
        )));
    }
    Ok(())
}

fn cli_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        crate::ctx::command("opencode")
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split_whitespace()
                    .last()
                    .map(|v| v.trim_start_matches('v').to_string())
            })
            .unwrap_or_else(|| "1.0.0".to_string())
    })
}

/// opencode-style ids: 12 hex digits of time (reversed for sessions) + 14 random characters.
pub fn oc_id(prefix: &str, ts_ms: i64, counter: u64, descending: bool) -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut v = (ts_ms as u64).wrapping_mul(0x1000).wrapping_add(counter);
    if descending {
        v = !v;
    }
    let random = uuid::Uuid::new_v4().into_bytes();
    let tail: String = random
        .iter()
        .take(14)
        .map(|b| CHARS[*b as usize % CHARS.len()] as char)
        .collect();
    format!("{prefix}_{:012x}{tail}", v & 0xFFFF_FFFF_FFFF)
}

fn slug(title: &str) -> String {
    let s: String = title
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                c
            } else {
                '-'
            }
        })
        .collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    let s = crate::ui::take_chars(&s, 40).to_string();
    if s.is_empty() { "imported".to_string() } else { s }
}

fn zero_tokens() -> Value {
    json!({"input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}})
}

/// A model, as {providerID, modelID, variant}.
#[derive(Clone)]
struct Model {
    provider: String,
    model: String,
    variant: Option<String>,
}

impl Model {
    fn from_fields(d: &Value, model_key: &str) -> Option<Model> {
        let provider = d.get("providerID")?.as_str()?.to_string();
        let model = d.get(model_key)?.as_str()?.to_string();
        Some(Model {
            provider,
            model,
            variant: d.get("variant").and_then(Value::as_str).map(String::from),
        })
    }

    /// In opencode 2.x's shape.
    fn v2(&self) -> Value {
        let mut m = json!({"id": self.model, "providerID": self.provider});
        if let Some(v) = &self.variant {
            m["variant"] = json!(v);
        }
        m
    }
}

/// The last model you used in opencode: copies continue with it.
fn last_model() -> Option<Model> {
    let sql = if version() == Some(2) {
        "select json_extract(data, '$.model') from session_message where type = 'assistant' order by time_created desc limit 1"
    } else {
        "select data from message where json_extract(data, '$.role') = 'assistant' order by time_created desc limit 1"
    };
    let raw = query(sql, [], |r| r.get::<_, Option<String>>(0))
        .into_iter()
        .next()
        .flatten()?;
    let d: Value = serde_json::from_str(&raw).ok()?;
    Model::from_fields(&d, if version() == Some(2) { "id" } else { "modelID" })
}

/// The model an export was using.
fn export_model(exp: &Value) -> Option<Model> {
    if format_of(exp) == 1 {
        let msgs = messages_of(exp);
        let last = msgs.iter().rev().find(|m| jstr(&m["info"], "role") == "assistant")?;
        return Model::from_fields(&last["info"], "modelID");
    }
    Model::from_fields(exp.get("info")?.get("model")?, "id")
}

fn message2(mid: &str, ts: i64, who: &str, text: &str, model: &Model) -> Value {
    if who == "user" {
        return json!({"id": mid, "time": {"created": ts}, "text": text, "files": [], "agents": [], "type": "user"});
    }
    json!({
        "id": mid, "time": {"created": ts, "streamed": ts, "completed": ts}, "type": "assistant", "agent": "build",
        "model": model.v2(), "content": [{"type": "text", "text": text}], "finish": "stop", "rawFinish": "stop",
        "cost": 0, "tokens": zero_tokens(),
    })
}

#[allow(clippy::too_many_arguments)]
fn message1(sid: &str, mid: &str, ts: i64, who: &str, text: &str, model: &Model, parent: &str, folder: &str) -> Value {
    let info = if who == "user" {
        json!({"id": mid, "sessionID": sid, "role": "user", "time": {"created": ts}, "agent": "build",
               "model": {"providerID": model.provider, "modelID": model.model}})
    } else {
        json!({"id": mid, "sessionID": sid, "role": "assistant", "parentID": parent, "modelID": model.model,
               "providerID": model.provider, "mode": "build", "agent": "build", "path": {"cwd": folder, "root": folder},
               "cost": 0, "tokens": zero_tokens(), "time": {"created": ts, "completed": ts}, "finish": "stop"})
    };
    let part =
        json!({"id": oc_id("prt", ts, 0, false), "sessionID": sid, "messageID": mid, "type": "text", "text": text});
    json!({"info": info, "parts": [part]})
}

fn no_model() -> Error {
    Error(tr!(
        "I don't know which model you use in opencode: open opencode once and try again"
    ))
}

/// Write turns as an opencode session (in the format of the opencode installed here). It continues
/// with the last model you used in opencode, or else `fallback`.
fn write_with(
    turns: &[Turn],
    folder: &str,
    title: &str,
    sid: Option<&str>,
    fallback: Option<Model>,
) -> Result<Session> {
    let model = last_model().or(fallback).ok_or_else(no_model)?;
    let (t0, t1) = (turns[0].ts, turns[turns.len() - 1].ts);
    let sid = sid.map(String::from).unwrap_or_else(|| oc_id("ses", t0, 0, true));
    let two = version() == Some(2);
    let (mut msgs, mut parent) = (Vec::new(), None::<String>);
    for (n, t) in turns.iter().enumerate() {
        let mid = oc_id("msg", t.ts, n as u64 + 1, false);
        if two {
            msgs.push(message2(&mid, t.ts, t.who, &t.text, &model));
        } else {
            msgs.push(message1(
                &sid,
                &mid,
                t.ts,
                t.who,
                &t.text,
                &model,
                parent.as_deref().unwrap_or(&mid),
                folder,
            ));
        }
        if t.who == "user" {
            parent = Some(mid);
        }
    }
    let info = if two {
        // projectID is required; import --directory replaces it with that folder's project
        json!({"id": sid, "projectID": "global", "agent": "build", "model": model.v2(), "cost": 0,
               "tokens": zero_tokens(), "outcome": "succeeded", "time": {"created": t0, "updated": t1},
               "title": title, "location": {"directory": folder}})
    } else {
        json!({"id": sid, "slug": slug(title), "projectID": "global", "directory": folder, "title": title,
               "version": cli_version(), "time": {"created": t0, "updated": t1}})
    };
    import(
        serde_json::to_string(&json!({"info": info, "messages": msgs}))?.as_bytes(),
        folder,
    )?;
    Ok(Session::local(
        "opencode",
        sid,
        title.to_string(),
        folder.to_string(),
        t1 as f64 / 1000.0,
        msgs.len() as u64,
    ))
}

pub fn write(turns: &[Turn], folder: &str, title: &str) -> Result<Session> {
    write_with(turns, folder, title, None, None)
}

pub fn push(s: &Session, tmp: &Path) -> Result<Pushed> {
    let data = export(&s.id, &s.path)?;
    let session = format!("session.json.{COMP}");
    write_compressed(&tmp.join(&session), data.as_slice())?;
    let mut children_files = Vec::new();
    for child in children(&s.id) {
        // subagents
        let name = format!("child-{child}.json.{COMP}");
        write_compressed(&tmp.join(&name), export(&child, &s.path)?.as_slice())?;
        children_files.push(name);
    }
    let exp: Value = serde_json::from_slice(&data)?;
    let files = json!({"session": session, "children": children_files, "format": format!("opencode-v{}", version().unwrap_or(2))});
    Ok(Pushed {
        files,
        updated: s.updated,
        weight: messages_of(&exp).len() as u64,
        seen: s.weight,
        preview: render(&export_messages(&exp), "opencode"),
    })
}

fn stored_export(r: &Session, fix: Fix, name: Option<&str>) -> Result<Vec<u8>> {
    Ok(apply(fix, &read_all(&r.stored_file(name))?))
}

fn stored_format(r: &Session) -> String {
    r.meta()["files"]
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("opencode-v2")
        .to_string()
}

pub fn install(r: &Session, local: Option<&Session>, folder: &str, fix: Fix) -> Result<u64> {
    let files = &r.meta()["files"];
    let mut names = vec![jstr(files, "session").to_string()];
    names.extend(
        files
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(String::from),
    );
    let exports = names
        .iter()
        .map(|n| stored_export(r, fix, Some(n)))
        .collect::<Result<Vec<_>>>()?;
    if let Some(l) = local {
        // import doesn't replace an existing session: back up, delete and import
        backup(l, Copy::Bytes(&export(&r.id, &l.path)?), "pull")?;
        delete(&r.id, &l.path)?;
    }
    let (stored, here) = (stored_format(r), format!("opencode-v{}", version().unwrap_or(2)));
    if stored == here {
        for data in &exports {
            import(data, folder)?;
        }
        return Ok(r.weight);
    }
    // saved by the other major version of opencode: convert it, keeping its id
    warn(&tr!(
        "«{title}» comes from opencode {a}.x and here you have opencode {b}.x: converted (tool calls become text)",
        title = r.title,
        a = &stored[stored.len() - 1..],
        b = &here[here.len() - 1..],
    ));
    let exp: Value = serde_json::from_slice(&exports[0])?;
    Ok(write_with(&turns(&exp), folder, &r.title, Some(&r.id), export_model(&exp))?.weight)
}

fn tool_part1(p: &Value) -> String {
    let state = p.get("state").cloned().unwrap_or(Value::Null);
    let output = state
        .get("output")
        .filter(|o| !o.is_null() && **o != json!(""))
        .or_else(|| state.get("error"))
        .cloned()
        .unwrap_or(Value::Null);
    tool_text(
        &tool_call(
            p.get("tool").and_then(Value::as_str).unwrap_or("?"),
            state.get("input").unwrap_or(&json!({})),
        ),
        &as_text(&output),
        None,
    )
}

fn tool_part2(p: &Value) -> String {
    let mut state = p.get("state").cloned().unwrap_or(Value::Null);
    if let Value::String(s) = &state {
        state = serde_json::from_str(s).unwrap_or(Value::Null);
    }
    let output = ["output", "error", "content"]
        .iter()
        .find_map(|k| state.get(k).filter(|v| !v.is_null() && *v != &json!("")))
        .cloned()
        .unwrap_or(Value::Null);
    tool_text(
        &tool_call(
            p.get("name").and_then(Value::as_str).unwrap_or("?"),
            state.get("input").unwrap_or(&json!({})),
        ),
        &as_text(&output),
        None,
    )
}

fn created(v: &Value) -> Option<i64> {
    v.get("time").and_then(|t| t.get("created")).and_then(Value::as_i64)
}

/// Turns of an opencode export (either version), from the last compaction on (with its summary).
pub fn turns(exp: &Value) -> Vec<Turn> {
    let mut msgs = messages_of(exp);
    let now = now_ms();
    let header = tr!("[Summary of the earlier part of the conversation]");
    let mut turns = Vec::new();
    if format_of(exp) == 1 {
        if let Some(start) = msgs
            .iter()
            .rposition(|m| m["info"].get("summary") == Some(&Value::Bool(true)))
        {
            let ts = created(&msgs[start]["info"]).unwrap_or(0);
            add_turn(&mut turns, "user", &format!("{header}\n{}", text1(&msgs[start])), ts);
            msgs = msgs.split_off(start + 1);
        }
        for m in &msgs {
            let info = &m["info"];
            let ts = created(info).unwrap_or(now);
            let who = if jstr(info, "role") == "user" {
                "user"
            } else {
                "assistant"
            };
            for p in m.get("parts").and_then(Value::as_array).into_iter().flatten() {
                match jstr(p, "type") {
                    "text" if !hidden(p) => add_turn(&mut turns, who, jstr(p, "text"), ts),
                    "tool" => add_turn(&mut turns, "assistant", &tool_part1(p), ts),
                    _ => {}
                }
            }
        }
        return turns;
    }
    if let Some(start) = msgs
        .iter()
        .rposition(|m| jstr(m, "type") == "compaction" && !jstr(m, "summary").is_empty())
    {
        let ts = created(&msgs[start]).unwrap_or(0);
        add_turn(
            &mut turns,
            "user",
            &format!("{header}\n{}", jstr(&msgs[start], "summary")),
            ts,
        );
        msgs = msgs.split_off(start + 1);
    }
    for m in &msgs {
        let ts = created(m).unwrap_or(now);
        match jstr(m, "type") {
            "user" => add_turn(&mut turns, "user", jstr(m, "text"), ts),
            "assistant" => {
                for part in m.get("content").and_then(Value::as_array).into_iter().flatten() {
                    match jstr(part, "type") {
                        "text" => add_turn(&mut turns, "assistant", jstr(part, "text"), ts),
                        "tool" => add_turn(&mut turns, "assistant", &tool_part2(part), ts),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    turns
}

pub fn local_turns(s: &Session) -> Result<(Vec<Turn>, Option<String>)> {
    let exp: Value = serde_json::from_slice(&export(&s.id, &s.path)?)?;
    Ok((turns(&exp), export_model(&exp).map(|m| m.model)))
}

pub fn stored_turns(r: &Session, fix: Fix) -> Result<(Vec<Turn>, Option<String>)> {
    let exp: Value = serde_json::from_slice(&stored_export(r, fix, None)?)?;
    Ok((turns(&exp), export_model(&exp).map(|m| m.model)))
}

fn msg_id(m: &Value) -> String {
    m.get("id")
        .and_then(Value::as_str)
        .or_else(|| m.get("info").and_then(|i| i.get("id")).and_then(Value::as_str))
        .unwrap_or("")
        .to_string()
}

pub fn other_branch(l: &Session, r: &Session, fix: Fix) -> Result<Vec<Turn>> {
    if stored_format(r) != format!("opencode-v{}", version().unwrap_or(2)) {
        return Err(Error(tr!(
            "can't merge between opencode 1.x and 2.x: pick this one or the other one"
        )));
    }
    let mine: std::collections::HashSet<String> = messages_of(&serde_json::from_slice(&export(&l.id, &l.path)?)?)
        .iter()
        .map(msg_id)
        .collect();
    let exp: Value = serde_json::from_slice(&stored_export(r, fix, None)?)?;
    let theirs: Vec<Value> = messages_of(&exp)
        .into_iter()
        .filter(|m| !mine.contains(&msg_id(m)))
        .collect();
    Ok(turns(&json!({"messages": theirs})))
}

pub fn append(l: &mut Session, text: &str, ack: &str) -> Result<()> {
    let raw = export(&l.id, &l.path)?;
    backup(l, Copy::Bytes(&raw), "merge")?;
    let mut exp: Value = serde_json::from_slice(&raw)?;
    let one = format_of(&exp) == 1;
    let model = export_model(&exp).or_else(last_model).ok_or_else(no_model)?;
    let now = now_ms();
    let (user_id, reply_id) = (oc_id("msg", now, 1, false), oc_id("msg", now, 2, false));
    let new = if one {
        [
            message1(&l.id, &user_id, now, "user", text, &model, &user_id, &l.path),
            message1(&l.id, &reply_id, now, "assistant", ack, &model, &user_id, &l.path),
        ]
    } else {
        [
            message2(&user_id, now, "user", text, &model),
            message2(&reply_id, now, "assistant", ack, &model),
        ]
    };
    let msgs = exp["messages"]
        .as_array_mut()
        .ok_or_else(|| Error("no messages".into()))?;
    msgs.extend(new);
    let count = msgs.len();
    delete(&l.id, &l.path)?;
    import(serde_json::to_string(&exp)?.as_bytes(), &l.path)?;
    l.weight = count as u64;
    l.updated = now as f64 / 1000.0;
    Ok(())
}

/// The stored version, whole, as a new session (new ids so it doesn't clash with the original).
pub fn branch_copy(r: &Session, folder: &str, fix: Fix, title: &str) -> Result<Session> {
    let mut exp: Value = serde_json::from_slice(&stored_export(r, fix, None)?)?;
    let t0 = exp.get("info").and_then(created).unwrap_or_else(now_ms);
    let sid = oc_id("ses", t0, 0, true);
    exp["info"]["id"] = json!(sid);
    exp["info"]["title"] = json!(title);
    let one = format_of(&exp) == 1;
    if one {
        exp["info"]["slug"] = json!(slug(title));
    }
    let msgs = exp["messages"]
        .as_array_mut()
        .ok_or_else(|| Error("no messages".into()))?;
    if one {
        let ids: std::collections::HashMap<String, String> = msgs
            .iter()
            .enumerate()
            .map(|(n, m)| {
                (
                    msg_id(m),
                    oc_id("msg", created(&m["info"]).unwrap_or(t0), n as u64 + 1, false),
                )
            })
            .collect();
        for m in msgs.iter_mut() {
            let new_id = ids[&msg_id(m)].clone();
            m["info"]["id"] = json!(new_id);
            m["info"]["sessionID"] = json!(sid);
            if let Some(parent) = m["info"]
                .get("parentID")
                .and_then(Value::as_str)
                .and_then(|p| ids.get(p))
                .cloned()
            {
                m["info"]["parentID"] = json!(parent);
            }
            for p in m["parts"].as_array_mut().into_iter().flatten() {
                p["id"] = json!(oc_id("prt", t0, 0, false));
                p["sessionID"] = json!(sid);
                p["messageID"] = json!(new_id);
                if let Some(tail) = p
                    .get("tail_start_id")
                    .and_then(Value::as_str)
                    .and_then(|t| ids.get(t))
                    .cloned()
                {
                    p["tail_start_id"] = json!(tail);
                }
            }
        }
    } else {
        for (n, m) in msgs.iter_mut().enumerate() {
            m["id"] = json!(oc_id("msg", created(m).unwrap_or(t0), n as u64 + 1, false));
        }
    }
    let count = msgs.len();
    import(serde_json::to_string(&exp)?.as_bytes(), folder)?;
    Ok(Session::local(
        "opencode",
        sid,
        title.to_string(),
        folder.to_string(),
        now(),
        count as u64,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn turns_of_opencode_2_start_at_the_last_compaction() {
        let mut exp = fixture("opencode_export.json");
        let t = turns(&exp);
        assert_eq!(t.iter().map(|t| t.who).collect::<Vec<_>>(), ["user", "assistant"]);
        assert!(
            t[1].text
                .contains("‹shell› $ grep -n submit src/login.js\n12:  form.onsubmit = null")
        );
        assert!(!t[1].text.contains("reset somewhere")); // reasoning is dropped
        exp["messages"].as_array_mut().unwrap().insert(
            2,
            json!({"type": "compaction", "summary": "We found the bug.", "time": {"created": 1}}),
        );
        let t = turns(&exp);
        assert!(t[0].text.ends_with("We found the bug.") && !t[0].text.contains("doesn't submit"));
    }

    #[test]
    fn turns_of_opencode_1() {
        let mut exp = fixture("opencode1_export.json");
        let t = turns(&exp);
        assert_eq!(t.iter().map(|t| t.who).collect::<Vec<_>>(), ["user", "assistant"]);
        assert!(
            t[1].text
                .contains("‹bash› $ grep -n submit src/login.js\n12:  form.onsubmit = null")
        );
        assert!(!t[1].text.contains("reset somewhere"));
        let summary = json!({"info": {"id": "msg_s", "role": "assistant", "summary": true, "time": {"created": 1}},
                             "parts": [{"type": "text", "text": "We found the bug."}]});
        exp["messages"].as_array_mut().unwrap().insert(2, summary);
        assert!(turns(&exp)[0].text.ends_with("We found the bug."));
    }

    #[test]
    fn ids_look_like_opencode_ones() {
        let id = oc_id("msg", 1790000000000, 1, false);
        assert!(id.starts_with("msg_") && id.len() == 4 + 12 + 14);
        assert_eq!(slug("Fix the login form!"), "fix-the-login-form");
    }
}
