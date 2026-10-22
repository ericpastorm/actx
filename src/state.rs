//! What this machine remembers between runs, and backups.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{Value, json};

use crate::ctx::{Result, ctx, now};
use crate::session::Session;

/// Backups older than this are deleted.
pub const KEEP_BACKUP_DAYS: u64 = 30;

#[derive(Default, Clone)]
pub struct State {
    /// key → (local weight, stored weight) at the last sync
    pub base: BTreeMap<String, (u64, u64)>,
    /// origin repo ("git:<first commit>") or path ("path:<path>") → folder picked on this machine
    pub folders: BTreeMap<String, String>,
    pub content: BTreeMap<String, crate::integrity::Baseline>,
    read_content: BTreeMap<String, crate::integrity::Baseline>,
    /// as read, so that saving writes only what this run changed (another run may have saved meanwhile)
    read_base: BTreeMap<String, (u64, u64)>,
    read_folders: BTreeMap<String, String>,
}

fn number(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_f64().map(|f| f as u64))
}

fn read_file(file: &Path) -> State {
    let raw: Value = crate::inspection::optional_json(file).unwrap_or(Value::Null);
    let mut st = State::default();
    if let Some(base) = raw.get("base").and_then(Value::as_object) {
        for (key, pair) in base {
            if let Some([a, b]) = pair.as_array().map(Vec::as_slice)
                && let (Some(a), Some(b)) = (number(a), number(b))
            {
                st.base.insert(key.clone(), (a, b));
            }
        }
    }
    if let Some(folders) = raw.get("folders").and_then(Value::as_object) {
        for (key, folder) in folders {
            if let Some(folder) = folder.as_str() {
                st.folders.insert(key.clone(), folder.to_string());
            }
        }
    }
    st.content = raw
        .get("content")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    st.read_content = st.content.clone();
    st.read_base = st.base.clone();
    st.read_folders = st.folders.clone();
    st
}

pub fn load() -> State {
    let file = ctx().state_file();
    if !file.exists() {
        let legacy = ctx().state_dir.parent().map(|p| p.join("ctx").join("state.json")); // before the rename (ctx → actx)
        if let Some(legacy) = legacy.filter(|p| p.exists()) {
            if crate::inspection::enabled() {
                return read_file(&legacy);
            }
            let _ = std::fs::create_dir_all(&ctx().state_dir);
            let _ = std::fs::copy(legacy, &file);
        }
    }
    read_file(&file)
}

/// Apply to `into` what changed from `was` to `is`.
fn apply_changes<V: Clone + PartialEq>(
    was: &BTreeMap<String, V>,
    is: &BTreeMap<String, V>,
    into: &mut BTreeMap<String, V>,
) {
    for (k, v) in is {
        if was.get(k) != Some(v) {
            into.insert(k.clone(), v.clone());
        }
    }
    for k in was.keys().filter(|k| !is.contains_key(*k)) {
        into.remove(k);
    }
}

pub fn save(st: &mut State) -> Result<()> {
    std::fs::create_dir_all(&ctx().state_dir)?;
    let guard = std::fs::File::create(ctx().state_dir.join("state.lock"))?;
    guard.lock()?;
    let mut now = read_file(&ctx().state_file());
    apply_changes(&st.read_base, &st.base, &mut now.base);
    apply_changes(&st.read_folders, &st.folders, &mut now.folders);
    apply_changes(&st.read_content, &st.content, &mut now.content);
    let base: serde_json::Map<String, Value> = now.base.iter().map(|(k, (a, b))| (k.clone(), json!([a, b]))).collect();
    let data = json!({"base": base, "folders": now.folders, "content": now.content});
    let tmp = ctx().state_dir.join(format!("state.tmp-{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(&data)?)?;
    std::fs::rename(tmp, ctx().state_file())?;
    now.read_content = now.content.clone();
    now.read_base = now.base.clone();
    now.read_folders = now.folders.clone();
    *st = now;
    crate::gui_index::invalidate();
    Ok(())
}

// ── backups ─────────────────────────────────────────────────────────────────

/// This run's backup folder (one per run), created when first needed. Old ones are cleaned up then.
fn backup_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        prune_backups();
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let mut d = ctx().backups().join(&stamp);
        let mut n = 1;
        while d.exists() {
            n += 1;
            d = ctx().backups().join(format!("{stamp}-{n}"));
        }
        let _ = std::fs::create_dir_all(&d);
        d
    })
    .clone()
}

fn prune_backups() {
    let limit = now() - (KEEP_BACKUP_DAYS * 86400) as f64;
    for d in std::fs::read_dir(ctx().backups())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
    {
        if d.path().is_dir() && crate::store::modified(&d.path()) < limit {
            let _ = std::fs::remove_dir_all(d.path());
        }
    }
}

pub enum Copy<'a> {
    File(&'a Path),
    Bytes(&'a [u8]),
}

/// Keep a copy of a conversation on this machine before it's overwritten (`why`: pull, merge, restore).
pub fn backup(s: &Session, copy: Copy, why: &str) -> Result<()> {
    let dir = backup_dir();
    let ext = if s.harness == "opencode" { "json" } else { "jsonl" };
    let name = format!("{}-{}.{ext}", s.harness, s.id);
    let dest = dir.join(&name);
    match copy {
        Copy::File(src) => copy_file(src, &dest)?,
        Copy::Bytes(data) => std::fs::write(&dest, data)?,
    }
    let meta = json!({
        "harness": s.harness, "id": s.id, "title": s.title, "path": s.path, "src": s.src,
        "file": name, "why": why, "at": now(),
    });
    std::fs::write(
        dir.join(format!("{}-{}.meta.json", s.harness, s.id)),
        serde_json::to_string_pretty(&meta)?,
    )?;
    Ok(())
}

fn copy_file(src: &Path, dest: &Path) -> Result<()> {
    // reflink: on btrfs/xfs the copy is instant and takes no space until something changes
    // (std::fs::copy already clones on macOS)
    let reflinked = cfg!(target_os = "linux")
        && std::process::Command::new("cp")
            .arg("--reflink=auto")
            .arg("-p")
            .arg(src)
            .arg(dest)
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
    if !reflinked {
        std::fs::copy(src, dest)?;
    }
    Ok(())
}

/// A backed-up conversation.
pub struct Backup {
    pub file: PathBuf,
    pub harness: &'static str,
    pub id: String,
    pub title: String,
    pub path: String,
    pub src: Option<PathBuf>,
    pub at: f64,
}

/// All backups, newest first. Those made before backups had a .meta.json are listed too.
pub fn backups() -> Vec<Backup> {
    let mut out = Vec::new();
    for d in std::fs::read_dir(ctx().backups())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
    {
        for f in std::fs::read_dir(d.path()).into_iter().flatten().filter_map(|e| e.ok()) {
            let file = f.path();
            let name = f.file_name().to_string_lossy().into_owned();
            if name.ends_with(".meta.json") || name.starts_with('.') {
                continue;
            }
            let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
            let Some((harness, id)) = stem.split_once('-') else {
                continue;
            };
            let Some(harness) = crate::harness::known(harness) else {
                continue;
            };
            let meta: Value = std::fs::read_to_string(d.path().join(format!("{stem}.meta.json")))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(Value::Null);
            let text = |k: &str| meta.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            out.push(Backup {
                harness,
                id: id.to_string(),
                title: text("title"),
                path: text("path"),
                src: meta.get("src").and_then(Value::as_str).map(PathBuf::from),
                at: meta
                    .get("at")
                    .and_then(Value::as_f64)
                    .unwrap_or_else(|| crate::store::modified(&file)),
                file,
            });
        }
    }
    out.sort_by(|a, b| b.at.total_cmp(&a.at));
    out
}

impl State {
    pub fn status(&self, local: Option<&Session>, remote: Option<&Session>) -> &'static str {
        if let (Some(l), Some(r)) = (local, remote)
            && let Some(b) = self
                .content
                .get(&r.key())
                .filter(|b| Some(&b.weights) == self.base.get(&r.key()))
        {
            let here = if crate::integrity::stamp(l) == b.stamp {
                Ok(b.local.clone())
            } else {
                crate::integrity::local(l)
            };
            let there = crate::integrity::fresh(r);
            return match (here, there) {
                (Ok(h), Ok(t)) => match (h != b.local, t != b.remote) {
                    (false, false) => "synced",
                    (true, false) => "push",
                    (false, true) => "pull",
                    (true, true) => "conflict",
                },
                _ => "unknown",
            };
        }
        crate::session::status(local, remote, remote.and_then(|r| self.base.get(&r.key()).copied()))
    }
    /// Catalog display must not download legacy shared payloads. Current publications
    /// provide their content hash in metadata; older ones retain their weight indicator.
    /// Action planning/revision checks still call the full `status` method above.
    pub fn display_status(&self, local: Option<&Session>, remote: Option<&Session>) -> &'static str {
        if let (Some(l), Some(r)) = (local, remote)
            && !r.meta().get("content_hash").is_some_and(Value::is_string)
            && let Some(b) = self
                .content
                .get(&r.key())
                .filter(|b| Some(&b.weights) == self.base.get(&r.key()))
        {
            let here = if crate::integrity::stamp(l) == b.stamp {
                Ok(b.local.clone())
            } else {
                crate::integrity::local(l)
            };
            return match here {
                Ok(hash) => match (hash != b.local, r.weight != b.weights.1) {
                    (false, false) => "synced",
                    (true, false) => "push",
                    (false, true) => "pull",
                    (true, true) => "conflict",
                },
                Err(_) => "unknown",
            };
        }
        self.status(local, remote)
    }
    pub fn checkpoint(&mut self, local: &Session, remote: &Session) -> Result<()> {
        let weights = *self
            .base
            .get(&remote.key())
            .ok_or_else(|| crate::ctx::Error("Missing sync baseline".into()))?;
        self.content
            .insert(remote.key(), crate::integrity::capture(local, remote, weights)?);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_display_does_not_open_legacy_shared_payloads() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("local.jsonl");
        std::fs::write(&file, "{\"type\":\"assistant\",\"message\":{\"content\":\"saved\"}}\n").unwrap();
        let mut local = Session::local(
            "claude",
            "legacy-display".into(),
            "title".into(),
            "/work".into(),
            1.,
            100,
        );
        local.src = Some(file);
        let mut remote = local.clone();
        remote.src = None;
        remote.meta = Some(json!({"files":{"session":"missing-payload.zst"}}));
        let mut state = State::default();
        state.base.insert(remote.key(), (100, 100));
        state.content.insert(
            remote.key(),
            crate::integrity::Baseline {
                local: crate::integrity::fresh(&local).unwrap(),
                remote: "stored-content".into(),
                stamp: crate::integrity::stamp(&local),
                weights: (100, 100),
            },
        );
        assert_eq!(state.display_status(Some(&local), Some(&remote)), "synced");
        remote.weight = 101;
        assert_eq!(state.display_status(Some(&local), Some(&remote)), "pull");
    }
    #[test]
    fn saving_keeps_what_others_saved_meanwhile() {
        let was: BTreeMap<String, u64> = [("a".into(), 1), ("b".into(), 1), ("gone".into(), 1)].into();
        let mut is = was.clone();
        is.insert("a".into(), 2); // this run pushed a
        is.remove("gone"); // and removed gone
        // meanwhile another run changed b and added c
        let mut disk = was.clone();
        disk.insert("b".into(), 5);
        disk.insert("c".into(), 1);
        apply_changes(&was, &is, &mut disk);
        let want: BTreeMap<String, u64> = [("a".into(), 2), ("b".into(), 5), ("c".into(), 1)].into();
        assert_eq!(disk, want);
    }
}
