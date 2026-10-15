//! The tools behind one interface. Adding a tool means implementing `Harness` for it.

use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::ctx::{Result, ctx, which};
use crate::paths::{Fix, apply, short};
use crate::session::Session;
use crate::store::{Pushed, open_read};
use crate::turns::Turn;
use crate::ui::shell_quote;
use crate::{claude, codex, opencode};

pub trait Harness: Sync {
    fn label(&self) -> &'static str;
    fn installed(&self) -> bool;
    fn local(&self) -> Vec<Session>;
    fn preview(&self, s: &Session) -> Vec<(String, String)>;
    /// Copy a conversation into an upload folder.
    fn push(&self, s: &Session, tmp: &Path) -> Result<Pushed>;
    /// Put a stored conversation in place on this machine; returns its local weight.
    fn install(&self, r: &Session, local: Option<&Session>, folder: &str, fix: Fix) -> Result<u64>;
    /// (turns, model) of a conversation on this machine or in the shared folder.
    fn local_turns(&self, s: &Session) -> Result<(Vec<Turn>, Option<String>)>;
    fn stored_turns(&self, r: &Session, fix: Fix) -> Result<(Vec<Turn>, Option<String>)>;
    /// Write turns as a new conversation of this tool.
    fn write(&self, turns: &[Turn], folder: &str, title: &str, model: Option<&str>) -> Result<Session>;
    /// Merging: what the stored version has that this one doesn't, adding a note, and the stored
    /// version as a separate conversation.
    fn other_branch(&self, l: &Session, r: &Session, fix: Fix) -> Result<Vec<Turn>>;
    fn append(&self, l: &mut Session, text: &str, ack: &str) -> Result<()>;
    fn branch_copy(&self, r: &Session, folder: &str, fix: Fix, title: &str) -> Result<Session>;
    fn resume(&self, sid: &str, folder: &str) -> String;
}

pub const NAMES: [&str; 3] = ["claude", "opencode", "codex"];

pub fn get(name: &str) -> &'static dyn Harness {
    match name {
        "claude" => &Claude,
        "opencode" => &Opencode,
        _ => &Codex,
    }
}

/// The tool's name as a static string, if actx knows it.
pub fn known(name: &str) -> Option<&'static str> {
    NAMES.iter().find(|n| **n == name).copied()
}

pub fn all_local() -> Vec<Session> {
    NAMES.iter().flat_map(|n| get(n).local()).collect()
}

fn shell_path(folder: &str) -> String {
    let p = short(folder);
    if p.chars().all(|c| c.is_alphanumeric() || "_./~+-".contains(c)) {
        p
    } else {
        shell_quote(folder)
    }
}

/// The lines of a stored conversation, decompressed and with this machine's paths, one at a time.
fn stored_line_iter<'a>(r: &Session, fix: Fix<'a>) -> Result<impl Iterator<Item = Vec<u8>> + 'a> {
    let reader = open_read(&r.stored_file(None))?;
    Ok(reader
        .split(b'\n')
        .map_while(std::io::Result::ok)
        .map(move |l| apply(fix, &l)))
}

struct Claude;
struct Opencode;
struct Codex;

impl Harness for Claude {
    fn label(&self) -> &'static str {
        "Claude Code"
    }
    fn installed(&self) -> bool {
        ctx().claude_projects.parent().is_some_and(Path::is_dir) || which("claude")
    }
    fn local(&self) -> Vec<Session> {
        claude::local()
    }
    fn preview(&self, s: &Session) -> Vec<(String, String)> {
        claude::preview(s)
    }
    fn push(&self, s: &Session, tmp: &Path) -> Result<Pushed> {
        claude::push(s, tmp)
    }
    fn install(&self, r: &Session, local: Option<&Session>, folder: &str, fix: Fix) -> Result<u64> {
        claude::install(r, local, folder, fix)
    }
    fn local_turns(&self, s: &Session) -> Result<(Vec<Turn>, Option<String>)> {
        let file = BufReader::new(std::fs::File::open(s.src.as_ref().expect("local"))?);
        Ok((claude::turns(file.split(b'\n').map_while(std::io::Result::ok)), None))
    }
    fn stored_turns(&self, r: &Session, fix: Fix) -> Result<(Vec<Turn>, Option<String>)> {
        Ok((claude::turns(stored_line_iter(r, fix)?), None))
    }
    fn write(&self, turns: &[Turn], folder: &str, title: &str, model: Option<&str>) -> Result<Session> {
        claude::write(turns, folder, title, model)
    }
    fn other_branch(&self, l: &Session, r: &Session, fix: Fix) -> Result<Vec<Turn>> {
        claude::other_branch(l, r, fix)
    }
    fn append(&self, l: &mut Session, text: &str, ack: &str) -> Result<()> {
        claude::append(l, text, ack)
    }
    fn branch_copy(&self, r: &Session, folder: &str, fix: Fix, title: &str) -> Result<Session> {
        claude::branch_copy(r, folder, fix, title)
    }
    fn resume(&self, sid: &str, folder: &str) -> String {
        format!("cd {} && claude --resume {sid}", shell_path(folder))
    }
}

impl Harness for Opencode {
    fn label(&self) -> &'static str {
        "opencode"
    }
    fn installed(&self) -> bool {
        which("opencode")
    }
    fn local(&self) -> Vec<Session> {
        opencode::local()
    }
    fn preview(&self, s: &Session) -> Vec<(String, String)> {
        opencode::preview(s)
    }
    fn push(&self, s: &Session, tmp: &Path) -> Result<Pushed> {
        opencode::push(s, tmp)
    }
    fn install(&self, r: &Session, local: Option<&Session>, folder: &str, fix: Fix) -> Result<u64> {
        opencode::install(r, local, folder, fix)
    }
    fn local_turns(&self, s: &Session) -> Result<(Vec<Turn>, Option<String>)> {
        opencode::local_turns(s)
    }
    fn stored_turns(&self, r: &Session, fix: Fix) -> Result<(Vec<Turn>, Option<String>)> {
        opencode::stored_turns(r, fix)
    }
    fn write(&self, turns: &[Turn], folder: &str, title: &str, _model: Option<&str>) -> Result<Session> {
        opencode::write(turns, folder, title)
    }
    fn other_branch(&self, l: &Session, r: &Session, fix: Fix) -> Result<Vec<Turn>> {
        opencode::other_branch(l, r, fix)
    }
    fn append(&self, l: &mut Session, text: &str, ack: &str) -> Result<()> {
        opencode::append(l, text, ack)
    }
    fn branch_copy(&self, r: &Session, folder: &str, fix: Fix, title: &str) -> Result<Session> {
        opencode::branch_copy(r, folder, fix, title)
    }
    fn resume(&self, sid: &str, folder: &str) -> String {
        format!("opencode {} -s {sid}", shell_path(folder))
    }
}

impl Harness for Codex {
    fn label(&self) -> &'static str {
        "Codex"
    }
    fn installed(&self) -> bool {
        ctx().codex_home.is_dir() || which("codex")
    }
    fn local(&self) -> Vec<Session> {
        codex::local()
    }
    fn preview(&self, s: &Session) -> Vec<(String, String)> {
        codex::preview(s)
    }
    fn push(&self, s: &Session, tmp: &Path) -> Result<Pushed> {
        codex::push(s, tmp)
    }
    fn install(&self, r: &Session, local: Option<&Session>, folder: &str, fix: Fix) -> Result<u64> {
        codex::install(r, local, folder, fix)
    }
    fn local_turns(&self, s: &Session) -> Result<(Vec<Turn>, Option<String>)> {
        Ok((codex::file_turns(s.src.as_ref().expect("local"))?, None))
    }
    fn stored_turns(&self, r: &Session, fix: Fix) -> Result<(Vec<Turn>, Option<String>)> {
        Ok((codex::turns(stored_line_iter(r, fix)?), None))
    }
    fn write(&self, turns: &[Turn], folder: &str, title: &str, _model: Option<&str>) -> Result<Session> {
        codex::write(turns, folder, title)
    }
    fn other_branch(&self, l: &Session, r: &Session, fix: Fix) -> Result<Vec<Turn>> {
        codex::other_branch(l, r, fix)
    }
    fn append(&self, l: &mut Session, text: &str, ack: &str) -> Result<()> {
        codex::append(l, text, ack)
    }
    fn branch_copy(&self, r: &Session, folder: &str, fix: Fix, title: &str) -> Result<Session> {
        codex::branch_copy(r, folder, fix, title)
    }
    fn resume(&self, sid: &str, folder: &str) -> String {
        format!("cd {} && codex resume {sid}", shell_path(folder))
    }
}
