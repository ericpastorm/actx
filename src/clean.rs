//! Explainable, opt-in cleanup of local conversations. Shared/linked copies are protected.
use crate::{
    agents,
    ctx::{Error, Result, ctx},
    harness, inspection,
    session::Session,
    store, tr,
    ui::{self, Row},
};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct Options {
    pub advanced: bool,
    pub dry_run: bool,
    pub days: u64,
    pub max_messages: usize,
    pub max_chars: usize,
    pub min_similar: usize,
    pub harnesses: Vec<&'static str>,
    pub folder: Option<PathBuf>,
    pub contains: Option<String>,
    pub rules: HashSet<String>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            advanced: false,
            dry_run: false,
            days: 7,
            max_messages: 4,
            max_chars: 500,
            min_similar: 3,
            harnesses: harness::CONVERTS.to_vec(),
            folder: None,
            contains: None,
            rules: ["agents", "repeated", "tests", "short"]
                .into_iter()
                .map(String::from)
                .collect(),
        }
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Summary {
    first: String,
    turns: Option<usize>,
    chars: Option<usize>,
    text: String,
    native_agent: bool,
}
pub struct Candidate {
    pub session: Session,
    pub reasons: Vec<String>,
    pub(crate) fingerprint: String,
    pub(crate) preview: String,
    pub(crate) bytes: u64,
}

pub(crate) fn parent_key(s: &Session) -> Option<String> {
    if s.harness == "opencode" {
        return crate::opencode::clean_parent(&s.id).map(|id| format!("opencode:{id}"));
    }
    if let Some(path) = &s.src
        && path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n == "subagents")
    {
        return path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .map(|id| format!("{}:{}", s.harness, id.to_string_lossy()));
    }
    if s.harness == "codex" {
        let meta = s.src.as_ref().map(|p| crate::codex::session_meta(p))?;
        return meta["source"]["subagent"]["thread_spawn"]["parent_thread_id"]
            .as_str()
            .or_else(|| meta["parent_thread_id"].as_str())
            .map(|id| format!("codex:{id}"));
    }
    None
}
pub(crate) fn native_agent(s: &Session) -> bool {
    if parent_key(s).is_some() {
        return true;
    }
    if s.src
        .as_ref()
        .is_some_and(|p| p.components().any(|c| c.as_os_str() == "subagents"))
    {
        return true;
    }
    if s.harness == "codex" {
        let meta = s
            .src
            .as_ref()
            .map(|p| crate::codex::session_meta(p))
            .unwrap_or_default();
        return meta
            .get("thread_source")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|s| s != "user")
            || meta
                .get("source")
                .is_some_and(|s| s.get("subagent").is_some() || s.as_str() == Some("subagent"));
    }
    false
}
fn normalized(text: &str) -> String {
    text.to_lowercase()
        .split_whitespace()
        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
fn summary(s: &Session) -> Result<Summary> {
    let path = s.src.as_deref().unwrap_or(&ctx().opencode_db);
    crate::discovery::memo(
        &format!("cleanup-summary-{}", s.key()),
        path,
        &serde_json::json!([s.key(), s.weight, s.title]),
        || summary_fresh(s),
    )
}
fn summary_fresh(s: &Session) -> Result<Summary> {
    let small = s
        .src
        .as_ref()
        .and_then(|p| p.metadata().ok())
        .is_some_and(|m| m.len() <= 1024 * 1024)
        || s.harness == "opencode" && s.weight <= 24;
    if small {
        let (turns, _) = harness::get(s.harness).local_turns(s)?;
        let first = turns
            .iter()
            .find(|t| t.who == "user")
            .map(|t| t.text.clone())
            .unwrap_or_else(|| s.title.clone());
        return Ok(Summary {
            first,
            turns: Some(turns.len()),
            chars: Some(turns.iter().map(|t| t.text.chars().count()).sum()),
            text: ui::render(
                &turns
                    .iter()
                    .take(8)
                    .map(|t| (t.who.to_string(), t.text.clone()))
                    .collect::<Vec<_>>(),
                s.harness,
            ),
            native_agent: native_agent(s),
        });
    }
    let first = if let Some(path) = &s.src
        && s.harness != "antigravity"
    {
        let (head, _) = crate::claude::chunks(path)?;
        let messages = if s.harness == "claude" {
            crate::claude::messages(&head)
        } else {
            crate::codex::messages(&head)
        };
        messages
            .into_iter()
            .find(|(who, _)| who == "user")
            .map(|(_, text)| text)
            .unwrap_or_else(|| s.title.clone())
    } else {
        s.title.clone()
    };
    Ok(Summary {
        first,
        turns: None,
        chars: None,
        text: s.title.clone(),
        native_agent: native_agent(s),
    })
}
fn test_prompt(text: &str) -> bool {
    let text = normalized(text);
    if text.chars().count() > 180 {
        return false;
    }
    [
        "responde solo",
        "responde únicamente",
        "responde unicamente",
        "reply only",
        "respond only",
        "just say",
        "say only",
    ]
    .iter()
    .any(|p| text.starts_with(p))
        && text
            .split_whitespace()
            .any(|w| ["ok", "okay", "hola", "hello", "pong"].contains(&w))
        || [
            "ping",
            "test",
            "prueba",
            "conexión ok",
            "conexion ok",
            "connection test",
        ]
        .contains(&text.as_str())
}
fn possible_agent(text: &str) -> bool {
    let text = normalized(text);
    [
        "eres un agente",
        "eres una agente",
        "you are an agent",
        "you are a coding agent",
        "you are a programming agent",
        "you are a model",
        "eres un modelo",
    ]
    .iter()
    .any(|p| text.starts_with(p))
}
fn stamp(path: &Path) -> Result<(u64, u128)> {
    let meta = path.metadata()?;
    Ok((
        meta.len(),
        meta.modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    ))
}
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut p = path.as_os_str().to_os_string();
    p.push(suffix);
    p.into()
}
pub(crate) fn fingerprint(s: &Session) -> Result<String> {
    fingerprint_inner(s, true)
}
pub(crate) fn revision(s: &Session) -> Result<String> {
    fingerprint_inner(s, false)
}
fn fingerprint_inner(s: &Session, full: bool) -> Result<String> {
    if full && s.harness == "opencode" {
        return Ok(format!("{:016x}", crate::opencode::clean_fingerprint(&s.id)?));
    }
    let paths = if let Some(path) = &s.src {
        let mut files = vec![path.clone()];
        if s.harness == "antigravity" {
            let wal = sidecar(path, "-wal");
            if wal.exists() {
                files.push(wal);
            }
        }
        files
    } else {
        let path = ctx().opencode_db.clone();
        let mut files = vec![path.clone()];
        let wal = sidecar(&path, "-wal");
        if wal.exists() {
            files.push(wal);
        }
        files
    };
    let mut hash = 0xcbf29ce484222325u64;
    for path in paths {
        let before = stamp(&path)?;
        let mut file = std::fs::File::open(&path)?;
        // The opencode DB is shared by all sessions: metadata changes invalidate every candidate.
        // Native transcript and per-session DB payloads are hashed in full, including WAL.
        if full && s.harness != "opencode" {
            let mut buf = [0u8; 65536];
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                for b in &buf[..n] {
                    hash = (hash ^ u64::from(*b)).wrapping_mul(0x100000001b3);
                }
            }
        }
        if stamp(&path)? != before {
            return Err(Error(tr!("Conversation changed while being inspected")));
        }
        #[cfg(unix)]
        let extra = {
            use std::os::unix::fs::MetadataExt;
            let meta = path.metadata()?;
            format!("{}:{}:{}", meta.ctime(), meta.ctime_nsec(), meta.ino())
        };
        #[cfg(not(unix))]
        let extra = String::new();
        for b in format!("{}:{}:{}:{extra}", path.display(), before.0, before.1).bytes() {
            hash = (hash ^ u64::from(b)).wrapping_mul(0x100000001b3);
        }
    }
    Ok(format!("{hash:016x}"))
}
pub(crate) fn protected(sessions: &[Session]) -> HashSet<String> {
    let mut keys: HashSet<_> = store::store_root(true)
        .map(|root| store::stored_sessions(&root).into_iter().map(|s| s.key()).collect())
        .unwrap_or_default();
    keys.extend(crate::multisync::linked_family());
    keys.extend(crate::state::load().base.into_keys());
    let aliases = crate::catalog::aliases();
    let families: HashSet<_> = keys.iter().map(|key| crate::catalog::root(&aliases, key)).collect();
    for s in sessions {
        if families.contains(&crate::catalog::root(&aliases, &s.key())) {
            keys.insert(s.key());
        }
    }
    loop {
        let before = keys.len();
        for s in sessions {
            if parent_key(s).is_some_and(|p| keys.contains(&p)) {
                keys.insert(s.key());
            }
        }
        if keys.len() == before {
            break;
        }
    }
    keys
}
/// Recheck one conversation against the shared folder, links, aliases and state without
/// rescanning every local session. `cached` is the set computed when the batch started.
pub(crate) fn now_protected(s: &Session, cached: &HashSet<String>) -> bool {
    if cached.contains(&s.key()) {
        return true;
    }
    let aliases = crate::catalog::aliases();
    let root = crate::catalog::root(&aliases, &s.key());
    managed_key(&root) || (root != s.key() && managed_key(&s.key()))
}
/// Is this key shared, linked or remembered as managed on this machine?
fn managed_key(key: &str) -> bool {
    if let Some((harness, id)) = key.split_once(':')
        && let Some(root) = store::store_root(true)
        && store::find_stored(&root, harness, id).is_some()
    {
        return true;
    }
    if crate::multisync::linked_family().iter().any(|k| k == key) {
        return true;
    }
    crate::state::load().base.contains_key(key)
}
pub(crate) fn parent_active(s: &Session, sessions: &[Session]) -> bool {
    parent_key(s).is_some_and(|key| sessions.iter().any(|p| p.key() == key && agents::in_use(p)))
}
pub(crate) fn locals(tools: &[&str]) -> Vec<Session> {
    tools
        .iter()
        .flat_map(|n| match *n {
            "claude" => crate::claude::clean_local(),
            "codex" => crate::codex::clean_local(),
            "opencode" => crate::opencode::clean_local(),
            _ => harness::get(n).local(),
        })
        .collect()
}
pub(crate) fn scan(options: &Options) -> Result<Vec<Candidate>> {
    inspection::enable();
    let _guard = Inspection;
    let sessions = locals(&options.harnesses);
    let protected = protected(&sessions);
    let cutoff = crate::ctx::now() - (options.days * 86400) as f64;
    let folder = options
        .folder
        .as_ref()
        .map(|p| crate::paths::canonical(p).unwrap_or_else(|_| p.clone()));
    let mut inspected = Vec::new();
    let mut skipped = 0;
    let total = sessions.len();
    let label = tr!("Analyzing conversations");
    for (index, s) in sessions.iter().enumerate() {
        crate::work::check()?;
        ui::progress(index + 1, total, &label);
        if s.updated <= 0.0
            || s.updated > cutoff
            || s.harness == "opencode" && crate::opencode::clean_has_children(&s.id)
            || protected.contains(&s.key())
            || agents::in_use(s)
            || parent_active(s, &sessions)
        {
            skipped += 1;
            continue;
        }
        if folder
            .as_ref()
            .is_some_and(|folder| !Path::new(&s.path).starts_with(folder))
        {
            continue;
        }
        let before = revision(s)?;
        match summary(s) {
            Ok(summary) => {
                if revision(s)? != before {
                    ui::warn(&tr!("Conversation changed while being inspected"));
                    continue;
                }
                inspected.push((s.clone(), summary, before));
            }
            Err(error) => ui::warn(&format!("{}: {error}", s.title)),
        }
    }
    let mut repeated: HashMap<(String, String), usize> = HashMap::new();
    for (s, data, _) in &inspected {
        let prefix = normalized(&data.first).chars().take(200).collect::<String>();
        if prefix.chars().count() >= 48 {
            *repeated.entry((s.path.clone(), prefix)).or_default() += 1;
        }
    }
    let mut candidates = Vec::new();
    for (session, data, observed_revision) in inspected {
        crate::work::check()?;
        let mut reasons = Vec::new();
        if options.rules.contains("agents") {
            if data.native_agent {
                reasons.push(tr!("Subagent confirmed by native metadata"));
            } else if possible_agent(&data.first) {
                reasons.push(tr!("Possible subagent: its opening prompt assigns an agent role"));
            }
        }
        if options.rules.contains("tests")
            && test_prompt(&data.first)
            && data.turns.is_some_and(|n| n <= options.max_messages)
            && data.chars.is_some_and(|n| n <= options.max_chars)
        {
            reasons.push(tr!("Short connection test or reply-only prompt"));
        }
        if options.rules.contains("short")
            && data.turns.is_some_and(|n| n <= options.max_messages)
            && data.chars.is_some_and(|n| n <= options.max_chars)
        {
            reasons.push(tr!(
                "Short conversation: {n} messages, {chars} characters",
                n = data.turns.unwrap(),
                chars = data.chars.unwrap()
            ));
        }
        let prefix = normalized(&data.first).chars().take(200).collect::<String>();
        let count = repeated.get(&(session.path.clone(), prefix)).copied().unwrap_or(0);
        if options.rules.contains("repeated") && count >= options.min_similar {
            reasons.push(tr!(
                "Similar opening in {n} inactive conversations in the same folder; not necessarily duplicates",
                n = count
            ));
        }
        if options.rules.contains("old") {
            reasons.push(tr!("Older than the selected inactivity threshold"));
        }
        if let Some(query) = &options.contains
            && !normalized(&format!("{} {}", session.title, data.first)).contains(&normalized(query))
        {
            continue;
        }
        if reasons.is_empty() {
            continue;
        }
        if revision(&session)? != observed_revision {
            continue;
        }
        let fingerprint = observed_revision;
        let bytes = session
            .src
            .as_ref()
            .and_then(|p| p.metadata().ok())
            .map_or(0, |m| m.len());
        candidates.push(Candidate {
            session,
            reasons,
            fingerprint,
            preview: data.text,
            bytes,
        });
    }
    candidates.sort_by(|a, b| {
        b.reasons
            .len()
            .cmp(&a.reasons.len())
            .then(a.session.updated.total_cmp(&b.session.updated))
            .then(a.session.key().cmp(&b.session.key()))
    });
    ui::progress_clear();
    ui::say(&tr!("Protected or recent conversations skipped: {n}", n = skipped));
    for error in inspection::errors() {
        ui::warn(&error);
    }
    if !inspection::errors().is_empty() {
        return Err(Error(inspection::errors().join("; ")));
    }
    Ok(candidates)
}
/// Full content checks are paid only for explicitly selected conversations.
pub(crate) fn seal(candidate: &mut Candidate) -> Result<()> {
    let _inspect = inspection::Guard::new();
    if revision(&candidate.session)? != candidate.fingerprint {
        return Err(Error(tr!("Conversation changed since selection; cleanup deferred")));
    }
    // Re-read the selected history classification without trusting the discovery cache.
    let _ = summary_fresh(&candidate.session)?;
    candidate.fingerprint = fingerprint(&candidate.session)?;
    Ok(())
}
struct Inspection;
impl Drop for Inspection {
    fn drop(&mut self) {
        inspection::finish();
    }
}

fn choose(header: &str, values: &[(&str, String)], current: &str) -> Option<String> {
    let rows: Vec<_> = values
        .iter()
        .map(|(key, line)| Row {
            key: key.to_string(),
            line: line.clone(),
            preview: String::new(),
        })
        .collect();
    ui::picker(&rows, header, false, false, &[], &[current.into()]).and_then(|p| p.keys.into_iter().next())
}
fn wizard(options: &mut Options) -> Option<()> {
    let rows: Vec<_> = [
        ("agents", tr!("Subagents and agent-role prompts")),
        ("repeated", tr!("Repeated opening prompts")),
        ("tests", tr!("Connection tests and reply-only prompts")),
        ("short", tr!("Very short conversations")),
        ("old", tr!("Any conversation older than the threshold")),
    ]
    .into_iter()
    .map(|(key, line)| Row {
        key: key.into(),
        line,
        preview: String::new(),
    })
    .collect();
    let selected = options.rules.iter().cloned().collect::<Vec<_>>();
    options.rules = ui::checkbox_picker(
        &rows,
        &tr!("1. Which kinds of conversations should clean suggest? Enter toggles · Next continues"),
        &["enter"],
        &selected,
        false,
    )?
    .keys
    .into_iter()
    .collect();
    let days = [0, 1, 7, 30, 90, 180, 365];
    let keys: Vec<_> = days.iter().map(ToString::to_string).collect();
    let values: Vec<_> = keys
        .iter()
        .map(|n| (n.as_str(), tr!("{n} days of inactivity", n = n)))
        .collect();
    options.days = choose(
        &tr!("2. Minimum inactivity? Open and recently active conversations are always protected"),
        &values,
        &options.days.to_string(),
    )?
    .parse()
    .ok()?;
    let rows: Vec<_> = harness::CONVERTS
        .iter()
        .map(|n| Row {
            key: n.to_string(),
            line: harness::get(n).label().into(),
            preview: String::new(),
        })
        .collect();
    let tools = ui::checkbox_picker(
        &rows,
        &tr!("3. Which harnesses? Enter toggles · Next continues"),
        &["enter"],
        &options.harnesses.iter().map(ToString::to_string).collect::<Vec<_>>(),
        false,
    )?;
    options.harnesses = tools.keys.iter().filter_map(|n| harness::known(n)).collect();
    if options.rules.contains("short") || options.rules.contains("tests") {
        let values: Vec<_> = ["2", "4", "8", "12", "20"]
            .into_iter()
            .map(|n| (n, tr!("At most {n} messages", n = n)))
            .collect();
        options.max_messages = choose(
            &tr!("4. How short should the conversation be?"),
            &values,
            &options.max_messages.to_string(),
        )?
        .parse()
        .ok()?;
        let values: Vec<_> = ["200", "500", "1000", "2000"]
            .into_iter()
            .map(|n| (n, tr!("At most {n} characters", n = n)))
            .collect();
        options.max_chars = choose(
            &tr!("5. Maximum conversation text length?"),
            &values,
            &options.max_chars.to_string(),
        )?
        .parse()
        .ok()?;
    }
    if options.rules.contains("repeated") {
        let values: Vec<_> = ["2", "3", "5", "10"]
            .into_iter()
            .map(|n| (n, tr!("At least {n} similar conversations", n = n)))
            .collect();
        options.min_similar = choose(
            &tr!("6. How many similar openings should form a group?"),
            &values,
            &options.min_similar.to_string(),
        )?
        .parse()
        .ok()?;
    }
    let query = ui::ask(
        &tr!("Optional text to match in the title or opening prompt (Enter keeps the current filter):"),
        false,
    );
    if !query.trim().is_empty() {
        options.contains = Some(query);
    }
    Some(())
}

pub fn run(mut options: Options) {
    if options.advanced {
        if !ui::interactive() && ctx().pick.is_none() {
            ui::die(&tr!(
                "Advanced clean needs an interactive terminal; use clean flags for a scripted preview"
            ));
        }
        if wizard(&mut options).is_none() {
            return;
        }
    }
    if options.days > 36500 {
        ui::die(&tr!("Inactivity threshold is too large"));
    }
    ui::say(&tr!(
        "Clean suggests local conversations only; shared, linked and open conversations are protected."
    ));
    let scanned = {
        let _loading = crate::ui::loading(&crate::tr!("Reviewing local conversations"));
        scan(&options)
    };
    let mut candidates = match scanned {
        Ok(c) => c,
        Err(error) => {
            ui::fail(&error.to_string());
            return;
        }
    };
    if candidates.is_empty() {
        ui::say(&tr!("No cleanup candidates match these criteria"));
        return;
    }
    ui::say(&tr!(
        "Found {n} candidates; nothing has been removed",
        n = candidates.len()
    ));
    if options.dry_run || (!ui::interactive() && ctx().pick.is_none()) {
        for c in &candidates {
            println!(
                "{} · {} · {} · {}\n  {}",
                c.session.key(),
                c.session.title,
                c.session.path,
                ui::ago(c.session.updated),
                c.reasons.join(" · ")
            );
        }
        return;
    }
    let rows: Vec<_> = candidates
        .iter()
        .map(|c| Row {
            key: c.session.key(),
            line: format!(
                "{} {}  {}  {}  {}",
                ui::harness_tag(c.session.harness),
                ui::ago(c.session.updated),
                ui::human(c.bytes),
                c.session.title,
                c.reasons.join(" · ")
            ),
            preview: format!(
                "{}\n{}\n{}\n\n{}\n\n{}",
                c.session.title,
                c.session.path,
                c.session.id,
                c.reasons.join("\n"),
                c.preview
            ),
        })
        .collect();
    let keys = ui::checkbox_picker(
        &rows,
        &tr!("Review cleanup candidates · Enter toggles · Next continues · Esc cancels"),
        &["enter"],
        &[],
        true,
    )
    .map(|selection| selection.keys)
    .unwrap_or_default();
    for candidate in candidates.iter_mut().filter(|c| keys.contains(&c.session.key())) {
        if let Err(e) = seal(candidate) {
            ui::warn(&e.to_string());
            return;
        }
    }
    let selected: Vec<_> = candidates.iter().filter(|c| keys.contains(&c.session.key())).collect();
    if selected.is_empty()
        || !ui::confirm(&tr!(
            "Move {n} selected local conversations to compressed quarantine? Shared copies are kept.",
            n = selected.len()
        ))
    {
        return;
    }
    if let Err(error) = crate::quarantine::apply(&selected) {
        ui::fail(&error.to_string());
    }
}

pub fn command() -> clap::Command {
    use clap::{Arg, ArgAction, Command};
    let flags = |command: Command| {
        command
            .arg(
                Arg::new("dry-run")
                    .long("dry-run")
                    .action(ArgAction::SetTrue)
                    .help(tr!("Show cleanup candidates without moving anything")),
            )
            .arg(
                Arg::new("older-than")
                    .long("older-than")
                    .value_name("DAYS")
                    .value_parser(clap::value_parser!(u64))
                    .help(tr!("Minimum inactivity in days (default: 7)")),
            )
            .arg(
                Arg::new("max-messages")
                    .long("max-messages")
                    .value_parser(clap::value_parser!(usize))
                    .help(tr!("Maximum messages in a short conversation")),
            )
            .arg(
                Arg::new("max-chars")
                    .long("max-chars")
                    .value_parser(clap::value_parser!(usize))
                    .help(tr!("Maximum characters in a short conversation")),
            )
            .arg(
                Arg::new("min-similar")
                    .long("min-similar")
                    .value_parser(clap::value_parser!(usize))
                    .help(tr!("Minimum similar openings to form a group")),
            )
            .arg(
                Arg::new("harnesses")
                    .long("harnesses")
                    .value_delimiter(',')
                    .value_parser(harness::CONVERTS)
                    .help(tr!("Only these harnesses")),
            )
            .arg(
                Arg::new("only")
                    .long("only")
                    .value_delimiter(',')
                    .value_parser(["agents", "repeated", "tests", "short", "old"])
                    .help(tr!("Suggestion categories; old enables age as the only reason")),
            )
            .arg(
                Arg::new("folder")
                    .long("folder")
                    .value_parser(clap::value_parser!(PathBuf))
                    .help(tr!("Only conversations whose work folder is inside this path")),
            )
            .arg(
                Arg::new("contains")
                    .long("contains")
                    .help(tr!("Literal text required in the title or opening prompt")),
            )
    };
    flags(Command::new("clean").about(tr!(
        "Review unused local conversations and move selected ones to quarantine"
    )))
    .args_conflicts_with_subcommands(true)
    .subcommand(flags(
        Command::new("advanced").about(tr!("Answer questions to refine cleanup suggestions")),
    ))
    .subcommand(Command::new("quarantine").about(tr!("List recoverable quarantined conversations")))
    .subcommand(
        Command::new("restore")
            .about(tr!("Recover a quarantined conversation without overwriting anything"))
            .arg(Arg::new("id")),
    )
    .subcommand(
        Command::new("purge")
            .about(tr!("Permanently delete quarantined conversations"))
            .arg(Arg::new("id").help(tr!("quarantine id; if omitted, choose from a list"))),
    )
}
pub fn dispatch(matches: &clap::ArgMatches) {
    if let Some(("quarantine", _)) = matches.subcommand() {
        crate::quarantine::list();
        return;
    }
    if let Some(("restore", m)) = matches.subcommand() {
        crate::quarantine::restore(m.get_one::<String>("id").map(String::as_str));
        return;
    }
    if let Some(("purge", m)) = matches.subcommand() {
        crate::quarantine::purge(m.get_one::<String>("id").map(String::as_str));
        return;
    }
    let advanced = matches.subcommand_name() == Some("advanced");
    let m = if advanced {
        matches.subcommand_matches("advanced").unwrap()
    } else {
        matches
    };
    let mut options = Options {
        advanced,
        dry_run: m.get_flag("dry-run"),
        ..Default::default()
    };
    if let Some(n) = m.get_one::<u64>("older-than") {
        options.days = *n;
    }
    if let Some(n) = m.get_one::<usize>("max-messages") {
        options.max_messages = *n;
    }
    if let Some(n) = m.get_one::<usize>("max-chars") {
        options.max_chars = *n;
    }
    if let Some(n) = m.get_one::<usize>("min-similar") {
        options.min_similar = *n;
    }
    if let Some(tools) = m.get_many::<String>("harnesses") {
        options.harnesses = tools.filter_map(|n| harness::known(n)).collect();
    }
    if let Some(rules) = m.get_many::<String>("only") {
        options.rules = rules.cloned().collect();
    }
    options.folder = m.get_one::<PathBuf>("folder").cloned();
    options.contains = m.get_one::<String>("contains").cloned();
    if options.min_similar < 2 || options.max_messages == 0 || options.max_chars == 0 {
        ui::die(&tr!(
            "Use at least 2 similar conversations and positive message and character limits"
        ));
    }
    run(options);
}
