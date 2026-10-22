//! Frontend-independent synchronization, conversion and launching. Never reads terminal input.
use crate::{
    actions::{self, Request},
    api::{Diagnostic, PlannedOperation, Response, Status},
    catalog::Catalog,
    commands,
    ctx::{Error, Result, ctx},
    harness, inspection,
    session::Session,
    state::{self, State},
    store, sync_plan, ui,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
};

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!(
        "{:016x}",
        bytes.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(*b))
            .wrapping_mul(0x100000001b3))
    )
}
fn fail(command: &'static str, diagnostic: Diagnostic) -> Response<Value> {
    Response::error(command, diagnostic)
}
fn ready(
    command: &'static str,
    req: &Request,
    data: Value,
    token: &str,
    blocked: Option<Status>,
) -> Option<Response<Value>> {
    if let Some(status) = blocked {
        return Some(actions::response(command, status, data, vec![]));
    }
    if !req.apply {
        return Some(actions::response(command, Status::Ok, data, vec![]));
    }
    if req.expected.get(command).map(String::as_str) != Some(token) {
        return Some(actions::response(
            command,
            Status::DecisionRequired,
            data,
            vec![Diagnostic::new(
                "preview_required",
                "Preview again and confirm the current revision",
            )],
        ));
    }
    None
}
pub(crate) fn local_guard() -> Result<std::fs::File> {
    std::fs::create_dir_all(&ctx().state_dir)?;
    let guard = std::fs::File::create(ctx().state_dir.join("actions.lock"))?;
    guard.try_lock().map_err(|e| Error(e.to_string()))?;
    Ok(guard)
}
struct SyncSnapshot {
    root: PathBuf,
    args: commands::Args,
    state: State,
    remote: Vec<Session>,
    local: HashMap<String, Session>,
    operations: Vec<PlannedOperation>,
    token: String,
}
fn sync_snapshot(req: &Request) -> std::result::Result<SyncSnapshot, Diagnostic> {
    let _guard = inspection::Guard::new();
    crate::api::validate_config()?;
    if req.general && req.project.is_some() {
        return Err(Diagnostic::new("invalid_scope", "Choose a project or General"));
    }
    let root = store::store_root(true)
        .ok_or_else(|| Diagnostic::new("shared_folder_unavailable", "A reachable shared folder is required"))?;
    let mut args = req.args();
    args.skip_retention = true;
    if !req.ids.is_empty() {
        args.keys = Some(req.ids.iter().cloned().collect());
        args.all = true;
    }
    args = commands::scope(&args, &root);
    if let Some(name) = &args.project
        && store::find_project(&root, name).is_none()
    {
        return Err(Diagnostic::new("project_not_found", name));
    }
    let mut remote = store::stored_sessions(&root);
    remote.retain(|s| {
        args.keys.as_ref().is_none_or(|k| k.contains(&s.key()))
            && (!args.general || s.is_general())
            && args
                .project
                .as_ref()
                .is_none_or(|p| s.project.as_ref().is_some_and(|n| n.eq_ignore_ascii_case(p)))
    });
    remote.sort_by_key(Session::key);
    for id in &req.ids {
        if !remote.iter().any(|r| r.key() == *id) {
            return Err(Diagnostic::new("conversation_not_found", id));
        }
    }
    let mut keys: HashSet<_> = remote.iter().map(Session::key).collect();
    crate::multisync::extend_keys(&mut keys);
    let aliases = crate::catalog::aliases();
    keys.extend(
        aliases
            .keys()
            .filter(|k| remote.iter().any(|r| crate::catalog::root(&aliases, k) == r.key()))
            .cloned(),
    );
    let local: HashMap<_, _> = harness::local_keys(&keys).into_iter().map(|s| (s.key(), s)).collect();
    let st = state::load();
    let mut operations: Vec<PlannedOperation> = sync_plan::prepare(&remote, &local, &st, &args)
        .into_iter()
        .filter(|o| !o.reason_code.starts_with("retention_"))
        .map(Into::into)
        .collect();
    operations.retain(|o| {
        req.target
            .as_deref()
            .is_none_or(|t| o.target_harness.as_deref().is_none_or(|h| h == t))
    });
    for op in &mut operations {
        let folder = req.folders.get(&op.session_id).or(req.folder.as_ref());
        if let Some(folder) = folder {
            if !Path::new(folder).is_dir() {
                return Err(Diagnostic::new("invalid_destination", folder));
            }
            op.destination = Some(folder.clone());
            op.requirements.retain(|r| *r != "destination_required");
            if op.target_harness.is_none()
                && op.kind == "synced"
                && local.get(&op.session_id).is_some_and(|s| s.path != *folder)
            {
                op.kind = "pull";
                op.reason = "shared_changed";
            }
            if op.reason == "work_folder_required" {
                op.kind = "check";
                op.reason = "source_sync_required";
                op.requirements = vec!["recheck_after_sync"];
            }
        }
    }
    if !inspection::errors().is_empty() {
        return Err(Diagnostic::new("inspection_failed", inspection::errors().join("; ")));
    }
    let mut revisions: BTreeMap<String, String> = local
        .iter()
        .map(|(id, s)| (id.clone(), actions::revision(s, remote.iter().find(|r| r.key() == *id))))
        .collect();
    for r in &remote {
        revisions
            .entry(r.key())
            .or_insert_with(|| actions::revision(r, Some(r)));
    }
    let token=digest(&serde_json::to_vec(&json!({"operations":operations,"revisions":revisions,"folders":st.folders,"base":st.base,"target":req.target})).unwrap());
    Ok(SyncSnapshot {
        root,
        args,
        state: st,
        remote,
        local,
        operations,
        token,
    })
}
fn sync_data(p: &SyncSnapshot, applied: bool) -> Value {
    json!({"applied":applied,"operations":p.operations,"expected":{"sync":p.token},"retention":false})
}
fn blocked(p: &SyncSnapshot) -> Option<Status> {
    if p.operations.iter().any(|o| o.kind == "conflict") {
        Some(Status::Conflict)
    } else if p
        .operations
        .iter()
        .any(|o| o.kind == "skip" || o.requirements.iter().any(|r| *r != "recheck_after_sync"))
    {
        Some(Status::DecisionRequired)
    } else {
        None
    }
}
pub fn sync(req: &Request) -> Response<Value> {
    let _quiet = ui::Silence::new();
    let _ = ui::take_reports();
    let mut p = match sync_snapshot(req) {
        Ok(p) => p,
        Err(d) => return fail("sync", d),
    };
    if let Some(r) = ready("sync", req, sync_data(&p, false), &p.token, blocked(&p)) {
        return r;
    }
    let _local = match local_guard() {
        Ok(l) => l,
        Err(e) => return actions::error("sync", "local_action_busy", e.to_string()),
    };
    let shared = match actions::acquire(&p.root) {
        Ok(l) => l,
        Err(d) => return fail("sync", d),
    };
    p = match sync_snapshot(req) {
        Ok(p) => p,
        Err(d) => return fail("sync", d),
    };
    if let Some(r) = ready("sync", req, sync_data(&p, false), &p.token, blocked(&p)) {
        return r;
    }
    let mut results = Vec::new();
    let mut diagnostics = Vec::new();
    for op in p.operations.iter().filter(|o| o.target_harness.is_none()) {
        let Some(r) = p.remote.iter().find(|r| r.key() == op.session_id) else {
            continue;
        };
        let local = p.local.get(&op.session_id);
        let result = match op.kind {
            "push" => local
                .ok_or_else(|| Error("Local copy disappeared".into()))
                .and_then(|s| {
                    store::push_one(
                        &p.root,
                        r.project.as_deref().unwrap_or(store::GENERAL),
                        s,
                        crate::git::git_info(&s.path),
                        &mut p.state,
                        &HashMap::new(),
                    )
                    .map(|_| ())
                }),
            "new" | "pull" => {
                if local.is_some_and(crate::agents::in_use_by_agent_now) {
                    Err(Error("Conversation is open in an agent".into()))
                } else if let Some(folder) = &op.destination {
                    if !r.is_general()
                        && let Some(name) = &r.project
                        && crate::project::has_binding(&p.root, name)
                        && let Err(e) = crate::project::bind(&p.root, name, folder, &mut p.state)
                    {
                        return actions::error("sync", "project_binding_failed", e.to_string());
                    }
                    commands::install_one(r, local, folder, &mut p.state).map(|_| ())
                } else {
                    Err(Error("A destination folder is required".into()))
                }
            }
            _ => continue,
        };
        results.push(
            json!({"session_id":op.session_id,"success":result.is_ok(),"message":result.err().map(|e|e.to_string())}),
        );
    }
    for op in p
        .operations
        .iter()
        .filter(|o| o.kind == "synced" && o.target_harness.is_none())
    {
        if let (Some(local), Some(remote)) = (
            p.local.get(&op.session_id),
            p.remote.iter().find(|r| r.key() == op.session_id),
        ) {
            p.state
                .base
                .entry(op.session_id.clone())
                .or_insert((local.weight, remote.weight));
            if let Err(e) = p.state.checkpoint(local, remote) {
                diagnostics.push(Diagnostic::new("checkpoint_failed", e.to_string()));
            }
        }
    }
    if let Err(e) = state::save(&mut p.state) {
        diagnostics.push(Diagnostic::new("state_save_failed", e.to_string()));
    }
    drop(shared);
    if diagnostics.is_empty()
        && results.iter().all(|r| r["success"] == true)
        && let Err(e) = crate::multisync::run_service(&p.root, &p.args)
    {
        diagnostics.push(Diagnostic::new("linked_sync_failed", e.to_string()));
    }
    diagnostics.extend(
        ui::take_reports()
            .into_iter()
            .map(|m| Diagnostic::new("sync_report", m)),
    );
    let mut data = sync_data(&p, true);
    data["results"] = json!(results);
    // Linked continuations may change the source; report fresh state instead of promising success.
    let remaining = sync_snapshot(req);
    let pending = remaining
        .as_ref()
        .is_ok_and(|p| p.operations.iter().any(|o| o.kind != "synced"));
    match remaining {
        Ok(next) => data["remaining"] = json!(next.operations),
        Err(d) => diagnostics.push(d),
    }
    let status = if diagnostics.is_empty() && !pending && results.iter().all(|r| r["success"] == true) {
        Status::Ok
    } else {
        Status::Incomplete
    };
    actions::response("sync", status, data, diagnostics)
}

struct Conversion {
    source: Session,
    destination: Option<String>,
    existing: Option<Session>,
    target: &'static str,
    token: String,
    requirements: Vec<&'static str>,
}
fn conversion(req: &Request) -> std::result::Result<Conversion, Diagnostic> {
    let _guard = inspection::Guard::new();
    crate::api::validate_config()?;
    let [id] = req.ids.as_slice() else {
        return Err(Diagnostic::new(
            "selection_required",
            "Choose exactly one physical source copy",
        ));
    };
    let target = req
        .target
        .as_deref()
        .and_then(harness::known)
        .ok_or_else(|| Diagnostic::new("target_required", "Choose a target harness"))?;
    let catalog = Catalog::load_all();
    let group = catalog
        .find(id)
        .ok_or_else(|| Diagnostic::new("conversation_not_found", id))?;
    let copy = group
        .copies
        .iter()
        .find(|c| c.session().key() == *id)
        .ok_or_else(|| Diagnostic::new("physical_copy_required", id))?;
    let source = copy.local.as_ref().or(copy.remote.as_ref()).unwrap().clone();
    let mut requirements = Vec::new();
    if matches!(copy.state, "pull" | "conflict" | "unknown") {
        requirements.push("sync_source_first");
    }
    if copy.local.as_ref().is_some_and(crate::agents::in_use_by_agent_now) {
        requirements.push("close_agent");
    }
    let copies: Vec<_> = group.copies.iter().filter(|c| c.session().harness == target).collect();
    if copies.len() > 1 {
        return Err(Diagnostic::new(
            "ambiguous_copy",
            "Select the existing physical target before converting",
        ));
    }
    let existing = copies.first().and_then(|c| c.local.as_ref()).cloned();
    if !copies.is_empty() && existing.is_none() {
        requirements.push("sync_target_first");
    }
    if !harness::get(target).installed() {
        requirements.push("install_harness");
    }
    let destination = req.folder.clone().or_else(|| {
        if source.meta.is_none() && Path::new(&source.path).is_dir() {
            Some(source.path.clone())
        } else if source.meta.is_some() {
            commands::session_folder(&source, &mut state::load(), false)
        } else {
            None
        }
    });
    if destination.as_ref().is_none_or(|p| !Path::new(p).is_dir()) {
        requirements.push("destination_required");
    }
    if !inspection::errors().is_empty() {
        return Err(Diagnostic::new("inspection_failed", inspection::errors().join("; ")));
    }
    let token=digest(&serde_json::to_vec(&json!({"source":actions::revision(&source,copy.remote.as_ref()),"target":target,"existing":existing.as_ref().map(|s|actions::revision(s,None)),"folder":destination,"state":copy.state})).unwrap());
    Ok(Conversion {
        source,
        destination,
        existing,
        target,
        token,
        requirements,
    })
}
fn convert_data(p: &Conversion, applied: bool) -> Value {
    json!({"applied":applied,"operations":[{"kind":if p.existing.is_some(){"reuse"}else{"convert"},"session_id":p.source.key(),"title":p.source.title,"target_harness":p.target,"target_id":p.existing.as_ref().map(Session::key),"destination":p.destination,"requirements":p.requirements}],"expected":{"convert":p.token}})
}
pub fn convert(req: &Request) -> Response<Value> {
    let _quiet = ui::Silence::new();
    let p = match conversion(req) {
        Ok(p) => p,
        Err(d) => return fail("convert", d),
    };
    if let Some(r) = ready(
        "convert",
        req,
        convert_data(&p, false),
        &p.token,
        (!p.requirements.is_empty()).then_some(Status::DecisionRequired),
    ) {
        return r;
    }
    let _local = match local_guard() {
        Ok(l) => l,
        Err(e) => return actions::error("convert", "local_action_busy", e.to_string()),
    };
    let root = store::store_root(true);
    let _shared = match root.as_ref().map(|r| actions::acquire(r)).transpose() {
        Ok(l) => l,
        Err(d) => return fail("convert", d),
    };
    let p = match conversion(req) {
        Ok(p) => p,
        Err(d) => return fail("convert", d),
    };
    if let Some(r) = ready(
        "convert",
        req,
        convert_data(&p, false),
        &p.token,
        (!p.requirements.is_empty()).then_some(Status::DecisionRequired),
    ) {
        return r;
    }
    let mut data = convert_data(&p, true);
    let new = match p.existing {
        Some(ref s) => s.clone(),
        None => match commands::convert_at(&p.source, p.target, p.destination.as_deref().unwrap()) {
            Ok(s) => s,
            Err(e) => return actions::error("convert", "conversion_failed", e.to_string()),
        },
    };
    let mut diagnostics = Vec::new();
    if let Err(e) = crate::catalog::remember(&crate::catalog::identity(&p.source.key()), &new.key()) {
        diagnostics.push(Diagnostic::new("identity_save_failed", e.to_string()));
    }
    data["target_id"] = json!(new.key());
    data["folder"] = json!(new.path);
    // Publishing/linking is a separate sync operation, never an implicit side effect of conversion.
    actions::response(
        "convert",
        if diagnostics.is_empty() {
            Status::Ok
        } else {
            Status::Incomplete
        },
        data,
        diagnostics,
    )
}

pub(crate) fn launch_args(s: &Session) -> (String, Vec<String>) {
    let program = if s.harness == "antigravity" { "agy" } else { s.harness }.to_string();
    let args = match s.harness {
        "claude" => vec!["--resume".into(), s.id.clone()],
        "codex" => vec!["resume".into(), s.id.clone()],
        "opencode" => vec!["-s".into(), s.id.clone()],
        _ => vec!["--conversation".into(), s.id.clone()],
    };
    (program, args)
}
pub(crate) fn launch_terminal(s: &Session) -> Result<u32> {
    let (program, args) = launch_args(s);
    let mut command;
    #[cfg(target_os = "windows")]
    {
        if !crate::ctx::which("wt") {
            return Err(Error("Windows Terminal (wt) is required to open an agent".into()));
        }
        command = std::process::Command::new("wt");
        command.args(["-d", &s.path, "cmd", "/k", &program]).args(&args);
    }
    #[cfg(target_os = "macos")]
    {
        let shell = format!(
            "cd {} && {} {}",
            ui::shell_quote(&s.path),
            ui::shell_quote(&program),
            args.iter().map(|a| ui::shell_quote(a)).collect::<Vec<_>>().join(" ")
        );
        let escaped = serde_json::to_string(&shell)?;
        command = std::process::Command::new("osascript");
        command.args(["-e", &format!("tell application \"Terminal\" to do script {escaped}")]);
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let terminal = [
            "xdg-terminal-exec",
            "x-terminal-emulator",
            "ghostty",
            "alacritty",
            "kitty",
            "foot",
            "gnome-terminal",
            "konsole",
            "xterm",
        ]
        .into_iter()
        .find(|p| crate::ctx::which(p))
        .ok_or_else(|| Error("No supported terminal found".into()))?;
        command = std::process::Command::new(terminal);
        if terminal == "gnome-terminal" || terminal == "xdg-terminal-exec" {
            command.arg("--");
        } else {
            command.arg("-e");
        }
        command.arg(&program).args(&args);
    }
    Ok(command
        .current_dir(&s.path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?
        .id())
}
pub fn resume(req: &Request) -> Response<Value> {
    let _quiet = ui::Silence::new();
    let _guard = inspection::Guard::new();
    if req.ids.len() != 1 {
        return actions::error("resume", "selection_required", "Choose one conversation");
    }
    let catalog = Catalog::load_all();
    let Some(group) = catalog.find(&req.ids[0]) else {
        return actions::error("resume", "conversation_not_found", &req.ids[0]);
    };
    if group.managed && store::configured_root().is_some() && store::store_root(true).is_none() {
        return actions::error(
            "resume",
            "shared_folder_unavailable",
            "Reconnect the shared folder before synchronized resume; use the CLI to explicitly open offline",
        );
    }
    let target = req
        .target
        .as_deref()
        .or_else(|| group.copies.first().map(|c| c.session().harness))
        .unwrap_or("claude");
    let copies: Vec<_> = group
        .copies
        .iter()
        .filter(|c| c.session().harness == target)
        .filter_map(|c| c.local.as_ref())
        .collect();
    let remote: Vec<_> = group.remote().iter().map(|s| s.key()).collect();
    let mut sync_req = req.clone();
    sync_req.action = actions::Action::Sync;
    sync_req.ids = remote;
    sync_req.target = Some(target.to_string());
    let sync_plan = if sync_req.ids.is_empty() {
        None
    } else {
        match sync_snapshot(&sync_req) {
            Ok(p) => Some(p),
            Err(d) => return fail("resume", d),
        }
    };
    let mut requirements = Vec::new();
    if copies.len() > 1 {
        return actions::error("resume", "ambiguous_copy", "Choose one physical copy in this harness");
    }
    let session = copies.first().copied();
    if group.managed
        && session.is_some_and(|s| s.key() != group.id)
        && !crate::profile::harnesses(&group.id).contains(&target)
        && group
            .copies
            .iter()
            .any(|c| c.session().harness == target && matches!(c.state, "local" | "unmanaged"))
    {
        requirements.push("enable_target_harness");
    }
    if session.is_none()
        && (sync_plan.is_none()
            || (!group
                .copies
                .iter()
                .any(|c| c.session().harness == target && c.remote.is_some())
                && !crate::profile::harnesses(&group.id).contains(&target)))
    {
        requirements.push("local_copy_required");
    }
    if session.is_some_and(crate::agents::in_use_by_agent_now) {
        requirements.push("close_agent");
    }
    let program = if target == "antigravity" { "agy" } else { target };
    if !crate::ctx::which(program) {
        requirements.push("install_harness");
    }
    let token=digest(&serde_json::to_vec(&json!({"copy":session.map(|s|actions::revision(s,None)),"sync":sync_plan.as_ref().map(|p|&p.token),"target":target,"launch":req.launch})).unwrap());
    let data = json!({"applied":false,"operations":[{"kind":"resume","session_id":req.ids[0],"target_harness":target,"destination":session.map(|s|&s.path),"requirements":requirements}],"sync_operations":sync_plan.as_ref().map(|p|&p.operations),"expected":{"resume":token},"launch":session.map(|s|{let(p,a)=launch_args(s);json!({"program":p,"args":a,"folder":s.path})})});
    let blocked = sync_plan
        .as_ref()
        .and_then(blocked)
        .or_else(|| (!requirements.is_empty()).then_some(Status::DecisionRequired));
    if let Some(r) = ready("resume", req, data.clone(), &token, blocked) {
        return r;
    }
    drop(_guard);
    if let Some(p) = sync_plan {
        sync_req.expected = BTreeMap::from([("sync".into(), p.token)]);
        let result = sync(&sync_req);
        if !matches!(result.status, Status::Ok) {
            return actions::response(
                "resume",
                result.status,
                json!({"applied":true,"sync":result.data}),
                result.diagnostics,
            );
        }
    }
    let catalog = Catalog::load_all();
    let Some(group) = catalog.find(&req.ids[0]) else {
        return actions::error("resume", "conversation_not_found", &req.ids[0]);
    };
    if group
        .copies
        .iter()
        .filter(|c| c.session().harness == target || c.session().key() == group.id)
        .any(|c| matches!(c.state, "conflict" | "unknown" | "push" | "pull"))
    {
        return actions::error(
            "resume",
            "pending_changes",
            "Sync or resolve pending changes before opening",
        );
    }
    let Some(session) = group
        .copies
        .iter()
        .find(|c| c.session().harness == target)
        .and_then(|c| c.local.as_ref())
    else {
        return actions::error(
            "resume",
            "local_copy_required",
            "Finish syncing the target harness first",
        );
    };
    if !Path::new(&session.path).is_dir() {
        return actions::error("resume", "invalid_destination", "Choose an existing work folder");
    }
    if crate::agents::in_use_by_agent_now(session) {
        return actions::error("resume", "conversation_active", "Close the running agent first");
    }
    let mut data = data;
    data["applied"] = json!(true);
    let (program, args) = launch_args(session);
    data["launch"] = json!({"program":program,"args":args,"folder":session.path});
    if req.launch {
        match launch_terminal(session) {
            Ok(pid) => data["terminal_pid"] = json!(pid),
            Err(e) => return actions::error("resume", "launch_failed", e.to_string()),
        }
    }
    actions::response("resume", Status::Ok, data, vec![])
}
