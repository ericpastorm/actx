//! Taking turns. A lock file in the shared folder keeps machines (and several runs on the same
//! one) from writing to it at the same time. It says who holds it, and is refreshed while held,
//! so a lock left behind by a run that died can be told apart from a slow one.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::ctx::{ctx, jstr, now};

/// A lock that hasn't been refreshed for this long was left behind.
const STALE_SECS: f64 = 300.0;
const REFRESH: Duration = Duration::from_secs(30);

/// Who holds the lock.
pub struct Holder {
    pub host: String,
    pub since: f64,
}

pub enum LockError {
    Busy(Holder),
    Io(std::io::Error),
}

pub struct SharedLock {
    path: PathBuf,
    token: String,
    stop: Option<mpsc::Sender<()>>,
    beat: Option<JoinHandle<()>>,
}

/// Is process `pid` (on this machine) still running?
pub fn alive(pid: u32) -> bool {
    if cfg!(windows) {
        // tasklist prints the process as a CSV row when it exists, and an INFO line when not
        return crate::ctx::quiet("tasklist")
            .args(["/NH", "/FO", "CSV", "/FI", &format!("PID eq {pid}")])
            .stderr(Stdio::null())
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")));
    }
    if Path::new("/proc/self").exists() {
        return Path::new(&format!("/proc/{pid}")).exists();
    }
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn read(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

fn holder(v: Option<&Value>) -> Holder {
    Holder {
        host: v.map(|v| jstr(v, "host").to_string()).unwrap_or_else(|| "?".into()),
        since: v.and_then(|v| v.get("since")).and_then(Value::as_f64).unwrap_or(0.0),
    }
}

fn stale(path: &Path, v: Option<&Value>) -> bool {
    match v {
        Some(v) => {
            let pid = v.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
            let at = v.get("at").and_then(Value::as_f64).unwrap_or(0.0);
            (jstr(v, "host") == ctx().host && !alive(pid)) || now() - at > STALE_SECS
        }
        // being rewritten right now, or garbage: only if it's been there for long
        None => {
            let modified = crate::store::modified(path);
            modified > 0.0 && now() - modified > 2.0 * STALE_SECS
        }
    }
}

impl SharedLock {
    /// Take the shared folder's lock, waiting up to `wait` for whoever has it. `waiting` is called
    /// once if we have to wait.
    pub fn acquire(root: &Path, wait: Duration, mut waiting: impl FnMut(&Holder)) -> Result<SharedLock, LockError> {
        let path = root.join(".lock");
        let token = uuid::Uuid::new_v4().to_string();
        let since = now();
        let deadline = Instant::now() + wait;
        let mut told = false;
        loop {
            match std::fs::File::options().write(true).create_new(true).open(&path) {
                Ok(_) => {
                    std::fs::write(&path, content(&token, since)).map_err(LockError::Io)?;
                    let (stop, beat) = heartbeat(path.clone(), token.clone(), since);
                    return Ok(SharedLock {
                        path,
                        token,
                        stop: Some(stop),
                        beat: Some(beat),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let v = read(&path);
                    if stale(&path, v.as_ref()) {
                        // look again right before removing it: someone may have just taken it
                        if read(&path) == v {
                            let _ = std::fs::remove_file(&path);
                        }
                        continue;
                    }
                    if Instant::now() >= deadline {
                        return Err(LockError::Busy(holder(v.as_ref())));
                    }
                    if !told {
                        waiting(&holder(v.as_ref()));
                        told = true;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
                Err(e) => return Err(LockError::Io(e)),
            }
        }
    }
}

fn content(token: &str, since: f64) -> String {
    json!({"host": ctx().host, "pid": std::process::id(), "since": since, "at": now(), "token": token}).to_string()
}

/// Refresh the lock every so often while it's held, so others know we're still alive.
fn heartbeat(path: PathBuf, token: String, since: f64) -> (mpsc::Sender<()>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<()>();
    let beat = std::thread::spawn(move || {
        while let Err(mpsc::RecvTimeoutError::Timeout) = rx.recv_timeout(REFRESH) {
            if read(&path).is_some_and(|v| jstr(&v, "token") == token) {
                let _ = std::fs::write(&path, content(&token, since));
            }
        }
    });
    (tx, beat)
}

impl Drop for SharedLock {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(beat) = self.beat.take() {
            let _ = beat.join();
        }
        if read(&self.path).is_some_and(|v| jstr(&v, "token") == self.token) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_at_a_time_and_left_behind_locks_expire() {
        let dir = tempfile::tempdir().unwrap();
        let first = SharedLock::acquire(dir.path(), Duration::ZERO, |_| {}).ok().unwrap();
        let mut waited = false;
        match SharedLock::acquire(dir.path(), Duration::from_millis(600), |_| waited = true) {
            Err(LockError::Busy(h)) => assert_eq!(h.host, ctx().host),
            _ => panic!("the lock is held"),
        }
        assert!(waited);
        drop(first);
        assert!(!dir.path().join(".lock").exists());
        let second = SharedLock::acquire(dir.path(), Duration::ZERO, |_| {}).ok().unwrap();
        drop(second);

        // a run on this machine that died, and one on another machine that stopped refreshing it
        let dead = json!({"host": ctx().host, "pid": u32::MAX - 1, "since": now(), "at": now(), "token": "x"});
        std::fs::write(dir.path().join(".lock"), dead.to_string()).unwrap();
        assert!(SharedLock::acquire(dir.path(), Duration::ZERO, |_| {}).is_ok());
        let old = json!({"host": "elsewhere", "pid": 1, "since": 0, "at": now() - 1000.0, "token": "y"});
        std::fs::write(dir.path().join(".lock"), old.to_string()).unwrap();
        assert!(SharedLock::acquire(dir.path(), Duration::ZERO, |_| {}).is_ok());
        let fresh = json!({"host": "elsewhere", "pid": 1, "since": now(), "at": now(), "token": "z"});
        std::fs::write(dir.path().join(".lock"), fresh.to_string()).unwrap();
        assert!(SharedLock::acquire(dir.path(), Duration::ZERO, |_| {}).is_err());
        // not ours: left alone
        assert!(dir.path().join(".lock").exists());
    }
}
