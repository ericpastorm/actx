//! Local linked copies. The shared native session remains the canonical identity across PCs.
use crate::{
    agents,
    ctx::{Error, Result, ctx},
    harness, profile,
    session::Session,
    state, store,
    turns::{self, Turn},
    ui,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path};

type History = (Vec<Turn>, Option<String>);
#[derive(Default)]
struct Histories(HashMap<String, std::rc::Rc<History>>);
impl Histories {
    fn get(&mut self, session: &Session) -> Result<std::rc::Rc<History>> {
        let key = session.key();
        if let Some(history) = self.0.get(&key) {
            return Ok(history.clone());
        }
        let history = std::rc::Rc::new(harness::get(session.harness).local_turns(session)?);
        self.0.insert(key, history.clone());
        Ok(history)
    }

    fn invalidate(&mut self, session: &Session) {
        self.0.remove(&session.key());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_last_replies_are_continuations_but_edits_remain_conflicts() {
        let turn = |who, text: &str| Turn {
            who,
            text: text.into(),
            ts: 1,
        };
        let before = vec![turn("user", "question"), turn("assistant", "答复: first")];
        let mut after = before.clone();
        after[1].text.push_str("\n\nmore details");
        let base = hashes(&before);
        let delta = continuation(&base, &after).unwrap();
        assert_eq!(delta, vec![turn("assistant", "more details")]);
        assert!(complete_delta(&delta));
        after.extend([turn("user", "next"), turn("assistant", "answer")]);
        assert_eq!(continuation(&base, &after).unwrap().len(), 3);
        after[0].text = "edited question".into();
        assert!(continuation(&base, &after).is_none());
        after[0] = before[0].clone();
        after[1].text = "答复: OTHER\n\nmore details".into();
        assert!(continuation(&base, &after).is_none());
        assert!(delta_change(Some(&[1]), Some(&[2])) == LinkChange::Diverged);
        assert!(delta_change(Some(&[1]), Some(&[1])) == LinkChange::Baselines);
        assert!(!complete_delta(&[turn("user", "unfinished")]));
    }
    #[test]
    fn browser_cache_is_invalidated_by_native_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let line =
            |text: &str| format!("{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"{text}\"}}}}\n");
        std::fs::write(&path, line("before")).unwrap();
        let mut s = Session::local("claude", "one".into(), String::new(), String::new(), 0.0, 0);
        s.src = Some(path.clone());
        let mut cache = ViewCache::default();
        let mut dirty = false;
        let first = cache.get(&s, &mut dirty).unwrap();
        assert!(dirty);
        dirty = false;
        assert_eq!(cache.get(&s, &mut dirty).unwrap(), first);
        assert!(!dirty);
        std::fs::write(&path, line("edited")).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
            .unwrap();
        assert_ne!(cache.get(&s, &mut dirty).unwrap(), first);
        assert!(dirty);
        std::fs::remove_file(path).unwrap();
        assert!(cache.get(&s, &mut dirty).is_none());
    }

    #[test]
    fn histories_reuse_reads_but_refresh_after_a_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let transcript =
            |text: &str| format!("{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"{text}\"}}}}\n");
        std::fs::write(&path, transcript("before")).unwrap();
        let mut session = Session::local("claude", "cache-test".into(), String::new(), String::new(), 0.0, 0);
        session.src = Some(path.clone());
        let mut histories = Histories::default();
        let first = histories.get(&session).unwrap();
        assert_eq!(first.0[0].text, "before");
        assert!(std::rc::Rc::ptr_eq(&first, &histories.get(&session).unwrap()));
        std::fs::write(&path, transcript("after")).unwrap();
        histories.invalidate(&session);
        let updated = histories.get(&session).unwrap();
        assert_eq!(updated.0[0].text, "after");
        assert!(!std::rc::Rc::ptr_eq(&first, &updated));
    }

    #[test]
    fn compaction_repairs_only_unchanged_mirrors_and_only_once_per_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"compacted\",\"payload\":{\"message\":\"summary\"}}\n",
        )
        .unwrap();
        let mut source = Session::local("codex", "one".into(), String::new(), String::new(), 0., 0);
        source.src = Some(path);
        let mut link = Link {
            source: source.key(),
            target: "claude:two".into(),
            a: vec![1],
            b: vec![3],
            epoch: None,
        };
        assert!(change(&link, &source, &[2], &[3]) == LinkChange::Refresh);
        assert!(change(&link, &source, &[2], &[4]) == LinkChange::Edited);
        link.epoch = epoch(&source);
        assert!(change(&link, &source, &[2], &[3]) == LinkChange::Edited);
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Links {
    #[serde(default)]
    copies: HashMap<String, Link>,
    #[serde(default)]
    retired: Vec<String>,
}
#[derive(Serialize, Deserialize)]
struct Link {
    source: String,
    target: String,
    a: Vec<u64>,
    b: Vec<u64>,
    #[serde(default)]
    epoch: Option<String>,
}
#[derive(PartialEq, Eq)]
enum LinkChange {
    Unchanged,
    Edited,
    Diverged,
    ToTarget,
    ToSource,
    Baselines,
    Refresh,
}
fn epoch(source: &Session) -> Option<String> {
    if source.harness != "codex" {
        return None;
    }
    crate::codex::compaction_key(source.src.as_ref()?).ok().flatten()
}
#[cfg(test)]
fn change(link: &Link, source: &Session, a: &[u64], b: &[u64]) -> LinkChange {
    let change = link_change(link, a, b);
    if change == LinkChange::Edited
        && b == link.b
        && let Some(epoch) = epoch(source)
        && Some(&epoch) != link.epoch.as_ref()
    {
        return LinkChange::Refresh;
    }
    change
}
#[cfg(test)]
fn link_change(link: &Link, a: &[u64], b: &[u64]) -> LinkChange {
    if a == link.a && b == link.b {
        return LinkChange::Unchanged;
    }
    if !a.starts_with(&link.a) || !b.starts_with(&link.b) {
        return LinkChange::Edited;
    }
    let da = &a[link.a.len()..];
    let db = &b[link.b.len()..];
    match (da.is_empty(), db.is_empty()) {
        (false, false) if da != db => LinkChange::Diverged,
        (false, true) => LinkChange::ToTarget,
        (true, false) => LinkChange::ToSource,
        _ => LinkChange::Baselines,
    }
}

/// Return only the new text/turns. A resumed harness can append to the last
/// assistant message; the saved hash must match an exact UTF-8 prefix to accept it.
fn continuation(base: &[u64], history: &[Turn]) -> Option<Vec<Turn>> {
    let current = hashes(history);
    if current.starts_with(base) {
        return Some(history[base.len()..].to_vec());
    }
    let last = base.len().checked_sub(1)?;
    if current.len() < base.len() || current[..last] != base[..last] || history[last].who != "assistant" {
        return None;
    }
    let turn = &history[last];
    let mut hash = turn
        .who
        .bytes()
        .chain([0])
        .fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    for (i, b) in turn.text.bytes().enumerate() {
        hash = (hash ^ b as u64).wrapping_mul(0x100000001b3);
        let end = i + 1;
        if hash == base[last] && turn.text.is_char_boundary(end) {
            let mut delta = vec![];
            let text = turn.text[end..].trim();
            if !text.is_empty() {
                delta.push(Turn {
                    text: text.into(),
                    ..turn.clone()
                });
            }
            delta.extend_from_slice(&history[base.len()..]);
            return Some(delta);
        }
    }
    None
}
fn delta_change(a: Option<&[u64]>, b: Option<&[u64]>) -> LinkChange {
    let (Some(a), Some(b)) = (a, b) else {
        return LinkChange::Edited;
    };
    match (a.is_empty(), b.is_empty()) {
        (true, true) => LinkChange::Unchanged,
        (false, true) => LinkChange::ToTarget,
        (true, false) => LinkChange::ToSource,
        (false, false) if a == b => LinkChange::Baselines,
        _ => LinkChange::Diverged,
    }
}
fn history_change(link: &Link, source: &Session, a: &[Turn], b: &[Turn]) -> LinkChange {
    let da = continuation(&link.a, a).map(|t| hashes(&t));
    let db = continuation(&link.b, b).map(|t| hashes(&t));
    let change = delta_change(da.as_deref(), db.as_deref());
    if change == LinkChange::Edited
        && hashes(b) == link.b
        && epoch(source).is_some_and(|e| Some(&e) != link.epoch.as_ref())
    {
        LinkChange::Refresh
    } else {
        change
    }
}
fn complete_delta(delta: &[Turn]) -> bool {
    // An exact extension of a saved assistant reply needs no invented user turn.
    let pairs = if delta.first().is_some_and(|t| t.who == "assistant") {
        &delta[1..]
    } else {
        delta
    };
    pairs.len().is_multiple_of(2) && pairs.chunks(2).all(|p| p[0].who == "user" && p[1].who == "assistant")
}

fn file() -> std::path::PathBuf {
    ctx().state_dir.join("harness-links.json")
}
fn read() -> Result<Links> {
    match std::fs::read(file()) {
        Ok(data) => Ok(serde_json::from_slice(&data)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Links::default()),
        Err(e) => Err(e.into()),
    }
}
fn save(links: &Links) -> Result<()> {
    let tmp = ctx().state_dir.join(format!("links-{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(links)?)?;
    std::fs::rename(tmp, file())?;
    Ok(())
}
fn hashes(turns: &[Turn]) -> Vec<u64> {
    turns
        .iter()
        .map(|t| {
            t.who
                .bytes()
                .chain([0])
                .chain(t.text.bytes())
                .fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
        })
        .collect()
}
fn local() -> HashMap<String, Session> {
    harness::CONVERTS
        .iter()
        .flat_map(|n| harness::get(n).local())
        .map(|s| (s.key(), s))
        .collect()
}
pub(crate) fn pair(key: &str, target: &str) -> Result<(Session, Session)> {
    let links = read()?;
    let source = crate::catalog::identity(key);
    let link = links
        .copies
        .get(&format!("{source}/{target}"))
        .ok_or_else(|| Error("Linked copy not found".into()))?;
    let mut copies: HashMap<_, _> = harness::local_keys(&std::collections::HashSet::from([
        link.source.clone(),
        link.target.clone(),
    ]))
    .into_iter()
    .map(|s| (s.key(), s))
    .collect();
    let a = copies
        .remove(&link.source)
        .ok_or_else(|| Error("Source copy missing".into()))?;
    let b = copies
        .remove(&link.target)
        .ok_or_else(|| Error("Linked copy missing".into()))?;
    Ok((a, b))
}
/// Explicit repair/merge keeps both original native histories. The caller holds action/store locks.
pub(crate) fn resolve_pair(
    root: &Path,
    source: &Session,
    target: &Session,
    choice: &str,
    st: &mut state::State,
) -> Result<()> {
    let guard = std::fs::File::create(ctx().state_dir.join("multiharness.lock"))?;
    guard.try_lock().map_err(|e| Error(e.to_string()))?;
    let mut links = read()?;
    let id = format!("{}/{}", source.key(), target.harness);
    let link = links
        .copies
        .get(&id)
        .ok_or_else(|| Error("Linked copy missing".into()))?;
    if link.target != target.key() {
        return Err(Error("Linked copy changed while confirming".into()));
    }
    if agents::in_use_by_agent_now(source) || agents::in_use_by_agent_now(target) {
        return Err(Error("Close both agents before repairing".into()));
    }
    let mut source = source.clone();
    let before = harness::get(target.harness).local_turns(target)?.0;
    let b = hashes(&before);
    if choice == "repair" && b != link.b {
        return Err(Error(
            "The linked copy has its own changes; choose merge or keep local instead".into(),
        ));
    }
    if source.harness == "codex"
        && source
            .src
            .as_ref()
            .is_some_and(|p| crate::codex::completed_end(p).ok().flatten().is_some())
    {
        return Err(Error("Finish the current source turn before repairing".into()));
    }
    if let Some(remote) = store::stored_sessions(root)
        .into_iter()
        .find(|r| r.key() == source.key())
        && matches!(st.status(Some(&source), Some(&remote)), "pull" | "conflict" | "unknown")
    {
        return Err(Error(
            "Sync or resolve the native source before repairing its linked copies".into(),
        ));
    }
    if choice == "merge" {
        if !b.starts_with(&link.b) {
            return Err(Error(
                "Edited target history cannot be merged automatically; both copies were kept".into(),
            ));
        }
        let delta = &before[link.b.len()..];
        if !delta.is_empty() {
            append(&mut source, delta)?;
        }
    }
    let (turns, model) = harness::get(source.harness).local_turns(&source)?;
    let content = crate::integrity::fresh(&source)?;
    let copy = harness::get(target.harness).write(
        &turns::fit(turns.clone(), harness::get(source.harness).label(), model.as_deref()),
        &source.path,
        &source.title,
        model.as_deref(),
    )?;
    if crate::integrity::fresh(&source)? != content {
        return Err(Error(
            "Source changed during repair; all native copies were kept".into(),
        ));
    }
    crate::catalog::detach(&source.key(), &target.key())?;
    links.retired.push(target.key());
    links.copies.insert(
        id,
        Link {
            source: source.key(),
            target: copy.key(),
            a: hashes(&turns),
            b: hashes(&harness::get(copy.harness).local_turns(&copy)?.0),
            epoch: epoch(&source),
        },
    );
    save(&links)?;
    if let Some(remote) = store::stored_sessions(root)
        .into_iter()
        .find(|r| r.key() == source.key())
    {
        store::push_one(
            root,
            remote.project.as_deref().unwrap_or(store::GENERAL),
            &source,
            crate::git::git_info(&source.path),
            st,
            &HashMap::new(),
        )?;
    }
    Ok(())
}
pub fn retired_keys() -> Vec<String> {
    read().map(|l| l.retired).unwrap_or_default()
}
pub fn linked_keys() -> Vec<String> {
    read()
        .map(|l| l.copies.into_values().map(|l| l.target).collect())
        .unwrap_or_default()
}
/// Every conversation named by a linked copy: both the source and its targets.
pub fn linked_family() -> Vec<String> {
    read()
        .map(|l| l.copies.into_values().flat_map(|l| [l.source, l.target]).collect())
        .unwrap_or_default()
}
pub fn extend_keys(keys: &mut std::collections::HashSet<String>) {
    let Ok(links) = read() else { return };
    loop {
        let before = keys.len();
        for link in links.copies.values() {
            if keys.contains(&link.source) || keys.contains(&link.target) {
                keys.insert(link.source.clone());
                keys.insert(link.target.clone());
            }
        }
        if keys.len() == before {
            break;
        }
    }
}
fn append(s: &mut Session, delta: &[Turn]) -> Result<()> {
    // Never manufacture replies or silently flatten an incomplete turn.
    if !complete_delta(delta) {
        return Err(Error(
            "Incomplete continuation: close the agent turn before syncing harnesses".into(),
        ));
    }
    if agents::in_use(s) || agents::in_use_by_agent_now(s) {
        return Err(Error("Conversation is open; linked update deferred".into()));
    }
    if let Some(src) = &s.src {
        // Antigravity is SQLite: use its online backup instead of copying a live WAL database.
        if s.harness == "antigravity" {
            return crate::antigravity::append_turns(s, delta);
        }
        state::backup(s, state::Copy::File(src), "multiharness")?;
    } else {
        let data = crate::opencode::export(&s.id, &s.path)?;
        state::backup(s, state::Copy::Bytes(&data), "multiharness")?;
    }
    let pairs = if let Some(first) = delta.first().filter(|t| t.who == "assistant") {
        match s.harness {
            "claude" => crate::claude::append_reply(s, &first.text)?,
            "codex" => crate::codex::append_reply(s, &first.text)?,
            "opencode" => crate::opencode::append_reply(s, &first.text)?,
            _ => return Err(Error("Unsupported reply continuation".into())),
        }
        &delta[1..]
    } else {
        delta
    };
    for pair in pairs.chunks(2) {
        harness::get(s.harness).append(s, &pair[0].text, &pair[1].text)?;
    }
    Ok(())
}
fn scoped(s: &Session, args: &crate::commands::Args) -> bool {
    args.keys.as_ref().is_none_or(|keys| keys.contains(&s.key()))
        && (!args.general || s.is_general())
        && args
            .project
            .as_ref()
            .is_none_or(|p| s.project.as_ref().is_some_and(|n| n.eq_ignore_ascii_case(p)))
}

pub fn run(root: &Path, args: &crate::commands::Args) {
    let config = profile::current();
    let enabled = config
        .get("harnesses")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|v| !v.is_empty())
        || config
            .get("conversation_profiles")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|profiles| profiles.values().any(|v| v.as_array().is_some_and(|v| !v.is_empty())));
    if !enabled {
        return;
    }
    if let Err(e) = run_inner(root, args, &config) {
        ui::warn(&format!("Multiharness: {e}"));
    }
}
fn run_inner(root: &Path, args: &crate::commands::Args, config: &serde_json::Value) -> Result<()> {
    std::fs::create_dir_all(&ctx().state_dir)?;
    let guard = std::fs::File::create(ctx().state_dir.join("multiharness.lock"))?;
    let _loading = ui::loading(&crate::tr!("Syncing linked harnesses"));
    guard.lock()?;
    let Some(_shared) = crate::commands::lock_store(root) else {
        return Ok(());
    };
    run_locked(root, args, config)
}

/// Noninteractive entry point; lock order matches the CLI and never waits on another frontend.
pub(crate) fn run_service(root: &Path, args: &crate::commands::Args) -> Result<()> {
    std::fs::create_dir_all(&ctx().state_dir)?;
    let guard = std::fs::File::create(ctx().state_dir.join("multiharness.lock"))?;
    guard.try_lock().map_err(|e| Error(e.to_string()))?;
    let _shared = crate::actions::acquire(root).map_err(|d| Error(d.message))?;
    run_locked(root, args, &profile::current())
}
fn run_locked(root: &Path, args: &crate::commands::Args, config: &serde_json::Value) -> Result<()> {
    let mut links = read()?;
    let mut st = state::load();
    let stored = store::stored_sessions(root);
    let needed: std::collections::HashSet<_> = stored
        .iter()
        .filter(|r| scoped(r, args))
        .flat_map(|r| {
            let targets: Vec<_> = profile::harnesses_in(config, &r.key())
                .into_iter()
                .filter(|n| *n != r.harness && args.target.as_deref().is_none_or(|target| target == *n))
                .collect();
            if targets.is_empty() {
                Vec::new()
            } else {
                std::iter::once(r.harness).chain(targets).collect()
            }
        })
        .collect();
    if needed.is_empty() {
        return Ok(());
    }
    let aliases = crate::catalog::aliases();
    let mut keys: std::collections::HashSet<_> = stored
        .iter()
        .filter(|r| scoped(r, args))
        .flat_map(|r| {
            [
                r.key(),
                r.meta()
                    .get("conversation_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ]
        })
        .collect();
    extend_keys(&mut keys);
    let converted: Vec<_> = aliases
        .keys()
        .filter(|key| keys.contains(&crate::catalog::root(&aliases, key)))
        .cloned()
        .collect();
    keys.extend(converted);
    let mut sessions: HashMap<_, _> = harness::local_keys(&keys)
        .into_iter()
        .filter(|s| needed.contains(s.harness))
        .map(|s| (s.key(), s))
        .collect();
    let family_ids: HashMap<_, _> = stored
        .iter()
        .map(|r| {
            (
                r.key(),
                r.meta()
                    .get("conversation_id")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from)
                    .unwrap_or_else(|| crate::catalog::root(&aliases, &r.key())),
            )
        })
        .collect();
    for r in stored.into_iter().filter(|r| scoped(r, args)) {
        let key = r.key();
        let family = family_ids.get(&key).cloned().unwrap_or_else(|| key.clone());
        if family != key && family_ids.contains_key(&family) {
            continue;
        }
        let targets: Vec<_> = profile::harnesses_in(config, &key)
            .into_iter()
            .filter(|n| *n != r.harness && args.target.as_deref().is_none_or(|target| target == *n))
            .collect();
        if targets.is_empty() {
            continue;
        }

        let Some(mut source) = sessions.get(&key).cloned() else {
            continue;
        };
        if r.meta().get("project_folder").is_some() {
            let Some(folder) = crate::commands::session_folder(&r, &mut st, !args.auto) else {
                continue;
            };
            source.path = folder;
        }
        if matches!(st.status(Some(&source), Some(&r)), "pull" | "conflict") {
            continue;
        }
        if agents::in_use(&source) || agents::in_use_by_agent_now(&source) {
            if !args.auto {
                ui::warn(&crate::tr!(
                    "{title}: linked copies deferred; close the source agent and retry (recent activity may take up to 2 minutes)",
                    title = source.title
                ));
            }
            continue;
        }
        if source.path.is_empty() || !Path::new(&source.path).is_dir() {
            ui::warn(&format!(
                "{}: choose an existing work folder before creating linked copies",
                source.title
            ));
            continue;
        }
        let original_weight = source.weight;
        if source.harness == "codex"
            && source
                .src
                .as_ref()
                .is_some_and(|p| crate::codex::completed_end(p).ok().flatten().is_some())
        {
            if !args.auto {
                ui::warn("Source turn is still in progress; linked copies deferred");
            }
            continue;
        }
        // Bound memory to one conversation family, even for a large shared library.
        let mut histories = Histories::default();
        // A late target can contribute new turns to the base. Visit the others again so they
        // receive that continuation in this run, rather than waiting for another agent hook.
        for _ in 0..2 {
            let mut source_changed = false;
            for target in targets.iter().copied() {
                let id = format!("{key}/{target}");
                let result = (|| -> Result<()> {
                    let _loading = ui::loading(&crate::tr!(
                        "Syncing {title} to {tool}",
                        title = source.title,
                        tool = target
                    ));
                    let history = histories.get(&source)?;
                    let (a_turns, model) = history.as_ref();
                    let a = hashes(a_turns);
                    if a.is_empty() {
                        return Ok(());
                    }
                    if let Some(link) = links.copies.get(&id) {
                        let Some(mut mirror) = sessions.get(&link.target).cloned() else {
                            ui::warn(&format!(
                                "{}: linked {target} copy missing; restore the copy or disable its profile",
                                source.title
                            ));
                            return Ok(());
                        };
                        let mirror_history = histories.get(&mirror)?;
                        let (b_turns, _) = mirror_history.as_ref();
                        let change = history_change(link, &source, a_turns, b_turns);
                        match change {
                            LinkChange::Refresh => {
                                if agents::in_use(&mirror) || agents::in_use_by_agent_now(&mirror) {
                                    return Err(Error(
                                        "Destination is active; compacted history repair deferred".into(),
                                    ));
                                }
                                let before = crate::integrity::fresh(&source)?;
                                let retired = mirror.key();
                                let copy = harness::get(target).write(
                                    &turns::fit(
                                        a_turns.clone(),
                                        harness::get(source.harness).label(),
                                        model.as_deref(),
                                    ),
                                    &source.path,
                                    &source.title,
                                    model.as_deref(),
                                )?;
                                if crate::integrity::fresh(&source)? != before {
                                    return Err(Error(
                                        "Source changed during repair; both native copies were kept".into(),
                                    ));
                                }
                                let b = hashes(&histories.get(&copy)?.0);
                                crate::catalog::detach(&key, &retired)?;
                                links.retired.push(retired);
                                links.copies.insert(
                                    id.clone(),
                                    Link {
                                        source: key.clone(),
                                        target: copy.key(),
                                        a,
                                        b,
                                        epoch: epoch(&source),
                                    },
                                );
                                sessions.insert(copy.key(), copy);
                                save(&links)?;
                                ui::ok(&crate::tr!(
                                    "Compacted history repaired; the previous native copy was preserved"
                                ));
                                return Ok(());
                            }
                            LinkChange::Unchanged => return Ok(()),
                            LinkChange::Edited => {
                                return Err(Error(
                                    "History edited or compacted; linked conflict left untouched".into(),
                                ));
                            }
                            LinkChange::Diverged => {
                                return Err(Error(
                                    "Continued in two harnesses: linked conflict left untouched".into(),
                                ));
                            }
                            _ => {}
                        }
                        if change == LinkChange::ToTarget {
                            histories.invalidate(&mirror);
                            append(
                                &mut mirror,
                                &continuation(&link.a, a_turns)
                                    .ok_or_else(|| Error("Source history changed".into()))?,
                            )?;
                        } else if change == LinkChange::ToSource {
                            if agents::in_use(&mirror)
                                || agents::in_use_by_agent_now(&mirror)
                                || mirror.harness == "codex"
                                    && mirror
                                        .src
                                        .as_ref()
                                        .is_some_and(|p| crate::codex::completed_end(p).ok().flatten().is_some())
                            {
                                return Err(Error("Source turn is still in progress; linked copies deferred".into()));
                            }
                            histories.invalidate(&source);
                            append(
                                &mut source,
                                &continuation(&link.b, b_turns)
                                    .ok_or_else(|| Error("Target history changed".into()))?,
                            )?;
                            source_changed = true;
                        }
                        let a = hashes(&histories.get(&source)?.0);
                        let b = hashes(&histories.get(&mirror)?.0);
                        links.copies.insert(
                            id.clone(),
                            Link {
                                source: key.clone(),
                                target: mirror.key(),
                                a,
                                b,
                                epoch: epoch(&source),
                            },
                        );
                        sessions.insert(mirror.key(), mirror);
                        ui::ok(&format!("{} ↔ {target}", source.title));
                    } else {
                        if !harness::get(target).installed() {
                            return Ok(());
                        }
                        let existing: Vec<_> = sessions
                            .values()
                            .filter(|s| {
                                s.harness == target
                                    && family_ids
                                        .get(&s.key())
                                        .cloned()
                                        .unwrap_or_else(|| crate::catalog::root(&aliases, &s.key()))
                                        == family
                            })
                            .cloned()
                            .collect();
                        if existing.len() > 1 {
                            return Err(Error(
                                "Several existing copies in this harness; choose one before linking".into(),
                            ));
                        }
                        if let Some(copy) = existing.into_iter().next() {
                            let copy_history = histories.get(&copy)?;
                            let b_turns = &copy_history.0;
                            // A conversion note differs by source harness; compare normalized text,
                            // but retain each exact baseline for detecting subsequent edits.
                            if hashes(&turns::fit(a_turns.clone(), "actx", None))
                                != hashes(&turns::fit(b_turns.clone(), "actx", None))
                            {
                                return Err(Error(
                                "Existing harness copy has different history; it was kept without creating a duplicate"
                                    .into(),
                            ));
                            }
                            links.copies.insert(
                                id.clone(),
                                Link {
                                    source: key.clone(),
                                    target: copy.key(),
                                    a,
                                    b: hashes(b_turns),
                                    epoch: epoch(&source),
                                },
                            );
                            save(&links)?;
                            ui::ok(&format!("{} ↔ {target} (existing copy linked)", source.title));
                            return Ok(());
                        }
                        let copy = harness::get(target).write(
                            &turns::fit(a_turns.clone(), harness::get(source.harness).label(), model.as_deref()),
                            &source.path,
                            &source.title,
                            model.as_deref(),
                        )?;
                        let b = hashes(&histories.get(&copy)?.0);
                        links.copies.insert(
                            id.clone(),
                            Link {
                                source: key.clone(),
                                target: copy.key(),
                                a,
                                b,
                                epoch: epoch(&source),
                            },
                        );
                        sessions.insert(copy.key(), copy);
                        ui::ok(&format!("{} → {target} (linked)", source.title));
                    }
                    save(&links)?;
                    Ok(())
                })();
                if let Err(e) = result {
                    ui::warn(&format!("{} → {target}: {e}", source.title));
                }
            }
            if !source_changed {
                break;
            }
        }
        if source.weight != original_weight {
            // A continuation imported from a linked harness must reach the shared base in
            // this run, even when the native planner initially saw everything as synced.
            let remote = HashMap::from([(key.clone(), r.clone())]);
            store::push_one(
                root,
                r.project.as_deref().unwrap_or(store::GENERAL),
                &source,
                crate::git::git_info(&source.path),
                &mut st,
                &remote,
            )?;
            state::save(&mut st)?;
        }
        sessions.insert(key, source);
    }
    Ok(())
}

pub fn prune(root: &Path, args: &crate::commands::Args) {
    let Some(days) = profile::days() else { return };
    let Some(_lock) = crate::commands::lock_store(root) else {
        return;
    };
    let local = local();
    let mut st = state::load();
    let cutoff = crate::ctx::now() - (days * 86400) as f64;
    let links = match read() {
        Ok(l) => l,
        Err(e) => {
            ui::warn(&e.to_string());
            return;
        }
    };
    for r in store::stored_sessions(root).into_iter().filter(|r| scoped(r, args)) {
        if r.updated <= 0.0 || r.updated >= cutoff {
            continue;
        }
        let key = r.key();
        if let Some(l) = local.get(&key)
            && (l.updated >= cutoff || agents::in_use(l) || !matches!(st.status(Some(l), Some(&r)), "synced"))
        {
            continue;
        }
        // A continuation in any local linked harness keeps the shared base alive.
        if links.copies.values().filter(|l| l.source == key).any(|l| {
            local.get(&l.target).is_some_and(|s| {
                agents::in_use_by_agent_now(s)
                    || harness::get(s.harness)
                        .local_turns(s)
                        .map(|t| hashes(&t.0) != l.b)
                        .unwrap_or(true)
            })
        }) {
            continue;
        }
        match store::remove_stored(&r) {
            Ok(()) => {
                st.base.remove(&key);
                ui::ok(&format!("Retention: {} (>{days} days; local originals kept)", r.title));
            }
            Err(e) => ui::warn(&e.to_string()),
        }
    }
    if let Err(e) = state::save(&mut st) {
        ui::warn(&e.to_string());
    }
}

/// Read-only UI projection. A missing or unreadable copy is never presented as up to date.
pub struct LinkView {
    pub source: String,
    pub target: String,
    pub state: &'static str,
}

#[derive(Serialize, Deserialize)]
struct CachedHashes {
    stamp: String,
    hashes: Vec<u64>,
    #[serde(default)]
    continuations: HashMap<String, Option<Vec<u64>>>,
}

// Only the browser uses this disposable cache. Sync still reads the native history before
// writing. File size + nanosecond mtime (and SQLite's WAL) invalidate edits of existing messages.
#[derive(Default, Serialize, Deserialize)]
struct ViewCache(HashMap<String, CachedHashes>);
impl ViewCache {
    fn stamp(s: &Session) -> Option<String> {
        let paths = match &s.src {
            Some(path) => vec![path.clone()],
            None if s.harness == "opencode" => vec![ctx().opencode_db.clone()],
            _ => return None,
        };
        let mut stamp = format!("{}:{}:{}", s.path, s.updated.to_bits(), s.weight);
        for path in paths {
            for suffix in ["", "-wal", "-journal"] {
                let mut name = path.as_os_str().to_os_string();
                name.push(suffix);
                let name = std::path::PathBuf::from(name);
                match std::fs::metadata(&name) {
                    Ok(meta) => {
                        let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
                        stamp.push_str(&format!(":{}:{}:{}", name.display(), meta.len(), modified.as_nanos()));
                    }
                    Err(e) if suffix.is_empty() || e.kind() != std::io::ErrorKind::NotFound => return None,
                    _ => stamp.push_str(":missing"),
                }
            }
        }
        Some(stamp)
    }
    fn get(&mut self, s: &Session, dirty: &mut bool) -> Option<Vec<u64>> {
        let stamp = Self::stamp(s)?;
        if let Some(entry) = self.0.get(&s.key()).filter(|entry| entry.stamp == stamp) {
            return Some(entry.hashes.clone());
        }
        let history = if s.harness == "opencode" {
            crate::opencode::read_turns(s)
        } else {
            harness::get(s.harness).local_turns(s)
        }
        .ok()?;
        if Self::stamp(s).as_ref() != Some(&stamp) {
            return None; // changed while reading: never cache or report a partial history as synced
        }
        let hashes = hashes(&history.0);
        self.0.insert(
            s.key(),
            CachedHashes {
                stamp,
                hashes: hashes.clone(),
                continuations: HashMap::new(),
            },
        );
        *dirty = true;
        Some(hashes)
    }
    fn delta(&mut self, s: &Session, base: &[u64], dirty: &mut bool) -> Option<Option<Vec<u64>>> {
        let current = self.get(s, dirty)?;
        if current.starts_with(base) {
            return Some(Some(current[base.len()..].to_vec()));
        }
        let key = crate::engine::digest(&serde_json::to_vec(base).ok()?);
        if let Some(delta) = self.0.get(&s.key())?.continuations.get(&key) {
            return Some(delta.clone());
        }
        let stamp = Self::stamp(s)?;
        let turns = harness::get(s.harness).local_turns(s).ok()?.0;
        if Self::stamp(s).as_ref() != Some(&stamp) {
            return None;
        }
        let delta = continuation(base, &turns).map(|t| hashes(&t));
        let entry = self.0.get_mut(&s.key())?;
        if entry.continuations.len() >= 8 {
            entry.continuations.clear();
        }
        entry.continuations.insert(key, delta.clone());
        *dirty = true;
        Some(delta)
    }
    fn save(&self, path: &Path) -> Result<()> {
        std::fs::create_dir_all(&ctx().cache_dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(&ctx().cache_dir)?;
        serde_json::to_writer(tmp.as_file_mut(), self)?;
        tmp.persist(path).map_err(|e| Error(e.to_string()))?;
        Ok(())
    }
}

pub fn views(local: &HashMap<String, Session>) -> Vec<LinkView> {
    let links = match read() {
        Ok(l) => l,
        Err(e) => {
            if crate::inspection::enabled() {
                crate::inspection::record_error(e.to_string());
            }
            ui::warn(&format!("Multiharness: {e}"));
            return Vec::new();
        }
    };
    let active = |l: &Link| {
        l.target
            .split_once(':')
            .is_some_and(|(tool, _)| profile::harnesses(&l.source).contains(&tool))
    };
    let cache_file = ctx().cache_dir.join("linked-history-v1.json");
    let mut disk: ViewCache = std::fs::read(&cache_file)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default();
    let mut dirty = false;
    let views = links
        .copies
        .into_values()
        .map(|l| {
            if !active(&l) {
                return LinkView {
                    source: l.source,
                    target: l.target,
                    state: "unmanaged",
                };
            }
            let a = local.get(&l.source).and_then(|s| disk.delta(s, &l.a, &mut dirty));
            let b = local.get(&l.target).and_then(|s| disk.delta(s, &l.b, &mut dirty));
            let state = match (a, b) {
                (_, None) => "missing",
                (None, Some(Some(b))) => {
                    if b.is_empty() {
                        "unknown"
                    } else {
                        "push"
                    }
                }
                (None, _) => "conflict",
                (Some(a), Some(b)) => match delta_change(a.as_deref(), b.as_deref()) {
                    LinkChange::Unchanged | LinkChange::Baselines => "synced",
                    LinkChange::ToTarget => "pull",
                    LinkChange::ToSource => "push",
                    _ if b.as_ref().is_some_and(Vec::is_empty)
                        && local
                            .get(&l.source)
                            .is_some_and(|s| epoch(s).is_some_and(|e| Some(&e) != l.epoch.as_ref())) =>
                    {
                        "pull"
                    }
                    _ => "conflict",
                },
            };
            LinkView {
                source: l.source,
                target: l.target,
                state,
            }
        })
        .collect();
    if dirty && !crate::inspection::enabled() {
        let _ = disk.save(&cache_file);
    }
    views
}

/// Inspect linked continuations and retention using the same baselines as the writer.
/// The caller enables inspection mode so SQLite/CLI adapters cannot mutate agent data.
pub fn preview(
    stored: &[Session],
    sessions: &HashMap<String, Session>,
    st: &state::State,
    args: &crate::commands::Args,
) -> Vec<crate::sync_plan::Operation> {
    use crate::{sync_plan::Operation, tr};
    let mut operations = Vec::new();
    let links = match read() {
        Ok(links) => links,
        Err(error) => {
            crate::inspection::record_error(error.to_string());
            if let Some(r) = stored.first() {
                operations.push(Operation::new(
                    "check",
                    r,
                    "linked_state_unreadable",
                    tr!("Cannot read linked state: {error}", error = error),
                ));
            }
            return operations;
        }
    };
    let config = profile::current();
    let aliases = crate::catalog::aliases();
    let families: HashMap<_, _> = stored
        .iter()
        .map(|r| {
            (
                r.key(),
                r.meta()
                    .get("conversation_id")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from)
                    .unwrap_or_else(|| crate::catalog::root(&aliases, &r.key())),
            )
        })
        .collect();
    let mut changing = std::collections::HashSet::new();
    for r in stored {
        let key = r.key();
        let family = &families[&key];
        if family != &key && families.contains_key(family) {
            continue;
        }
        let targets: Vec<_> = profile::harnesses_in(&config, &key)
            .into_iter()
            .filter(|n| *n != r.harness && args.target.as_deref().is_none_or(|target| target == *n))
            .collect();
        let mut source = sessions.get(&key).cloned();
        if r.meta().get("project_folder").is_some()
            && let Some(source) = source.as_mut()
        {
            source.path = crate::commands::session_folder(r, &mut st.clone(), false).unwrap_or_default();
        }
        let source = source.as_ref();
        let mut histories = Histories::default();
        let family_start = operations.len();
        for target in targets {
            let mut op = Operation::new("check", r, "recheck_after_sync", String::new());
            op.harness = format!("{} ↔ {target}", r.harness);
            op.target_harness = Some(target.to_string());
            op.conversation_id = family.clone();
            op.destination = crate::commands::session_folder(r, &mut st.clone(), false)
                .or_else(|| source.map(|s| s.path.clone()).filter(|p| !p.is_empty()));

            let inspect = (|| -> Result<(&'static str, &'static str, String)> {
                let Some(source) = source else {
                    return Ok((
                        "check",
                        "source_download_required",
                        tr!("Create or update linked copies after downloading the source and choosing its folder"),
                    ));
                };
                if matches!(st.status(Some(source), Some(r)), "pull" | "conflict") {
                    return Ok((
                        "check",
                        "source_sync_required",
                        tr!("Recheck linked copies after downloading or resolving the shared conflict"),
                    ));
                }
                if agents::in_use(source) || args.busy.as_deref() == Some(key.as_str()) {
                    return Ok((
                        "skip",
                        "source_active",
                        tr!("Source open or active in the last 2 minutes; linked copies are deferred"),
                    ));
                }
                if source.path.is_empty() || !Path::new(&source.path).is_dir() {
                    return Ok((
                        "skip",
                        "work_folder_required",
                        tr!("An existing work folder is required for linked copies"),
                    ));
                }
                let history = histories.get(source)?;
                let a = hashes(&history.0);
                if a.is_empty() {
                    return Ok(("skip", "no_turns", tr!("No conversation turns to convert")));
                }
                if let Some(link) = links.copies.get(&format!("{key}/{target}")) {
                    op.target_id = Some(link.target.clone());
                    let Some(mirror) = sessions.get(&link.target) else {
                        return Ok((
                            "skip",
                            "linked_copy_missing",
                            tr!("Linked copy missing; restore it or change the conversation profile"),
                        ));
                    };
                    let mirror_history = histories.get(mirror)?;
                    let (kind, delta, destination) = match history_change(link, source, &history.0, &mirror_history.0) {
                        LinkChange::Unchanged => {
                            return Ok((
                                "synced",
                                "linked_copies_match",
                                tr!("Linked histories match their saved baselines"),
                            ));
                        }
                        LinkChange::Refresh => {
                            if agents::in_use(mirror) {
                                return Ok((
                                    "skip",
                                    "destination_active",
                                    tr!("Destination conversation open or recently active; update deferred"),
                                ));
                            }
                            return Ok((
                                "linked_refresh",
                                "source_compacted",
                                tr!("Compacted history repaired; the previous native copy was preserved"),
                            ));
                        }
                        LinkChange::Edited => {
                            return Ok((
                                "conflict",
                                "history_edited",
                                tr!("Linked history edited or compacted; both copies are kept"),
                            ));
                        }
                        LinkChange::Diverged => {
                            return Ok((
                                "conflict",
                                "histories_diverged",
                                tr!("Both harnesses continued independently; both copies are kept"),
                            ));
                        }
                        LinkChange::ToTarget => ("linked_pull", continuation(&link.a, &history.0).unwrap(), mirror),
                        LinkChange::ToSource => {
                            ("linked_push", continuation(&link.b, &mirror_history.0).unwrap(), source)
                        }
                        LinkChange::Baselines => {
                            return Ok((
                                "link",
                                "baselines_need_update",
                                tr!("Same continuation in both harnesses; update linked baselines"),
                            ));
                        }
                    };
                    if kind == "linked_push"
                        && (agents::in_use(mirror)
                            || mirror.harness == "codex"
                                && mirror
                                    .src
                                    .as_ref()
                                    .is_some_and(|p| crate::codex::completed_end(p).ok().flatten().is_some()))
                    {
                        return Ok((
                            "skip",
                            "source_active",
                            tr!("Source open or active in the last 2 minutes; linked copies are deferred"),
                        ));
                    }
                    if !complete_delta(&delta) {
                        return Ok((
                            "skip",
                            "incomplete_turn",
                            tr!("Incomplete continuation; finish the agent turn before syncing"),
                        ));
                    }
                    if agents::in_use(destination) {
                        return Ok((
                            "skip",
                            "destination_active",
                            tr!("Destination conversation open or recently active; update deferred"),
                        ));
                    }
                    return Ok((
                        kind,
                        "continuation_ready",
                        tr!("Append complete new turns; preserve conversation identifiers and existing history"),
                    ));
                }
                if !harness::get(target).installed() {
                    return Ok(("skip", "harness_not_installed", tr!("Target harness is not installed")));
                }
                let existing: Vec<_> = sessions
                    .values()
                    .filter(|s| {
                        s.harness == target
                            && families
                                .get(&s.key())
                                .cloned()
                                .unwrap_or_else(|| crate::catalog::root(&aliases, &s.key()))
                                == *family
                    })
                    .collect();
                if existing.len() > 1 {
                    return Ok((
                        "conflict",
                        "multiple_copies",
                        tr!("Several identified copies exist in this harness; choose one before linking"),
                    ));
                }
                if let Some(copy) = existing.first() {
                    op.target_id = Some(copy.key());
                    let b = histories.get(copy)?;
                    if hashes(&turns::fit(history.0.clone(), "actx", None))
                        != hashes(&turns::fit(b.0.clone(), "actx", None))
                    {
                        return Ok((
                            "conflict",
                            "copy_history_differs",
                            tr!("Existing harness copy has different history; no duplicate will be created"),
                        ));
                    }
                    return Ok((
                        "link",
                        "matching_copy_found",
                        tr!("Adopt the existing matching harness copy without creating a duplicate"),
                    ));
                }
                Ok((
                    "convert",
                    "linked_copy_required",
                    tr!("Convert the source into a new linked conversation"),
                ))
            })();
            match inspect {
                Ok((kind, code, reason)) => {
                    op.reason_code = code;
                    op.kind = kind;
                    op.reason = reason;
                }
                Err(error) => {
                    crate::inspection::record_error(error.to_string());
                    op.reason_code = "linked_history_unreadable";
                    op.kind = "check";
                    op.reason = tr!("Cannot inspect linked history: {error}", error = error);
                }
            }
            if op.kind != "synced" {
                changing.insert(key.clone());
            }
            operations.push(op);
        }
        // A reverse continuation changes the source used by later targets. Do not promise
        // that those targets are already current, or that several reverse changes can merge.
        let family_ops = &mut operations[family_start..];
        let reverse = family_ops.iter().filter(|op| op.kind == "linked_push").count();
        if reverse > 0 {
            for op in family_ops {
                if op.kind == "synced" || reverse > 1 && op.kind == "linked_push" {
                    op.kind = "check";
                    op.reason_code = "linked_source_changed";
                    op.reason = tr!(
                        "A linked continuation changes the source; recheck propagation and conflicts between harnesses during sync"
                    );
                }
            }
        }
    }
    if let Some(days) = profile::days() {
        let cutoff = crate::ctx::now() - (days * 86400) as f64;
        for r in stored.iter().filter(|r| r.updated > 0.0 && r.updated < cutoff) {
            let key = r.key();
            let mut op = Operation::new(
                "retention",
                r,
                "retention_expired",
                tr!(
                    "Older than {days} days; delete only the shared copy, keeping local originals",
                    days = days
                ),
            );
            let local = sessions.get(&key);
            if changing.contains(&key)
                || local.is_some_and(|s| {
                    s.updated >= cutoff || agents::in_use(s) || st.status(Some(s), Some(r)) != "synced"
                })
            {
                op.kind = "check";
                op.reason_code = "retention_pending_changes";
                op.reason = tr!("Retention deferred until pending changes and linked copies are resolved");
            } else {
                for link in links.copies.values().filter(|l| l.source == key) {
                    if let Some(mirror) = sessions.get(&link.target)
                        && (agents::in_use_by_agent(mirror)
                            || harness::get(mirror.harness)
                                .local_turns(mirror)
                                .map(|h| hashes(&h.0) != link.b)
                                .unwrap_or(true))
                    {
                        op.kind = "skip";
                        op.reason_code = "retention_linked_copy_not_ready";
                        op.reason = tr!("Retention deferred: a linked copy is active, changed or unreadable");
                        break;
                    }
                }
            }
            operations.push(op);
        }
    }
    operations
}
