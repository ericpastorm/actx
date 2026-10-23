//! Antigravity CLI: converting from and to it. Its conversations are built here the way agy 1.4
//! stores them (checked by hand against the real agy, which resumes what actx writes).

mod common;

use std::path::{Path, PathBuf};

use common::*;
use rusqlite::{Connection, params};

/// Protocol buffers, just enough to write steps like agy's and read back actx's.
#[derive(Clone, Default)]
struct Msg(Vec<u8>);

impl Msg {
    fn put(&mut self, mut n: u64) {
        while n >= 0x80 {
            self.0.push((n as u8 & 0x7f) | 0x80);
            n >>= 7;
        }
        self.0.push(n as u8);
    }
    fn int(mut self, num: u64, v: u64) -> Msg {
        self.put(num << 3);
        self.put(v);
        self
    }
    fn text(mut self, num: u64, v: &str) -> Msg {
        self.put(num << 3 | 2);
        self.put(v.len() as u64);
        self.0.extend_from_slice(v.as_bytes());
        self
    }
    fn msg(self, num: u64, v: Msg) -> Msg {
        let mut m = self;
        m.put(num << 3 | 2);
        m.put(v.0.len() as u64);
        m.0.extend(v.0);
        m
    }
}

/// The texts at a path of field numbers.
fn texts_at(b: &[u8], path: &[u64]) -> Vec<String> {
    fn varint(b: &[u8], i: &mut usize) -> u64 {
        let (mut n, mut s) = (0, 0);
        loop {
            let c = b[*i];
            *i += 1;
            n |= u64::from(c & 0x7f) << s;
            s += 7;
            if c < 0x80 {
                return n;
            }
        }
    }
    let Some((first, rest)) = path.split_first() else {
        return vec![String::from_utf8_lossy(b).into_owned()];
    };
    let (mut i, mut out) = (0, Vec::new());
    while i < b.len() {
        let key = varint(b, &mut i);
        match key & 7 {
            0 => {
                varint(b, &mut i);
            }
            2 => {
                let n = varint(b, &mut i) as usize;
                if key >> 3 == *first {
                    out.extend(texts_at(&b[i..i + n], rest));
                }
                i += n;
            }
            _ => panic!("unexpected wire type"),
        }
    }
    out
}

enum Step<'a> {
    User(&'a str),
    Reply(&'a str),
    /// (call id, command)
    Command(&'a str, &'a str),
    /// (call id, command, output)
    Output(&'a str, &'a str, &'a str),
    /// (sender, title, content)
    Notice(&'a str, &'a str, &'a str),
    Error(&'a str),
    Summary(&'a str),
}

fn agy_home(m: &Machine) -> PathBuf {
    m.home.join(".gemini/antigravity-cli")
}

/// A conversation as agy 1.4 writes it; returns its id.
fn agy_conversation(m: &Machine, folder: &Path, title: &str, steps: &[Step]) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let home = agy_home(m);
    std::fs::create_dir_all(home.join("conversations")).unwrap();
    std::fs::create_dir_all(home.join("annotations")).unwrap();
    let uri = format!("file://{}", folder.display());
    let con = Connection::open(home.join(format!("conversations/{id}.db"))).unwrap();
    con.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE trajectory_meta (trajectory_id text, cascade_id text, trajectory_type integer, source integer, PRIMARY KEY (trajectory_id));
         CREATE TABLE steps (idx integer, step_type integer NOT NULL DEFAULT 0, status integer NOT NULL DEFAULT 0, has_subtrajectory numeric NOT NULL DEFAULT false, metadata blob, error_details blob, permissions blob, task_details blob, render_info blob, step_payload blob, step_format integer NOT NULL DEFAULT 0, PRIMARY KEY (idx));
         CREATE TABLE trajectory_metadata_blob (id text DEFAULT 'main', data blob, PRIMARY KEY (id));",
    )
    .unwrap();
    let meta = Msg::default().msg(1, Msg::default().text(1, &uri)).text(7, &uri);
    con.execute(
        "insert into trajectory_metadata_blob values ('main', ?1)",
        params![meta.0],
    )
    .unwrap();
    for (i, step) in steps.iter().enumerate() {
        let when = Msg::default().int(1, 1_792_000_000 + i as u64 * 60);
        let mut step_meta = Msg::default().msg(1, when);
        let (kind, body) = match step {
            Step::User(t) => (14, Msg::default().msg(19, Msg::default().text(2, t))),
            Step::Reply(t) => (
                15,
                Msg::default().msg(20, Msg::default().text(1, t).text(3, "thinking…")),
            ),
            Step::Command(id, cmd) => {
                let args = serde_json::json!({"CommandLine": cmd, "Cwd": folder, "toolAction": "Running"});
                let call = Msg::default()
                    .text(1, id)
                    .text(2, "run_command")
                    .text(3, &args.to_string());
                (
                    15,
                    Msg::default().msg(20, Msg::default().text(3, "let me look").msg(7, call)),
                )
            }
            Step::Output(id, cmd, out) => {
                let args = serde_json::json!({"CommandLine": cmd});
                step_meta = step_meta.msg(
                    4,
                    Msg::default()
                        .text(1, id)
                        .text(2, "run_command")
                        .text(3, &args.to_string()),
                );
                let result = Msg::default().msg(2, Msg::default().text(1, out));
                (132, Msg::default().msg(140, result))
            }
            Step::Notice(sender, what, content) => {
                let notice = Msg::default()
                    .text(1, content)
                    .msg(2, Msg::default().text(1, what))
                    .msg(4, Msg::default().text(3, sender).text(4, content));
                (101, Msg::default().msg(114, notice))
            }
            Step::Error(e) => (
                17,
                Msg::default().msg(24, Msg::default().msg(3, Msg::default().text(2, e))),
            ),
            Step::Summary(s) => (23, Msg::default().msg(30, Msg::default().text(5, s))),
        };
        let payload = Msg::default().int(1, kind).int(4, 3).msg(5, step_meta.clone());
        let mut payload = payload.0;
        payload.extend(body.0);
        con.execute(
            "insert into steps (idx, step_type, status, metadata, step_payload) values (?1, ?2, 3, ?3, ?4)",
            params![i as i64, kind as i64, step_meta.0, payload],
        )
        .unwrap();
    }
    let index = Connection::open(home.join("conversation_summaries.db")).unwrap();
    index
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS conversation_summaries (conversation_id text, title text NOT NULL DEFAULT '', preview text NOT NULL DEFAULT '', step_count integer NOT NULL DEFAULT 0, last_modified_time datetime NOT NULL, workspace_uris text NOT NULL, raw_summary blob, PRIMARY KEY (conversation_id));",
        )
        .unwrap();
    index
        .execute(
            "insert into conversation_summaries (conversation_id, title, preview, step_count, last_modified_time, workspace_uris) values (?1, ?2, ?2, ?3, '2026-10-18 12:05:14.434378518+00:00', ?4)",
            params![id, title, steps.len() as i64, serde_json::json!([uri]).to_string()],
        )
        .unwrap();
    std::fs::write(
        home.join(format!("annotations/{id}.pbtxt")),
        format!("title:\"{title}\"\n"),
    )
    .unwrap();
    id
}

#[test]
fn antigravity_converts_to_claude() {
    let w = World::new();
    let m = w.machine("pc");
    let repo = git_repo(&m.home.join("code/troll"));
    let id = agy_conversation(
        &m,
        &repo,
        "Voxel troll",
        &[
            Step::User("make me a voxel troll"),
            Step::Command("call_1", "python3 troll.py"),
            Step::Error("API error (attempt 1): UNAVAILABLE"),
            Step::Output(
                "call_1",
                "python3 troll.py",
                "\nThe command exited with code 0.\nOutput:\nSculpting troll\n",
            ),
            Step::Notice(
                "system",
                "System Message",
                "[Notice] All your subagents have been stopped",
            ),
            Step::Notice("conv/task-3", "Render finished", "preview.png written"),
            Step::Reply("Here is your troll: troll.vox"),
            Step::User("bigger club, please"),
            Step::Reply("Done: the club is twice as big"),
        ],
    );
    std::fs::create_dir_all(m.claude_dir()).unwrap();
    let out = m.pick(&["cv"], &[&id, "claude"]);
    assert!(out.contains("Voxel troll (from Antigravity)"), "{out}");
    let path = find(&m.claude_dir(), ".jsonl").expect("a Claude conversation");
    let sid = path.file_stem().unwrap().to_string_lossy().into_owned();
    let (_, texts, cwds) = claude_transcript(&m, &sid);
    assert_eq!(cwds, [repo.to_string_lossy()]);
    assert!(texts[0].ends_with("make me a voxel troll"), "{texts:?}");
    let reply = &texts[1];
    assert!(reply.contains("‹run_command› $ python3 troll.py"), "{reply}");
    assert!(reply.contains("Sculpting troll"));
    assert!(reply.contains("‹task› Render finished") && reply.contains("preview.png written"));
    assert!(reply.ends_with("Here is your troll: troll.vox"));
    assert!(!reply.contains("subagents have been stopped") && !reply.contains("UNAVAILABLE"));
    assert!(
        !reply.contains("thinking") && !reply.contains("let me look"),
        "no reasoning"
    );
    assert_eq!(&texts[2..], ["bigger club, please", "Done: the club is twice as big"]);
}

#[test]
fn after_a_summary_antigravity_starts_from_it() {
    let w = World::new();
    let m = w.machine("pc");
    let id = agy_conversation(
        &m,
        &m.home,
        "Long one",
        &[
            Step::User("old question"),
            Step::Reply("old answer"),
            Step::Summary("<summary>We built a troll.</summary>"),
            Step::User("now the orc"),
            Step::Reply("Orc ready"),
        ],
    );
    std::fs::create_dir_all(m.claude_dir()).unwrap();
    m.pick(&["cv"], &[&id, "claude"]);
    let path = find(&m.claude_dir(), ".jsonl").expect("a Claude conversation");
    let (_, texts, _) = claude_transcript(&m, &path.file_stem().unwrap().to_string_lossy());
    assert!(
        texts[0].contains("We built a troll.") && texts[0].ends_with("now the orc"),
        "{texts:?}"
    );
    assert!(!texts.iter().any(|t| t.contains("old question")));
    assert_eq!(texts.last().unwrap(), "Orc ready");
}

#[test]
fn claude_converts_to_antigravity_and_back() {
    let w = World::new();
    let m = w.machine("pc");
    let repo = git_repo(&m.home.join("code/shop"));
    let session = claude_session(
        &m,
        &repo,
        &[
            ("user", "my cat is called Pimiento"),
            ("assistant", "Noted: Pimiento"),
            ("user", "and my colour is turquoise"),
            ("assistant", "Turquoise it is"),
        ],
        Some("Cat and colour"),
    );
    let sid = session.file_stem().unwrap().to_string_lossy().into_owned();
    std::fs::create_dir_all(agy_home(&m)).unwrap();
    let out = m.pick(&["cv"], &[&sid, "antigravity"]);
    assert!(out.contains("agy --conversation"), "{out}");

    // what agy reads: the steps, the index (and its own copy of it), the title
    let home = agy_home(&m);
    let index = Connection::open(home.join("conversation_summaries.db")).unwrap();
    let (id, title, uris, raw): (String, String, String, Vec<u8>) = index
        .query_row(
            "select conversation_id, title, workspace_uris, raw_summary from conversation_summaries",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(title, "Cat and colour (from Claude Code)");
    assert_eq!(
        uris,
        // file:///C:/Users/me/shop on Windows
        serde_json::json!([format!(
            "file://{}{}",
            if cfg!(windows) { "/" } else { "" },
            slash(&repo)
        )])
        .to_string()
    );
    assert_eq!(texts_at(&raw, &[1]), [title.as_str()]);
    let cache = std::fs::read(home.join("jetbox_summaries_proto.pb")).unwrap();
    assert_eq!(texts_at(&cache, &[1, 1]), [id.as_str()]);
    assert_eq!(texts_at(&cache, &[1, 2, 1]), [title.as_str()]);
    assert_eq!(
        std::fs::read_to_string(home.join(format!("annotations/{id}.pbtxt"))).unwrap(),
        format!("title:\"{title}\"\n")
    );
    let con = Connection::open(home.join(format!("conversations/{id}.db"))).unwrap();
    let steps: Vec<(i64, Vec<u8>)> = con
        .prepare("select step_type, step_payload from steps order by idx")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let kinds: Vec<i64> = steps.iter().map(|s| s.0).collect();
    assert_eq!(kinds, [14, 15, 14, 15]);
    assert!(texts_at(&steps[0].1, &[19, 2])[0].ends_with("my cat is called Pimiento"));
    assert_eq!(texts_at(&steps[3].1, &[20, 1]), ["Turquoise it is"]);
    assert_eq!(texts_at(&steps[1].1, &[5, 20, 4]), [id.as_str()]);

    // and back, through actx's own reading. The original is linked to the new Antigravity
    // copy, so remove it first: this checks a real conversion instead of reusing that copy.
    std::fs::remove_file(&session).unwrap();
    let out = m.pick(&["cv"], &[&id, "claude"]);
    assert!(out.contains("Cat and colour (from Antigravity)"), "{out}");
    let back = std::fs::read_dir(m.claude_dir().join(encode_claude(&repo)))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p != &session)
        .expect("the copy back");
    let (_, texts, _) = claude_transcript(&m, &back.file_stem().unwrap().to_string_lossy());
    assert!(texts[0].ends_with("my cat is called Pimiento"));
    assert_eq!(
        &texts[1..],
        ["Noted: Pimiento", "and my colour is turquoise", "Turquoise it is"]
    );
}

#[test]
fn folderless_antigravity_asks_for_a_working_folder_without_init() {
    let w = World::new();
    let mut m = w.machine("solo");
    m.store = PathBuf::new();
    let id = agy_conversation(
        &m,
        &m.home,
        "No workspace",
        &[Step::User("hello"), Step::Reply("answer")],
    );
    let home = agy_home(&m);
    let db = home.join(format!("conversations/{id}.db"));
    Connection::open(&db)
        .unwrap()
        .execute("delete from trajectory_metadata_blob", [])
        .unwrap();
    Connection::open(home.join("conversation_summaries.db"))
        .unwrap()
        .execute("update conversation_summaries set workspace_uris = '[]'", [])
        .unwrap();
    std::fs::create_dir_all(m.claude_dir()).unwrap();
    let original = std::fs::read(&db).unwrap();
    let out = m.actx_with(
        &["cv", "--all"],
        Opts {
            pick: Some(&[&id, "claude", "::other"]),
            answer: Some(m.home.to_str().unwrap()),
            ..Default::default()
        },
    );
    assert!(out.contains("No workspace (from Antigravity)"), "{out}");
    assert_eq!(std::fs::read(&db).unwrap(), original);
    let copy = find(&m.claude_dir(), ".jsonl").unwrap();
    let (_, texts, cwds) = claude_transcript(&m, &copy.file_stem().unwrap().to_string_lossy());
    assert_eq!(cwds, [m.home.to_string_lossy()]);
    assert_eq!(texts.last().unwrap(), "answer");
}
