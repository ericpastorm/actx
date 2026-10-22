//! Read-only inspection. SQLite works on private temporary snapshots, never on agent files.
use crate::ctx::{Error, Result};
use rusqlite::{Connection, OpenFlags};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::SystemTime,
};

static ENABLED: AtomicUsize = AtomicUsize::new(0);
type Stamp = Option<(u64, SystemTime)>;
#[derive(Default)]
struct Inputs {
    stamps: BTreeMap<PathBuf, Stamp>,
    databases: BTreeMap<PathBuf, (tempfile::TempDir, [Stamp; 3])>,
    errors: Vec<String>,
}
fn inputs() -> &'static Mutex<Inputs> {
    static INPUTS: OnceLock<Mutex<Inputs>> = OnceLock::new();
    INPUTS.get_or_init(Default::default)
}
/// Nested calls keep one snapshot alive for the whole operation (clean, preview, ...).
pub fn enable() {
    ENABLED.fetch_add(1, Ordering::Relaxed);
}
pub fn finish() {
    if ENABLED.load(Ordering::Relaxed) == 0 {
        return;
    }
    if ENABLED.fetch_sub(1, Ordering::Relaxed) == 1 {
        *inputs().lock().unwrap() = Inputs::default();
    }
}
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed) > 0
}
/// Enables inspection for as long as it lives; nests with other guards.
pub struct Guard;
impl Guard {
    pub fn new() -> Guard {
        enable();
        Guard
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        finish();
    }
}
fn stamp(path: &Path) -> Stamp {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| Some((m.len(), m.modified().ok()?)))
}
pub fn observe(path: &Path) {
    inputs()
        .lock()
        .unwrap()
        .stamps
        .entry(path.to_path_buf())
        .or_insert_with(|| stamp(path));
}
pub fn tree(path: &Path) {
    fn walk(path: &Path, seen: &mut std::collections::HashSet<PathBuf>) {
        observe(path);
        if std::fs::metadata(path).is_ok_and(|m| m.is_dir()) {
            if let Ok(real) = std::fs::canonicalize(path)
                && !seen.insert(real)
            {
                return;
            }
            match std::fs::read_dir(path) {
                Ok(entries) => {
                    for entry in entries {
                        match entry {
                            Ok(entry) => walk(&entry.path(), seen),
                            Err(error) => record_error(error.to_string()),
                        }
                    }
                }
                Err(error) => record_error(error.to_string()),
            }
        }
    }
    walk(path, &mut Default::default());
}
pub fn changed() -> bool {
    inputs().lock().unwrap().stamps.iter().any(|(p, was)| stamp(p) != *was)
}
pub fn errors() -> Vec<String> {
    inputs().lock().unwrap().errors.clone()
}
pub fn record_error(error: String) {
    let mut inputs = inputs().lock().unwrap();
    if !inputs.errors.contains(&error) {
        inputs.errors.push(error);
    }
}

/// Preserve optional-file behavior, but never hide corrupt data from an inspection result.
pub fn optional_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    match std::fs::read(path) {
        Ok(data) => match serde_json::from_slice(&data) {
            Ok(value) => Some(value),
            Err(error) => {
                if enabled() {
                    record_error(format!("{}: {error}", path.display()));
                }
                None
            }
        },
        Err(error) => {
            if enabled() && error.kind() != std::io::ErrorKind::NotFound {
                record_error(format!("{}: {error}", path.display()));
            }
            None
        }
    }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

pub fn database(path: &Path) -> Result<Connection> {
    let result = (|| -> Result<Connection> {
        let mut inputs = inputs().lock().unwrap();
        if !path.is_file() {
            return Err(Error(format!("Database unavailable: {}", path.display())));
        }
        let wal = sidecar(path, "-wal");
        let journal = sidecar(path, "-journal");
        if journal.exists() {
            return Err(Error(format!("Database has a rollback journal: {}", path.display())));
        }
        let before = [stamp(path), stamp(&wal), stamp(&journal)];
        inputs.stamps.entry(path.to_path_buf()).or_insert(before[0]);
        inputs.stamps.entry(wal.clone()).or_insert(before[1]);
        inputs.stamps.entry(journal.clone()).or_insert(before[2]);
        let fresh = inputs.databases.get(path).is_some_and(|(_, cached)| *cached == before);
        if !fresh {
            let dir = tempfile::Builder::new().prefix("actx-preview-").tempdir()?;
            std::fs::copy(path, dir.path().join("snapshot.db"))?;
            if wal.exists() {
                std::fs::copy(&wal, dir.path().join("snapshot.db-wal"))?;
            }
            if before != [stamp(path), stamp(&wal), stamp(&journal)] {
                return Err(Error(format!("Database changed while reading: {}", path.display())));
            }
            inputs.databases.insert(path.to_path_buf(), (dir, before));
        }
        let snapshot = inputs.databases[path].0.path().join("snapshot.db");
        Ok(Connection::open_with_flags(
            snapshot,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?)
    })();
    if let Err(error) = &result {
        // A missing optional AGY index is normal. Existing but unreadable data is not.
        if path.exists() {
            inputs().lock().unwrap().errors.push(error.to_string());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_changed_and_added_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input");
        std::fs::write(&path, "before").unwrap();
        tree(dir.path());
        assert!(!changed());
        std::fs::write(&path, "after, longer").unwrap();
        assert!(changed());
    }
    #[test]
    fn nested_guards_keep_snapshots_and_refresh_changed_databases() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        {
            let con = Connection::open(&path).unwrap();
            con.execute_batch("create table t (v); insert into t values (1);")
                .unwrap();
        }
        enable();
        enable();
        let first: i64 = database(&path)
            .unwrap()
            .query_row("select v from t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(first, 1);
        finish();
        assert!(enabled(), "the outer guard is still active");
        {
            let con = Connection::open(&path).unwrap();
            con.execute("update t set v = 2", []).unwrap();
        }
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::now() + std::time::Duration::from_secs(2))
            .unwrap();
        let second: i64 = database(&path)
            .unwrap()
            .query_row("select v from t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(second, 2, "a changed database is snapshotted again");
        finish();
        assert!(!enabled());
    }
}
