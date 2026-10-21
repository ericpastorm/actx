//! Read-only inspection. SQLite works on private temporary snapshots, never on agent files.
use crate::ctx::{Error, Result};
use rusqlite::{Connection, OpenFlags};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

static ENABLED: AtomicBool = AtomicBool::new(false);
type Stamp = Option<(u64, SystemTime)>;
#[derive(Default)]
struct Inputs {
    stamps: BTreeMap<PathBuf, Stamp>,
    databases: BTreeMap<PathBuf, tempfile::TempDir>,
    errors: Vec<String>,
}
fn inputs() -> &'static Mutex<Inputs> {
    static INPUTS: OnceLock<Mutex<Inputs>> = OnceLock::new();
    INPUTS.get_or_init(Default::default)
}
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}
pub fn finish() {
    *inputs().lock().unwrap() = Inputs::default();
    ENABLED.store(false, Ordering::Relaxed);
}
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
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
        if !inputs.databases.contains_key(path) {
            let wal = sidecar(path, "-wal");
            let journal = sidecar(path, "-journal");
            if journal.exists() {
                return Err(Error(format!("Database has a rollback journal: {}", path.display())));
            }
            let before = [stamp(path), stamp(&wal), stamp(&journal)];
            inputs.stamps.entry(path.to_path_buf()).or_insert(before[0]);
            inputs.stamps.entry(wal.clone()).or_insert(before[1]);
            inputs.stamps.entry(journal.clone()).or_insert(before[2]);
            let dir = tempfile::Builder::new().prefix("actx-preview-").tempdir()?;
            std::fs::copy(path, dir.path().join("snapshot.db"))?;
            if wal.exists() {
                std::fs::copy(&wal, dir.path().join("snapshot.db-wal"))?;
            }
            if before != [stamp(path), stamp(&wal), stamp(&journal)] {
                return Err(Error(format!("Database changed while reading: {}", path.display())));
            }
            inputs.databases.insert(path.to_path_buf(), dir);
        }
        let snapshot = inputs.databases[path].path().join("snapshot.db");
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
}
