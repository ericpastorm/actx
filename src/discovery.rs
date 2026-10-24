//! Disposable discovery metadata. Reads never write; desktop workers persist explicit deltas.
use crate::{
    ctx::{Error, Result, ctx},
    session::Session,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
    sync::{Mutex, OnceLock},
};
const MAX_BYTES: u64 = 32 * 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    signature: String,
    value: Value,
}
#[derive(Serialize, Deserialize)]
struct File {
    version: u8,
    context: String,
    entries: BTreeMap<String, Entry>,
}
#[derive(Default)]
struct Memory {
    loaded: bool,
    entries: BTreeMap<String, Entry>,
    delta: BTreeMap<String, Entry>,
}
fn memory() -> &'static Mutex<Memory> {
    static CACHE: OnceLock<Mutex<Memory>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
fn context() -> String {
    let c = ctx();
    crate::engine::digest(
        &serde_json::to_vec(&json!([c.home, c.host, c.claude_projects, c.codex_home, c.opencode_db])).unwrap(),
    )
}
fn load(path: &Path) -> BTreeMap<String, Entry> {
    let read = (|| -> Option<File> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > MAX_BYTES {
            return None;
        }
        serde_json::from_slice(&bytes).ok()
    })();
    read.filter(|f| f.version == 1 && f.context == context())
        .map(|f| f.entries)
        .unwrap_or_default()
}
fn stamp(path: &Path) -> Value {
    std::fs::metadata(path)
        .ok()
        .map(|m| {
            #[cfg(unix)]
            let extra = {
                use std::os::unix::fs::MetadataExt;
                json!([m.ctime(), m.ctime_nsec(), m.ino()])
            };
            #[cfg(not(unix))]
            let extra = Value::Null;
            json!([
                m.len(),
                m.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos().to_string()),
                extra
            ])
        })
        .unwrap_or(Value::Null)
}
fn signature(path: &Path, extra: &Value) -> Result<String> {
    crate::work::check()?;
    if !path.is_file() {
        return Err(Error(format!("Discovery source unavailable: {}", path.display())));
    }
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    Ok(crate::engine::digest(
        &serde_json::to_vec(&json!([stamp(path), stamp(Path::new(&wal)), extra])).unwrap(),
    ))
}
pub fn memo<T: Serialize + DeserializeOwned>(
    kind: &str,
    path: &Path,
    extra: &Value,
    compute: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let before = signature(path, extra)?;
    let key = format!("{kind}:{}", path.to_string_lossy());
    {
        let mut cache = memory().lock().unwrap();
        if !cache.loaded {
            cache.entries = load(&ctx().cache_dir.join("discovery-v1.json"));
            cache.loaded = true;
        }
        if let Some(entry) = cache.entries.get(&key).filter(|entry| entry.signature == before)
            && let Ok(value) = serde_json::from_value(entry.value.clone())
        {
            return Ok(value);
        }
    }
    let errors = crate::inspection::errors().len();
    let value = compute()?;
    if crate::inspection::errors().len() != errors {
        return Ok(value);
    }
    if signature(path, extra)? != before {
        return Err(Error("Conversation changed while being inspected".into()));
    }
    let entry = Entry {
        signature: before,
        value: serde_json::to_value(&value)?,
    };
    // Huge prompts/previews remain readable, but are deliberately excluded from the index.
    let limit = if kind.starts_with("opencode-inventory:") {
        4 * 1024 * 1024
    } else {
        65536
    };
    if serde_json::to_vec(&entry)?.len() < limit {
        let mut cache = memory().lock().unwrap();
        cache.entries.insert(key.clone(), entry.clone());
        cache.delta.insert(key, entry);
    }
    Ok(value)
}
#[derive(Serialize, Deserialize)]
pub(crate) struct Record {
    id: String,
    title: String,
    path: String,
    updated: f64,
    weight: u64,
    size: Option<u64>,
}
impl From<&Session> for Record {
    fn from(s: &Session) -> Self {
        Self {
            id: s.id.clone(),
            title: s.title.clone(),
            path: s.path.clone(),
            updated: s.updated,
            weight: s.weight,
            size: s.size,
        }
    }
}
impl Record {
    pub(crate) fn session(self, harness: &'static str, src: Option<&Path>) -> Session {
        let mut s = Session::local(harness, self.id, self.title, self.path, self.updated, self.weight);
        s.size = self.size;
        s.src = src.map(Path::to_path_buf);
        s
    }
}
pub fn session(
    harness: &'static str,
    path: &Path,
    extra: &Value,
    compute: impl FnOnce() -> Result<Option<Session>>,
) -> Result<Option<Session>> {
    let row: Option<Record> = memo(&format!("session-{harness}"), path, extra, || {
        compute().map(|s| s.as_ref().map(Record::from))
    })?;
    Ok(row.map(|r| r.session(harness, Some(path))))
}
pub fn inventory(path: &Path, extra: &Value, compute: impl FnOnce() -> Vec<Session>) -> Vec<Session> {
    let result: Result<Vec<Record>> = memo(
        &format!(
            "opencode-inventory:{}",
            crate::engine::digest(&serde_json::to_vec(extra).unwrap())
        ),
        path,
        extra,
        || Ok(compute().iter().map(Record::from).collect()),
    );
    match result {
        Ok(rows) => rows.into_iter().map(|r| r.session("opencode", None)).collect(),
        Err(e) => {
            if crate::inspection::enabled() {
                crate::inspection::record_error(e.to_string());
            }
            vec![]
        }
    }
}
/// Other disposable caches an inspecting read refreshes (content fingerprints, linked-copy
/// hashes). Like the index above, they reach disk only through the desktop job that ran it.
const KEPT: [&str; 2] = ["content-v1.json", "linked-history-v1.json"];
type Kept = BTreeMap<&'static str, serde_json::Map<String, Value>>;
fn kept() -> &'static Mutex<Kept> {
    static KEPT_ENTRIES: OnceLock<Mutex<Kept>> = OnceLock::new();
    KEPT_ENTRIES.get_or_init(Default::default)
}
pub fn keep(file: &'static str, key: String, value: Value) {
    if std::env::var("ACTX_PROGRESS").as_deref() == Ok("1") && KEPT.contains(&file) {
        kept().lock().unwrap().entry(file).or_default().insert(key, value);
    }
}
/// Emitted only by GUI jobs; ordinary CLI/JSON analysis never writes an index.
pub fn emit() {
    if std::env::var("ACTX_PROGRESS").as_deref() != Ok("1") {
        return;
    }
    let delta = std::mem::take(&mut memory().lock().unwrap().delta);
    let mut events = Vec::new();
    if !delta.is_empty() {
        events.push(json!({"event":"discovery_index","version":1,"context":context(),"entries":delta}));
    }
    for (file, entries) in std::mem::take(&mut *kept().lock().unwrap()) {
        events.push(json!({"event":"cache_entries","version":1,"context":context(),"file":file,"entries":entries}));
    }
    for event in events {
        if let Ok(bytes) = serde_json::to_vec(&event)
            && bytes.len() as u64 <= MAX_BYTES
        {
            let mut stderr = std::io::stderr().lock();
            let _ = stderr.write_all(&bytes);
            let _ = stderr.write_all(b"\n");
        }
    }
}
/// Merge entries a desktop job handed over into their cache file. Losing a race costs a cache miss.
#[cfg(feature = "gui")]
pub fn persist_entries(event: &Value) -> Result<()> {
    let Some(file) = KEPT.iter().find(|f| event["file"] == **f) else {
        return Ok(());
    };
    let Some(entries) = event["entries"].as_object() else {
        return Ok(());
    };
    if event["version"] != 1 || event["context"] != context() {
        return Ok(());
    }
    let dir = &ctx().cache_dir;
    std::fs::create_dir_all(dir)?;
    let lock = std::fs::File::create(dir.join(format!("{file}.lock")))?;
    if lock.try_lock().is_err() {
        return Ok(());
    }
    let path = dir.join(file);
    let mut all: serde_json::Map<String, Value> = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    all.extend(entries.clone());
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(tmp.as_file_mut(), &all)?;
    tmp.persist(path).map_err(|e| Error(e.to_string()))?;
    Ok(())
}
#[cfg(feature = "gui")]
pub fn persist(event: &Value) -> Result<()> {
    if event["version"] != 1 || event["context"] != context() {
        return Ok(());
    }
    let delta: BTreeMap<String, Entry> = serde_json::from_value(event["entries"].clone())?;
    let dir = &ctx().cache_dir;
    std::fs::create_dir_all(dir)?;
    let lock = std::fs::File::create(dir.join("discovery.lock"))?;
    if lock.try_lock().is_err() {
        return Ok(());
    }
    let path = dir.join("discovery-v1.json");
    let mut entries = load(&path);
    entries.extend(delta);
    while entries.len() > 20000 {
        entries.pop_first();
    }
    let bytes = serde_json::to_vec(&File {
        version: 1,
        context: context(),
        entries,
    })?;
    if bytes.len() as u64 > MAX_BYTES {
        return Ok(());
    }
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(&bytes)?;
    tmp.persist(path).map_err(|e| Error(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    #[test]
    fn metadata_reuses_unchanged_sources_and_invalidates_edits_and_wal_changes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("history.jsonl");
        std::fs::write(&file, b"first").unwrap();
        let reads = Cell::new(0);
        let read = || {
            reads.set(reads.get() + 1);
            Ok(std::fs::read_to_string(&file)?)
        };
        assert_eq!(memo("test-live", &file, &Value::Null, read).unwrap(), "first");
        assert_eq!(memo("test-live", &file, &Value::Null, read).unwrap(), "first");
        assert_eq!(reads.get(), 1);
        let original = file.metadata().unwrap().modified().unwrap();
        std::fs::write(&file, b"other").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(if cfg!(unix) {
                original // Unix also invalidates equal-mtime edits via ctime.
            } else {
                original + std::time::Duration::from_secs(1)
            })
            .unwrap();
        assert_eq!(memo("test-live", &file, &Value::Null, read).unwrap(), "other");
        assert_eq!(reads.get(), 2);
        std::fs::write(file.with_file_name("history.jsonl-wal"), b"new data").unwrap();
        let _ = memo("test-live", &file, &Value::Null, read).unwrap();
        assert_eq!(reads.get(), 3);
        std::fs::remove_file(&file).unwrap();
        assert!(memo("test-live", &file, &Value::Null, read).is_err());
    }
    #[test]
    fn corrupt_or_foreign_discovery_indexes_fall_back_to_real_sources() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("index.json");
        std::fs::write(&file, b"partial {").unwrap();
        assert!(load(&file).is_empty());
        let entries = BTreeMap::from([(
            "source".into(),
            Entry {
                signature: "one".into(),
                value: json!("metadata"),
            },
        )]);
        std::fs::write(
            &file,
            serde_json::to_vec(&File {
                version: 1,
                context: "another computer".into(),
                entries: entries.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        assert!(load(&file).is_empty());
        std::fs::write(
            &file,
            serde_json::to_vec(&File {
                version: 1,
                context: context(),
                entries,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(load(&file).len(), 1);
    }
}
