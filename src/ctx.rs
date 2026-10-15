//! Where things live on this machine (read once from the environment) and the error type.

use std::fmt;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::Value;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Ctx {
    pub home: PathBuf,
    pub config_file: PathBuf,
    pub config: Value,
    pub state_dir: PathBuf,
    pub claude_projects: PathBuf,
    pub opencode_db: PathBuf,
    pub codex_home: PathBuf,
    /// where to look for repos when placing a conversation (ACTX_SCAN: paths separated by ':')
    pub scan_roots: Vec<PathBuf>,
    /// extra flags for opencode's export/import/delete, e.g. "--standalone" for opencode 2.x
    pub opencode_args: Vec<String>,
    pub host: String,
    pub self_path: PathBuf,
    pub no_color: bool,
    /// tests: what pickers choose (key substrings, comma-separated in ACTX_PICK)
    pub pick: Option<Vec<String>>,
    /// tests: the answer to every question (ACTX_ANSWER)
    pub answer: Option<String>,
    /// tests: how long to wait for the shared folder's lock, in seconds (ACTX_LOCK_WAIT)
    pub lock_wait: Option<u64>,
    /// tests: hooks wait for the sync they start instead of leaving it in the background (ACTX_HOOK_WAIT)
    pub hook_wait: bool,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

impl Ctx {
    fn load() -> Ctx {
        let home = PathBuf::from(env("HOME").unwrap_or_else(|| "/".into()));
        let xdg = |var: &str, default: &str| env(var).map(PathBuf::from).unwrap_or_else(|| home.join(default));
        let config_file = xdg("XDG_CONFIG_HOME", ".config").join("actx").join("config.json");
        let config = std::fs::read_to_string(&config_file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| Value::Object(Default::default()));
        let self_path = std::env::current_exe()
            .and_then(|p| p.canonicalize())
            .unwrap_or_default();
        Ctx {
            config_file,
            config,
            state_dir: xdg("XDG_STATE_HOME", ".local/state").join("actx"),
            claude_projects: env("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".claude"))
                .join("projects"),
            opencode_db: xdg("XDG_DATA_HOME", ".local/share")
                .join("opencode")
                .join("opencode.db"),
            codex_home: env("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex")),
            scan_roots: env("ACTX_SCAN")
                .map(|v| std::env::split_paths(&v).collect())
                .unwrap_or_else(|| vec![home.clone()]),
            opencode_args: env("ACTX_OPENCODE_ARGS")
                .map(|v| v.split_whitespace().map(String::from).collect())
                .unwrap_or_default(),
            host: env("ACTX_HOST").unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into_owned()),
            self_path,
            no_color: std::env::var_os("NO_COLOR").is_some() || !std::io::stdout().is_terminal(),
            pick: std::env::var("ACTX_PICK")
                .ok()
                .map(|v| v.split(',').filter(|p| !p.is_empty()).map(String::from).collect()),
            answer: std::env::var("ACTX_ANSWER").ok(),
            lock_wait: env("ACTX_LOCK_WAIT").and_then(|v| v.parse().ok()),
            hook_wait: env("ACTX_HOOK_WAIT").is_some(),
            home,
        }
    }

    pub fn codex_sessions(&self) -> PathBuf {
        self.codex_home.join("sessions")
    }

    pub fn codex_index(&self) -> PathBuf {
        self.codex_home.join("session_index.jsonl")
    }

    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }

    pub fn backups(&self) -> PathBuf {
        self.state_dir.join("backups")
    }

    pub fn home_str(&self) -> String {
        self.home.to_string_lossy().into_owned()
    }
}

static CTX: OnceLock<Ctx> = OnceLock::new();

pub fn ctx() -> &'static Ctx {
    CTX.get_or_init(Ctx::load)
}

/// Something that went wrong, already phrased for the user.
#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error(e.to_string())
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error(e.to_string())
    }
}

impl From<String> for Error {
    fn from(e: String) -> Self {
        Error(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A JSON field as text ("" when missing).
pub fn jstr<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn now_ms() -> i64 {
    (now() * 1000.0) as i64
}

/// Is `cmd` on the PATH?
pub fn which(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(cmd).is_file()))
        .unwrap_or(false)
}
