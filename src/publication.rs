//! Durable shared-folder publications and explicit recovery of interrupted swaps.
use crate::{
    actions::{self, Request},
    api::{Response, Status},
    ctx::{Error, Result, ctx},
    harness, store,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
};
fn component(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('.') && !s.contains(['/', '\\', ':', '\0'])
}
fn present(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn hash_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = 0xcbf29ce484222325u64;
    let mut bytes = [0; 65536];
    loop {
        crate::work::check()?;
        let n = file.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        for b in &bytes[..n] {
            hash = (hash ^ u64::from(*b)).wrapping_mul(0x100000001b3);
        }
    }
    Ok(format!("{hash:016x}"))
}
fn files(dir: &Path) -> Result<BTreeMap<String, String>> {
    if std::fs::symlink_metadata(dir)?.file_type().is_symlink() || !dir.is_dir() {
        return Err(Error("Recovery source must be a regular stored directory".into()));
    }
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type()?.is_file() || !component(&name) {
            return Err(Error("Recovery source contains unsupported files or links".into()));
        }
        out.insert(name, hash_file(&entry.path())?);
    }
    Ok(out)
}
#[derive(Clone, Serialize, Deserialize)]
struct Journal {
    version: u8,
    nonce: String,
    host: String,
    pid: u32,
    at: f64,
    project: String,
    harness: String,
    id: String,
    temporary: String,
    previous: String,
    phase: String,
    checksums: BTreeMap<String, String>,
}
pub struct Transaction {
    parent: PathBuf,
    journal: Journal,
}
impl Transaction {
    /// The caller must hold this root's shared lock throughout preparation and publication.
    pub fn begin(root: &Path, project: &str, harness: &str, id: &str) -> Result<Self> {
        if !component(project) || !harness::syncs(harness) || !component(id) {
            return Err(Error("Invalid shared publication identity".into()));
        }
        if !crate::lock::owned(root) {
            return Err(Error(
                "Shared publication lock is no longer owned; retry the operation".into(),
            ));
        }
        let parent = root.join(project).join(harness);
        std::fs::create_dir_all(&parent)?;
        let nonce = uuid::Uuid::new_v4().to_string();
        let suffix = format!("{id}-{}-{nonce}", std::process::id());
        let journal = Journal {
            version: 1,
            nonce,
            host: ctx().host.clone(),
            pid: std::process::id(),
            at: crate::ctx::now(),
            project: project.into(),
            harness: harness.into(),
            id: id.into(),
            temporary: format!(".tmp-{suffix}"),
            previous: format!(".old-{suffix}"),
            phase: "preparing".into(),
            checksums: BTreeMap::new(),
        };
        let t = Self { parent, journal };
        t.save()?;
        Ok(t)
    }
    fn root(&self) -> &Path {
        self.parent.parent().unwrap().parent().unwrap()
    }
    fn journal_path(&self) -> PathBuf {
        self.parent.join(format!(".publish-{}.json", self.journal.nonce))
    }
    pub fn temporary(&self) -> PathBuf {
        self.parent.join(&self.journal.temporary)
    }
    fn previous(&self) -> PathBuf {
        self.parent.join(&self.journal.previous)
    }
    fn destination(&self) -> PathBuf {
        self.parent.join(&self.journal.id)
    }
    fn save(&self) -> Result<()> {
        let mut tmp = tempfile::NamedTempFile::new_in(&self.parent)?;
        tmp.write_all(&serde_json::to_vec_pretty(&self.journal)?)?;
        tmp.as_file().sync_all()?;
        tmp.persist(self.journal_path()).map_err(|e| Error(e.to_string()))?;
        sync_dir(&self.parent)
    }
    pub fn ready(&mut self) -> Result<()> {
        self.journal.checksums = files(&self.temporary())?;
        for name in self.journal.checksums.keys() {
            std::fs::File::options()
                .write(true)
                .open(self.temporary().join(name))?
                .sync_all()?;
        }
        sync_dir(&self.temporary())?;
        self.journal.phase = "ready".into();
        self.save()
    }
    pub fn publish(&mut self, retain_previous: bool) -> Result<()> {
        if !crate::lock::owned(self.root()) {
            return Err(Error(
                "Shared publication lock changed; staged files were preserved".into(),
            ));
        }
        if self.journal.checksums.is_empty() || files(&self.temporary())? != self.journal.checksums {
            return Err(Error("Staged publication changed; all versions were preserved".into()));
        }
        let old = self.previous();
        let dest = self.destination();
        let tmp = self.temporary();
        if present(&old)? {
            return Err(Error(
                "Previous publication already exists; all versions were preserved".into(),
            ));
        }
        let result = (|| -> Result<()> {
            if present(&dest)? {
                let kind = std::fs::symlink_metadata(&dest)?.file_type();
                if !kind.is_dir() {
                    return Err(Error(
                        "Current shared publication is not a regular directory; it was kept".into(),
                    ));
                }
                std::fs::rename(&dest, &old)?;
                sync_dir(&self.parent)?;
            }
            self.journal.phase = "previous_saved".into();
            self.save()?;
            std::fs::rename(&tmp, &dest)?;
            sync_dir(&self.parent)?;
            self.journal.phase = "published".into();
            self.save()?;
            Ok(())
        })();
        if result.is_err() {
            if old.exists() && !dest.exists() {
                let _ = std::fs::rename(&old, &dest);
                let _ = sync_dir(&self.parent);
            }
            return result;
        }
        crate::gui_index::invalidate();
        if !retain_previous
            && old.exists()
            && let Err(e) = std::fs::remove_dir_all(&old)
        {
            crate::ui::warn(&format!("Previous shared version retained for recovery: {e}"));
            return Ok(());
        }
        if !old.exists() {
            let _ = std::fs::remove_file(self.journal_path());
        }
        sync_dir(&self.parent)
    }
}
#[derive(Clone)]
struct Artifact {
    path: PathBuf,
    project: String,
    harness: String,
    native_id: String,
    kind: &'static str,
    meta: Option<Value>,
    journal: Option<Journal>,
    active: bool,
    at: f64,
    problem: Option<String>,
}
impl Artifact {
    fn id(&self) -> String {
        format!(
            "publication:{}",
            crate::engine::digest(self.path.to_string_lossy().as_bytes())
        )
    }
    fn destination(&self) -> PathBuf {
        self.path.parent().unwrap().join(&self.native_id)
    }
    fn row(&self) -> Value {
        json!({"id":self.id(),"kind":"publication","session_id":format!("{}:{}",self.harness,self.native_id),"title":self.meta.as_ref().and_then(|m|m["title"].as_str()).unwrap_or(&self.native_id),"folder":self.destination(),"at":self.at,"status":if self.active{"in_progress"}else if self.problem.is_some(){"incomplete"}else if self.kind=="old"{"previous_shared_version"}else{"staged_version"},"can_restore":!self.active&&self.problem.is_none(),"detail":self.problem,"preserves_current":true,"pending":self.problem.is_some()||self.journal.as_ref().is_none_or(|j|j.phase!="published")})
    }
}
fn metadata(path: &Path, harness: &str, id: &str) -> Result<Value> {
    let file = path.join("meta.json");
    if !std::fs::symlink_metadata(&file)?.file_type().is_file() {
        return Err(Error("Recovery metadata must be a regular file".into()));
    }
    if file.metadata()?.len() > 1024 * 1024 {
        return Err(Error("Recovery metadata is too large".into()));
    }
    let meta: Value = serde_json::from_slice(&std::fs::read(file)?)?;
    if meta["harness"] != harness || meta["id"] != id || !meta["files"].is_object() {
        return Err(Error("Recovery metadata does not match its stored identity".into()));
    }
    for name in payload_names(&meta)? {
        let file = path.join(name);
        if !std::fs::symlink_metadata(&file)?.file_type().is_file() {
            return Err(Error("Recovery payload is missing or is a link".into()));
        }
    }
    Ok(meta)
}
fn payload_names(meta: &Value) -> Result<Vec<String>> {
    let mut names = vec![
        meta["files"]["session"]
            .as_str()
            .ok_or_else(|| Error("Missing recovery conversation payload".into()))?
            .to_string(),
    ];
    if let Some(extra) = meta["files"]["extra"].as_str() {
        names.push(extra.into());
    }
    if let Some(children) = meta["files"]["children"].as_array() {
        for child in children {
            names.push(
                child
                    .as_str()
                    .ok_or_else(|| Error("Invalid recovery child payload".into()))?
                    .into(),
            );
        }
    }
    if names.iter().any(|n| !component(n)) {
        return Err(Error(
            "Recovery payload paths must remain inside their stored directory".into(),
        ));
    }
    Ok(names)
}
fn identity(name: &str) -> Option<(&'static str, String)> {
    let (kind, rest) = if let Some(s) = name.strip_prefix(".tmp-") {
        ("tmp", s)
    } else {
        ("old", name.strip_prefix(".old-")?)
    };
    if rest.len() > 37 {
        let split = rest.len() - 37;
        if rest.as_bytes().get(split) == Some(&b'-') && uuid::Uuid::parse_str(&rest[split + 1..]).is_ok() {
            let (id, pid) = rest[..split].rsplit_once('-')?;
            if component(id) && pid.parse::<u32>().is_ok() {
                return Some((kind, id.into()));
            }
        }
    }
    // Legacy names end in PID; journalled names end in PID + UUID.
    if let Some((id, pid)) = rest.rsplit_once('-')
        && pid.parse::<u32>().is_ok()
        && component(id)
    {
        return Some((kind, id.into()));
    }
    None
}
fn scan(root: &Path) -> Result<Vec<Artifact>> {
    let active = crate::lock::busy_other(root);
    let mut out = Vec::new();
    for project in store::collection_dirs(root) {
        if std::fs::symlink_metadata(&project)?.file_type().is_symlink() {
            continue;
        }
        let collection = project.file_name().unwrap().to_string_lossy().into_owned();
        for harness in harness::NAMES {
            let parent = project.join(harness);
            if !parent.is_dir() {
                continue;
            }
            if std::fs::symlink_metadata(&parent)?.file_type().is_symlink() {
                continue;
            }
            let mut journals = BTreeMap::new();
            let mut artifacts = Vec::new();
            for entry in std::fs::read_dir(&parent)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(".publish-") && entry.file_type()?.is_file() {
                    if entry.metadata()?.len() > 1024 * 1024 {
                        continue;
                    }
                    if let Ok(bytes) = std::fs::read(entry.path())
                        && let Ok(j) = serde_json::from_slice::<Journal>(&bytes)
                        && j.version == 1
                        && j.harness == harness
                        && j.project == collection
                        && component(&j.id)
                        && name == format!(".publish-{}.json", j.nonce)
                        && uuid::Uuid::parse_str(&j.nonce).is_ok()
                        && j.temporary == format!(".tmp-{}-{}-{}", j.id, j.pid, j.nonce)
                        && j.previous == format!(".old-{}-{}-{}", j.id, j.pid, j.nonce)
                    {
                        journals.insert(j.temporary.clone(), j.clone());
                        journals.insert(j.previous.clone(), j);
                    }
                } else if entry.file_type()?.is_dir() && identity(&name).is_some() {
                    artifacts.push((name, entry.path()));
                }
            }
            for (name, path) in artifacts {
                let (kind, id) = identity(&name).unwrap();
                let journal = journals.get(&name).cloned();
                let result = metadata(&path, harness, &id);
                let problem = result.as_ref().err().map(ToString::to_string);
                let meta = result.ok();
                let at = journal.as_ref().map(|j| j.at).unwrap_or_else(|| store::modified(&path));
                out.push(Artifact {
                    path,
                    project: collection.clone(),
                    harness: harness.into(),
                    native_id: id,
                    kind,
                    meta,
                    journal,
                    active,
                    at,
                    problem,
                });
            }
        }
    }
    Ok(out)
}
pub fn entries() -> Result<Vec<Value>> {
    let Some(root) = store::store_root(true) else {
        return if store::configured_root().is_some() {
            Err(Error(
                "Shared recovery unavailable; local backups remain available".into(),
            ))
        } else {
            Ok(vec![])
        };
    };
    Ok(scan(&root)?.iter().map(Artifact::row).collect())
}
fn validate(artifact: &Artifact) -> Result<BTreeMap<String, String>> {
    let meta = metadata(&artifact.path, &artifact.harness, &artifact.native_id)?;
    let checksums = files(&artifact.path)?;
    let main = meta["files"]["session"].as_str().unwrap();
    let hash = crate::integrity::records(&artifact.harness, store::open_read(&artifact.path.join(main))?)?;
    if meta["content_hash"].as_str().is_some_and(|expected| expected != hash) {
        return Err(Error(
            "Recovery conversation fingerprint does not match its metadata".into(),
        ));
    }
    for name in payload_names(&meta)?.iter().filter(|n| n.as_str() != main) {
        if meta["files"]["extra"].as_str() == Some(name) {
            let mut archive = tar::Archive::new(store::open_read(&artifact.path.join(name))?);
            for entry in archive.entries()? {
                let mut entry = entry?;
                let path = entry.path()?;
                if path.is_absolute() || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
                    return Err(Error("Unsafe recovery attachment path".into()));
                }
                std::io::copy(&mut entry, &mut std::io::sink())?;
            }
            std::io::copy(&mut archive.into_inner(), &mut std::io::sink())?;
        } else {
            let _ = crate::integrity::records("opencode", store::open_read(&artifact.path.join(name))?)?;
        }
    }
    if artifact.kind == "tmp"
        && let Some(j) = &artifact.journal
        && !j.checksums.is_empty()
        && j.checksums != checksums
    {
        return Err(Error(
            "Staged recovery files differ from the publication journal".into(),
        ));
    }
    Ok(checksums)
}
fn snapshot(root: &Path, id: &str) -> Result<(Artifact, String)> {
    let artifact = scan(root)?
        .into_iter()
        .find(|a| a.id() == id)
        .ok_or_else(|| Error("Interrupted publication no longer exists".into()))?;
    if artifact.active {
        return Err(Error(
            "A shared-folder writer is active; retry recovery when it finishes".into(),
        ));
    }
    if let Some(problem) = &artifact.problem {
        return Err(Error(problem.clone()));
    }
    let source = validate(&artifact)?;
    let current = if present(&artifact.destination())? {
        Some(files(&artifact.destination())?)
    } else {
        None
    };
    let token = crate::engine::digest(
        &serde_json::to_vec(
            &json!({"source":source,"current":current,"journal":artifact.journal,"path":artifact.path}),
        )
        .unwrap(),
    );
    Ok((artifact, token))
}
pub fn status_label(status: &str) -> String {
    match status {
        "previous_shared_version" => crate::tr!("Previous shared version"),
        "staged_version" => crate::tr!("Staged shared version"),
        "in_progress" => crate::tr!("Shared writer active"),
        _ => crate::tr!("Incomplete publication"),
    }
}
fn preview(artifact: &Artifact) -> Result<Value> {
    let meta = artifact
        .meta
        .as_ref()
        .ok_or_else(|| Error("Missing recovery metadata".into()))?;
    let mut stored = crate::session::Session::local(
        crate::harness::known(&artifact.harness).unwrap(),
        artifact.native_id.clone(),
        meta["title"].as_str().unwrap_or_default().into(),
        meta["path"].as_str().unwrap_or_default().into(),
        0.,
        0,
    );
    stored.meta = Some(meta.clone());
    stored.store_dir = Some(artifact.path.clone());
    stored.project = Some(artifact.project.clone());
    let (turns, _) = harness::get(&artifact.harness).stored_turns(&stored, None)?;
    Ok(
        json!({"total_messages":turns.len(),"messages":turns.iter().skip(turns.len().saturating_sub(6)).map(|t|json!({"role":t.who,"text":crate::ui::take_chars(&t.text,2000)})).collect::<Vec<_>>(),"truncated":turns.len()>6}),
    )
}
pub fn restore(req: &Request) -> Response<Value> {
    let Some(root) = store::store_root(true) else {
        return actions::error(
            "restore",
            "shared_folder_unavailable",
            "Connect the shared folder to recover this publication",
        );
    };
    let Some(id) = req.backup.as_deref() else {
        return actions::error("restore", "selection_required", "Choose a publication to recover");
    };
    let (artifact, token) = match snapshot(&root, id) {
        Ok(s) => s,
        Err(e) => return actions::error("restore", "publication_unavailable", e.to_string()),
    };
    let mut data = json!({"operations":[{"kind":"restore_publication","session_id":format!("{}:{}",artifact.harness,artifact.native_id),"title":artifact.meta.as_ref().and_then(|m|m["title"].as_str()),"source":artifact.path,"destination":artifact.destination()}],"preserves_current":true,"replaces_shared":artifact.destination().exists(),"local_histories_unchanged":true});
    data["recovery_preview"] = match preview(&artifact) {
        Ok(p) => p,
        Err(e) => return actions::error("restore", "publication_unreadable", e.to_string()),
    };
    if let Some(response) = crate::recovery::confirmation(req, "restore", &mut data, token.clone()) {
        return response;
    }
    let _local = match crate::engine::local_guard() {
        Ok(g) => g,
        Err(e) => return actions::error("restore", "local_action_busy", e.to_string()),
    };
    let _shared = match actions::acquire(&root) {
        Ok(g) => g,
        Err(d) => return Response::error("restore", d),
    };
    let (artifact, current) = match snapshot(&root, id) {
        Ok(s) => s,
        Err(e) => return actions::error("restore", "publication_unavailable", e.to_string()),
    };
    if current != token {
        return actions::error(
            "restore",
            "preview_required",
            "Publication changed; review its recovery again",
        );
    }
    let result = (|| -> Result<()> {
        let mut transaction = Transaction::begin(&root, &artifact.project, &artifact.harness, &artifact.native_id)?;
        std::fs::rename(&artifact.path, transaction.temporary())?;
        sync_dir(artifact.path.parent().unwrap())?;
        transaction.ready()?;
        transaction.publish(true)
    })();
    match result {
        Ok(()) => {
            data["applied"] = json!(true);
            actions::response("restore", Status::Ok, data, vec![])
        }
        Err(e) => actions::error("restore", "publication_recovery_failed", e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn transaction(root: &Path) -> Transaction {
        std::fs::write(
            root.join(".lock"),
            json!({"host":ctx().host,"pid":std::process::id(),"at":crate::ctx::now()}).to_string(),
        )
        .unwrap();
        let t = Transaction::begin(root, "Project", "claude", "one").unwrap();
        std::fs::create_dir(t.temporary()).unwrap();
        std::fs::write(t.temporary().join("payload"), b"next").unwrap();
        t
    }
    #[test]
    fn publication_rolls_back_after_the_previous_target_was_moved_if_journal_update_fails() {
        let root = tempfile::tempdir().unwrap();
        let mut t = transaction(root.path());
        std::fs::create_dir(t.destination()).unwrap();
        std::fs::write(t.destination().join("payload"), b"previous").unwrap();
        t.ready().unwrap();
        // Fail exactly after destination -> previous, without a production test hook.
        std::fs::remove_file(t.journal_path()).unwrap();
        std::fs::create_dir(t.journal_path()).unwrap();
        assert!(t.publish(false).is_err());
        assert_eq!(std::fs::read(t.destination().join("payload")).unwrap(), b"previous");
        assert_eq!(std::fs::read(t.temporary().join("payload")).unwrap(), b"next");
    }
    #[test]
    fn a_lost_shared_lease_preserves_both_versions_and_success_keeps_unrelated_remnants() {
        let root = tempfile::tempdir().unwrap();
        let mut t = transaction(root.path());
        std::fs::create_dir(t.destination()).unwrap();
        std::fs::write(t.destination().join("payload"), b"previous").unwrap();
        let unrelated = t.parent.join(".old-one-888888");
        std::fs::create_dir(&unrelated).unwrap();
        std::fs::write(unrelated.join("payload"), b"another recovery").unwrap();
        t.ready().unwrap();
        let lease = std::fs::read(root.path().join(".lock")).unwrap();
        std::fs::write(root.path().join(".lock"), b"lost lease").unwrap();
        assert!(t.publish(false).is_err());
        assert!(t.temporary().exists() && t.destination().exists());
        std::fs::write(root.path().join(".lock"), lease).unwrap();
        t.publish(false).unwrap();
        assert_eq!(std::fs::read(t.destination().join("payload")).unwrap(), b"next");
        assert_eq!(std::fs::read(unrelated.join("payload")).unwrap(), b"another recovery");
        assert!(!t.journal_path().exists());
    }
    #[test]
    fn legacy_and_journalled_names_have_unambiguous_native_identities() {
        assert_eq!(
            identity(".old-ses_with-hyphens-123"),
            Some(("old", "ses_with-hyphens".into()))
        );
        assert_eq!(
            identity(".tmp-ses_with-hyphens-123-00000000-0000-0000-0000-000000000001"),
            Some(("tmp", "ses_with-hyphens".into()))
        );
        assert!(identity(".tmp-unrelated").is_none());
        assert!(!component("../outside"));
    }
}
