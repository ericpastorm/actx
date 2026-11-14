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
        // The snapshots stay: an agent database can be gigabytes, and the next operation of this
        // process reuses its copy for as long as the database is exactly as it was. One whose
        // database moved on would be copied again anyway: a long-lived process doesn't hold it.
        let mut inputs = inputs().lock().unwrap();
        inputs.stamps.clear();
        inputs.errors.clear();
        inputs.databases.retain(|path, (_, taken)| *taken == stamps(path));
    }
}
/// The size and time of a database and of the files SQLite keeps next to it.
fn stamps(path: &Path) -> [Stamp; 3] {
    [
        stamp(path),
        stamp(&sidecar(path, "-wal")),
        stamp(&sidecar(path, "-journal")),
    ]
}
/// Remove this process's snapshots. They are private to it, so nothing else will.
pub fn release() {
    inputs().lock().unwrap().databases.clear();
}

const SNAPSHOT: &str = "actx-preview-";
/// A folder for one snapshot, named after this process so a later run can tell it is orphaned.
/// Never in the user's folders (inspection writes nothing there), and on disk where there is a
/// choice: the default temporary folder is memory on most Linux systems (tmpfs), where copying a
/// gigabyte database takes a gigabyte of RAM. /var/tmp is on disk, and when it shares a filesystem
/// with the home (Btrfs subvolumes) the copy shares its blocks with the original. A TMPDIR someone
/// set is their choice; macOS sets one, on disk.
fn snapshot_dir() -> std::io::Result<tempfile::TempDir> {
    let name = format!("{SNAPSHOT}{}-", std::process::id());
    let on_disk = Path::new("/var/tmp");
    if cfg!(unix) && std::env::var_os("TMPDIR").is_none() && on_disk.is_dir() {
        sweep(on_disk);
        if let Ok(dir) = tempfile::Builder::new().prefix(&name).tempdir_in(on_disk) {
            return Ok(dir);
        }
    }
    sweep(&std::env::temp_dir());
    tempfile::Builder::new().prefix(&name).tempdir()
}
/// Snapshots left by processes that ended without removing theirs (killed, or exited early).
fn sweep(parent: &Path) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let owner = name
            .to_str()
            .and_then(|n| n.strip_prefix(SNAPSHOT))
            .and_then(|n| n.split_once('-'))
            .and_then(|(pid, _)| pid.parse::<u32>().ok());
        if let Some(pid) = owner
            && pid != std::process::id()
            && !crate::lock::alive(pid)
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
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
        let before = stamps(path);
        inputs.stamps.entry(path.to_path_buf()).or_insert(before[0]);
        inputs.stamps.entry(wal.clone()).or_insert(before[1]);
        inputs.stamps.entry(journal.clone()).or_insert(before[2]);
        // (a snapshot someone removed from under us is taken again rather than failing to open)
        let fresh = inputs
            .databases
            .get(path)
            .is_some_and(|(dir, cached)| *cached == before && dir.path().join("snapshot.db").is_file());
        if !fresh {
            // release the previous copy first: both are never needed at once
            inputs.databases.remove(path);
            let dir = snapshot_dir()?;
            std::fs::copy(path, dir.path().join("snapshot.db"))?;
            if wal.exists() {
                std::fs::copy(&wal, dir.path().join("snapshot.db-wal"))?;
            }
            if before != stamps(path) {
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
    /// The tests below count guards, which are one per process: one at a time.
    static GUARDS: Mutex<()> = Mutex::new(());
    #[test]
    fn snapshots_outlive_the_operation_until_the_database_changes() {
        let _alone = GUARDS.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reused.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch("create table t (v); insert into t values (1);")
            .unwrap();
        // one whole operation each time: the guard ends before the next one starts
        let operation = || {
            let _guard = Guard::new();
            let con = database(&path).unwrap();
            let value: i64 = con.query_row("select v from t", [], |r| r.get(0)).unwrap();
            (PathBuf::from(con.path().unwrap()), value)
        };
        let (first, _) = operation();
        assert!(first.is_file());
        assert_eq!(operation().0, first, "an unchanged database is not copied again");
        std::fs::remove_dir_all(first.parent().unwrap()).unwrap();
        let (second, value) = operation();
        assert!(
            second != first && value == 1,
            "a snapshot removed from under us is taken again"
        );
        Connection::open(&path)
            .unwrap()
            .execute("update t set v = 2", [])
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::now() + std::time::Duration::from_secs(2))
            .unwrap();
        let (third, value) = operation();
        assert!(third != second && value == 2, "a changed database is snapshotted again");
        assert!(!second.exists(), "and its previous copy is gone");
        release();
        assert!(!third.exists(), "releasing removes what this process copied");
    }
    #[test]
    fn orphaned_snapshots_are_swept_but_not_those_of_live_processes() {
        let dir = tempfile::tempdir().unwrap();
        let mine = dir.path().join(format!("{SNAPSHOT}{}-live", std::process::id()));
        // no process has this id: Linux caps them at 2^22, and macOS and Windows far below
        let orphan = dir.path().join(format!("{SNAPSHOT}4294967294-dead"));
        let foreign = dir.path().join(format!("{SNAPSHOT}aB3dE9"));
        for folder in [&mine, &orphan, &foreign] {
            std::fs::create_dir(folder).unwrap();
            std::fs::write(folder.join("snapshot.db"), "x").unwrap();
        }
        sweep(dir.path());
        assert!(mine.exists() && foreign.exists() && !orphan.exists());
    }
    #[test]
    fn nested_guards_keep_snapshots_and_refresh_changed_databases() {
        let _alone = GUARDS.lock().unwrap_or_else(|e| e.into_inner());
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
