//! Read-only synchronization plans. Native classification is shared with the executor.
use crate::{
    agents,
    commands::{self, Args},
    ctx::ctx,
    harness, inspection,
    session::Session,
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
            (st.status(l, Some(&r)), r)
        })
        .collect()
}

pub struct Operation {
    pub kind: &'static str,
    pub session_id: String,
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub source_harness: String,
    pub target_harness: Option<String>,
    pub target_id: Option<String>,
    pub reason_code: &'static str,
    pub notices: Vec<&'static str>,
    pub title: String,
    pub project: String,
    pub harness: String,
    pub reason: String,
    pub destination: Option<String>,
}
impl Operation {
    pub fn new(kind: &'static str, session: &Session, reason_code: &'static str, reason: String) -> Self {
        Self {
            kind,
            session_id: session.key(),
            conversation_id: session
                .meta
                .as_ref()
                .and_then(|m| m.get("conversation_id"))
                .and_then(serde_json::Value::as_str)
                .map(String::from)
                .unwrap_or_else(|| crate::catalog::identity(&session.key())),
            project_id: session.project.clone(),
            source_harness: session.harness.to_string(),
            target_harness: None,
            target_id: None,
            reason_code,
            notices: Vec::new(),
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

pub struct Plan {
    pub operations: Vec<Operation>,
    pub changed: bool,
    pub errors: Vec<String>,
}

/// Read the current scope without asking questions or executing any operation.
/// Both the text CLI and the structured interface use this same planner.
pub fn inspect(root: &Path, args: &Args) -> Plan {
    let _guard = inspection::Guard::new();
    let _loading = ui::loading(&tr!("Preparing synchronization preview"));
    inspection::tree(root);
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
    let mut remote = store::stored_sessions(root);
    remote.retain(|r| {
        args.keys.as_ref().is_none_or(|keys| keys.contains(&r.key()))
            && (!args.general || r.is_general())
            && args
                .project
                .as_ref()
                .is_none_or(|name| r.project.as_ref().is_some_and(|p| p.eq_ignore_ascii_case(name)))
    });
    remote.sort_by(|a, b| {
        a.project
            .cmp(&b.project)
            .then(a.title.cmp(&b.title))
            .then(a.key().cmp(&b.key()))
    });
    let st = state::load();
    let mut keys: HashSet<_> = remote.iter().map(Session::key).collect();
    crate::multisync::extend_keys(&mut keys);
    let aliases = crate::catalog::aliases();
    let converted: Vec<_> = aliases
        .keys()
        .filter(|key| keys.contains(&crate::catalog::root(&aliases, key)))
        .cloned()
        .collect();
    keys.extend(converted);
    let locals = harness::local_keys(&keys).into_iter().map(|s| (s.key(), s)).collect();
    let operations = prepare(&remote, &locals, &st, args);
    Plan {
        operations,
        changed: inspection::changed(),
        errors: inspection::errors(),
    }
}

pub fn preview(args: &Args) {
    if args.json {
        crate::api::print(crate::api::preview(args));
        return;
    }
    // Scope inference reads machine state too; keep legacy migration disabled from the start.
    let _guard = inspection::Guard::new();
    let Some(root) = store::store_root(true) else {
        ui::die(&tr!(
            "A reachable shared folder is required for a preview; no folder was created"
        ));
    };
    let mut args = commands::scope(args, &root);
    if args.project.is_none() && !args.general && !args.all && !args.auto && ui::interactive() && ctx().answer.is_none()
    {
        let remote = store::stored_sessions(&root);
        let projects: BTreeMap<_, Vec<_>> = remote.iter().fold(BTreeMap::new(), |mut map, r| {
            map.entry(r.project.clone().unwrap_or_default()).or_default().push(r);
            map
        });
        let rows: Vec<_> = projects
            .iter()
            .map(|(name, sessions)| ui::Row {
                key: name.clone(),
                line: format!("{} ({})", store::collection_label(name), sessions.len()),
                preview: String::new(),
            })
            .collect();
        let chosen: HashSet<_> = ui::fzf(
            &rows,
            &tr!("Preview which projects? TAB marks several · Enter confirms · Esc cancels"),
            true,
            false,
        )
        .into_iter()
        .collect();
        args.keys = Some(
            remote
                .iter()
                .filter(|r| chosen.contains(r.project.as_deref().unwrap_or_default()))
                .map(Session::key)
                .collect(),
        );
    }
    let plan = inspect(&root, &args);
    println!("{}", ui::bold(&tr!("Synchronization preview — no changes")));
    if plan.changed {
        ui::warn(&tr!(
            "Data changed while preparing the preview; discard this plan and run it again"
        ));
        return;
    }
    for error in &plan.errors {
        ui::warn(&tr!("Incomplete preview: {error}", error = error));
    }
    if plan.operations.is_empty() {
        println!("{}", tr!("No shared conversations in the selected scope"));
        return;
    }
    let mut counts = BTreeMap::new();
    for op in &plan.operations {
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
                "push" => "local_changed",
                "pull" => "shared_changed",
                "new" => "local_copy_missing",
                "conflict" => "both_changed",
                _ => "copies_match",
            },
            match kind {
                "push" => tr!("Local changes since the last sync"),
                "pull" => tr!("Shared changes since the last sync"),
                "new" => tr!("The shared conversation is not on this machine"),
                "conflict" => tr!("Both copies changed; choose merge, local, remote or defer during sync"),
                _ => tr!("Local and shared copies match"),
            },
        );
        if kind == "push"
            && local.is_some_and(|s| {
                s.harness == "codex"
                    && s.src
                        .as_ref()
                        .is_some_and(|p| crate::codex::completed_end(p).ok().flatten().is_some())
            })
        {
            op.kind = "skip";
            op.reason_code = "turn_in_progress";
            op.reason = tr!("Finish the current agent turn before synchronizing");
        }
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
                op.reason_code = "conversation_active";
                op.reason =
                    tr!("Conversation open or active; automatic sync defers it, manual sync requires confirmation");
            }
            op.destination = destination;
            if let Some(folder) = &op.destination {
                inspection::observe(Path::new(folder));
            }
            if op.destination.is_none() {
                op.notices.push("destination_required");
                op.reason.push_str(&format!(
                    " · {}",
                    tr!("Destination folder needs a decision; automatic sync defers it")
                ));
            }
            if r.harness == "opencode" && !crate::ctx::which("opencode") {
                op.kind = "skip";
                op.reason_code = "opencode_cli_required";
                op.reason = tr!("opencode CLI is required to import this conversation");
            }
        }
        operations.push(op);
    }
    operations.extend(crate::multisync::preview(remote, locals, st, args));
    operations
}
