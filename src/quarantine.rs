//! Compressed local recovery storage. Prepared manifests survive interrupted cleanup.
use crate::{
    agents,
    clean::{self, Candidate},
    ctx::{Error, Result, ctx},
    harness, inspection,
    session::Session,
    store, tr,
    ui::{self, Row},
};
use rusqlite::{Connection, OpenFlags, types::Value as SqlValue};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
struct Entry {
    version: u8,
    id: String,
    harness: String,
    session_id: String,
    title: String,
    folder: String,
    source: Option<PathBuf>,
    at: f64,
    status: String,
    #[serde(default)]
    archived: Option<PathBuf>,
    #[serde(default)]
    digest: u64,
}
impl Entry {
    fn session(&self) -> Result<Session> {
        let tool = harness::known(&self.harness).ok_or_else(|| Error("Unknown quarantined harness".into()))?;
        let mut s = Session::local(
            tool,
            self.session_id.clone(),
            self.title.clone(),
            self.folder.clone(),
            0.0,
            0,
        );
        s.src = self.source.clone();
        Ok(s)
    }
}
fn root() -> PathBuf {
    ctx().state_dir.join("quarantine")
}
fn save(dir: &Path, entry: &Entry) -> Result<()> {
    let path = dir.join("entry.tmp");
    let mut file = std::fs::File::create(&path)?;
    file.write_all(&serde_json::to_vec_pretty(entry)?)?;
    file.sync_all()?;
    std::fs::rename(path, dir.join("entry.json"))?;
    #[cfg(unix)]
    {
        std::fs::File::open(dir)?.sync_all()?;
        if let Some(parent) = dir.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
    }
    Ok(())
}
fn entries() -> Vec<(PathBuf, Entry)> {
    let mut all: Vec<_> = std::fs::read_dir(root())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|d| {
            let bytes = std::fs::read(d.path().join("entry.json")).ok()?;
            let e: Entry = serde_json::from_slice(&bytes).ok()?;
            (e.version == 1 && d.file_name().to_str() == Some(e.id.as_str()) && e.status != "restored")
                .then_some((d.path(), e))
        })
        .collect();
    all.sort_by(|a, b| b.1.at.total_cmp(&a.1.at));
    all
}
fn lock() -> Result<std::fs::File> {
    std::fs::create_dir_all(&ctx().state_dir)?;
    let f = std::fs::File::create(ctx().state_dir.join("clean.lock"))?;
    f.lock()?;
    Ok(f)
}
fn digest(mut source: impl Read) -> Result<u64> {
    let mut hash = 0xcbf29ce484222325u64;
    let mut buf = [0u8; 65536];
    loop {
        let n = source.read(&mut buf)?;
        if n == 0 {
            break;
        }
        for b in &buf[..n] {
            hash = (hash ^ u64::from(*b)).wrapping_mul(0x100000001b3);
        }
    }
    Ok(hash)
}
fn compressed(dir: &Path, path: &Path) -> Result<u64> {
    let target = dir.join("payload.zst");
    store::write_compressed(&target, std::fs::File::open(path)?)?;
    std::fs::File::open(&target)?.sync_all()?;
    digest(store::open_read(&target)?)
}
fn snapshot_db(source: &Path, target: &Path) -> Result<()> {
    inspection::enable();
    let result = (|| {
        let con = inspection::database(source)?;
        con.execute("VACUUM INTO ?1", [target.to_string_lossy().as_ref()])?;
        Ok(())
    })();
    inspection::finish();
    result
}
fn capture(s: &Session, dir: &Path) -> Result<u64> {
    if s.harness == "opencode" {
        let bytes = crate::opencode::export(&s.id, &s.path)?;
        store::write_compressed(&dir.join("payload.zst"), bytes.as_slice())?;
        return digest(bytes.as_slice());
    }
    let source = s.src.as_ref().ok_or_else(|| Error("Missing source file".into()))?;
    if s.harness == "antigravity" {
        let file = dir.join("snapshot.db");
        snapshot_db(source, &file)?;
        agy_capture(s, dir)?;
        let hash = compressed(dir, &file)?;
        std::fs::remove_file(file)?;
        return Ok(hash);
    }
    compressed(dir, source)
}
fn unchanged(c: &Candidate) -> Result<()> {
    if agents::in_use(&c.session) || agents::in_use_by_agent_now(&c.session) {
        return Err(Error(tr!("Conversation is open or recently active; cleanup deferred")));
    }
    if clean::fingerprint(&c.session)? != c.fingerprint {
        return Err(Error(tr!("Conversation changed since selection; cleanup deferred")));
    }
    Ok(())
}
pub fn apply(selected: &[&Candidate]) -> Result<()> {
    let _lock = lock()?;
    let mut count = 0;
    for c in selected {
        let result = (|| -> Result<()> {
            unchanged(c)?;
            inspection::enable();
            let sessions = clean::locals(&harness::CONVERTS);
            let protected = clean::protected(&sessions);
            let parent_open = clean::parent_key(&c.session).is_some_and(|key| {
                sessions
                    .iter()
                    .any(|s| s.key() == key && (agents::in_use(s) || agents::in_use_by_agent_now(s)))
            });
            inspection::finish();
            if protected.contains(&c.session.key()) || parent_open {
                return Err(Error(tr!(
                    "Conversation became shared, linked or owned by an active parent; cleanup deferred"
                )));
            }
            if c.session.harness == "opencode" && crate::opencode::clean_has_children(&c.session.id) {
                return Err(Error(tr!(
                    "Conversation has child sessions; clean the children individually"
                )));
            }
            let id = uuid::Uuid::new_v4().to_string();
            let dir = root().join(&id);
            std::fs::create_dir_all(&dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(root(), std::fs::Permissions::from_mode(0o700))?;
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
            }
            let mut entry = Entry {
                version: 1,
                id,
                harness: c.session.harness.into(),
                session_id: c.session.id.clone(),
                title: c.session.title.clone(),
                folder: c.session.path.clone(),
                source: c.session.src.clone(),
                at: crate::ctx::now(),
                status: "prepared".into(),
                archived: None,
                digest: 0,
            };
            entry.digest = capture(&c.session, &dir)?;
            for file in std::fs::read_dir(&dir)? {
                let file = file?;
                if file.file_type()?.is_file() {
                    std::fs::File::open(file.path())?.sync_all()?;
                }
            }
            save(&dir, &entry)?;
            unchanged(c)?;
            match c.session.harness {
                "opencode" => crate::opencode::delete(&c.session.id, &c.session.path)?,
                "antigravity" => agy_remove(&c.session)?,
                "codex" if crate::ctx::which("codex") => {
                    let name = c
                        .session
                        .src
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .ok_or_else(|| Error("Missing rollout name".into()))?;
                    let archived = ctx().codex_home.join("archived_sessions").join(name);
                    entry.archived = Some(archived.clone());
                    save(&dir, &entry)?;
                    crate::codex::clean_request(&c.session.id, "thread/archive")?;
                    if digest(std::fs::File::open(&archived)?)? != entry.digest {
                        return Err(Error(tr!("Archived conversation changed; its native archive was kept")));
                    }
                    std::fs::remove_file(&archived)?;
                }
                _ => {
                    if c.session.harness == "codex" && has_codex_db() {
                        return Err(Error(tr!(
                            "Codex CLI is required to update its conversation index; the original was kept"
                        )));
                    }
                    std::fs::remove_file(
                        c.session
                            .src
                            .as_ref()
                            .ok_or_else(|| Error("Missing transcript".into()))?,
                    )?;
                }
            }
            entry.status = "quarantined".into();
            save(&dir, &entry)?;
            count += 1;
            ui::ok(&tr!("Quarantined: {title}", title = c.session.title));
            Ok(())
        })();
        if let Err(error) = result {
            ui::warn(&format!("{}: {error}", c.session.title));
        }
    }
    ui::say(&tr!(
        "{n} local conversations quarantined. Recover with actx clean restore. Shared copies were kept.",
        n = count
    ));
    Ok(())
}
fn has_codex_db() -> bool {
    std::fs::read_dir(&ctx().codex_home)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.path().extension().is_some_and(|x| x == "sqlite"))
}
pub fn list() {
    let all = entries();
    if all.is_empty() {
        ui::say(&tr!("Quarantine is empty"));
    }
    for (_, e) in all {
        println!("{} · {} · {} · {} · {}", e.id, e.harness, e.title, e.folder, e.status);
    }
}
fn restore_file(payload: &Path, destination: &Path, allow_existing: bool, expected: u64) -> Result<()> {
    if destination.exists() {
        if allow_existing && digest(std::fs::File::open(destination)?)? == expected {
            return Ok(());
        }
        return Err(Error(tr!("Destination already exists; nothing was overwritten")));
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let parent = destination
        .parent()
        .ok_or_else(|| Error("Missing restore directory".into()))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    crate::paths::make_private(temp.path())?;
    std::io::copy(&mut store::open_read(payload)?, temp.as_file_mut())?;
    temp.as_file().sync_all()?;
    temp.persist_noclobber(destination)
        .map_err(|error| Error(error.error.to_string()))?;
    Ok(())
}
pub fn restore(id: Option<&str>) {
    let all = entries();
    if all.is_empty() {
        ui::say(&tr!("Quarantine is empty"));
        return;
    }
    let key = if let Some(id) = id {
        id.to_string()
    } else {
        let rows: Vec<_> = all
            .iter()
            .map(|(_, e)| Row {
                key: e.id.clone(),
                line: format!("{} {} {}", e.harness, e.title, e.folder),
                preview: format!("{}\n{}\n{}", e.title, e.session_id, e.folder),
            })
            .collect();
        let Some(key) = ui::fzf(&rows, &tr!("Choose a quarantined conversation to recover"), false, true)
            .into_iter()
            .next()
        else {
            return;
        };
        key
    };
    let Some((dir, mut entry)) = all.into_iter().find(|(_, e)| e.id == key) else {
        ui::fail(&tr!("Quarantine entry not found"));
        return;
    };
    if !ui::confirm(&tr!(
        "Recover {title} to its original harness? Existing conversations will not be overwritten.",
        title = entry.title
    )) {
        return;
    }
    let result = (|| -> Result<()> {
        let _lock = lock()?;
        let saved: Entry = serde_json::from_slice(&std::fs::read(dir.join("entry.json"))?)?;
        if saved.status == "restored" {
            return Err(Error(tr!("Conversation was already recovered")));
        }
        let s = entry.session()?;
        if agents::in_use_by_agent_now(&s) {
            return Err(Error(tr!("Conversation is open or recently active; cleanup deferred")));
        }
        let payload = dir.join("payload.zst");
        if digest(store::open_read(&payload)?)? != entry.digest {
            return Err(Error(tr!("Quarantine payload failed its integrity check")));
        }
        let resuming = saved.status == "restoring";
        entry.status = "restoring".into();
        save(&dir, &entry)?;
        if s.harness == "opencode" {
            if !Path::new(&s.path).is_dir() {
                return Err(Error(tr!(
                    "The original work folder is missing; restore it before recovering this conversation"
                )));
            }
            if crate::opencode::clean_local().iter().any(|c| c.id == s.id) {
                return Err(Error(tr!("Destination already exists; nothing was overwritten")));
            }
            crate::opencode::import(&store::read_all(&payload)?, &s.path)?;
        } else if let Some(archived) = &entry.archived {
            if s.src.as_ref().is_some_and(|p| p.exists()) {
                return Err(Error(tr!("Destination already exists; nothing was overwritten")));
            }
            restore_file(&payload, archived, resuming, entry.digest)?;
            crate::codex::clean_request(&s.id, "thread/unarchive")?;
        } else {
            let destination = s
                .src
                .as_ref()
                .ok_or_else(|| Error("Missing original location".into()))?;
            restore_file(&payload, destination, resuming, entry.digest)?;
            if s.harness == "antigravity" {
                agy_restore(&s, &dir)?;
            }
            if s.harness == "codex" && crate::ctx::which("codex") {
                crate::codex::clean_request(&s.id, "thread/read")?;
            }
        }
        entry.status = "restored".into();
        save(&dir, &entry)?;
        Ok(())
    })();
    match result {
        Ok(()) => ui::ok(&tr!("Recovered: {title}", title = entry.title)),
        Err(error) => ui::fail(&error.to_string()),
    }
}

#[derive(Serialize, Deserialize)]
enum Cell {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}
impl From<SqlValue> for Cell {
    fn from(v: SqlValue) -> Self {
        match v {
            SqlValue::Null => Self::Null,
            SqlValue::Integer(v) => Self::Integer(v),
            SqlValue::Real(v) => Self::Real(v),
            SqlValue::Text(v) => Self::Text(v),
            SqlValue::Blob(v) => Self::Blob(v),
        }
    }
}
impl From<Cell> for SqlValue {
    fn from(v: Cell) -> Self {
        match v {
            Cell::Null => Self::Null,
            Cell::Integer(v) => Self::Integer(v),
            Cell::Real(v) => Self::Real(v),
            Cell::Text(v) => Self::Text(v),
            Cell::Blob(v) => Self::Blob(v),
        }
    }
}
fn agy_capture(s: &Session, dir: &Path) -> Result<()> {
    inspection::enable();
    let result = (|| -> Result<()> {
        let index = crate::antigravity::home().join("conversation_summaries.db");
        if index.exists() {
            let con = inspection::database(&index)?;
            let mut q = con.prepare("select * from conversation_summaries where conversation_id=?1")?;
            let columns = q.column_names().iter().map(|s| s.to_string()).collect::<Vec<_>>();
            let mut rows = q.query([&s.id])?;
            if let Some(row) = rows.next()? {
                let cells: Vec<_> = columns
                    .iter()
                    .enumerate()
                    .map(|(i, name)| Ok((name.clone(), Cell::from(row.get::<_, SqlValue>(i)?))))
                    .collect::<rusqlite::Result<_>>()?;
                std::fs::write(dir.join("agy-index.json"), serde_json::to_vec(&cells)?)?;
            }
        }
        let annotation = crate::antigravity::home()
            .join("annotations")
            .join(format!("{}.pbtxt", s.id));
        if annotation.exists() {
            std::fs::copy(annotation, dir.join("annotation.pbtxt"))?;
        }
        let cache = crate::antigravity::home().join("jetbox_summaries_proto.pb");
        if cache.exists() {
            let bytes = std::fs::read(cache)?;
            let entries: Vec<_> = crate::pb::all(&bytes, &[1])
                .into_iter()
                .filter(|m| crate::pb::text(m, &[1]).as_deref() == Some(&s.id))
                .map(Vec::from)
                .collect();
            std::fs::write(dir.join("agy-cache.json"), serde_json::to_vec(&entries)?)?;
        }
        Ok(())
    })();
    inspection::finish();
    result
}
fn cache_filter(id: &str, restore: Option<&Path>) -> Result<()> {
    let cache = crate::antigravity::home().join("jetbox_summaries_proto.pb");
    let bytes = std::fs::read(&cache).unwrap_or_default();
    let mut out = crate::pb::filter_bytes(&bytes, 1, |v| crate::pb::text(v, &[1]).as_deref() != Some(id))
        .ok_or_else(|| Error("Invalid Antigravity summary cache; original kept".into()))?;
    if let Some(dir) = restore
        && dir.join("agy-cache.json").exists()
    {
        let entries: Vec<Vec<u8>> = serde_json::from_slice(&std::fs::read(dir.join("agy-cache.json"))?)?;
        for entry in entries {
            out.extend(crate::pb::Msg::new().bytes(1, &entry).done());
        }
    }
    if cache.exists() || !out.is_empty() {
        let tmp = cache.with_extension("pb.actx-clean-tmp");
        std::fs::write(&tmp, out)?;
        crate::paths::make_private(&tmp)?;
        std::fs::rename(tmp, cache)?;
    }
    Ok(())
}
fn agy_remove(s: &Session) -> Result<()> {
    cache_filter(&s.id, None)?;
    let index = crate::antigravity::home().join("conversation_summaries.db");
    if index.exists() {
        let con = Connection::open_with_flags(index, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        con.execute("delete from conversation_summaries where conversation_id=?1", [&s.id])?;
    }
    let source = s.src.as_ref().ok_or_else(|| Error("Missing AGY database".into()))?;
    std::fs::remove_file(source)?;
    for suffix in ["-wal", "-shm"] {
        let mut p = source.as_os_str().to_os_string();
        p.push(suffix);
        let p = PathBuf::from(p);
        if p.exists() {
            std::fs::remove_file(p)?;
        }
    }
    let annotation = crate::antigravity::home()
        .join("annotations")
        .join(format!("{}.pbtxt", s.id));
    if annotation.exists() {
        std::fs::remove_file(annotation)?;
    }
    Ok(())
}
fn agy_restore(s: &Session, dir: &Path) -> Result<()> {
    if dir.join("agy-index.json").exists() {
        let cells: Vec<(String, Cell)> = serde_json::from_slice(&std::fs::read(dir.join("agy-index.json"))?)?;
        let names = cells
            .iter()
            .map(|(n, _)| format!("\"{}\"", n.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(",");
        let params = (0..cells.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let values: Vec<SqlValue> = cells.into_iter().map(|(_, v)| v.into()).collect();
        let con = Connection::open_with_flags(
            crate::antigravity::home().join("conversation_summaries.db"),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        let mut query = con.prepare("select * from conversation_summaries where conversation_id=?1")?;
        let mut rows = query.query([&s.id])?;
        if let Some(row) = rows.next()? {
            let existing = (0..values.len())
                .map(|i| row.get::<_, SqlValue>(i))
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if existing != values {
                return Err(Error(tr!("Destination already exists; nothing was overwritten")));
            }
        } else {
            con.execute(
                &format!("insert into conversation_summaries ({names}) values ({params})"),
                rusqlite::params_from_iter(values),
            )?;
        }
    }
    cache_filter(&s.id, Some(dir))?;
    if dir.join("annotation.pbtxt").exists() {
        let dest = crate::antigravity::home()
            .join("annotations")
            .join(format!("{}.pbtxt", s.id));
        if dest.exists() {
            if std::fs::read(&dest)? == std::fs::read(dir.join("annotation.pbtxt"))? {
                return Ok(());
            }
            return Err(Error(tr!("Destination already exists; nothing was overwritten")));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(dir.join("annotation.pbtxt"), dest)?;
    }
    Ok(())
}
