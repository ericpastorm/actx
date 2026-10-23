//! Reviewed organization actions. No terminal input and no writes during preview.
use super::{MovePlan, RenamePlan};
use crate::{
    actions::{self, Action, Request},
    api::{Diagnostic, Response, Status},
    catalog::Catalog,
    inspection, state, store,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

enum Plan {
    Move(MovePlan, HashMap<String, String>),
    Rename(RenamePlan),
}
impl Plan {
    fn root(&self) -> &PathBuf {
        match self {
            Self::Move(p, _) => &p.root,
            Self::Rename(p) => &p.root,
        }
    }
    fn revision(&self) -> String {
        match self {
            Self::Move(p, _) => p.revision(),
            Self::Rename(p) => p.revision(),
        }
    }
    fn data(&self) -> Value {
        match self {
            Self::Move(p, _) => {
                json!({"applied":false,"destination":p.destination,"conversation_count":p.identities.len(),"operations":p.copies.iter().filter(|s|s.project.as_deref()!=Some(&p.destination)).map(|s|json!({"kind":"move","session_id":s.key(),"title":s.title,"collection":s.project,"source":store::collection_label(s.project.as_deref().unwrap_or_default()),"destination":store::collection_label(&p.destination),"harness":s.harness})).collect::<Vec<_>>()})
            }
            Self::Rename(p) => {
                json!({"applied":false,"source":p.old,"destination":p.new,"operations":[{"kind":"rename_project","title":p.old,"source":p.old,"destination":p.new}]})
            }
        }
    }
    fn apply(&self) -> crate::ctx::Result<Option<String>> {
        let mut state = state::load();
        match self {
            Self::Move(p, folders) => {
                p.apply(&mut state, folders)?;
            }
            Self::Rename(p) => p.apply(&mut state)?,
        }
        Ok(state::save(&mut state).err().map(|e| e.to_string()))
    }
}
fn invalid(message: &str) -> Diagnostic {
    Diagnostic::new("invalid_move", message)
}
fn prepare(req: &Request) -> Result<Plan, Diagnostic> {
    let _guard = inspection::Guard::new();
    crate::api::validate_config()?;
    let root = store::store_root(true)
        .ok_or_else(|| Diagnostic::new("shared_folder_unavailable", "A reachable shared folder is required"))?;
    let plan = match req.action {
        Action::RenameProject => {
            if req.general || !req.ids.is_empty() || req.choice.is_some() {
                return Err(invalid("Choose a project and a new name"));
            }
            let old = req.project.as_deref().ok_or_else(|| invalid("Choose a project"))?;
            let new = req.target.as_deref().ok_or_else(|| invalid("Choose a new name"))?;
            Plan::Rename(RenamePlan::prepare(&root, old, new).map_err(|e| invalid(&e.to_string()))?)
        }
        Action::Move => {
            if req.ids.is_empty() {
                return Err(Diagnostic::new(
                    "selection_required",
                    "Select saved conversations to move",
                ));
            }
            if req.general == req.project.is_some()
                || req.choice.as_deref().is_some_and(|s| s != "create")
                || req.general && req.choice.is_some()
            {
                return Err(invalid("Choose an existing project, a new project, or General"));
            }
            let destination = req.project.as_deref().unwrap_or(store::GENERAL);
            // A project request cannot address the reserved General directory by name.
            if !req.general && !crate::project::valid_name(destination) {
                return Err(invalid("Invalid destination project name"));
            }
            let catalog = Catalog::load();
            let mut seen = HashSet::new();
            let mut copies = Vec::new();
            let mut folders = HashMap::new();
            for id in &req.ids {
                let group = catalog
                    .find(id)
                    .ok_or_else(|| Diagnostic::new("conversation_not_found", id))?;
                if !seen.insert(&group.id) {
                    continue;
                }
                let remote = group.remote();
                if remote.is_empty() {
                    return Err(invalid("Only saved conversations can be moved"));
                }
                if remote.iter().all(|s| s.project.as_deref() == Some(destination)) {
                    continue;
                }
                copies.extend(remote);
                for s in group.copies.iter().filter_map(|c| c.local.as_ref()) {
                    folders.insert(s.key(), s.path.clone());
                }
            }
            Plan::Move(
                MovePlan::prepare(&root, copies, destination, req.choice.is_some())
                    .map_err(|e| invalid(&e.to_string()))?,
                folders,
            )
        }
        _ => return Err(invalid("Unsupported organization action")),
    };
    let errors = inspection::errors();
    if !errors.is_empty() || inspection::changed() {
        return Err(Diagnostic::new("inspection_failed", errors.join("; ")));
    }
    Ok(plan)
}
pub(crate) fn run(req: &Request) -> Response<Value> {
    if crate::work::cancelled() {
        return actions::response(req.command(), Status::Cancelled, json!({"applied":false}), vec![]);
    }
    let response = execute(req);
    if crate::work::cancelled() && response.diagnostics.iter().any(|d| d.message == "Operation cancelled") {
        return actions::response(req.command(), Status::Cancelled, json!({"applied":false}), vec![]);
    }
    response
}
fn execute(req: &Request) -> Response<Value> {
    let command = req.command();
    let plan = match prepare(req) {
        Ok(p) => p,
        Err(d) => return Response::error(command, d),
    };
    let token = plan.revision();
    let mut data = plan.data();
    let has_operations = data["operations"].as_array().is_some_and(|ops| !ops.is_empty());
    if has_operations {
        data["expected"] = json!({command:token});
    }
    if !req.apply {
        return actions::response(command, Status::Ok, data, vec![]);
    }
    if req.expected.get(command) != Some(&token) {
        return actions::response(
            command,
            Status::DecisionRequired,
            data,
            vec![Diagnostic::new(
                "preview_required",
                "Review the current source and destination before applying",
            )],
        );
    }
    if !has_operations {
        return actions::response(command, Status::Ok, data, vec![]);
    }
    let _local = match crate::engine::local_guard() {
        Ok(guard) => guard,
        Err(e) => return actions::error(command, "local_action_busy", e.to_string()),
    };
    let _shared = match actions::acquire(plan.root()) {
        Ok(guard) => guard,
        Err(d) => return Response::error(command, d),
    };
    let current = match prepare(req) {
        Ok(p) => p,
        Err(d) => return Response::error(command, d),
    };
    if current.revision() != token {
        data = current.data();
        data["expected"] = json!({command:current.revision()});
        return actions::response(
            command,
            Status::DecisionRequired,
            data,
            vec![Diagnostic::new(
                "preview_required",
                "The shared folder changed; review the move again",
            )],
        );
    }
    match current.apply() {
        Ok(state_error) => {
            data["applied"] = json!(true);
            if let Some(error) = state_error {
                actions::response(
                    command,
                    Status::Incomplete,
                    data,
                    vec![Diagnostic::new("state_save_failed", error)],
                )
            } else {
                actions::response(command, Status::Ok, data, vec![])
            }
        }
        Err(e) => actions::error(command, "move_failed", e.to_string()),
    }
}
