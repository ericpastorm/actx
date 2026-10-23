//! Versioned, read-only operations for scripts and the desktop interface. No terminal parsing.
use crate::{
    catalog::{Catalog, Conversation},
    commands::{self, Args},
    ctx::ctx,
    inspection, profile,
    session::Session,
    state, store, sync_plan, ui,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Conflict,
    Incomplete,
    Error,
    DecisionRequired,
    Cancelled,
}
impl Status {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Ok => 0,
            Self::Error => 1,
            Self::Conflict => 3,
            Self::Incomplete => 4,
            Self::DecisionRequired => 5,
            Self::Cancelled => 0,
        }
    }
}
#[derive(Serialize)]
pub struct Diagnostic {
    pub code: &'static str,
    pub message: String,
    pub details: Value,
}
impl Diagnostic {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: json!({}),
        }
    }
}
#[derive(Serialize)]
pub struct Response<T: Serialize> {
    pub schema_version: u32,
    pub command: &'static str,
    pub status: Status,
    pub data: Option<T>,
    pub diagnostics: Vec<Diagnostic>,
}
impl<T: Serialize> Response<T> {
    pub(crate) fn error(command: &'static str, diagnostic: Diagnostic) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            command,
            status: Status::Error,
            data: None,
            diagnostics: vec![diagnostic],
        }
    }
}

#[derive(Serialize)]
pub struct Scope {
    pub project: Option<String>,
    pub general: bool,
    pub all: bool,
}
impl From<&Args> for Scope {
    fn from(args: &Args) -> Self {
        Self {
            project: args.project.clone(),
            general: args.general,
            all: args.all,
        }
    }
}
#[derive(Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Collection {
    pub kind: &'static str,
    pub id: Option<String>,
}
impl Collection {
    fn new(name: &str) -> Self {
        if name == store::GENERAL {
            Self {
                kind: "general",
                id: None,
            }
        } else {
            Self {
                kind: "project",
                id: Some(name.to_string()),
            }
        }
    }
}
#[derive(Serialize)]
pub struct Project {
    pub collection: Collection,
    pub local_folder: Option<String>,
    pub auto_add: Option<crate::adoption::Rule>,
}
#[derive(Serialize)]
pub struct Location {
    pub folder: String,
    pub updated_at: f64,
    pub weight: u64,
    pub weight_unit: &'static str,
    pub host: Option<String>,
}
impl Location {
    fn new(s: &Session, shared: bool) -> Self {
        let folder = if shared {
            s.meta
                .as_ref()
                .and_then(|m| m.get("path"))
                .and_then(Value::as_str)
                .unwrap_or(&s.path)
        } else {
            &s.path
        };
        Self {
            folder: folder.to_string(),
            updated_at: s.updated,
            weight: s.weight,
            weight_unit: match s.harness {
                "opencode" => "messages",
                "antigravity" => "steps",
                _ => "bytes",
            },
            host: if shared {
                s.meta
                    .as_ref()
                    .and_then(|m| m.get("host"))
                    .and_then(Value::as_str)
                    .map(String::from)
            } else {
                None
            },
        }
    }
}
#[derive(Serialize)]
pub struct Copy {
    pub id: String,
    pub harness: &'static str,
    pub state: &'static str,
    pub local: Option<Location>,
    pub shared: Option<Location>,
}
#[derive(Serialize)]
pub struct ConversationData {
    pub id: String,
    pub title: String,
    pub managed: bool,
    pub state: &'static str,
    pub updated_at: f64,
    pub collections: BTreeSet<Collection>,
    pub desired_harnesses: Vec<&'static str>,
    pub copies: Vec<Copy>,
}
impl From<&Conversation> for ConversationData {
    fn from(g: &Conversation) -> Self {
        let state = ["conflict", "unknown", "missing", "push", "pull", "new"]
            .into_iter()
            .find(|state| g.copies.iter().any(|c| c.state == *state))
            .unwrap_or(if g.managed { "synced" } else { "local" });
        Self {
            id: g.id.clone(),
            title: g.title.clone(),
            managed: g.managed,
            state,
            updated_at: g.updated,
            collections: g
                .copies
                .iter()
                .filter_map(|c| c.remote.as_ref()?.project.as_deref())
                .map(Collection::new)
                .collect(),
            desired_harnesses: profile::harnesses(&g.id),
            copies: g
                .copies
                .iter()
                .map(|c| Copy {
                    id: c.session().key(),
                    harness: c.session().harness,
                    state: c.state,
                    local: c.local.as_ref().map(|s| Location::new(s, false)),
                    shared: c.remote.as_ref().map(|s| Location::new(s, true)),
                })
                .collect(),
        }
    }
}
#[derive(Serialize)]
pub struct ListData {
    pub scope: Scope,
    pub shared_available: bool,
    pub complete: bool,
    pub projects: Vec<Project>,
    pub conversations: Vec<ConversationData>,
}

pub(crate) fn validate_config() -> Result<(), Diagnostic> {
    match std::fs::read(&ctx().config_file) {
        Ok(data) if serde_json::from_slice::<Value>(&data).is_ok_and(|v| v.is_object()) => Ok(()),
        Ok(_) => Err(Diagnostic::new(
            "invalid_config",
            "The actx configuration must be a JSON object",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Diagnostic::new("config_unreadable", error.to_string())),
    }
}
fn validate_project(root: Option<&std::path::Path>, args: &Args) -> Result<(), Diagnostic> {
    if let Some(name) = &args.project {
        let Some(root) = root else {
            return Err(Diagnostic::new(
                "shared_folder_unavailable",
                "Cannot resolve a project without the shared folder",
            ));
        };
        if store::find_project(root, name).is_none() {
            let mut error = Diagnostic::new("project_not_found", "The requested actx project does not exist");
            error.details = json!({"project":name});
            return Err(error);
        }
    }
    Ok(())
}
fn inspection_diagnostics() -> Vec<Diagnostic> {
    inspection::errors()
        .into_iter()
        .map(|e| Diagnostic::new("inspection_failed", e))
        .collect()
}

/// A catalog without previews, terminal output, state migration or cache writes.
pub fn list(args: &Args) -> Response<ListData> {
    list_with_catalog(args, None)
}
pub(crate) fn list_with_catalog(args: &Args, supplied: Option<Catalog>) -> Response<ListData> {
    let _quiet = ui::Silence::new();
    let _guard = inspection::Guard::new();
    if let Err(error) = validate_config() {
        return Response::error("ls", error);
    }
    let root = store::store_root(true);
    if let Err(error) = validate_project(root.as_deref(), args) {
        return Response::error("ls", error);
    }
    let catalog = supplied.unwrap_or_else(|| Catalog::load_for(args.all));
    let st = state::load();
    let projects = root
        .as_ref()
        .map(|root| {
            store::collection_dirs(root)
                .into_iter()
                .filter_map(|dir| {
                    let name = dir.file_name()?.to_string_lossy();
                    if (args.general && name != store::GENERAL)
                        || args.project.as_ref().is_some_and(|p| !p.eq_ignore_ascii_case(&name))
                    {
                        return None;
                    }
                    Some(Project {
                        collection: Collection::new(&name),
                        local_folder: st
                            .folders
                            .get(&crate::project::key(&name))
                            .cloned()
                            .or_else(|| crate::project::remembered_folder(root, &name, &st)),
                        auto_add: (name != store::GENERAL).then(|| crate::adoption::rule(root, &name)),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut conversations: Vec<ConversationData> = catalog.scoped(args).into_iter().map(Into::into).collect();
    if root.is_none() {
        for conversation in conversations.iter_mut().filter(|c| c.managed) {
            conversation.state = "unknown";
            for copy in &mut conversation.copies {
                copy.state = "unknown";
            }
        }
    }
    let mut diagnostics = inspection_diagnostics();
    if root.is_none() && store::configured_root().is_some() {
        diagnostics.push(Diagnostic::new(
            "shared_folder_unavailable",
            "Only locally known conversations are available",
        ));
    }
    let complete = diagnostics.is_empty();
    let status = if !complete {
        Status::Incomplete
    } else if conversations.iter().any(|c| c.state == "conflict") {
        Status::Conflict
    } else {
        Status::Ok
    };
    Response {
        schema_version: SCHEMA_VERSION,
        command: "ls",
        status,
        data: Some(ListData {
            scope: args.into(),
            shared_available: root.is_some(),
            complete,
            projects,
            conversations,
        }),
        diagnostics,
    }
}

#[derive(Serialize)]
pub struct PlannedOperation {
    pub kind: &'static str,
    pub session_id: String,
    pub conversation_id: String,
    pub title: String,
    pub collection: Option<Collection>,
    pub source_harness: String,
    pub target_harness: Option<String>,
    pub target_id: Option<String>,
    pub reason: &'static str,
    pub requirements: Vec<&'static str>,
    pub destination: Option<String>,
}
impl From<sync_plan::Operation> for PlannedOperation {
    fn from(op: sync_plan::Operation) -> Self {
        let mut requirements = op.notices;
        match op.reason_code {
            "conversation_active" | "source_active" | "destination_active" => requirements.push("close_agent"),
            "work_folder_required" => requirements.push("destination_required"),
            "linked_copy_missing" => requirements.push("restore_linked_copy"),
            "harness_not_installed" | "opencode_cli_required" => requirements.push("install_harness"),
            "source_download_required" | "source_sync_required" | "linked_source_changed" => {
                requirements.push("recheck_after_sync")
            }
            _ => {}
        }
        if op.kind == "conflict" {
            requirements.push("resolve_conflict");
        }
        requirements.sort();
        requirements.dedup();
        Self {
            kind: op.kind,
            session_id: op.session_id,
            conversation_id: op.conversation_id,
            title: op.title,
            collection: op.project_id.as_deref().map(Collection::new),
            source_harness: op.source_harness,
            target_harness: op.target_harness,
            target_id: op.target_id,
            reason: op.reason_code,
            requirements,
            destination: op.destination,
        }
    }
}
#[derive(Serialize)]
pub struct PlanData {
    pub scope: Scope,
    pub mode: &'static str,
    pub read_only: bool,
    pub valid: bool,
    pub obsolete: bool,
    pub counts: BTreeMap<&'static str, usize>,
    pub operations: Vec<PlannedOperation>,
}

/// A fresh plan is data, not an executable token. Execution must re-read and validate everything.
pub fn preview(args: &Args) -> Response<PlanData> {
    let _quiet = ui::Silence::new();
    let _guard = inspection::Guard::new();
    if let Err(error) = validate_config() {
        return Response::error("sync_preview", error);
    }
    let Some(root) = store::store_root(true) else {
        let configured = store::configured_root().is_some();
        return Response::error(
            "sync_preview",
            Diagnostic::new(
                if configured {
                    "shared_folder_unavailable"
                } else {
                    "shared_folder_not_configured"
                },
                "A reachable shared folder is required for a synchronization preview",
            ),
        );
    };
    let args = commands::scope(args, &root);
    if let Err(error) = validate_project(Some(&root), &args) {
        return Response::error("sync_preview", error);
    }
    let plan = sync_plan::inspect(&root, &args);
    let mut diagnostics: Vec<_> = plan
        .errors
        .into_iter()
        .map(|e| Diagnostic::new("inspection_failed", e))
        .collect();
    if plan.changed {
        diagnostics.push(Diagnostic::new(
            "snapshot_changed",
            "Data changed while preparing this plan; request a new preview",
        ));
    }
    let valid = diagnostics.is_empty();
    let mut counts = BTreeMap::new();
    for op in &plan.operations {
        *counts.entry(op.kind).or_insert(0) += 1;
    }
    let status = if !valid {
        Status::Incomplete
    } else if counts.contains_key("conflict") {
        Status::Conflict
    } else {
        Status::Ok
    };
    Response {
        schema_version: SCHEMA_VERSION,
        command: "sync_preview",
        status,
        data: Some(PlanData {
            scope: (&args).into(),
            mode: if args.auto { "auto" } else { "manual" },
            read_only: true,
            valid,
            obsolete: plan.changed,
            counts,
            operations: plan.operations.into_iter().map(Into::into).collect(),
        }),
        diagnostics,
    }
}

pub fn print<T: Serialize>(response: Response<T>) {
    let status = response.status.exit_code();
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut stdout = std::io::stdout().lock();
        serde_json::to_writer(&mut stdout, &response)?;
        writeln!(stdout)?;
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("Cannot write JSON output: {error}");
        std::process::exit(1);
    }
    if status != 0 {
        std::process::exit(status);
    }
}
