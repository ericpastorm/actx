//! The shared folder: <project>/<tool>/<id>/{meta.json, preview.txt, the conversation, compressed}.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::ctx::{Result, ctx, jstr, now};
use crate::session::Session;
use crate::state::State;
use crate::tr;
use crate::ui::{bold, die, warn};

pub const COMP: &str = "zst";
/// Reserved storage scope, hidden from older clients' project enumeration.
pub const GENERAL: &str = "_general";

pub fn collection_label(name: &str) -> String {
    if name == GENERAL {
        tr!("General (no project)")
    } else {
        name.to_string()
    }
}

/// A stored file, decompressed (zstd, or gzip from actx versions running on Python < 3.14).
pub fn open_read(path: &Path) -> Result<Box<dyn BufRead>> {
    let file = File::open(path)?;
    Ok(match path.extension().and_then(|e| e.to_str()) {
        Some("zst") => Box::new(BufReader::new(zstd::Decoder::new(file)?)),
        Some("gz") => Box::new(BufReader::new(flate2::read::MultiGzDecoder::new(file))),
        _ => Box::new(BufReader::new(file)),
    })
}

pub fn read_all(path: &Path) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    open_read(path)?.read_to_end(&mut data)?;
    Ok(data)
}

/// Compress `src` into `dest`; returns how many bytes were read.
pub fn write_compressed(dest: &Path, mut src: impl Read) -> Result<u64> {
    let mut enc = zstd::Encoder::new(File::create(dest)?, 3)?;
    let n = io::copy(&mut src, &mut enc)?;
    enc.finish()?;
    Ok(n)
}

pub fn tar_dir(dest: &Path, dir: &Path, name: &str) -> Result<()> {
    let mut tar = tar::Builder::new(zstd::Encoder::new(File::create(dest)?, 3)?);
    tar.follow_symlinks(false);
    tar.append_dir_all(name, dir)?;
    tar.into_inner()?.finish()?;
    Ok(())
}

pub fn untar(archive: &Path, dest: &Path) -> Result<()> {
    tar::Archive::new(open_read(archive)?).unpack(dest)?;
    Ok(())
}

/// A conversation file up to its last complete line (an agent may be writing the next one), and
/// its whole size.
pub fn complete_lines(path: &Path) -> Result<(io::Take<File>, u64)> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut buf = vec![0u8; 64 << 10];
    let mut end = size;
    while end > 0 {
        let start = end.saturating_sub(buf.len() as u64);
        let chunk = &mut buf[..(end - start) as usize];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(chunk)?;
        if let Some(i) = memchr::memrchr(b'\n', chunk) {
            file.seek(SeekFrom::Start(0))?;
            return Ok((file.take(start + i as u64 + 1), size));
        }
        end = start;
    }
    file.seek(SeekFrom::Start(0))?;
    Ok((file.take(size), size))
}

/// Each line of a stored file, decompressed and with this machine's paths.
pub fn stored_lines(path: &Path, fix: crate::paths::Fix) -> Result<Vec<Vec<u8>>> {
    let mut lines = Vec::new();
    for line in open_read(path)?.split(b'\n') {
        lines.push(crate::paths::apply(fix, &line?));
    }
    Ok(lines)
}

/// The shared folder's path, as configured.
pub fn configured_root() -> Option<PathBuf> {
    std::env::var("ACTX_ROOT")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| ctx().config.get("root").and_then(Value::as_str).map(String::from))
        .map(|s| PathBuf::from(shellexpand(&s)))
}

/// The shared folder, if it's there, without waiting more than `timeout` for it (a network share
/// that's gone can hang for a minute) and without saying anything. For automatic syncs.
pub fn reachable_root(timeout: std::time::Duration) -> Option<PathBuf> {
    let root = configured_root()?;
    let (tx, rx) = std::sync::mpsc::channel();
    let probe = root.clone();
    std::thread::spawn(move || {
        let _ = tx.send(probe.join(".actx").exists() || probe.is_dir());
    });
    rx.recv_timeout(timeout).unwrap_or(false).then_some(root)
}

pub fn store_root(optional: bool) -> Option<PathBuf> {
    let _loading = crate::ui::loading(&tr!("Connecting to the shared folder"));
    let Some(root) = configured_root() else {
        if optional {
            return None;
        }
        die(&tr!(
            "No shared folder configured yet. Run: {cmd}",
            cmd = bold("actx init")
        ));
    };
    if optional {
        return root.is_dir().then_some(root);
    }
    if let Err(e) = std::fs::create_dir(&root)
        && e.kind() != io::ErrorKind::AlreadyExists
    {
        if optional {
            warn(&tr!(
                "Can't reach the shared folder ({err}): using only what's on this machine",
                err = e
            ));
            return None;
        }
        die(&format!(
            "{}\n  {}",
            tr!(
                "Can't reach the shared folder ({root}): {err}.",
                root = root.display(),
                err = e
            ),
            tr!("If it's a network share, check that it's mounted (away from home you may need a VPN).")
        ));
    }
    Some(root)
}

pub fn shellexpand(path: &str) -> String {
    match path.strip_prefix("~") {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => format!("{}{rest}", ctx().home_str()),
        _ => path.to_string(),
    }
}

fn names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

pub fn project_dirs(root: &Path) -> Vec<PathBuf> {
    names(root)
        .into_iter()
        .filter(|n| !n.starts_with('.') && !n.starts_with('_'))
        .map(|n| root.join(n))
        .filter(|p| p.is_dir())
        .collect()
}

pub fn collection_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = project_dirs(root);
    if root.join(GENERAL).is_dir() {
        dirs.push(root.join(GENERAL));
    }
    dirs
}

pub fn find_project(root: &Path, name: &str) -> Option<PathBuf> {
    project_dirs(root)
        .into_iter()
        .find(|d| d.file_name().map(|n| n.to_string_lossy().to_lowercase()) == Some(name.to_lowercase()))
}

/// The conversation stored in `dir` (<project>/<tool>/<id>).
fn stored_session(dir: PathBuf, project: &str) -> Option<Session> {
    let file = dir.join("meta.json");
    let meta = crate::inspection::optional_json::<Value>(&file);
    if meta.is_none() && dir.is_dir() && !file.exists() && crate::inspection::enabled() {
        crate::inspection::record_error(format!("Missing session metadata: {}", file.display()));
    }
    let meta = meta?; // none: a half-written upload
    let harness = crate::harness::known(jstr(&meta, "harness"));
    if (harness.is_none() || jstr(&meta, "id").is_empty()) && crate::inspection::enabled() {
        crate::inspection::record_error(format!(
            "Unsupported or incomplete session metadata: {}",
            file.display()
        ));
    }
    let harness = harness?; // stored by a newer actx that knows more tools
    let path = match meta.get("rel").and_then(Value::as_str) {
        Some(rel) => crate::paths::under_home(rel),
        None => jstr(&meta, "path").to_string(),
    };
    let mut s = Session::local(
        harness,
        jstr(&meta, "id").to_string(),
        jstr(&meta, "title").to_string(),
        path,
        meta.get("updated").and_then(Value::as_f64).unwrap_or(0.0),
        meta.get("weight").and_then(Value::as_u64).unwrap_or(0),
    );
    s.meta = Some(meta);
    s.store_dir = Some(dir);
    s.project = Some(project.to_string());
    Some(s)
}

fn project_name(dir: &Path) -> String {
    dir.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

pub fn stored_sessions(root: &Path) -> Vec<Session> {
    let mut out = Vec::new();
    for project in collection_dirs(root) {
        let pname = project_name(&project);
        let context = crate::inspection::optional_json::<Value>(&project.join(".project.json"));
        for tool in names(&project).into_iter().filter(|n| !n.starts_with('.')) {
            for id in names(&project.join(&tool)).into_iter().filter(|n| !n.starts_with('.')) {
                if let Some(mut s) = stored_session(project.join(&tool).join(&id), &pname) {
                    if let (Some(context), Some(meta)) = (&context, s.meta.as_mut()) {
                        meta["project_folder"] = context.clone();
                    }
                    out.push(s);
                }
            }
        }
    }
    out
}

/// One conversation, wherever it's stored.
pub fn find_stored(root: &Path, harness: &str, id: &str) -> Option<Session> {
    if id.is_empty() || id.contains(['/', '\\']) || id.starts_with('.') {
        return None;
    }
    collection_dirs(root)
        .into_iter()
        .find_map(|p| stored_session(p.join(harness).join(id), &project_name(&p)))
}

/// When a stored conversation was last pushed, to notice if someone pushed it again meanwhile.
pub fn pushed_at(r: &Session) -> f64 {
    r.meta().get("pushed_at").and_then(Value::as_f64).unwrap_or(0.0)
}

/// key → when it was last pushed, for everything in the shared folder now.
pub fn pushed_now(root: &Path) -> HashMap<String, f64> {
    stored_sessions(root).iter().map(|r| (r.key(), pushed_at(r))).collect()
}

/// What a tool's push leaves in the upload folder.
pub struct Pushed {
    pub files: Value,
    pub updated: f64,
    pub weight: u64,
    /// this machine's weight of what was pushed (it may still be growing)
    pub seen: u64,
    pub preview: String,
}

/// Store `s` in `project`. If it was in another project, move it and return that project's name.
pub fn push_one(
    root: &Path,
    project: &str,
    s: &Session,
    git: Option<Value>,
    st: &mut State,
    remote: &HashMap<String, Session>,
) -> Result<Option<String>> {
    let _loading = crate::ui::loading(&tr!("Saving {title}", title = s.title));
    let baseline_stamp = crate::integrity::stamp(s);
    let before = crate::integrity::fresh(s)?;
    let mut transaction = crate::publication::Transaction::begin(root, project, s.harness, &s.id)?;
    let tmp = transaction.temporary();
    std::fs::create_dir(&tmp)?;
    let result = (|| -> Result<(u64, u64)> {
        let pushed = crate::harness::get(s.harness).push(s, &tmp)?;
        if crate::integrity::fresh(s)? != before {
            return Err(crate::ctx::Error(
                "Conversation changed while preparing the upload; retry after its turn finishes".into(),
            ));
        }
        let content_hash = crate::integrity::records(s.harness, open_read(&tmp.join(jstr(&pushed.files, "session")))?)?;
        let identity = crate::catalog::identity(&s.key());
        let identity = if identity == s.key() {
            remote
                .get(&s.key())
                .and_then(|r| r.meta().get("conversation_id"))
                .and_then(Value::as_str)
                .unwrap_or(&identity)
                .to_string()
        } else {
            identity
        };
        let meta = json!({
            "conversation_id": identity,
            "v": 2, "harness": s.harness, "id": s.id, "title": s.title,
            "path": s.path, "rel": crate::paths::rel_home(&s.path), "home": ctx().home_str(),
            "host": ctx().host, "pushed_at": now(), "updated": pushed.updated, "weight": pushed.weight,
            "files": pushed.files, "git": if project == GENERAL { None } else { git }, "content_hash": content_hash,
        });
        std::fs::write(tmp.join("meta.json"), serde_json::to_string_pretty(&meta)?)?;
        std::fs::write(tmp.join("preview.txt"), &pushed.preview)?;
        transaction.ready()?;
        transaction.publish(false)?;
        let weights = (pushed.seen, pushed.weight);
        st.content.insert(
            s.key(),
            crate::integrity::Baseline {
                local: if s.harness == "opencode" {
                    before.clone()
                } else {
                    content_hash.clone()
                },
                remote: content_hash,
                stamp: if pushed.seen == s.weight {
                    baseline_stamp.clone()
                } else {
                    String::new()
                },
                weights,
            },
        );
        Ok(weights)
    })();
    st.base.insert(s.key(), result?);
    if project == GENERAL && Path::new(&s.path).is_dir() {
        st.folders.insert(format!("session:{}", s.key()), s.path.clone());
    }
    if let Some(prev) = remote.get(&s.key())
        && prev.project.as_deref() != Some(project)
    {
        remove_stored(prev)?;
        return Ok(prev.project.clone());
    }
    Ok(None)
}

pub fn remove_stored(r: &Session) -> Result<()> {
    let _loading = crate::ui::loading(&tr!("Deleting {title}", title = r.title));
    std::fs::remove_dir_all(r.store_dir())?;
    crate::gui_index::invalidate();
    if let Some(parent) = r.store_dir().parent() {
        let _ = std::fs::remove_dir(parent); // the tool's folder, if it's now empty
    }
    Ok(())
}

/// How this copy of actx was installed: a "dev" build, a system "package", or a "user" file.
pub fn install_kind() -> &'static str {
    let me = &ctx().self_path;
    let dev = crate::paths::slashed(&me.to_string_lossy()).contains("/target/")
        && me
            .ancestors()
            .any(|d| d.join("Cargo.toml").exists() && d.join(".git").exists());
    if dev {
        return "dev";
    }
    let writable = me.parent().map(|d| tempfile::tempfile_in(d).is_ok()).unwrap_or(false);
    if me.starts_with("/usr") || me.starts_with("/opt") || !writable {
        "package"
    } else {
        "user"
    }
}

/// Where machines without a package find actx in the shared folder (binaries differ per platform).
pub fn shared_binary(root: &Path) -> PathBuf {
    root.join("_bin")
        .join(format!("actx-{}-{}", std::env::consts::ARCH, std::env::consts::OS))
}

fn mtime(p: &Path) -> f64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn modified(p: &Path) -> f64 {
    mtime(p)
}

/// Stands in for the Python actx of old in the shared folder (src/bridge.py).
const BRIDGE: &str = include_str!("bridge.py");

/// The script with `\n` line endings: a checkout on Windows may have turned them into `\r\n`,
/// and `python3\r` is not a program.
fn bridge_script() -> String {
    BRIDGE.replace("\r\n", "\n")
}

/// Leave a copy of actx in the shared folder, to install it on machines without a package.
pub fn sync_self(root: &Path) {
    let kind = install_kind();
    if kind == "package" {
        return;
    }
    share_binary(root, kind);
    leave_bridge(root);
}

/// Machines still on the Python version update from _bin/actx, a Python script: leave one there
/// that moves them to this program.
fn leave_bridge(root: &Path) {
    let bridge = root.join("_bin").join("actx");
    if !shared_binary(root).exists() || std::fs::read(&bridge).is_ok_and(|b| b == bridge_script().as_bytes()) {
        return;
    }
    let tmp = bridge.with_extension("tmp");
    if std::fs::write(&tmp, bridge_script()).is_ok() {
        let _ = std::fs::rename(&tmp, &bridge);
    }
}

fn share_binary(root: &Path, kind: &str) {
    let _loading = crate::ui::loading(&tr!("Updating the shared actx binary"));
    let shared = shared_binary(root);
    let me = &ctx().self_path;
    let len = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
    // the copy keeps this one's date: same size and date is the same file, without reading it all
    let same = len(&shared).is_some()
        && len(&shared) == len(me)
        && (mtime(&shared) == mtime(me)
            || std::fs::read(&shared)
                .ok()
                .zip(std::fs::read(me).ok())
                .is_some_and(|(a, b)| a == b));
    if same {
        return;
    }
    if shared.exists() && mtime(&shared) > mtime(me) + 2.0 {
        if kind == "user" && !crate::ui::is_auto() {
            warn(&tr!(
                "There's a newer actx in the shared folder: {cmd}",
                cmd = bold("actx update")
            ));
        }
        return;
    }
    let _ = std::fs::create_dir_all(shared.parent().unwrap_or(root));
    let tmp = shared.with_extension("tmp");
    if std::fs::copy(me, &tmp).is_ok() {
        let _ = File::open(me)
            .and_then(|f| f.metadata())
            .and_then(|m| File::options().write(true).open(&tmp)?.set_modified(m.modified()?));
        let _ = std::fs::rename(&tmp, &shared);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_being_written_is_left_for_next_time() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("s.jsonl");
        let long = format!("{{\"x\":\"{}\"}}\n", "y".repeat(100_000)); // longer than one read
        for (text, kept) in [
            (format!("{{\"a\":1}}\n{long}{{\"b\":"), format!("{{\"a\":1}}\n{long}")),
            ("{\"a\":1}\n".to_string(), "{\"a\":1}\n".to_string()),
            ("{\"a\":".to_string(), "{\"a\":".to_string()), // nothing complete: all of it
            (String::new(), String::new()),
        ] {
            std::fs::write(&f, &text).unwrap();
            let (mut file, size) = complete_lines(&f).unwrap();
            let mut got = String::new();
            file.read_to_string(&mut got).unwrap();
            assert_eq!((got, size), (kept, text.len() as u64));
        }
    }
}
