//! Antigravity CLI (agy): ~/.gemini/antigravity-cli. Each conversation is a SQLite database of
//! steps in protocol buffers (conversations/<id>.db), listed in conversation_summaries.db (rebuilt
//! from jetbox_summaries_proto.pb), with its title in annotations/<id>.pbtxt. actx converts to and
//! from it; it doesn't sync it.
//!
//! The steps actx reads, by type: 14 your message, 15 a reply (text, reasoning, tool calls), 132 a
//! tool's result, 101 a notice or background task finishing, 23 a summary of what came before.
//! It writes only the first two: that's all a converted conversation has, and agy resumes it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};

use crate::ctx::{Result, ctx, now_ms};
use crate::pb::{self, Msg};
use crate::session::Session;
use crate::tr;
use crate::turns::{Turn, add_turn, tool_call, tool_text};

const USER: u64 = 14;
const REPLY: u64 = 15;
const TOOL_RESULT: u64 = 132;
const NOTICE: u64 = 101;
const SUMMARY: u64 = 23;
/// a finished step
const DONE: u64 = 3;
const PROJECT: &str = "default-cli-project";

pub fn home() -> PathBuf {
    ctx().home.join(".gemini").join("antigravity-cli")
}

fn conversation_db(id: &str) -> PathBuf {
    home().join("conversations").join(format!("{id}.db"))
}

fn annotation(id: &str) -> PathBuf {
    home().join("annotations").join(format!("{id}.pbtxt"))
}

/// For reading only. Not opened read-only: in WAL mode that leaves -wal and -shm files behind,
/// which a connection that can write removes when it's the last one to close.
fn open_ro(path: &Path) -> Result<Connection> {
    if crate::inspection::enabled() {
        return crate::inspection::database(path);
    }
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?)
}

/// file:///a%20b → /a b
fn uri_path(uri: &str) -> String {
    let raw = uri.strip_prefix("file://").unwrap_or(uri);
    // file:///C:/Users/me is C:\Users\me
    let raw = match raw.strip_prefix('/') {
        Some(rest) if cfg!(windows) && crate::paths::is_windows_path(rest) => rest,
        _ => raw,
    }
    .as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let hex = raw
            .get(i + 1..i + 3)
            .and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match (raw[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    crate::paths::native(&String::from_utf8_lossy(&out))
}

fn path_uri(path: &str) -> String {
    let windows = crate::paths::is_windows_path(path);
    let path = crate::paths::slashed(path);
    let mut out = String::from(if windows { "file:///" } else { "file://" });
    for (i, b) in path.bytes().enumerate() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) || (windows && i == 1 && b == b':') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// title:"…" in a text-format protocol buffer.
fn read_title(id: &str) -> Option<String> {
    let text = std::fs::read_to_string(annotation(id)).ok()?;
    let rest = &text[text.find("title:")? + 6..];
    let rest = rest.trim_start().strip_prefix('"')?;
    let (mut out, mut chars) = (Vec::new(), rest.bytes().peekable());
    while let Some(c) = chars.next() {
        match c {
            b'"' => break,
            b'\\' => match chars.next()? {
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                d @ b'0'..=b'7' => {
                    let mut n = u32::from(d - b'0');
                    for _ in 0..2 {
                        match chars.peek() {
                            Some(d @ b'0'..=b'7') => {
                                n = n * 8 + u32::from(d - b'0');
                                chars.next();
                            }
                            _ => break,
                        }
                    }
                    out.push(n as u8);
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned()).filter(|t| !t.is_empty())
}

fn write_title(id: &str, title: &str) -> Result<()> {
    let escaped = title.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
    std::fs::create_dir_all(home().join("annotations"))?;
    std::fs::write(annotation(id), format!("title:\"{escaped}\"\n"))?;
    Ok(())
}

/// What the index says about each conversation: (title, folder, updated, steps).
fn index() -> HashMap<String, (String, String, f64, u64)> {
    let mut out = HashMap::new();
    let Ok(con) = open_ro(&home().join("conversation_summaries.db")) else {
        return out;
    };
    let Ok(mut q) = con.prepare(
        "select conversation_id, title, workspace_uris, last_modified_time, step_count from conversation_summaries",
    ) else {
        return out;
    };
    let rows = q.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1).unwrap_or_default(),
            r.get::<_, String>(2).unwrap_or_default(),
            r.get::<_, String>(3).unwrap_or_default(),
            r.get::<_, i64>(4).unwrap_or(0),
        ))
    });
    for (id, title, uris, when, steps) in rows.into_iter().flatten().flatten() {
        let folder = serde_json::from_str::<Vec<String>>(&uris)
            .ok()
            .and_then(|u| u.into_iter().next())
            .map(|u| uri_path(&u))
            .unwrap_or_default();
        let updated = chrono::DateTime::parse_from_str(&when, "%Y-%m-%d %H:%M:%S%.f%:z")
            .map(|t| t.timestamp_millis() as f64 / 1000.0)
            .unwrap_or(0.0);
        out.insert(id, (title, folder, updated, steps.max(0) as u64));
    }
    out
}

pub fn local() -> Vec<Session> {
    local_keys(None)
}
pub fn local_keys(keys: Option<&std::collections::HashSet<String>>) -> Vec<Session> {
    let index = index();
    let mut out = Vec::new();
    for e in std::fs::read_dir(home().join("conversations"))
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
    {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("db") {
            continue;
        }
        let id = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        if keys.is_some_and(|keys| !keys.contains(&format!("antigravity:{id}"))) {
            continue;
        }
        let (title, mut folder, updated, steps) = index.get(&id).cloned().unwrap_or_default();
        let Ok(con) = open_ro(&path) else { continue };
        let count: i64 = con
            .query_row("select count(*) from steps", [], |r| r.get(0))
            .unwrap_or(0);
        if count == 0 {
            continue; // opened and closed without a word
        }
        if folder.is_empty() {
            folder = con
                .query_row("select data from trajectory_metadata_blob", [], |r| {
                    r.get::<_, Vec<u8>>(0)
                })
                .ok()
                .and_then(|m| pb::text(&m, &[7]))
                .map(|u| uri_path(&u))
                .unwrap_or_default();
        }
        let updated = if updated > 0.0 {
            updated
        } else {
            crate::store::modified(&path)
        };
        let title = read_title(&id)
            .or(Some(title).filter(|t| !t.is_empty()))
            .or_else(|| first_message(&con))
            .unwrap_or_else(|| id.clone());
        let mut s = Session::local("antigravity", id, title, folder, updated, steps.max(count as u64));
        s.src = Some(path);
        out.push(s);
    }
    out
}

fn first_message(con: &Connection) -> Option<String> {
    let payload: Vec<u8> = con
        .query_row(
            "select step_payload from steps where step_type = ?1 order by idx limit 1",
            [USER as i64],
            |r| r.get(0),
        )
        .ok()?;
    let text = pb::text(&payload, &[19, 2])?;
    Some(crate::ui::take_chars(text.lines().next()?.trim(), 60).to_string()).filter(|t| !t.is_empty())
}

/// A tool call as agy records it: (id, text for the conversation).
fn call(c: &[u8]) -> (String, String) {
    let id = pb::text(c, &[1]).unwrap_or_default();
    let name = pb::text(c, &[2]).unwrap_or_default();
    let mut args: Value = pb::text(c, &[3])
        .and_then(|a| serde_json::from_str(&a).ok())
        .unwrap_or(Value::Null);
    if let Some(o) = args.as_object_mut() {
        // what it shows while running, not what it does
        o.remove("toolAction");
        o.remove("toolSummary");
        if let Some(cmd) = o.remove("CommandLine") {
            args = json!({"command": cmd});
        }
    }
    (id, tool_call(&name, &args))
}

fn step_ms(payload: &[u8]) -> i64 {
    match (pb::int(payload, &[5, 1, 1]), pb::int(payload, &[5, 1, 2])) {
        (Some(s), nanos) => (s * 1000 + nanos.unwrap_or(0) / 1_000_000) as i64,
        _ => now_ms(),
    }
}

/// The conversation as turns.
pub fn turns(db: &Path) -> Result<Vec<Turn>> {
    let con = open_ro(db)?;
    let mut q = con.prepare("select step_type, step_payload from steps order by idx")?;
    let rows = q.query_map([], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1).unwrap_or_default()))
    })?;
    let (mut turns, mut calls) = (Vec::new(), HashMap::new());
    for row in rows {
        let (kind, p) = row?;
        let ts = step_ms(&p);
        match kind as u64 {
            USER => {
                let text = pb::text(&p, &[19, 2]).or_else(|| pb::text(&p, &[19, 3, 1]));
                add_turn(&mut turns, "user", &text.unwrap_or_default(), ts);
            }
            REPLY => {
                add_turn(&mut turns, "assistant", &pb::text(&p, &[20, 1]).unwrap_or_default(), ts);
                for c in pb::all(&p, &[20, 7]) {
                    let (id, text) = call(c);
                    calls.insert(id, text);
                }
            }
            TOOL_RESULT => {
                let (id, text) = pb::get(&p, &[5, 4]).map(call).unwrap_or_default();
                let text = calls.remove(&id).unwrap_or(text);
                let output = pb::text(&p, &[140, 2, 1]).unwrap_or_default();
                add_turn(&mut turns, "assistant", &tool_text(&text, &output, None), ts);
            }
            NOTICE => {
                // a background task finishing; notices from agy itself (it restarted…) aren't news
                if pb::text(&p, &[114, 4, 3]).as_deref() != Some("system") {
                    let what = pb::text(&p, &[114, 2, 1]).unwrap_or_default();
                    let output = pb::text(&p, &[114, 4, 4]).unwrap_or_default();
                    add_turn(
                        &mut turns,
                        "assistant",
                        &tool_text(&format!("‹task› {what}"), &output, None),
                        ts,
                    );
                }
            }
            SUMMARY => {
                if let Some(summary) = pb::text(&p, &[30, 5]) {
                    turns.clear();
                    let note = tr!("[Summary of the earlier part of the conversation]");
                    add_turn(&mut turns, "user", &format!("{note}\n{summary}"), ts);
                }
            }
            _ => {}
        }
    }
    Ok(turns)
}

pub fn preview(s: &Session) -> Vec<(String, String)> {
    s.src
        .as_ref()
        .and_then(|db| turns(db).ok())
        .map(|t| {
            let from = t.len().saturating_sub(8);
            t[from..].iter().map(|t| (t.who.to_string(), t.text.clone())).collect()
        })
        .unwrap_or_default()
}

fn time(ms: i64) -> Msg {
    Msg::new()
        .int(1, (ms / 1000) as u64)
        .int(2, (ms % 1000) as u64 * 1_000_000)
}

fn datetime(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .format("%Y-%m-%d %H:%M:%S%.9f+00:00")
        .to_string()
}

/// agy keeps its own copy of the index, and rebuilds the index from it.
fn add_to_cache(id: &str, summary: &Msg) -> Result<()> {
    let cache = home().join("jetbox_summaries_proto.pb");
    let mut data = std::fs::read(&cache).unwrap_or_default();
    data.extend(Msg::new().msg(1, &Msg::new().text(1, id).msg(2, summary)).done());
    let tmp = cache.with_extension("pb.actx-tmp");
    std::fs::write(&tmp, &data)?;
    crate::paths::make_private(&tmp)?;
    std::fs::rename(&tmp, &cache)?;
    Ok(())
}

const SCHEMA: &str = "
CREATE TABLE `trajectory_meta` (`trajectory_id` text,`cascade_id` text,`trajectory_type` integer,`source` integer,PRIMARY KEY (`trajectory_id`));
CREATE TABLE `steps` (`idx` integer,`step_type` integer NOT NULL DEFAULT 0,`status` integer NOT NULL DEFAULT 0,`has_subtrajectory` numeric NOT NULL DEFAULT false,`metadata` blob,`error_details` blob,`permissions` blob,`task_details` blob,`render_info` blob,`step_payload` blob,`step_format` integer NOT NULL DEFAULT 0,PRIMARY KEY (`idx`));
CREATE INDEX `idx_steps_status` ON `steps`(`status`);
CREATE INDEX `idx_steps_step_type` ON `steps`(`step_type`);
CREATE TABLE `gen_metadata` (`idx` integer,`data` blob,`size` integer NOT NULL DEFAULT 0,PRIMARY KEY (`idx`));
CREATE TABLE `executor_metadata` (`idx` integer,`data` blob,PRIMARY KEY (`idx`));
CREATE TABLE `parent_references` (`idx` integer,`data` blob,PRIMARY KEY (`idx`));
CREATE TABLE `trajectory_metadata_blob` (`id` text DEFAULT \"main\",`data` blob,PRIMARY KEY (`id`));
CREATE TABLE `battle_mode_infos` (`idx` integer,`data` blob,PRIMARY KEY (`idx`));
PRAGMA user_version = 1;
";

const INDEX_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS `conversation_summaries` (`conversation_id` text,`title` text NOT NULL DEFAULT \"\",`preview` text NOT NULL DEFAULT \"\",`step_count` integer NOT NULL DEFAULT 0,`last_modified_time` datetime NOT NULL,`workspace_uris` text NOT NULL,`status` text NOT NULL DEFAULT \"\",`source` text NOT NULL DEFAULT \"\",`project_id` text NOT NULL DEFAULT \"\",`agent_name` text NOT NULL DEFAULT \"\",`parent_conversation_id` text NOT NULL DEFAULT \"\",`nesting_depth` integer NOT NULL DEFAULT 0,`battle_id` text NOT NULL DEFAULT \"\",`winning_conversation_id` text NOT NULL DEFAULT \"\",`not_fully_idle` numeric NOT NULL DEFAULT false,`killed` numeric NOT NULL DEFAULT false,`last_user_input_time` datetime NOT NULL,`last_user_input_step_index` integer NOT NULL DEFAULT -1,`app_data_dir` text NOT NULL DEFAULT \"\",`raw_summary` blob,`group_id` text NOT NULL DEFAULT \"\",PRIMARY KEY (`conversation_id`));
CREATE INDEX IF NOT EXISTS `idx_conversation_summaries_last_user_input_time` ON `conversation_summaries`(`last_user_input_time`);
CREATE INDEX IF NOT EXISTS `idx_conversation_summaries_last_modified_time` ON `conversation_summaries`(`last_modified_time`);
";

/// Write turns as a new conversation agy can resume (`agy --conversation <id>`).
pub fn write(turns: &[Turn], folder: &str, title: &str) -> Result<Session> {
    let id = uuid::Uuid::new_v4().to_string();
    let trajectory = uuid::Uuid::new_v4().to_string();
    let run = uuid::Uuid::new_v4().to_string();
    let uri = path_uri(folder);
    let t0 = turns.first().map_or_else(now_ms, |t| t.ts);
    let workspace = Msg::new().text(1, &uri).text(3, "");
    let meta = Msg::new()
        .msg(1, &workspace)
        .msg(2, &time(t0))
        .text(3, &uuid::Uuid::new_v4().to_string())
        .text(6, &id)
        .text(7, &uri)
        .text(18, PROJECT);
    let path = conversation_db(&id);
    std::fs::create_dir_all(path.parent().expect("a folder"))?;
    let written = (|| -> Result<(usize, i64)> {
        let mut con = Connection::open(&path)?;
        con.execute_batch(SCHEMA)?;
        let tx = con.transaction()?;
        tx.execute(
            "insert into trajectory_meta values (?1, ?2, 4, 17)",
            params![trajectory, id],
        )?;
        tx.execute(
            "insert into trajectory_metadata_blob values ('main', ?1)",
            params![meta.clone().done()],
        )?;
        let mut last_user = (0, t0);
        for (i, t) in turns.iter().enumerate() {
            let mut at = Msg::new().text(1, &trajectory);
            if i > 0 {
                at = at.int(2, i as u64);
            }
            let user = t.who == "user";
            let step_meta = Msg::new()
                .msg(1, &time(t.ts))
                .int(3, if user { 4 } else { 2 })
                .text(12, &run)
                .msg(20, &at.text(4, &id));
            let (kind, body) = if user {
                last_user = (i, t.ts);
                (
                    USER,
                    Msg::new().msg(19, &Msg::new().text(2, &t.text).msg(3, &Msg::new().text(1, &t.text))),
                )
            } else {
                (
                    REPLY,
                    Msg::new().msg(20, &Msg::new().text(1, &t.text).text(8, &t.text).int(12, 2)),
                )
            };
            let mut payload = Msg::new().int(1, kind).int(4, DONE).msg(5, &step_meta).done();
            payload.extend(body.done());
            tx.execute(
                "insert into steps (idx, step_type, status, metadata, step_payload) values (?1, ?2, ?3, ?4, ?5)",
                params![i as i64, kind as i64, DONE as i64, step_meta.done(), payload],
            )?;
        }
        tx.commit()?;
        Ok(last_user)
    })();
    let (last_user, user_ts) = match written {
        Ok(w) => w,
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
    };
    let t_last = turns.last().map_or(t0, |t| t.ts);
    let summary = Msg::new()
        .text(1, title)
        .int(2, turns.len() as u64)
        .msg(3, &time(t_last))
        .text(4, &trajectory)
        .int(5, 1)
        .msg(7, &time(t0))
        .msg(9, &workspace)
        .msg(10, &time(user_ts))
        .msg(15, &Msg::new().text(1, title))
        .int(16, last_user as u64)
        .msg(17, &meta)
        .int(22, 4)
        .int(28, 4);
    let index = Connection::open(home().join("conversation_summaries.db"))?;
    index.execute_batch(INDEX_SCHEMA)?;
    index.execute(
        "insert into conversation_summaries (conversation_id, title, preview, step_count, last_modified_time,
            workspace_uris, status, project_id, last_user_input_time, last_user_input_step_index, app_data_dir, raw_summary)
         values (?1, ?2, ?2, ?3, ?4, ?5, 'CASCADE_RUN_STATUS_IDLE', ?6, ?7, ?8, 'antigravity-cli', ?9)",
        params![
            id,
            title,
            turns.len() as i64,
            datetime(t_last),
            serde_json::to_string(&[&uri])?,
            PROJECT,
            datetime(user_ts),
            last_user as i64,
            summary.clone().done()
        ],
    )?;
    add_to_cache(&id, &summary)?;
    // agy takes the title from here when it rewrites the index
    write_title(&id, title)?;
    let mut s = Session::local(
        "antigravity",
        id,
        title.to_string(),
        folder.to_string(),
        t_last as f64 / 1000.0,
        turns.len() as u64,
    );
    s.src = Some(path);
    Ok(s)
}

/// Append completed linked turns without replacing the trajectory or changing its identity.
pub fn append_turns(s: &mut Session, turns: &[Turn]) -> Result<()> {
    let path = s.src.as_ref().expect("local Antigravity session");
    let mut con = Connection::open(path)?;
    // VACUUM INTO includes WAL content and makes a consistent recovery copy.
    let backup = tempfile::tempdir()?;
    let snapshot = backup.path().join("conversation.db");
    con.execute("VACUUM INTO ?1", params![snapshot.to_string_lossy()])?;
    crate::state::backup(s, crate::state::Copy::File(&snapshot), "multiharness")?;
    let trajectory: String = con.query_row("select trajectory_id from trajectory_meta limit 1", [], |r| r.get(0))?;
    let start: i64 = con.query_row("select coalesce(max(idx), -1) + 1 from steps", [], |r| r.get(0))?;
    let run = uuid::Uuid::new_v4().to_string();
    let tx = con.transaction()?;
    for (n, t) in turns.iter().enumerate() {
        let i = start + n as i64;
        let user = t.who == "user";
        let kind = if user { USER } else { REPLY };
        let meta = Msg::new()
            .msg(1, &time(t.ts))
            .int(3, if user { 4 } else { 2 })
            .text(12, &run)
            .msg(20, &Msg::new().text(1, &trajectory).int(2, i as u64).text(4, &s.id));
        let body = if user {
            Msg::new().msg(19, &Msg::new().text(2, &t.text).msg(3, &Msg::new().text(1, &t.text)))
        } else {
            Msg::new().msg(20, &Msg::new().text(1, &t.text).text(8, &t.text).int(12, 2))
        };
        let mut payload = Msg::new().int(1, kind).int(4, DONE).msg(5, &meta).done();
        payload.extend(body.done());
        tx.execute(
            "insert into steps (idx,step_type,status,metadata,step_payload) values (?1,?2,?3,?4,?5)",
            params![i, kind as i64, DONE as i64, meta.done(), payload],
        )?;
    }
    tx.commit()?;
    let last = turns.last().map_or_else(now_ms, |t| t.ts);
    let count = start + turns.len() as i64;
    let index = Connection::open(home().join("conversation_summaries.db"))?;
    // Preserve unknown summary fields; only replace the activity/count fields.
    let raw: Vec<u8> = index.query_row(
        "select raw_summary from conversation_summaries where conversation_id=?1",
        params![s.id],
        |r| r.get::<_, Option<Vec<u8>>>(0).map(Option::unwrap_or_default),
    )?;
    let mut summary = Msg::new();
    for (n, v) in pb::fields(&raw).unwrap_or_default() {
        if [2, 3, 10, 16].contains(&n) {
            continue;
        }
        summary = match v {
            pb::Val::Int(v) => summary.int(n, v),
            pb::Val::Bytes(v) => summary.bytes(n, v),
        };
    }
    let last_user = turns.iter().rposition(|t| t.who == "user").unwrap_or(0);
    summary = summary
        .int(2, count as u64)
        .msg(3, &time(last))
        .msg(10, &time(turns[last_user].ts))
        .int(16, start as u64 + last_user as u64);
    index.execute(
        "update conversation_summaries set step_count=?1,last_modified_time=?2,raw_summary=?3 where conversation_id=?4",
        params![count, datetime(last), summary.clone().done(), s.id],
    )?;
    add_to_cache(&s.id, &summary)?;
    s.weight = count as u64;
    s.updated = last as f64 / 1000.0;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_uris() {
        assert_eq!(path_uri("/home/a b/ñ"), "file:///home/a%20b/%C3%B1");
        assert_eq!(
            uri_path("file:///home/a%20b/%C3%B1"),
            crate::paths::native("/home/a b/ñ")
        );
        assert_eq!(uri_path(&path_uri("/x/100%/y")), crate::paths::native("/x/100%/y"));
        assert_eq!(path_uri(r"C:\Users\a b\shop"), "file:///C:/Users/a%20b/shop");
        if cfg!(windows) {
            assert_eq!(uri_path("file:///C:/Users/a%20b/shop"), r"C:\Users\a b\shop");
        }
    }
}
