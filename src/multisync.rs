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
    (!args.general || s.is_general())
        && args
            .project
            .as_ref()
            .is_none_or(|p| s.project.as_ref().is_some_and(|n| n.eq_ignore_ascii_case(p)))
}

pub fn run(root: &Path, args: &crate::commands::Args) {
    if ctx().config.get("harnesses").is_none() && ctx().config.get("conversation_profiles").is_none() {
        return;
    }
    if let Err(e) = run_inner(root, args) {
        ui::warn(&format!("Multiharness: {e}"));
    }
}
fn run_inner(root: &Path, args: &crate::commands::Args) -> Result<()> {
    std::fs::create_dir_all(&ctx().state_dir)?;
    let guard = std::fs::File::create(ctx().state_dir.join("multiharness.lock"))?;
    guard.lock()?;
    let Some(_shared) = crate::commands::lock_store(root) else {
        return Ok(());
    };
    let mut links = read()?;
    let mut sessions = local();
    let mut st = state::load();
    for r in store::stored_sessions(root).into_iter().filter(|r| scoped(r, args)) {
        let key = r.key();
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
            if !args.auto && !profile::harnesses(&key).is_empty() {
                ui::warn(&format!(
                    "{}: linked copies deferred while the source is open or recently active",
                    source.title
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
        let source_harness = source.harness;
        let targets: Vec<_> = profile::harnesses(&key)
            .into_iter()
            .filter(|n| *n != source_harness)
            .collect();
        // A late target can contribute new turns to the base. Visit the others again so they
        // receive that continuation in this run, rather than waiting for another agent hook.
        for target in targets.iter().copied().cycle().take(targets.len() * 2) {
            let id = format!("{key}/{target}");
            let result = (|| -> Result<()> {
                let (a_turns, model) = harness::get(source.harness).local_turns(&source)?;
                let a = hashes(&a_turns);
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
                    let (b_turns, _) = harness::get(target).local_turns(&mirror)?;
                    let b = hashes(&b_turns);
                    if a == link.a && b == link.b {
                        return Ok(());
                    }
                    if !a.starts_with(&link.a) || !b.starts_with(&link.b) {
                        return Err(Error(
                            "History edited or compacted; linked conflict left untouched".into(),
                        ));
                    }
                    let da = &a[link.a.len()..];
                    let db = &b[link.b.len()..];
                    if !da.is_empty() && !db.is_empty() && da != db {
                        return Err(Error(
                            "Continued in two harnesses: linked conflict left untouched".into(),
                        ));
                    }
                    if !da.is_empty() && db.is_empty() {
                        append(&mut mirror, &a_turns[link.a.len()..])?;
                    } else if da.is_empty() && !db.is_empty() {
                        append(&mut source, &b_turns[link.b.len()..])?;
                    }
                    let a = hashes(&harness::get(source.harness).local_turns(&source)?.0);
                    let b = hashes(&harness::get(target).local_turns(&mirror)?.0);
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
                    let copy = harness::get(target).write(
                        &turns::fit(a_turns, harness::get(source.harness).label(), model.as_deref()),
                        &source.path,
                        &source.title,
                        model.as_deref(),
                    )?;
                    let b = hashes(&harness::get(target).local_turns(&copy)?.0);
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
