//! Reviewed local cleanup, shared with the native desktop. No permanent deletion action.
use crate::{
    actions::{self, Action, Request},
    api::{Diagnostic, Response, Status},
    clean,
    ctx::{Error, Result},
    harness, ui,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub days: u64,
    pub max_messages: usize,
    pub max_chars: usize,
    pub min_similar: usize,
    pub harnesses: Vec<String>,
    pub rules: Vec<String>,
    pub folder: Option<String>,
    pub contains: Option<String>,
}
impl Default for Settings {
    fn default() -> Self {
        let o = clean::Options::default();
        Self {
            days: o.days,
            max_messages: o.max_messages,
            max_chars: o.max_chars,
            min_similar: o.min_similar,
            harnesses: o.harnesses.iter().map(|s| s.to_string()).collect(),
            rules: ["agents", "repeated", "tests", "short"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            folder: None,
            contains: None,
        }
    }
}
impl Settings {
    fn options(&self) -> Result<clean::Options> {
        if self.days > 36500
            || self.max_messages > 100000
            || self.max_chars > 1000000
            || self.min_similar < 2
            || self.harnesses.is_empty()
            || self
                .rules
                .iter()
                .any(|r| !["agents", "repeated", "tests", "short", "old"].contains(&r.as_str()))
        {
            return Err(Error("Invalid cleanup criteria".into()));
        }
        let tools = self
            .harnesses
            .iter()
            .map(|n| harness::known(n).ok_or_else(|| Error("Unknown cleanup harness".into())))
            .collect::<Result<Vec<_>>>()?;
        let folder = self
            .folder
            .as_ref()
            .filter(|s| !s.trim().is_empty())
            .map(std::path::PathBuf::from);
        if folder.as_ref().is_some_and(|p| !p.is_dir()) {
            return Err(Error("Choose an existing work folder".into()));
        }
        Ok(clean::Options {
            days: self.days,
            max_messages: self.max_messages,
            max_chars: self.max_chars,
            min_similar: self.min_similar,
            harnesses: tools,
            rules: self.rules.iter().cloned().collect(),
            folder,
            contains: self.contains.clone().filter(|s| !s.trim().is_empty()),
            ..Default::default()
        })
    }
}
fn rows(candidates: &[clean::Candidate]) -> Vec<Value> {
    candidates.iter().map(|c|json!({"id":c.session.key(),"title":c.session.title,"harness":c.session.harness,"folder":c.session.path,"updated_at":c.session.updated,"bytes":c.bytes,"reasons":c.reasons,"preview":ui::take_chars(&c.preview,8000),"revision":c.fingerprint})).collect()
}
pub fn run(req: &Request) -> Response<Value> {
    let command = req.command();
    let _quiet = ui::Silence::new();
    let _ = ui::take_reports();
    if crate::store::configured_root().is_some() && crate::store::store_root(true).is_none() {
        return actions::error(
            command,
            "shared_folder_unavailable",
            "Connect the shared folder before cleaning local conversations",
        );
    }
    let settings = req.cleanup.clone().unwrap_or_default();
    let options = match settings.options() {
        Ok(o) => o,
        Err(e) => return actions::error(command, "invalid_cleanup_criteria", e.to_string()),
    };
    let candidates = match clean::scan(&options) {
        Ok(c) => c,
        Err(_) if crate::work::cancelled() => {
            return actions::response(command, Status::Cancelled, json!({"applied":false}), vec![]);
        }
        Err(e) => return actions::error(command, "cleanup_scan_failed", e.to_string()),
    };
    let diagnostics: Vec<_> = ui::take_reports()
        .into_iter()
        .map(|s| Diagnostic::new("cleanup_scan_incomplete", s))
        .collect();
    if matches!(req.action, Action::CleanupScan) {
        return actions::response(
            command,
            if diagnostics.is_empty() {
                Status::Ok
            } else {
                Status::Incomplete
            },
            json!({"candidates":rows(&candidates),"settings":settings,"read_only":true}),
            diagnostics,
        );
    }
    if req.ids.is_empty() {
        return actions::error(
            command,
            "selection_required",
            "Select local conversations to quarantine",
        );
    }
    let mut selected: Vec<_> = candidates
        .into_iter()
        .filter(|c| req.ids.contains(&c.session.key()))
        .collect();
    selected.sort_by_key(|c| c.session.key());
    if req
        .ids
        .iter()
        .any(|id| !selected.iter().any(|c| c.session.key() == *id))
    {
        return actions::error(
            command,
            "cleanup_selection_changed",
            "A selected conversation changed or is protected; analyze again",
        );
    }
    if !diagnostics.is_empty() {
        return actions::response(command, Status::Incomplete, json!({"applied":false}), diagnostics);
    }
    for candidate in &mut selected {
        if let Err(e) = clean::seal(candidate) {
            return actions::error(command, "cleanup_selection_changed", e.to_string());
        }
    }
    let token =
        crate::engine::digest(&serde_json::to_vec(&json!({"settings":settings,"selected":rows(&selected)})).unwrap());
    let mut data = json!({"operations":selected.iter().map(|c|json!({"kind":"quarantine","session_id":c.session.key(),"title":c.session.title,"destination":c.session.path,"reasons":c.reasons,"bytes":c.bytes})).collect::<Vec<_>>()});
    if let Some(response) = crate::recovery::confirmation(req, command, &mut data, token) {
        return response;
    }
    let _local = match crate::engine::local_guard() {
        Ok(l) => l,
        Err(e) => return actions::error(command, "local_action_busy", e.to_string()),
    };
    let selected: Vec<_> = selected.iter().collect();
    match crate::quarantine::apply(&selected) {
        Ok(results) => {
            let status = if crate::work::cancelled() {
                Status::Cancelled
            } else if results.iter().all(|r| r.success) {
                Status::Ok
            } else {
                Status::Incomplete
            };
            data["applied"] = json!(true);
            data["results"] = json!(results);
            actions::response(command, status, data, vec![])
        }
        Err(e) => actions::error(command, "quarantine_failed", e.to_string()),
    }
}
