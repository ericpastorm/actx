//! Read-only synchronization plans. Native classification is shared with the executor.
use crate::{
    agents,
    commands::{self, Args},
    ctx::ctx,
    harness, inspection,
    session::{Session, status},
    state::{self, State},
    store, tr, ui,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

pub fn native(local: &HashMap<String, Session>, remote: Vec<Session>, st: &State) -> Vec<(&'static str, Session)> {
    remote
        .into_iter()
        .map(|r| {
            let l = local.get(&r.key());
            (status(l, Some(&r), st.base.get(&r.key()).copied()), r)
        })
        .collect()
}

pub struct Operation {
    pub kind: &'static str,
    pub title: String,
    pub project: String,
    pub harness: String,
    pub reason: String,
    pub destination: Option<String>,
}
impl Operation {
    pub fn new(kind: &'static str, session: &Session, reason: String) -> Self {
        Self {
            kind,
            title: session.title.clone(),
            project: session.project_label(),
            harness: session.harness.into(),
            reason,
            destination: None,
        }
    }
}
fn label(kind: &str) -> String {
    match kind {
        "push" => tr!("Upload"),
        "pull" => tr!("Download"),
        "new" => tr!("New local conversation"),
        "conflict" => tr!("Conflict"),
        "skip" => tr!("Deferred"),
        "synced" => tr!("Up to date"),
        "convert" => tr!("Create linked copy"),
        "linked_push" => tr!("Import linked continuation and upload"),
        "linked_pull" => tr!("Update linked copy"),
        "link" => tr!("Link existing copy"),
        "retention" => tr!("Delete expired shared copy"),
        _ => tr!("Recheck after synchronization"),
    }
}

pub fn preview(args: &Args) {
    // This branch is entered before auto logs, migration, locks, maintenance or binary updates.
    let Some(root) = store::store_root(true) else {
        ui::die(&tr!(
            "A reachable shared folder is required for a preview; no folder was created"
        ));
    };
    inspection::enable();
    let _cleanup = Cleanup;
    inspection::tree(&root);
    inspection::tree(&ctx().claude_projects);
    inspection::tree(&ctx().codex_home.join("sessions"));
    inspection::observe(&ctx().codex_index());
    inspection::tree(&crate::antigravity::home());
    inspection::observe(&ctx().opencode_db);
    inspection::observe(&ctx().config_file);
    inspection::tree(&ctx().state_dir);
    if let Some(parent) = ctx().state_dir.parent() {
        inspection::tree(&parent.join("ctx"));
    }
    let mut remote = store::stored_sessions(&root);
    if let Some(keys) = &args.keys {
        remote.retain(|r| keys.contains(&r.key()));
    }
    let projects: BTreeMap<_, Vec<_>> = remote.iter().fold(BTreeMap::new(), |mut map, r| {
        map.entry(r.project.clone().unwrap_or_default()).or_default().push(r);
        map
    });
    let wanted = args
        .general
        .then_some(store::GENERAL.to_string())
        .or_else(|| args.project.clone());
    let chosen: HashSet<String> = if let Some(wanted) = wanted {
        projects
            .keys()
            .filter(|name| name.eq_ignore_ascii_case(&wanted))
            .cloned()
            .collect()
    } else if args.all || args.auto || !ui::interactive() || ctx().answer.is_some() {
        projects.keys().cloned().collect()
    } else {
        let rows: Vec<_> = projects
            .iter()
            .map(|(name, sessions)| ui::Row {
                key: name.clone(),
                line: format!("{} ({})", store::collection_label(name), sessions.len()),
                preview: String::new(),
            })
            .collect();
        ui::fzf(
            &rows,
            &tr!("Preview which projects? TAB marks several · Enter confirms · Esc cancels"),
            true,
            false,
        )
        .into_iter()
        .collect()
    };
    remote.retain(|r| chosen.contains(r.project.as_deref().unwrap_or_default()));
    println!("{}", ui::bold(&tr!("Synchronization preview — no changes")));
    if remote.is_empty() {
        println!("{}", tr!("No shared conversations in the selected scope"));
        return;
    }
    let st = state::load();
    let locals: HashMap<_, _> = harness::CONVERTS
        .iter()
        .flat_map(|n| harness::get(n).local())
        .map(|s| (s.key(), s))
        .collect();
    remote.sort_by(|a, b| {
        a.project
            .cmp(&b.project)
            .then(a.title.cmp(&b.title))
            .then(a.key().cmp(&b.key()))
    });
    let operations = prepare(&remote, &locals, &st, args);
    if inspection::changed() {
        ui::warn(&tr!(
            "Data changed while preparing the preview; discard this plan and run it again"
        ));
        return;
    }
    for error in inspection::errors() {
        ui::warn(&tr!("Incomplete preview: {error}", error = error));
    }
    let mut counts = BTreeMap::new();
    for op in &operations {
        *counts.entry(label(op.kind)).or_insert(0usize) += 1;
        println!(
            "\n  {} · {} · {} · {}",
            ui::bold(&label(op.kind)),
            op.title.replace(['\n', '\r'], " "),
            op.project,
            op.harness
        );
        println!("    {}", op.reason);
        if let Some(folder) = &op.destination {
            println!("    {}: {}", tr!("Destination"), folder);
        }
    }
    println!(
        "\n{}",
        counts
            .iter()
            .map(|(kind, n)| format!("{kind}: {n}"))
            .collect::<Vec<_>>()
            .join(" · ")
    );
    println!(
        "{}",
        ui::dim(&tr!(
            "Snapshot only: sync recalculates and rechecks the plan before writing. No data or actx binaries were changed."
        ))
    );
}
/// Prepare presentation-independent operations without executing them.
/// Inspection mode must be enabled by the caller before reading harness data.
pub fn prepare(remote: &[Session], locals: &HashMap<String, Session>, st: &State, args: &Args) -> Vec<Operation> {
    let mut operations = Vec::new();
    for (kind, r) in native(locals, remote.to_vec(), st) {
        let local = locals.get(&r.key());
        let mut op = Operation::new(
            kind,
            &r,
            match kind {
                "push" => tr!("Local changes since the last sync"),
                "pull" => tr!("Shared changes since the last sync"),
                "new" => tr!("The shared conversation is not on this machine"),
                "conflict" => tr!("Both copies changed; choose merge, local, remote or defer during sync"),
                _ => tr!("Local and shared copies match"),
            },
        );
        if matches!(kind, "pull" | "new") {
            let destination = local
                .filter(|s| Path::new(&s.path).is_dir())
                .map(|s| s.path.clone())
                .or_else(|| commands::session_folder(&r, &mut st.clone(), false));
            if let Some(l) = local
                && (agents::in_use_by_agent(l)
                    || args.auto && (agents::in_use(l) || args.busy.as_deref() == Some(l.key().as_str())))
            {
                op.kind = "skip";
                op.reason =
                    tr!("Conversation open or active; automatic sync defers it, manual sync requires confirmation");
            }
            op.destination = destination;
            if let Some(folder) = &op.destination {
                inspection::observe(Path::new(folder));
            }
            if op.destination.is_none() {
                op.reason.push_str(&format!(
                    " · {}",
                    tr!("Destination folder needs a decision; automatic sync defers it")
                ));
            }
            if r.harness == "opencode" && !crate::ctx::which("opencode") {
                op.kind = "skip";
                op.reason = tr!("opencode CLI is required to import this conversation");
            }
        }
        operations.push(op);
    }
    operations.extend(crate::multisync::preview(remote, locals, st, args));
    operations
}

struct Cleanup;
impl Drop for Cleanup {
    fn drop(&mut self) {
        inspection::finish();
    }
}
