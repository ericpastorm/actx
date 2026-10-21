//! Local linked copies. The shared native session remains the canonical identity across PCs.
use crate::{
    agents,
    ctx::{Error, Result, ctx},
    harness, profile,
    session::{Session, status},
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
}

#[derive(Default, Serialize, Deserialize)]
struct Links {
    #[serde(default)]
    copies: HashMap<String, Link>,
}
#[derive(Serialize, Deserialize)]
struct Link {
    source: String,
    target: String,
    a: Vec<u64>,
    b: Vec<u64>,
}
#[derive(PartialEq, Eq)]
enum LinkChange {
    Unchanged,
    Edited,
    Diverged,
    ToTarget,
    ToSource,
    Baselines,
}
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
fn append(s: &mut Session, delta: &[Turn]) -> Result<()> {
    // Never manufacture replies or silently flatten an incomplete turn.
    if !delta.len().is_multiple_of(2) || delta.chunks(2).any(|p| p[0].who != "user" || p[1].who != "assistant") {
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
    for pair in delta.chunks(2) {
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
    guard.lock()?;
    let Some(_shared) = crate::commands::lock_store(root) else {
        return Ok(());
    };
    let mut links = read()?;
    let mut st = state::load();
    let stored = store::stored_sessions(root);
    let needed: std::collections::HashSet<_> = stored
        .iter()
        .filter(|r| scoped(r, args))
        .flat_map(|r| {
            let targets: Vec<_> = profile::harnesses_in(config, &r.key())
                .into_iter()
                .filter(|n| *n != r.harness)
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
    let mut sessions: HashMap<_, _> = needed
        .into_iter()
        .flat_map(|n| harness::get(n).local())
        .map(|s| (s.key(), s))
        .collect();
    let aliases = crate::catalog::aliases();
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
            .filter(|n| *n != r.harness)
            .collect();
        if targets.is_empty() {
            continue;
        }

        let Some(mut source) = sessions.get(&key).cloned() else {
            continue;
        };
        if matches!(
            status(Some(&source), Some(&r), st.base.get(&key).copied()),
            "pull" | "conflict"
        ) {
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
        // Bound memory to one conversation family, even for a large shared library.
        let mut histories = Histories::default();
        // A late target can contribute new turns to the base. Visit the others again so they
        // receive that continuation in this run, rather than waiting for another agent hook.
        for _ in 0..2 {
            let mut source_changed = false;
            for target in targets.iter().copied() {
                let id = format!("{key}/{target}");
                let result = (|| -> Result<()> {
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
                        let b = hashes(b_turns);
                        let change = link_change(link, &a, &b);
                        match change {
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
                            append(&mut mirror, &a_turns[link.a.len()..])?;
                        } else if change == LinkChange::ToSource {
                            histories.invalidate(&source);
                            append(&mut source, &b_turns[link.b.len()..])?;
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
            && (l.updated >= cutoff
                || agents::in_use(l)
                || !matches!(status(Some(l), Some(&r), st.base.get(&key).copied()), "synced"))
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
pub fn views(local: &HashMap<String, Session>) -> Vec<LinkView> {
    let links = match read() {
        Ok(l) => l,
        Err(e) => {
            ui::warn(&format!("Multiharness: {e}"));
            return Vec::new();
        }
    };
    let active = |l: &Link| {
        l.target
            .split_once(':')
            .is_some_and(|(tool, _)| profile::harnesses(&l.source).contains(&tool))
    };
    let mut cache = HashMap::new();
    for key in links
        .copies
        .values()
        .filter(|l| active(l))
        .flat_map(|l| [&l.source, &l.target])
    {
        cache.entry(key.clone()).or_insert_with(|| {
            local
                .get(key)
                .and_then(|s| harness::get(s.harness).local_turns(s).ok())
                .map(|t| hashes(&t.0))
        });
    }
    links
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
            let state = match (
                cache.get(&l.source).and_then(Option::as_ref),
                cache.get(&l.target).and_then(Option::as_ref),
            ) {
                (_, None) => "missing",
                (None, Some(b)) => {
                    if b == &l.b {
                        "unknown"
                    } else {
                        "push"
                    }
                }
                (Some(a), Some(b)) if !a.starts_with(&l.a) || !b.starts_with(&l.b) => "conflict",
                (Some(a), Some(b)) => match (a != &l.a, b != &l.b) {
                    (false, false) => "synced",
                    (true, false) => "pull",
                    (false, true) => "push",
                    (true, true) if a[l.a.len()..] == b[l.b.len()..] => "synced",
                    _ => "conflict",
                },
            };
            LinkView {
                source: l.source,
                target: l.target,
                state,
            }
        })
        .collect()
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
            if let Some(r) = stored.first() {
                operations.push(Operation::new(
                    "check",
                    r,
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
            .filter(|n| *n != r.harness)
            .collect();
        let source = sessions.get(&key);
        let mut histories = Histories::default();
        let family_start = operations.len();
        for target in targets {
            let mut op = Operation::new("check", r, String::new());
            op.harness = format!("{} ↔ {target}", r.harness);
            let inspect = (|| -> Result<(&'static str, String)> {
                let Some(source) = source else {
                    return Ok((
                        "check",
                        tr!("Create or update linked copies after downloading the source and choosing its folder"),
                    ));
                };
                if matches!(
                    status(Some(source), Some(r), st.base.get(&key).copied()),
                    "pull" | "conflict"
                ) {
                    return Ok((
                        "check",
                        tr!("Recheck linked copies after downloading or resolving the shared conflict"),
                    ));
                }
                if agents::in_use(source) || args.busy.as_deref() == Some(key.as_str()) {
                    return Ok((
                        "skip",
                        tr!("Source open or active in the last 2 minutes; linked copies are deferred"),
                    ));
                }
                if source.path.is_empty() || !Path::new(&source.path).is_dir() {
                    return Ok(("skip", tr!("An existing work folder is required for linked copies")));
                }
                let history = histories.get(source)?;
                let a = hashes(&history.0);
                if a.is_empty() {
                    return Ok(("skip", tr!("No conversation turns to convert")));
                }
                if let Some(link) = links.copies.get(&format!("{key}/{target}")) {
                    let Some(mirror) = sessions.get(&link.target) else {
                        return Ok((
                            "skip",
                            tr!("Linked copy missing; restore it or change the conversation profile"),
                        ));
                    };
                    let mirror_history = histories.get(mirror)?;
                    let b = hashes(&mirror_history.0);
                    let (kind, delta, destination) = match link_change(link, &a, &b) {
                        LinkChange::Unchanged => {
                            return Ok(("synced", tr!("Linked histories match their saved baselines")));
                        }
                        LinkChange::Edited => {
                            return Ok((
                                "conflict",
                                tr!("Linked history edited or compacted; both copies are kept"),
                            ));
                        }
                        LinkChange::Diverged => {
                            return Ok((
                                "conflict",
                                tr!("Both harnesses continued independently; both copies are kept"),
                            ));
                        }
                        LinkChange::ToTarget => ("linked_pull", &history.0[link.a.len()..], mirror),
                        LinkChange::ToSource => ("linked_push", &mirror_history.0[link.b.len()..], source),
                        LinkChange::Baselines => {
                            return Ok((
                                "link",
                                tr!("Same continuation in both harnesses; update linked baselines"),
                            ));
                        }
                    };
                    if !delta.len().is_multiple_of(2)
                        || delta.chunks(2).any(|p| p[0].who != "user" || p[1].who != "assistant")
                    {
                        return Ok((
                            "skip",
                            tr!("Incomplete continuation; finish the agent turn before syncing"),
                        ));
                    }
                    if agents::in_use(destination) {
                        return Ok((
                            "skip",
                            tr!("Destination conversation open or recently active; update deferred"),
                        ));
                    }
                    return Ok((
                        kind,
                        tr!("Append complete new turns; preserve conversation identifiers and existing history"),
                    ));
                }
                if !harness::get(target).installed() {
                    return Ok(("skip", tr!("Target harness is not installed")));
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
                        tr!("Several identified copies exist in this harness; choose one before linking"),
                    ));
                }
                if let Some(copy) = existing.first() {
                    let b = histories.get(copy)?;
                    if hashes(&turns::fit(history.0.clone(), "actx", None))
                        != hashes(&turns::fit(b.0.clone(), "actx", None))
                    {
                        return Ok((
                            "conflict",
                            tr!("Existing harness copy has different history; no duplicate will be created"),
                        ));
                    }
                    return Ok((
                        "link",
                        tr!("Adopt the existing matching harness copy without creating a duplicate"),
                    ));
                }
                Ok(("convert", tr!("Convert the source into a new linked conversation")))
            })();
            match inspect {
                Ok((kind, reason)) => {
                    op.kind = kind;
                    op.reason = reason;
                }
                Err(error) => {
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
                tr!(
                    "Older than {days} days; delete only the shared copy, keeping local originals",
                    days = days
                ),
            );
            let local = sessions.get(&key);
            if changing.contains(&key)
                || local.is_some_and(|s| {
                    s.updated >= cutoff
                        || agents::in_use(s)
                        || status(Some(s), Some(r), st.base.get(&key).copied()) != "synced"
                })
            {
                op.kind = "check";
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
