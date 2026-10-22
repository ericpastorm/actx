//! Explicit, noninteractive actions shared by frontends. Previews do not write anything.
use crate::{
    api::{self, Diagnostic, Response, Status},
    commands::Args,
    ctx::{Result, ctx},
    harness, inspection,
    lock::{LockError, SharedLock},
    session::Session,
    state::{self, State},
    store, ui,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    List,
    Preview,
    Add,
    Delete,
    Sync,
    Convert,
    Resume,
    Settings,
    Configure,
    CreateProject,
    Messages,
    Candidates,
    Compare,
    Resolve,
    Recoveries,
    Restore,
    Diagnostics,
    Automatic,
    DeleteProject,
    Adoption,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub action: Action,
    #[serde(default)]
    pub ids: Vec<String>,
    pub project: Option<String>,
    #[serde(default)]
    pub general: bool,
    #[serde(default)]
    pub all: bool,
    pub folder: Option<String>,
    pub target: Option<String>,
    pub choice: Option<String>,
    pub backup: Option<String>,
    pub adoption: Option<crate::adoption::Rule>,
    pub root: Option<String>,
    pub lang: Option<String>,
    pub harnesses: Option<Vec<String>>,
    #[serde(default)]
    pub launch: bool,
    #[serde(default)]
    pub folders: BTreeMap<String, String>,
    #[serde(default)]
    pub apply: bool,
    #[serde(default)]
    pub cancel: bool,
    #[serde(default)]
    pub expected: BTreeMap<String, String>,
}
impl Request {
    pub fn args(&self) -> Args {
        Args {
            project: self.project.clone(),
            general: self.general,
            all: self.all,
            target: self.target.clone(),
            ..Default::default()
        }
    }
    fn command(&self) -> &'static str {
        match self.action {
            Action::List => "ls",
            Action::Preview => "sync_preview",
            Action::Add => "add",
            Action::Delete => "delete",
            Action::Sync => "sync",
            Action::Convert => "convert",
            Action::Resume => "resume",
            Action::Settings => "settings",
            Action::Configure => "configure",
            Action::CreateProject => "create_project",
            Action::Messages => "messages",
            Action::Candidates => "candidates",
            Action::Compare => "compare",
            Action::Resolve => "resolve",
            Action::Recoveries => "recoveries",
            Action::Restore => "restore",
            Action::Diagnostics => "diagnostics",
            Action::Automatic => "automatic",
            Action::DeleteProject => "delete_project",
            Action::Adoption => "adoption",
        }
    }
}
#[derive(Serialize)]
pub struct Operation {
    pub kind: &'static str,
    pub session_id: String,
    pub title: String,
    pub collection: String,
    pub destination: Option<String>,
    pub revision: String,
    pub requirements: Vec<&'static str>,
}
#[derive(Serialize)]
pub struct Outcome {
    pub session_id: String,
    pub success: bool,
    pub message: Option<String>,
}
pub struct Prepared {
    pub root: PathBuf,
    pub state: State,
    pub remote: HashMap<String, Session>,
    pub selected: Vec<Session>,
    pub operations: Vec<Operation>,
    pub collection: String,
}
pub(crate) fn response(
    command: &'static str,
    status: Status,
    data: Value,
    diagnostics: Vec<Diagnostic>,
) -> Response<Value> {
    Response {
        schema_version: api::SCHEMA_VERSION,
        command,
        status,
        data: Some(data),
        diagnostics,
    }
}
pub(crate) fn error(command: &'static str, code: &'static str, message: impl Into<String>) -> Response<Value> {
    Response::error(command, Diagnostic::new(code, message))
}
pub(crate) fn revision(s: &Session, remote: Option<&Session>) -> String {
    let mut bytes = format!("{}:{}:{}:{}", s.key(), s.weight, s.updated.to_bits(), s.path).into_bytes();
    match crate::integrity::fresh(s) {
        Ok(hash) => bytes.extend_from_slice(hash.as_bytes()),
        Err(e) => {
            bytes.extend_from_slice(e.to_string().as_bytes());
            crate::inspection::record_error(e.to_string());
        }
    }
    if let Some(src) = &s.src {
        stamp(src, &mut bytes);
    }
    if s.harness == "opencode" {
        stamp(&ctx().opencode_db, &mut bytes);
        stamp(
            &PathBuf::from(format!("{}-wal", ctx().opencode_db.display())),
            &mut bytes,
        );
    }
    if let Some(r) = remote {
        bytes.extend_from_slice(r.project.as_deref().unwrap_or_default().as_bytes());
        bytes.extend_from_slice(&serde_json::to_vec(r.meta()).unwrap_or_default());
        stamp(&r.store_dir().join("meta.json"), &mut bytes);
    }
    format!(
        "{:016x}",
        bytes.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(*b))
            .wrapping_mul(0x100000001b3))
    )
}
fn stamp(path: &Path, bytes: &mut Vec<u8>) {
    if let Ok(m) = std::fs::metadata(path) {
        bytes.extend_from_slice(format!("{}:{:?}", m.len(), m.modified()).as_bytes());
    }
}
pub(crate) fn acquire(root: &Path) -> std::result::Result<SharedLock, Diagnostic> {
    SharedLock::acquire(root, Duration::ZERO, |_| {}).map_err(|e| match e {
        LockError::Busy(h) => Diagnostic::new(
            "shared_folder_busy",
            format!("{} is writing to the shared folder", h.host),
        ),
        LockError::Io(e) => Diagnostic::new("lock_failed", e.to_string()),
    })
}
pub(crate) fn prepare(req: &Request) -> std::result::Result<Prepared, Diagnostic> {
    api::validate_config()?;
    if req.general && req.project.is_some() {
        return Err(Diagnostic::new("invalid_scope", "Choose a project or General"));
    }
    if req.ids.is_empty() {
        return Err(Diagnostic::new(
            "selection_required",
            "Select at least one physical conversation ID",
        ));
    }
    let root = store::store_root(true)
        .ok_or_else(|| Diagnostic::new("shared_folder_unavailable", "A reachable shared folder is required"))?;
    let st = state::load();
    let project = req.project.clone().or_else(|| {
        if matches!(req.action, Action::Add) && !req.general {
            std::env::current_dir()
                .ok()
                .and_then(|p| crate::project::current(&root, &st, &p.to_string_lossy()))
        } else {
            None
        }
    });
    let collection = match project {
        Some(name) => store::find_project(&root, &name)
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .ok_or_else(|| Diagnostic::new("project_not_found", name))?,
        None => store::GENERAL.to_string(),
    };
    if let Some(folder) = &req.folder
        && !Path::new(folder).is_dir()
    {
        return Err(Diagnostic::new("invalid_destination", "Choose an existing work folder"));
    }
    if matches!(req.action, Action::Add)
        && collection != store::GENERAL
        && let Some(folder) = &req.folder
        && let Some(meta) = crate::project::metadata(&root, &collection)
        && let Some(expected) = meta["git"]["root"].as_str()
        && crate::git::git_id(folder).and_then(|g| g.root).as_deref() != Some(expected)
    {
        return Err(Diagnostic::new(
            "project_binding_failed",
            "The selected work folder belongs to another repository",
        ));
    }
    let remote: HashMap<_, _> = store::stored_sessions(&root)
        .into_iter()
        .map(|s| (s.key(), s))
        .collect();
    let keys = req.ids.iter().cloned().collect();
    let local: HashMap<_, _> = if matches!(req.action, Action::Add) {
        harness::local_keys(&keys).into_iter().map(|s| (s.key(), s)).collect()
    } else {
        HashMap::new()
    };
    let mut selected = Vec::new();
    let mut operations = Vec::new();
    for id in &req.ids {
        if selected.iter().any(|s: &Session| s.key() == *id) {
            continue;
        }
        let s = if matches!(req.action, Action::Add) {
            local.get(id)
        } else {
            remote.get(id)
        }
        .ok_or_else(|| Diagnostic::new("conversation_not_found", id))?;
        if s.harness == "antigravity" {
            return Err(Diagnostic::new(
                "unsupported_sync_harness",
                "Antigravity supports conversion, not native shared synchronization",
            ));
        }
        if matches!(req.action, Action::Delete)
            && (req.general || req.project.is_some())
            && s.project.as_deref() != Some(&collection)
        {
            return Err(Diagnostic::new("selection_outside_scope", id));
        }
        let mut requirements = Vec::new();
        if matches!(req.action, Action::Add)
            && remote
                .get(id)
                .is_some_and(|r| matches!(st.status(Some(s), Some(r)), "pull" | "conflict"))
        {
            requirements.push("resolve_conflict");
        }
        let r = remote.get(id);
        operations.push(Operation {
            kind: if matches!(req.action, Action::Add) {
                "add"
            } else {
                "delete"
            },
            session_id: id.clone(),
            title: s.title.clone(),
            collection: if matches!(req.action, Action::Add) {
                collection.clone()
            } else {
                s.project.clone().unwrap_or_default()
            },
            destination: if matches!(req.action, Action::Add) {
                if collection == store::GENERAL {
                    Some(s.path.clone())
                } else {
                    req.folder
                        .clone()
                        .or_else(|| crate::project::folder(&root, &collection, &st))
                        .or_else(|| Some(s.path.clone()))
                }
            } else {
                None
            },
            revision: revision(s, r),
            requirements,
        });
        selected.push(s.clone());
    }
    if !inspection::errors().is_empty() {
        return Err(Diagnostic::new("inspection_failed", inspection::errors().join("; ")));
    }
    Ok(Prepared {
        root,
        state: st,
        remote,
        selected,
        operations,
        collection,
    })
}
/// Called with the shared lock held, after checking the selection against current metadata.
pub(crate) fn save_selected(
    root: &Path,
    collection: &str,
    selected: &[&Session],
    st: &mut State,
    remote: &HashMap<String, Session>,
) -> Vec<Outcome> {
    selected
        .iter()
        .map(|s| {
            outcome(
                &s.key(),
                store::push_one(root, collection, s, crate::git::git_info(&s.path), st, remote)
                    .and_then(|_| crate::adoption::allow(root, &s.key())),
            )
        })
        .collect()
}
pub(crate) fn delete_selected(selected: &[&Session], st: &mut State) -> Vec<Outcome> {
    selected
        .iter()
        .map(|s| {
            let result = s
                .store_dir()
                .ancestors()
                .nth(3)
                .ok_or_else(|| crate::ctx::Error("Missing shared root".into()))
                .and_then(|root| crate::adoption::suppress(root, &s.key()))
                .and_then(|_| store::remove_stored(s));
            if result.is_ok() {
                st.base.remove(&s.key());
                st.folders.remove(&format!("session:{}", s.key()));
            }
            outcome(&s.key(), result)
        })
        .collect()
}
fn outcome(id: &str, result: Result<()>) -> Outcome {
    Outcome {
        session_id: id.into(),
        success: result.is_ok(),
        message: result.err().map(|e| e.to_string()),
    }
}

pub fn run(req: Request) -> Response<Value> {
    let writes = req.apply && !req.cancel;
    let response = execute(req);
    if writes && response.data.as_ref().is_some_and(|data| data["applied"] == true) {
        crate::gui_index::invalidate();
    }
    response
}
fn execute(req: Request) -> Response<Value> {
    let _quiet = ui::Silence::new();
    let command = req.command();
    if req.cancel {
        return response(command, Status::Cancelled, json!({"applied":false}), vec![]);
    }
    match req.action {
        Action::List => return serde_response(api::list(&req.args())),
        Action::Preview => return serde_response(api::preview(&req.args())),
        Action::Sync => return crate::engine::sync(&req),
        Action::Convert => return crate::engine::convert(&req),
        Action::Resume => return crate::engine::resume(&req),
        Action::Adoption => return crate::adoption::configure(&req),
        Action::Compare | Action::Resolve | Action::Recoveries | Action::Restore => return crate::recovery::run(&req),
        Action::Settings
        | Action::Configure
        | Action::CreateProject
        | Action::Messages
        | Action::Candidates
        | Action::Diagnostics
        | Action::Automatic
        | Action::DeleteProject => {
            return crate::desktop_api::run(&req);
        }
        _ => {}
    }
    let inspect = || {
        let _guard = inspection::Guard::new();
        prepare(&req)
    };
    let mut prepared = match inspect() {
        Ok(p) => p,
        Err(d) => return Response::error(command, d),
    };
    let data = |p: &Prepared, applied: bool, results: &[Outcome]| json!({"applied":applied,"operations":p.operations,"results":results,"expected":p.operations.iter().map(|o|(o.session_id.clone(),o.revision.clone())).collect::<BTreeMap<_,_>>()});
    if prepared
        .operations
        .iter()
        .any(|o| o.requirements.contains(&"resolve_conflict"))
    {
        return response(command, Status::Conflict, data(&prepared, false, &[]), vec![]);
    }
    if !req.apply {
        return response(command, Status::Ok, data(&prepared, false, &[]), vec![]);
    }
    let _lock = match acquire(&prepared.root) {
        Ok(l) => l,
        Err(d) => return Response::error(command, d),
    };
    prepared = match inspect() {
        Ok(p) => p,
        Err(d) => return Response::error(command, d),
    };
    if prepared
        .operations
        .iter()
        .any(|o| req.expected.get(&o.session_id) != Some(&o.revision))
    {
        return response(
            command,
            Status::DecisionRequired,
            data(&prepared, false, &[]),
            vec![Diagnostic::new(
                "preview_required",
                "Preview again and confirm the current revision of every selected copy",
            )],
        );
    }
    if prepared.operations.iter().any(|o| !o.requirements.is_empty()) {
        return response(command, Status::Conflict, data(&prepared, false, &[]), vec![]);
    }
    if matches!(req.action, Action::Add)
        && prepared.collection != store::GENERAL
        && (req.folder.is_some() || !crate::project::has_binding(&prepared.root, &prepared.collection))
    {
        let folder = req.folder.clone().or_else(|| {
            let folders: std::collections::BTreeSet<_> = prepared
                .selected
                .iter()
                .map(|s| {
                    crate::git::git_id(&s.path)
                        .map(|g| g.top)
                        .unwrap_or_else(|| s.path.clone())
                })
                .filter(|p| !p.is_empty() && p != &ctx().home_str())
                .collect();
            if folders.len() == 1 {
                folders.first().cloned()
            } else {
                None
            }
        });
        if let Some(folder) = folder
            && let Err(e) = crate::project::bind(&prepared.root, &prepared.collection, &folder, &mut prepared.state)
        {
            return error(command, "project_binding_failed", e.to_string());
        }
    }
    let selected: Vec<_> = prepared.selected.iter().collect();
    let results = if matches!(req.action, Action::Add) {
        save_selected(
            &prepared.root,
            &prepared.collection,
            &selected,
            &mut prepared.state,
            &prepared.remote,
        )
    } else {
        delete_selected(&selected, &mut prepared.state)
    };
    let mut diagnostics = Vec::new();
    if let Err(e) = state::save(&mut prepared.state) {
        diagnostics.push(Diagnostic::new("state_save_failed", e.to_string()));
    }
    let status = if results.iter().any(|r| !r.success) || !diagnostics.is_empty() {
        Status::Incomplete
    } else {
        Status::Ok
    };
    response(command, status, data(&prepared, true, &results), diagnostics)
}
pub(crate) fn serde_response<T: Serialize>(r: Response<T>) -> Response<Value> {
    Response {
        schema_version: r.schema_version,
        command: r.command,
        status: r.status,
        data: r.data.map(|d| serde_json::to_value(d).expect("serializable API data")),
        diagnostics: r.diagnostics,
    }
}
pub fn stdin_request() {
    let mut data = Vec::new();
    if let Err(e) = std::io::stdin().take(1024 * 1024 + 1).read_to_end(&mut data) {
        api::print(error("api", "request_unreadable", e.to_string()));
        return;
    }
    if data.len() > 1024 * 1024 {
        api::print(error("api", "request_too_large", "Requests must be at most 1 MiB"));
        return;
    }
    match serde_json::from_slice(&data) {
        Ok(req) => api::print(run(req)),
        Err(e) => api::print(error("api", "invalid_request", e.to_string())),
    }
}
