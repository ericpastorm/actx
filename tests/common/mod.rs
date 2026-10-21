//! Test helpers: fake machines that share one folder, each with its own home, Claude Code,
//! opencode and Codex data, running the real actx binary. Plus git repos and conversations.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Windows: no console window for programs the tests start.
pub fn hidden(c: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

pub const ACTX: &str = env!("CARGO_BIN_EXE_actx");

/// `program` as a path where it's on the PATH (Windows: finding opencode.cmd and the like).
pub fn resolve(program: &str) -> PathBuf {
    let names: Vec<String> = if cfg!(windows) {
        vec![
            format!("{program}.exe"),
            format!("{program}.cmd"),
            format!("{program}.bat"),
        ]
    } else {
        vec![program.to_string()]
    };
    std::env::var_os("PATH")
        .and_then(|p| std::env::split_paths(&p).find_map(|d| names.iter().map(|n| d.join(n)).find(|f| f.is_file())))
        .unwrap_or_else(|| PathBuf::from(program))
}

/// A path the way this machine writes it (Windows: no `/` left over from `join("a/b")`).
pub fn native(path: &Path) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(path.to_string_lossy().replace('/', "\\"))
    } else {
        path.to_path_buf()
    }
}

pub fn which(cmd: &str) -> bool {
    let names: Vec<String> = if cfg!(windows) {
        vec![
            cmd.to_string(),
            format!("{cmd}.exe"),
            format!("{cmd}.cmd"),
            format!("{cmd}.bat"),
        ]
    } else {
        vec![cmd.to_string()]
    };
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| names.iter().any(|n| d.join(n).is_file())))
}

/// `canonicalize` without the `\\?\` prefix Windows adds.
pub fn canon(path: &Path) -> PathBuf {
    let p = path.canonicalize().unwrap();
    match p.to_string_lossy().strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p,
    }
}

/// The installed opencode's major version (1 or 2), or None.
pub fn opencode_major() -> Option<u8> {
    static MAJOR: OnceLock<Option<u8>> = OnceLock::new();
    *MAJOR.get_or_init(|| {
        if !which("opencode") {
            return None;
        }
        let out = hidden(&mut Command::new(resolve("opencode")))
            .args(["session", "--help"])
            .output()
            .ok()?;
        let help = String::from_utf8_lossy(&[out.stdout, out.stderr].concat()).into_owned();
        Some(if help.contains("import") { 2 } else { 1 })
    })
}

/// Skip a test (returning early) when a tool it needs isn't installed.
#[macro_export]
macro_rules! needs {
    ($tool:expr) => {
        if !common::which($tool) {
            eprintln!("skipped: {} is not installed", $tool);
            return;
        }
    };
}

pub struct World {
    pub dir: tempfile::TempDir,
}

impl World {
    pub fn new() -> World {
        // canonical (on macOS /var is a link to /private/var): actx compares real paths
        let dir = tempfile::tempdir_in(canon(&std::env::temp_dir())).unwrap();
        std::fs::create_dir(dir.path().join("store")).unwrap();
        World { dir }
    }

    pub fn store(&self) -> PathBuf {
        self.dir.path().join("store")
    }

    pub fn machine(&self, name: &str) -> Machine {
        let home = self.dir.path().join(name);
        std::fs::create_dir_all(&home).unwrap();
        Machine {
            name: name.into(),
            home,
            store: self.store(),
            lang: "en".into(),
        }
    }
}

pub struct Machine {
    pub name: String,
    pub home: PathBuf,
    pub store: PathBuf,
    pub lang: String,
}

#[derive(Default)]
pub struct Opts<'a> {
    pub cwd: Option<&'a Path>,
    pub pick: Option<&'a [&'a str]>,
    pub answer: Option<&'a str>,
}

impl Machine {
    pub fn env(&self) -> Vec<(String, String)> {
        let h = |p: &str| self.home.join(p).to_string_lossy().into_owned();
        let mut env = vec![
            ("HOME".into(), self.home.to_string_lossy().into_owned()),
            ("PATH".into(), std::env::var("PATH").unwrap_or_default()),
            ("XDG_CONFIG_HOME".into(), h(".config")),
            ("XDG_STATE_HOME".into(), h(".local/state")),
            ("XDG_DATA_HOME".into(), h(".local/share")),
            ("XDG_CACHE_HOME".into(), h(".cache")),
            ("CLAUDE_CONFIG_DIR".into(), h(".claude")),
            ("CODEX_HOME".into(), h(".codex")),
            ("ACTX_ROOT".into(), self.store.to_string_lossy().into_owned()),
            ("ACTX_SCAN".into(), self.home.to_string_lossy().into_owned()),
            ("ACTX_LANG".into(), self.lang.clone()),
            ("ACTX_HOST".into(), self.name.clone()),
            ("GIT_AUTHOR_NAME".into(), "t".into()),
            ("GIT_AUTHOR_EMAIL".into(), "t@t".into()),
            ("GIT_COMMITTER_NAME".into(), "t".into()),
            ("GIT_COMMITTER_EMAIL".into(), "t@t".into()),
            ("NO_COLOR".into(), "1".into()),
        ];
        if cfg!(windows) {
            // the home on Windows, and what programs need to start there at all
            env.push(("USERPROFILE".into(), self.home.to_string_lossy().into_owned()));
            for v in [
                "SystemRoot",
                "SYSTEMDRIVE",
                "TEMP",
                "TMP",
                "PATHEXT",
                "COMSPEC",
                "windir",
            ] {
                if let Ok(value) = std::env::var(v) {
                    env.push((v.into(), value));
                }
            }
        }
        if opencode_major() == Some(2) {
            // never talk to a real opencode service (1.x has none)
            env.push(("ACTX_OPENCODE_ARGS".into(), "--standalone".into()));
        }
        env
    }

    pub fn command(&self, program: &str) -> Command {
        let mut c = Command::new(if ["opencode", "codex"].contains(&program) {
            resolve(program)
        } else {
            PathBuf::from(program)
        });
        hidden(&mut c)
            .env_clear()
            .envs(self.env())
            .current_dir(&self.home)
            .stdin(Stdio::null());
        c
    }

    /// Run actx on this machine and return what it printed.
    pub fn actx_with(&self, args: &[&str], o: Opts) -> String {
        let mut c = self.command(ACTX);
        c.args(args);
        if let Some(cwd) = o.cwd {
            c.current_dir(cwd);
        }
        if let Some(pick) = o.pick {
            c.env("ACTX_PICK", pick.join(","));
        }
        if let Some(answer) = o.answer {
            c.env("ACTX_ANSWER", answer);
        }
        let out = c.output().unwrap();
        String::from_utf8_lossy(&[out.stdout, out.stderr].concat()).into_owned()
    }

    pub fn actx(&self, args: &[&str]) -> String {
        self.actx_with(args, Opts::default())
    }

    pub fn pick(&self, args: &[&str], pick: &[&str]) -> String {
        self.actx_with(
            args,
            Opts {
                pick: Some(pick),
                ..Default::default()
            },
        )
    }

    pub fn claude_dir(&self) -> PathBuf {
        self.home.join(".claude/projects")
    }

    pub fn opencode_db(&self) -> PathBuf {
        self.home.join(".local/share/opencode/opencode.db")
    }

    pub fn codex_sessions(&self) -> PathBuf {
        self.home.join(".codex/sessions")
    }
}

pub fn git_repo(path: &Path) -> PathBuf {
    std::fs::create_dir_all(path).unwrap();
    let run = |args: &[&str]| {
        let ok = hidden(&mut Command::new("git"))
            .args(args)
            .current_dir(path)
            .envs([
                ("GIT_AUTHOR_NAME", "t"),
                ("GIT_AUTHOR_EMAIL", "t@t"),
                ("GIT_COMMITTER_NAME", "t"),
                ("GIT_COMMITTER_EMAIL", "t@t"),
            ])
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    };
    run(&["init", "-q", "-b", "main"]);
    // each its own first commit (empty ones made in the same second would be the same)
    let name = path.file_name().unwrap().to_string_lossy();
    run(&["commit", "-q", "--allow-empty", "-m", &format!("init {name}")]);
    native(path)
}

pub fn git_clone(src: &Path, dest: &Path) -> PathBuf {
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    assert!(
        hidden(&mut Command::new("git"))
            .arg("clone")
            .arg("-q")
            .arg(src)
            .arg(dest)
            .output()
            .unwrap()
            .status
            .success()
    );
    native(dest)
}

fn line(v: Value) -> String {
    format!("{v}\n")
}

pub fn encode_claude(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Write a Claude Code transcript the way Claude Code does (compact JSON, a uuid chain).
pub fn claude_session(m: &Machine, folder: &Path, turns: &[(&str, &str)], title: Option<&str>) -> PathBuf {
    let sid = uuid::Uuid::new_v4().to_string();
    let dir = m.claude_dir().join(encode_claude(folder));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{sid}.jsonl"));
    std::fs::write(&path, "").unwrap();
    claude_continue(&path, turns, Some(folder));
    if let Some(title) = title {
        append(
            &path,
            &line(json!({"type": "ai-title", "aiTitle": title, "sessionId": sid})),
        );
    }
    path
}

pub fn append(path: &Path, text: &str) {
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

/// Append turns to a transcript, chained to its last message (as if you kept talking).
pub fn claude_continue(path: &Path, turns: &[(&str, &str)], folder: Option<&Path>) {
    let entries: Vec<Value> = std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let chained: Vec<&Value> = entries.iter().filter(|e| e.get("uuid").is_some()).collect();
    let mut parent = chained.last().map(|e| e["uuid"].as_str().unwrap().to_string());
    let cwd = folder
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| chained.last().unwrap()["cwd"].as_str().unwrap().to_string());
    let sid = path.file_stem().unwrap().to_string_lossy().into_owned();
    let mut out = String::new();
    for (n, (who, text)) in turns.iter().enumerate() {
        let u = uuid::Uuid::new_v4().to_string();
        let message = if *who == "user" {
            json!({"role": "user", "content": text})
        } else {
            json!({"role": "assistant", "type": "message", "id": format!("msg_{}", &u[..8]), "model": "test", "content": [{"type": "text", "text": text}]})
        };
        out += &line(json!({
            "parentUuid": parent, "isSidechain": false, "type": who, "message": message, "uuid": u,
            "timestamp": format!("2026-01-01T10:{:02}:00.000Z", chained.len() + n), "cwd": cwd, "sessionId": sid,
        }));
        parent = Some(u);
    }
    append(path, &out);
}

/// The texts of a machine's copy of a Claude conversation, and the cwds it mentions.
pub fn claude_transcript(m: &Machine, sid: &str) -> (PathBuf, Vec<String>, Vec<String>) {
    let path = find(&m.claude_dir(), &format!("{sid}.jsonl")).expect("the conversation is there");
    let mut texts = Vec::new();
    let mut cwds = Vec::new();
    for l in std::fs::read_to_string(&path).unwrap().lines() {
        let e: Value = serde_json::from_str(l).unwrap();
        if let Some(c) = e.get("cwd").and_then(Value::as_str)
            && !cwds.contains(&c.to_string())
        {
            cwds.push(c.to_string());
        }
        match e.get("message").and_then(|m| m.get("content")) {
            Some(Value::String(s)) => texts.push(s.clone()),
            Some(Value::Array(items)) => texts.extend(
                items
                    .iter()
                    .filter(|b| b["type"] == "text")
                    .map(|b| b["text"].as_str().unwrap().to_string()),
            ),
            _ => {}
        }
    }
    (path, texts, cwds)
}

/// The first file named `name` under `dir`.
pub fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() {
            if let Some(found) = find(&p, name) {
                return Some(found);
            }
        } else if p.file_name().is_some_and(|n| n.to_string_lossy().ends_with(name)) {
            return Some(p);
        }
    }
    None
}

pub fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() {
            out.extend(files_under(&p))
        } else {
            out.push(p)
        }
    }
    out
}

// ── opencode ────────────────────────────────────────────────────────────────

pub const OC_SID: &str = "ses_e5a1b2c3d4e5Fx7Qk2LmNp9RsT";

/// The test opencode conversation, in the format of opencode `major`.
pub fn opencode_fixture(folder: &Path, major: u8) -> Value {
    let name = if major == 1 {
        "opencode1_export.json"
    } else {
        "opencode_export.json"
    };
    let mut exp: Value = serde_json::from_str(
        &std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap(),
    )
    .unwrap();
    let dir = json!(folder.to_string_lossy());
    if major == 1 {
        exp["info"]["directory"] = dir;
    } else {
        exp["info"]["location"]["directory"] = dir;
    }
    exp
}

fn opencode(m: &Machine, sub: &[&str], rest: &[&str], cwd: &Path) -> String {
    let mut c = m.command("opencode");
    c.args(sub);
    if opencode_major() == Some(2) {
        c.arg("--standalone");
    }
    c.args(rest).current_dir(cwd);
    let out = c.output().unwrap();
    String::from_utf8_lossy(&[out.stdout, out.stderr].concat()).into_owned()
}

pub fn opencode_import(m: &Machine, exp: &Value, folder: &Path) {
    let file = m.home.join("import.json");
    std::fs::write(&file, exp.to_string()).unwrap();
    let file = file.to_string_lossy().into_owned();
    let folder_s = folder.to_string_lossy().into_owned();
    let out = if opencode_major() == Some(2) {
        opencode(m, &["session", "import"], &["--directory", &folder_s, &file], folder)
    } else {
        opencode(m, &["import"], &[&file], folder)
    };
    assert!(out.contains("Imported session"), "{out}");
}

pub fn opencode_export(m: &Machine, sid: &str) -> Value {
    let file = m.home.join("export.json");
    let mut c = m.command("opencode");
    if opencode_major() == Some(2) {
        c.args(["session", "export", "--standalone", sid]);
    } else {
        c.args(["export", sid]);
    }
    c.stdout(std::fs::File::create(&file).unwrap()).status().unwrap();
    serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap()
}

/// {id: (title, directory, number of messages)} straight from opencode's database.
pub fn opencode_sessions(m: &Machine) -> std::collections::HashMap<String, (String, String, i64)> {
    let con =
        rusqlite::Connection::open_with_flags(m.opencode_db(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let sql = if opencode_major() == Some(2) {
        "select s.id, s.title, s.directory, (select count(*) from session_message m where m.session_id = s.id) from session_v2 s"
    } else {
        "select s.id, s.title, s.directory, (select count(*) from message m where m.session_id = s.id) from session s"
    };
    let mut st = con.prepare(sql).unwrap();
    st.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Add a question and an answer, as if you kept talking in opencode.
pub fn opencode_continue(m: &Machine, folder: &Path, text: &str, reply: &str) {
    let mut exp = opencode_export(m, OC_SID);
    let now = 1_795_000_000_000i64 + (exp["messages"].as_array().unwrap().len() as i64) * 1000;
    let id = |n: u32| {
        format!(
            "msg_{:012x}{}Ab3dE5fG7hJ9{n}",
            now as u64 * 0x1000 % 0xFFFF_FFFF_FFFF,
            &m.name[..1]
        )
    };
    let new = if opencode_major() == Some(2) {
        let model = exp["info"]["model"].clone();
        vec![
            json!({"id": id(1), "time": {"created": now}, "text": text, "files": [], "agents": [], "type": "user"}),
            json!({"id": id(2), "time": {"created": now, "streamed": now, "completed": now}, "type": "assistant", "agent": "build",
                   "model": model, "content": [{"type": "text", "text": reply}], "finish": "stop", "rawFinish": "stop", "cost": 0,
                   "tokens": {"input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}}}),
        ]
    } else {
        let f = folder.to_string_lossy();
        vec![
            json!({"info": {"id": id(1), "sessionID": OC_SID, "role": "user", "time": {"created": now}, "agent": "build",
                            "model": {"providerID": "test", "modelID": "test-model"}},
                   "parts": [{"id": format!("prt_{}1", &id(1)[4..]), "sessionID": OC_SID, "messageID": id(1), "type": "text", "text": text}]}),
            json!({"info": {"id": id(2), "sessionID": OC_SID, "role": "assistant", "parentID": id(1), "modelID": "test-model",
                            "providerID": "test", "mode": "build", "agent": "build", "path": {"cwd": f, "root": f}, "cost": 0,
                            "tokens": {"input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}},
                            "time": {"created": now, "completed": now}, "finish": "stop"},
                   "parts": [{"id": format!("prt_{}2", &id(2)[4..]), "sessionID": OC_SID, "messageID": id(2), "type": "text", "text": reply}]}),
        ]
    };
    exp["messages"].as_array_mut().unwrap().extend(new);
    opencode(m, &["session", "delete"], &[OC_SID], folder);
    opencode_import(m, &exp, folder);
}

// ── Codex ───────────────────────────────────────────────────────────────────

/// Write a Codex rollout the way Codex does: session_meta, then per turn what the model saw
/// (response_item) and what Codex shows (event_msg).
pub fn codex_rollout(m: &Machine, folder: &Path, turns: &[(&str, &str)], title: Option<&str>) -> PathBuf {
    let sid = uuid::Uuid::now_v7().to_string();
    let dir = m.codex_sessions().join("2026/01/01");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("rollout-2026-01-01T10-00-00-{sid}.jsonl"));
    let l = |kind: &str, payload: Value| {
        line(json!({"timestamp": "2026-01-01T10:00:00.000Z", "type": kind, "payload": payload}))
    };
    let mut out = l(
        "session_meta",
        json!({"id": sid, "session_id": sid, "timestamp": "2026-01-01T10:00:00.000Z",
        "cwd": folder.to_string_lossy(), "originator": "codex-tui", "cli_version": "0.160.0", "source": "cli", "model_provider": "openai"}),
    );
    out += &l(
        "response_item",
        json!({"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "<permissions>...</permissions>"}]}),
    );
    std::fs::write(&path, out).unwrap();
    codex_continue(&path, turns, None);
    if let Some(title) = title {
        let index = m.home.join(".codex/session_index.jsonl");
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(index)
            .unwrap()
            .write_all(line(json!({"id": sid, "thread_name": title, "updated_at": "2026-01-01T10:00:00Z"})).as_bytes())
            .unwrap();
    }
    path
}

pub fn rollout_id(path: &Path) -> String {
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    name[name.len() - 42..name.len() - 6].to_string()
}

/// Append turns to a rollout (as if you kept talking in Codex). `tool`: a command and its output.
pub fn codex_continue(path: &Path, turns: &[(&str, &str)], tool: Option<(&str, &str)>) {
    let sid = rollout_id(path);
    let l = |kind: &str, payload: Value| {
        line(json!({"timestamp": "2026-01-01T10:05:00.000Z", "type": kind, "payload": payload}))
    };
    let mut out = String::new();
    for (who, text) in turns {
        let turn = uuid::Uuid::new_v4().to_string();
        if *who == "user" {
            out += &l(
                "event_msg",
                json!({"type": "task_started", "turn_id": turn, "started_at": 1790000000}),
            );
            out += &l(
                "response_item",
                json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]}),
            );
            out += &l(
                "event_msg",
                json!({"type": "item_completed", "thread_id": sid, "turn_id": turn,
                "item": {"type": "UserMessage", "id": turn, "content": [{"type": "text", "text": text, "text_elements": []}]},
                "started_at_ms": 1790000000000i64, "completed_at_ms": 1790000000000i64}),
            );
        } else {
            if let Some((cmd, output)) = tool {
                out += &l(
                    "response_item",
                    json!({"type": "function_call", "name": "shell", "call_id": "c1",
                    "arguments": json!({"command": ["bash", "-lc", cmd]}).to_string()}),
                );
                out += &l(
                    "response_item",
                    json!({"type": "function_call_output", "call_id": "c1", "output": output}),
                );
            }
            out += &l(
                "response_item",
                json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]}),
            );
            out += &l(
                "event_msg",
                json!({"type": "item_completed", "thread_id": sid, "turn_id": turn,
                "item": {"type": "AgentMessage", "id": turn, "content": [{"type": "Text", "text": text}], "phase": "final_answer"},
                "started_at_ms": 1790000001000i64, "completed_at_ms": 1790000001000i64}),
            );
            out += &l(
                "event_msg",
                json!({"type": "task_complete", "turn_id": turn, "last_agent_message": text,
                "started_at": 1790000000, "completed_at": 1790000001, "duration_ms": 1000}),
            );
        }
    }
    append(path, &out);
}

/// What Codex itself sees on a machine, through its app-server: {id: (preview, turns, cwd)}.
pub fn codex_threads(m: &Machine) -> std::collections::HashMap<String, (String, usize, String)> {
    let mut child = m
        .command("codex")
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<Value>();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str(&l) {
                let _ = tx.send(v);
            }
        }
    });
    let mut call = |n: Option<i64>, method: &str, params: Value| -> Value {
        let Some(n) = n else {
            writeln!(stdin, "{}", json!({"method": method})).unwrap(); // a notification: no reply
            return Value::Null;
        };
        writeln!(stdin, "{}", json!({"id": n, "method": method, "params": params})).unwrap();
        let end = Instant::now() + Duration::from_secs(60);
        while let Ok(msg) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            if msg["id"] == n {
                return msg["result"].clone();
            }
        }
        Value::Null
    };
    call(
        Some(0),
        "initialize",
        json!({"clientInfo": {"name": "actx-tests", "version": "0"}}),
    );
    call(None, "initialized", Value::Null);
    let listed = call(Some(1), "thread/list", json!({}));
    let mut out = std::collections::HashMap::new();
    for (n, t) in listed["data"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let id = t["id"].as_str().unwrap().to_string();
        let thread = call(
            Some(n as i64 + 2),
            "thread/read",
            json!({"threadId": id, "includeTurns": true}),
        );
        let turns = thread["thread"]["turns"].as_array().map_or(0, Vec::len);
        out.insert(
            id,
            (
                t["preview"].as_str().unwrap_or("").to_string(),
                turns,
                thread["thread"]["cwd"].as_str().unwrap_or("").to_string(),
            ),
        );
    }
    let _ = child.kill();
    let _ = child.wait();
    out
}

/// A path with `/` separators, the way opencode keeps folders on Windows.
pub fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
