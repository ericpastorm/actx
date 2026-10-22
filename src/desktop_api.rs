//! Small configuration and preview services used by the desktop, available to scripts too.
use crate::{
    actions::{self, Action, Request},
    api::{Diagnostic, Response, Status},
    harness, i18n, inspection, profile, state, store, ui,
};
use serde_json::{Value, json};
use std::path::Path;
pub fn run(req: &Request) -> Response<Value> {
    let _quiet = ui::Silence::new();
    if let Err(d) = crate::api::validate_config() {
        return Response::error("settings", d);
    }
    match req.action {
        Action::Settings => actions::response(
            "settings",
            Status::Ok,
            json!({"root":store::configured_root(),"root_from_env":std::env::var("ACTX_ROOT").is_ok_and(|s|!s.is_empty()),"lang":i18n::code(),"harnesses":profile::current()["harnesses"],"retention":profile::current()["retention"],"tools":harness::CONVERTS.iter().map(|h|json!({"id":h,"installed":harness::get(h).installed(),"sync":harness::syncs(h)})).collect::<Vec<_>>()}),
            vec![],
        ),
        Action::Messages => messages(req),
        Action::Candidates => candidates(req),
        Action::Configure => configure(req),
        Action::CreateProject => create_project(req),
        _ => actions::error("api", "invalid_action", "Unsupported action"),
    }
}
fn candidates(req: &Request) -> Response<Value> {
    let _guard = inspection::Guard::new();
    let mut args = req.args();
    args.all = true;
    args.project = None;
    args.general = false;
    let mut listing = crate::api::list(&args);
    listing.command = "candidates";
    if let Some(name) = &req.project {
        let Some(root) = store::store_root(true) else {
            return actions::error(
                "candidates",
                "shared_folder_unavailable",
                "A reachable shared folder is required",
            );
        };
        let Some(folder) = crate::project::folder(&root, name, &state::load()) else {
            return actions::error(
                "candidates",
                "invalid_destination",
                "Choose a bound project work folder",
            );
        };
        if !req.all {
            let keys: std::collections::HashSet<_> = harness::all_local()
                .into_iter()
                .filter(|s| crate::project::candidate(&folder, s))
                .map(|s| s.key())
                .collect();
            if let Some(data) = listing.data.as_mut() {
                data.conversations
                    .retain(|g| g.copies.iter().any(|c| keys.contains(&c.id)));
            }
        }
    }
    actions::serde_response(listing)
}
fn messages(req: &Request) -> Response<Value> {
    let _guard = inspection::Guard::new();
    let [id] = req.ids.as_slice() else {
        return actions::error("messages", "selection_required", "Choose one physical copy");
    };
    let local = harness::local_keys(&std::collections::HashSet::from([id.clone()]));
    let source = local.into_iter().next().or_else(|| {
        store::store_root(true).and_then(|r| store::stored_sessions(&r).into_iter().find(|s| s.key() == *id))
    });
    let Some(source) = source else {
        return actions::error("messages", "conversation_not_found", id);
    };
    let turns = if source.meta.is_some() {
        harness::get(source.harness).stored_turns(&source, None)
    } else {
        harness::get(source.harness).local_turns(&source)
    };
    match turns {
        Ok((turns, _)) => {
            let total = turns.len();
            let messages:Vec<_>=turns.iter().skip(total.saturating_sub(20)).map(|t|json!({"role":t.who,"text":crate::ui::take_chars(&t.text,4000),"timestamp_ms":t.ts,"truncated":t.text.chars().count()>4000})).collect();
            let errors = inspection::errors();
            actions::response(
                "messages",
                if errors.is_empty() {
                    Status::Ok
                } else {
                    Status::Incomplete
                },
                json!({"session_id":id,"total_messages":total,"messages":messages,"truncated":total>20}),
                errors
                    .into_iter()
                    .map(|m| Diagnostic::new("inspection_failed", m))
                    .collect(),
            )
        }
        Err(e) => actions::error("messages", "history_unreadable", e.to_string()),
    }
}
fn confirmation(req: &Request, command: &'static str, data: &mut Value, token: String) -> Option<Response<Value>> {
    data["applied"] = json!(false);
    data["expected"] = json!({command:token});
    if !req.apply {
        return Some(actions::response(command, Status::Ok, data.clone(), vec![]));
    }
    if req.expected.get(command) != Some(&token) {
        return Some(actions::response(
            command,
            Status::DecisionRequired,
            data.clone(),
            vec![Diagnostic::new(
                "preview_required",
                "Preview again and confirm current settings",
            )],
        ));
    }
    None
}
fn configure(req: &Request) -> Response<Value> {
    if req.root.as_ref().is_some_and(|p| !Path::new(p).is_dir()) {
        return actions::error("configure", "invalid_shared_folder", "Choose an existing shared folder");
    }
    if req
        .lang
        .as_deref()
        .is_some_and(|l| !["en", "es", "zh", "zh-CN"].contains(&l))
    {
        return actions::error("configure", "invalid_language", "Supported languages: en, es, zh-CN");
    }
    let mut config = profile::current();
    let before = config.clone();
    if let Some(root) = &req.root {
        config["root"] = json!(root);
    }
    if let Some(lang) = &req.lang {
        config["lang"] = json!(match i18n::parse(lang) {
            i18n::Language::En => "en",
            i18n::Language::Es => "es",
            i18n::Language::ZhCn => "zh-CN",
        });
    }
    if let Some(h) = &req.harnesses
        && let Err(e) = profile::configure(&mut config, Some(&h.join(",")), None, false, None)
    {
        return actions::error("configure", "invalid_harness", e.to_string());
    }
    let token = crate::engine::digest(&serde_json::to_vec(&json!({"before":before,"after":config})).unwrap());
    let mut data = json!({"operations":[{"kind":"configure","destination":config["root"],"language":config["lang"],"harnesses":config["harnesses"]}]});
    if let Some(r) = confirmation(req, "configure", &mut data, token) {
        return r;
    }
    let _local = match crate::engine::local_guard() {
        Ok(l) => l,
        Err(e) => return actions::error("configure", "local_action_busy", e.to_string()),
    };
    if profile::current() != before {
        return actions::response(
            "configure",
            Status::DecisionRequired,
            data,
            vec![Diagnostic::new("settings_changed", "Settings changed while confirming")],
        );
    }
    match profile::save(&config) {
        Ok(()) => {
            data["applied"] = json!(true);
            actions::response("configure", Status::Ok, data, vec![])
        }
        Err(e) => actions::error("configure", "config_save_failed", e.to_string()),
    }
}
fn create_project(req: &Request) -> Response<Value> {
    let Some(name) = req.project.as_deref().filter(|s| {
        s.chars().next().is_some_and(char::is_alphanumeric)
            && s.chars().all(|c| c.is_alphanumeric() || "_.-".contains(c))
    }) else {
        return actions::error(
            "create_project",
            "invalid_project_name",
            "Use letters, numbers, '-', '_' or '.'",
        );
    };
    let Some(folder) = req.folder.as_deref().filter(|p| Path::new(p).is_dir()) else {
        return actions::error(
            "create_project",
            "invalid_destination",
            "Choose an existing work folder",
        );
    };
    let Some(root) = store::store_root(true) else {
        return actions::error(
            "create_project",
            "shared_folder_unavailable",
            "Configure a reachable shared folder first",
        );
    };
    if store::find_project(&root, name).is_some() {
        return actions::error("create_project", "project_exists", name);
    }
    let token = crate::engine::digest(
        &serde_json::to_vec(&json!({"name":name,"folder":folder,"root":root,"git":crate::git::git_info(folder)}))
            .unwrap(),
    );
    let mut data = json!({"operations":[{"kind":"create_project","collection":name,"destination":folder}]});
    if let Some(r) = confirmation(req, "create_project", &mut data, token) {
        return r;
    }
    let _shared = match actions::acquire(&root) {
        Ok(l) => l,
        Err(d) => return Response::error("create_project", d),
    };
    if store::find_project(&root, name).is_some() {
        return actions::error("create_project", "project_exists", name);
    }
    let mut st = state::load();
    let result = std::fs::create_dir(root.join(name))
        .map_err(crate::ctx::Error::from)
        .and_then(|_| crate::project::bind(&root, name, folder, &mut st))
        .and_then(|_| state::save(&mut st));
    match result {
        Ok(()) => {
            data["applied"] = json!(true);
            actions::response("create_project", Status::Ok, data, vec![])
        }
        Err(e) => actions::error("create_project", "project_create_failed", e.to_string()),
    }
}
