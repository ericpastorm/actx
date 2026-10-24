//! Where things live on this machine (read once from the environment) and the error type.

use std::fmt;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::Value;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Ctx {
    pub home: PathBuf,
    pub config_file: PathBuf,
    pub config: Value,
    pub state_dir: PathBuf,
    pub cache_dir: PathBuf,
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
        // Windows: USERPROFILE is the real home; HOME may be a Git Bash style /c/Users/me
        let home = PathBuf::from(
            (if cfg!(windows) {
                env("USERPROFILE").or_else(|| env("HOME"))
            } else {
                env("HOME")
            })
            .unwrap_or_else(|| if cfg!(windows) { "C:\\".into() } else { "/".into() }),
        );
        let xdg = |var: &str, default: &str| {
            env(var)
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(crate::paths::native(default)))
        };
        let config_file = xdg("XDG_CONFIG_HOME", ".config").join("actx").join("config.json");
        let config = std::fs::read_to_string(&config_file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| Value::Object(Default::default()));
        let self_path = std::env::current_exe()
            .and_then(|p| crate::paths::canonical(&p))
            .unwrap_or_default();
        Ctx {
            config_file,
            config,
            state_dir: xdg("XDG_STATE_HOME", ".local/state").join("actx"),
            cache_dir: xdg("XDG_CACHE_HOME", ".cache").join("actx"),
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
            no_color: std::env::var_os("NO_COLOR").is_some() || !std::io::stdout().is_terminal() || !ansi_terminal(),
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
        // Windows, ERROR_CLOUD_FILE_ACCESS_DENIED: Synology Drive, OneDrive… keep the file online only
        if cfg!(windows) && e.raw_os_error() == Some(395) {
            return Error(crate::tr!(
                "{err}. The shared folder keeps its files online only: set it to «Always keep on this device» (or run: attrib +P \"<folder>\" /S /D)",
                err = e
            ));
        }
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

/// Can the terminal show colors? Always, except on Windows consoles that don't say they can.
fn ansi_terminal() -> bool {
    !cfg!(windows)
        || ["WT_SESSION", "TERM", "ConEmuANSI", "ANSICON", "TERM_PROGRAM"]
            .iter()
            .any(|v| std::env::var_os(v).is_some())
}

/// Where `cmd` is on the PATH. On Windows `claude` finds claude.exe, claude.cmd (npm's shims)…
pub fn find_program(cmd: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) && Path::new(cmd).extension().is_none() {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(|e| e.to_lowercase())
            .collect()
    } else {
        Vec::new()
    };
    let find = |dir: PathBuf| {
        let plain = dir.join(cmd);
        if exts.is_empty() {
            return plain.is_file().then_some(plain);
        }
        exts.iter().map(|e| dir.join(format!("{cmd}{e}"))).find(|p| p.is_file())
    };
    std::env::split_paths(&paths).find_map(find).or_else(|| {
        // Hooks and timers may start with a bare PATH: look where per-user installers put agents.
        let home = PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?);
        [
            ".local/share/mise/shims",
            ".local/bin",
            ".cargo/bin",
            ".bun/bin",
            ".volta/bin",
            ".npm-global/bin",
            ".local/share/pnpm",
            ".asdf/shims",
            ".nix-profile/bin",
            ".opencode/bin",
            ".claude/local",
        ]
        .into_iter()
        .find_map(|dir| find(home.join(crate::paths::native(dir))))
    })
}

/// Is `cmd` on the PATH?
pub fn which(cmd: &str) -> bool {
    find_program(cmd).is_some()
}

/// Windows: run a console program without opening a window for it (a program started without a
/// console of its own, like a hook or a scheduled task, would otherwise flash one).
pub fn hide_window(c: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// A command for a system tool (`powershell.exe`, `tasklist`…), with no window on Windows.
pub fn quiet(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut c = std::process::Command::new(program.as_ref());
    #[cfg(windows)]
    {
        let name = Path::new(program.as_ref())
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        if name.eq_ignore_ascii_case("powershell") || name.eq_ignore_ascii_case("pwsh") {
            // Even -NoProfile can write ModuleAnalysisCache in the user's profile.
            // Keep process inspection read-only without changing the user's environment.
            c.env("PSModuleAnalysisCachePath", "NUL");
        }
    }
    hide_window(&mut c);
    c
}

/// Keep PowerShell's startup/JIT caches out of the profile being inspected, too.
/// Prefer PowerShell 7 when available; Windows PowerShell remains the fallback.
pub fn powershell(script: &str) -> std::io::Result<std::process::Output> {
    let scratch = tempfile::Builder::new().prefix("actx-powershell-").tempdir()?;
    let program = find_program("pwsh").unwrap_or_else(|| PathBuf::from("powershell.exe"));
    quiet(program)
        .env("HOME", scratch.path())
        .env("USERPROFILE", scratch.path())
        .env("LOCALAPPDATA", scratch.path().join("Local"))
        .env("APPDATA", scratch.path().join("Roaming"))
        .env("XDG_CACHE_HOME", scratch.path())
        .env("XDG_STATE_HOME", scratch.path())
        .env("PSModuleAnalysisCachePath", "NUL")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .stdin(std::process::Stdio::null())
        .output()
}

/// A command that runs `program` the way a shell would: on Windows that finds .cmd and .bat
/// shims too (std only looks for .exe), and opens no window.
pub fn command(program: &str) -> std::process::Command {
    quiet(find_program(program).unwrap_or_else(|| PathBuf::from(program)))
}

/// Like `command`, for a program that draws on the terminal (fzf): it keeps our console.
pub fn console_command(program: &str) -> std::process::Command {
    std::process::Command::new(find_program(program).unwrap_or_else(|| PathBuf::from(program)))
}
