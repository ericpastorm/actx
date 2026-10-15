//! Codex. A session is a "rollout" file, ~/.codex/sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl,
//! holding what the model saw (response_item) and what Codex shows (event_msg); titles live in
//! session_index.jsonl. Codex finds rollouts by scanning that folder, so copying the file is enough.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};
use std::time::UNIX_EPOCH;

use regex::Regex;
use serde_json::{Value, json};

use crate::claude::{chunks, jsonl, lines};
use crate::ctx::{Result, ctx, jstr, now_ms};
use crate::paths::Fix;
use crate::session::Session;
use crate::state::backup_file;
use crate::store::{COMP, Pushed, stored_lines, write_compressed};
use crate::tr;
use crate::turns::{Turn, add_turn, as_text, iso, iso_ms, tool_call, tool_text};
use crate::ui::{local_time, render};

static ROLLOUT_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\.jsonl$").unwrap());

fn epoch(p: &Path) -> f64 {
    p.metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_secs_f64())
}

fn titles() -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(text) = std::fs::read_to_string(ctx().codex_index()) else {
        return out;
    };
    for line in text.lines() {
        if let Ok(e) = serde_json::from_str::<Value>(line) {
            let (id, name) = (jstr(&e, "id"), jstr(&e, "thread_name"));
            if !id.is_empty() && !name.is_empty() {
                out.insert(id.to_string(), name.to_string());
            }
        }
    }
    out
}

fn set_title(sid: &str, title: &str) -> Result<()> {
    if titles().get(sid).map(String::as_str) == Some(title) {
        return Ok(());
    }
    let index = ctx().codex_index();
    if let Some(parent) = index.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let stamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
    let mut f = File::options().create(true).append(true).open(index)?;
    f.write_all(jsonl(&json!({"id": sid, "thread_name": title, "updated_at": stamp})).as_bytes())?;
    Ok(())
}

/// Text of a response_item message from `role`, leaving out injected context like <environment_context>.
fn text(p: &Value, role: &str) -> Option<String> {
    if jstr(p, "type") != "message" || jstr(p, "role") != role {
        return None;
    }
    let kinds: [&str; 2] = if role == "user" {
        ["input_text", "text"]
    } else {
        ["output_text", "text"]
    };
    let texts: Vec<&str> = p
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|c| kinds.contains(&jstr(c, "type")) && !jstr(c, "text").trim_start().starts_with('<'))
        .map(|c| jstr(c, "text"))
        .collect();
    let t = texts.join("\n");
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

pub fn messages(buf: &[u8]) -> Vec<(String, String)> {
    let mut msgs = Vec::new();
    for e in lines(buf) {
        if jstr(&e, "type") == "response_item" {
            for role in ["user", "assistant"] {
                if let Some(t) = text(&e["payload"], role) {
                    msgs.push((role.to_string(), t));
                }
            }
        }
    }
    msgs
}

fn session_meta(f: &Path) -> Value {
    let mut first = String::new();
    let _ = File::open(f)
        .map(BufReader::new)
        .and_then(|mut r| r.read_line(&mut first));
    match serde_json::from_str::<Value>(&first) {
        Ok(e) if jstr(&e, "type") == "session_meta" => e["payload"].clone(),
        _ => Value::Null,
    }
}

fn rollouts() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let level = |d: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(d)
            .map(|it| it.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default()
    };
    for year in level(&ctx().codex_sessions()) {
        for month in level(&year) {
            for day in level(&month) {
                out.extend(level(&day).into_iter().filter(|f| {
                    let name = f.file_name().unwrap_or_default().to_string_lossy();
                    name.starts_with("rollout-") && name.ends_with(".jsonl")
                }));
            }
        }
    }
    out
}

pub fn local() -> Vec<Session> {
    let titles = titles();
    let mut out = Vec::new();
    for f in rollouts() {
        let name = f.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let Some(id) = ROLLOUT_ID.captures(&name).map(|c| c[1].to_string()) else {
            continue;
        };
        let meta = session_meta(&f);
        let thread_source = meta.get("thread_source").and_then(Value::as_str);
        let cwd = jstr(&meta, "cwd").to_string();
        if cwd.is_empty() || !matches!(thread_source, None | Some("user")) {
            continue; // unreadable, or a subagent's thread
        }
        let Ok((head, tail)) = chunks(&f) else { continue };
        let said: Vec<String> = messages(&head)
            .into_iter()
            .chain(messages(&tail))
            .filter(|(role, _)| role == "user")
            .map(|(_, t)| t)
            .collect();
        let Some(first) = said.first() else { continue };
        let title = titles
            .get(&id)
            .cloned()
            .unwrap_or_else(|| crate::ui::take_chars(first.lines().next().unwrap_or(""), 80).to_string());
        let size = f.metadata().map(|m| m.len()).unwrap_or(0);
        let mut s = Session::local("codex", id, title, cwd, epoch(&f), size);
        s.src = Some(f);
        out.push(s);
    }
    out
}

pub fn preview(s: &Session) -> Vec<(String, String)> {
    s.src
        .as_ref()
        .and_then(|f| chunks(f).ok())
        .map(|(_, tail)| messages(&tail))
        .unwrap_or_default()
}

pub fn version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        std::process::Command::new("codex")
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split_whitespace()
                    .last()
                    .map(String::from)
            })
            .unwrap_or_else(|| "0.0.0".to_string())
    })
}

fn line(kind: &str, payload: Value, ts: i64) -> String {
    jsonl(&json!({"timestamp": iso(ts), "type": kind, "payload": payload}))
}

fn task_end(turn: &str, last: &str, ts: i64) -> String {
    line(
        "event_msg",
        json!({"type": "task_complete", "turn_id": turn, "last_agent_message": last,
               "started_at": ts / 1000, "completed_at": ts / 1000, "duration_ms": 0}),
        ts,
    )
}

/// Rollout lines for turns: what the model sees (response_item) and what Codex shows (event_msg).
fn turn_lines(sid: &str, turns: &[Turn]) -> Vec<String> {
    let (mut out, mut turn, mut last, mut ts) = (Vec::new(), None::<String>, String::new(), 0);
    for t in turns {
        ts = t.ts;
        if t.who == "user" || turn.is_none() {
            if let Some(open) = &turn {
                out.push(task_end(open, &last, ts));
            }
            let id = uuid::Uuid::now_v7().to_string();
            out.push(line(
                "event_msg",
                json!({"type": "task_started", "turn_id": id, "started_at": ts / 1000}),
                ts,
            ));
            turn = Some(id);
            last.clear();
        }
        let item_id = uuid::Uuid::new_v4().to_string();
        let (item, shown) = if t.who == "user" {
            (
                json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": t.text}]}),
                json!({"type": "UserMessage", "id": item_id, "content": [{"type": "text", "text": t.text, "text_elements": []}]}),
            )
        } else {
            last = t.text.clone();
            (
                json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": t.text}]}),
                json!({"type": "AgentMessage", "id": item_id, "content": [{"type": "Text", "text": t.text}], "phase": "final_answer"}),
            )
        };
        out.push(line("response_item", item, ts));
        out.push(line(
            "event_msg",
            json!({"type": "item_completed", "thread_id": sid, "turn_id": turn, "item": shown,
                   "started_at_ms": ts, "completed_at_ms": ts}),
            ts,
        ));
    }
    if let Some(open) = &turn {
        out.push(task_end(open, &last, ts));
    }
    out
}

fn new_path(sid: &str) -> Result<PathBuf> {
    let now = crate::ctx::now();
    let dir = ctx().codex_sessions().join(local_time(now, "%Y/%m/%d"));
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("rollout-{}-{sid}.jsonl", local_time(now, "%Y-%m-%dT%H-%M-%S"))))
}

fn finished(sid: String, title: &str, folder: &str, path: PathBuf) -> Result<Session> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    set_title(&sid, title)?;
    let size = path.metadata()?.len();
    let mut s = Session::local("codex", sid, title.to_string(), folder.to_string(), epoch(&path), size);
    s.src = Some(path);
    Ok(s)
}

pub fn write(turns: &[Turn], folder: &str, title: &str) -> Result<Session> {
    let sid = uuid::Uuid::now_v7().to_string();
    let path = new_path(&sid)?;
    let t0 = turns[0].ts;
    let meta = json!({"id": sid, "session_id": sid, "timestamp": iso(t0), "cwd": folder, "originator": "codex-tui",
                      "cli_version": version(), "source": "cli", "model_provider": "openai"});
    let mut out = io::BufWriter::new(File::create(&path)?);
    out.write_all(line("session_meta", meta, t0).as_bytes())?;
    for l in turn_lines(&sid, turns) {
        out.write_all(l.as_bytes())?;
    }
    out.flush()?;
    drop(out);
    finished(sid, title, folder, path)
}

pub fn push(s: &Session, tmp: &Path) -> Result<Pushed> {
    let src = s.src.as_ref().expect("a local Codex conversation");
    let session = format!("rollout.jsonl.{COMP}");
    let dir = src
        .parent()
        .and_then(|d| d.strip_prefix(ctx().codex_sessions()).ok())
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    let files = json!({"session": session, "name": src.file_name().unwrap_or_default().to_string_lossy(), "dir": dir});
    let updated = epoch(src);
    let weight = write_compressed(&tmp.join(&session), File::open(src)?)?;
    Ok(Pushed {
        files,
        updated,
        weight,
        preview: render(&messages(&chunks(src)?.1), "codex"),
    })
}

pub fn install(r: &Session, local: Option<&Session>, _folder: &str, fix: Fix) -> Result<u64> {
    let files = &r.meta()["files"];
    let dir = ctx().codex_sessions().join(jstr(files, "dir"));
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(jstr(files, "name"));
    let local_src = local.and_then(|l| l.src.clone());
    if let Some(src) = &local_src {
        backup_file(src, &format!("codex-{}.jsonl", r.id))?;
    }
    let size = crate::claude::install_file(r, &dest, fix)?;
    if let Some(src) = local_src.filter(|s| *s != dest) {
        let _ = std::fs::remove_file(src);
    }
    set_title(&r.id, &r.title)?;
    Ok(size)
}

/// Turns of a Codex rollout, from the last compaction on.
pub fn turns<L: AsRef<[u8]>>(raw_lines: impl IntoIterator<Item = L>) -> Vec<Turn> {
    let mut items: Vec<(i64, Value)> = Vec::new();
    for raw in raw_lines {
        let Ok(e) = serde_json::from_slice::<Value>(raw.as_ref()) else {
            continue;
        };
        let ts = iso_ms(jstr(&e, "timestamp"));
        let p = e.get("payload").cloned().unwrap_or(Value::Null);
        match jstr(&e, "type") {
            "compacted" => {
                items = match p.get("replacement_history").and_then(Value::as_array) {
                    Some(history) => history
                        .iter()
                        .filter(|x| x.is_object())
                        .map(|x| (ts, x.clone()))
                        .collect(),
                    None => {
                        let summary = format!(
                            "{}\n{}",
                            tr!("[Summary of the earlier part of the conversation]"),
                            jstr(&p, "message")
                        );
                        vec![(
                            ts,
                            json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": summary}]}),
                        )]
                    }
                }
            }
            "response_item" => items.push((ts, p)),
            _ => {}
        }
    }
    let (mut turns, mut calls) = (Vec::new(), HashMap::new());
    for (ts, p) in &items {
        match jstr(p, "type") {
            "message" => {
                if let Some(t) = text(p, "user") {
                    add_turn(&mut turns, "user", &t, *ts);
                } else if let Some(t) = text(p, "assistant") {
                    add_turn(&mut turns, "assistant", &t, *ts);
                }
            }
            kind @ ("function_call" | "custom_tool_call" | "local_shell_call") => {
                let raw = p
                    .get("arguments")
                    .or_else(|| p.get("input"))
                    .or_else(|| p.get("action"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let args = match &raw {
                    Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| json!({"command": s})),
                    other => other.clone(),
                };
                let name = p
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|n| !n.is_empty())
                    .unwrap_or(kind);
                calls.insert(jstr(p, "call_id").to_string(), tool_call(name, &args));
            }
            "function_call_output" | "custom_tool_call_output" | "local_shell_call_output" => {
                let mut output = p.get("output").cloned().unwrap_or(Value::Null);
                if output.is_object() {
                    output = output
                        .get("content")
                        .or_else(|| output.get("output"))
                        .cloned()
                        .unwrap_or(output);
                }
                let call = calls.get(jstr(p, "call_id")).map(String::as_str).unwrap_or("‹tool›");
                add_turn(&mut turns, "assistant", &tool_text(call, &as_text(&output), None), *ts);
            }
            _ => {}
        }
    }
    turns
}

pub fn file_turns(path: &Path) -> Result<Vec<Turn>> {
    let reader = BufReader::new(File::open(path)?);
    Ok(turns(reader.split(b'\n').map_while(io::Result::ok)))
}

/// Rollouts only grow: what the other one has after the part both share.
pub fn other_branch(l: &Session, r: &Session, fix: Fix) -> Result<Vec<Turn>> {
    let data = std::fs::read(l.src.as_ref().expect("local"))?;
    let mine: Vec<&[u8]> = data.split(|b| *b == b'\n').collect();
    let theirs = stored_lines(&r.stored_file(None), fix)?;
    let shared = mine
        .iter()
        .zip(&theirs)
        .take_while(|(a, b)| **a == b.as_slice())
        .count();
    Ok(turns(&theirs[shared..]))
}

pub fn append(l: &mut Session, text: &str, ack: &str) -> Result<()> {
    let src = l.src.clone().expect("a local Codex conversation");
    let now = now_ms();
    let new = [
        Turn {
            who: "user",
            text: text.into(),
            ts: now,
        },
        Turn {
            who: "assistant",
            text: ack.into(),
            ts: now,
        },
    ];
    let mut f = File::options().append(true).open(&src)?;
    for l in turn_lines(&l.id, &new) {
        f.write_all(l.as_bytes())?;
    }
    l.weight = src.metadata()?.len();
    l.updated = epoch(&src);
    Ok(())
}

pub fn branch_copy(r: &Session, folder: &str, fix: Fix, title: &str) -> Result<Session> {
    let sid = uuid::Uuid::now_v7().to_string();
    let path = new_path(&sid)?;
    let finder = memchr::memmem::Finder::new(r.id.as_bytes());
    let mut out = io::BufWriter::new(File::create(&path)?);
    for line in stored_lines(&r.stored_file(None), fix)? {
        if line.is_empty() {
            continue;
        }
        let mut renamed = Vec::with_capacity(line.len());
        let mut pos = 0;
        for at in finder.find_iter(&line) {
            renamed.extend_from_slice(&line[pos..at]);
            renamed.extend_from_slice(sid.as_bytes());
            pos = at + r.id.len();
        }
        renamed.extend_from_slice(&line[pos..]);
        out.write_all(&renamed)?;
        out.write_all(b"\n")?;
    }
    out.flush()?;
    drop(out);
    finished(sid, title, folder, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(kind: &str, payload: Value) -> String {
        json!({"timestamp": "2026-01-01T10:00:00.000Z", "type": kind, "payload": payload}).to_string()
    }

    #[test]
    fn turns_with_tools_injected_context_and_compaction() {
        let mut ls = vec![
            l("session_meta", json!({"id": "x", "cwd": "/w"})),
            l(
                "response_item",
                json!({"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "<permissions>"}]}),
            ),
            l(
                "response_item",
                json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "<environment_context>x</environment_context>"}, {"type": "input_text", "text": "run it"}]}),
            ),
            l("response_item", json!({"type": "reasoning", "summary": []})),
            l(
                "response_item",
                json!({"type": "function_call", "name": "shell", "call_id": "c1", "arguments": json!({"command": ["bash", "-lc", "make"]}).to_string()}),
            ),
            l(
                "response_item",
                json!({"type": "function_call_output", "call_id": "c1", "output": "ok\n"}),
            ),
            l(
                "response_item",
                json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "it runs"}]}),
            ),
        ];
        let t = turns(&ls);
        assert_eq!(
            t.iter().map(|t| (t.who, t.text.as_str())).collect::<Vec<_>>(),
            [
                ("user", "run it"),
                ("assistant", "‹shell› $ bash -lc make\nok\n\nit runs")
            ]
        );
        ls.push(l("compacted", json!({"message": "We fixed the build."})));
        let t = turns(&ls);
        assert_eq!(t.len(), 1);
        assert!(t[0].text.ends_with("We fixed the build."));
    }

    #[test]
    fn written_turns_open_and_close_codex_tasks() {
        let ts = 1790000000000;
        let lines = turn_lines(
            "sid",
            &[
                Turn {
                    who: "user",
                    text: "hi".into(),
                    ts,
                },
                Turn {
                    who: "assistant",
                    text: "hello".into(),
                    ts,
                },
            ],
        );
        let kinds: Vec<String> = lines
            .iter()
            .map(|l| {
                let e: Value = serde_json::from_str(l).unwrap();
                format!("{}/{}", jstr(&e, "type"), jstr(&e["payload"], "type"))
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "event_msg/task_started",
                "response_item/message",
                "event_msg/item_completed",
                "response_item/message",
                "event_msg/item_completed",
                "event_msg/task_complete",
            ]
        );
    }
}
