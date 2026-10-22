//! Revision-checked comparisons and recovery, shared by terminal and desktop frontends.
use crate::{
    actions::{self, Action, Request},
    api::{Response, Status},
    ctx::{Error, Result, ctx},
    harness, inspection,
    session::Session,
    state, store, tr,
    turns::Turn,
    ui,
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

fn error(command: &'static str, code: &'static str, e: impl ToString) -> Response<Value> {
    actions::error(command, code, e.to_string())
}
pub(crate) fn confirmation(
    req: &Request,
    command: &'static str,
    data: &mut Value,
    token: String,
) -> Option<Response<Value>> {
    data["expected"] = json!({command:token});
    data["applied"] = json!(false);
    if !req.apply {
        return Some(actions::response(command, Status::Ok, data.clone(), vec![]));
    }
    if req.expected.get(command) != Some(&token) {
        return Some(actions::response(
            command,
            Status::DecisionRequired,
            data.clone(),
            vec![crate::api::Diagnostic::new(
                "preview_required",
                "Preview again; the selected data changed",
            )],
        ));
    }
    None
}
struct Comparison {
    left: Session,
    right: Session,
    native: bool,
    root: std::path::PathBuf,
    token: String,
    data: Value,
}
fn preview_turns(turns: &[Turn]) -> Value {
    let start = turns.len().saturating_sub(16);
    json!({"total_messages":turns.len(),"omitted_messages":start,"messages":turns[start..].iter().map(|t|json!({"role":t.who,"text":ui::take_chars(&t.text,4000),"truncated":t.text.chars().count()>4000})).collect::<Vec<_>>()})
}
fn compare(req: &Request) -> Result<Comparison> {
    let _inspect = inspection::Guard::new();
    let [key] = req.ids.as_slice() else {
        return Err(Error("Choose exactly one conversation".into()));
    };
    let root = store::store_root(true).ok_or_else(|| Error("Shared folder unavailable".into()))?;
    let (left, right, native) = if let Some(target) = req.target.as_deref() {
        let (a, b) = crate::multisync::pair(key, target)?;
        (a, b, false)
    } else {
        let left = harness::local_keys(&HashSet::from([key.clone()]))
            .into_iter()
            .find(|s| s.key() == *key)
            .ok_or_else(|| Error("Local conversation not found".into()))?;
        let right = store::stored_sessions(&root)
            .into_iter()
            .find(|s| s.key() == *key)
            .ok_or_else(|| Error("Shared conversation not found".into()))?;
        (left, right, true)
    };
    let a = harness::get(left.harness).local_turns(&left)?.0;
    let b = if native {
        let fix = crate::git::fixer_for(right.meta(), &left.path);
        harness::get(right.harness).stored_turns(&right, fix.as_ref())?.0
    } else {
        harness::get(right.harness).local_turns(&right)?.0
    };
    let normalize = |t: &Turn| {
        crate::turns::fit(vec![t.clone()], "actx", None)
            .into_iter()
            .map(|t| (t.who, t.text))
            .collect::<Vec<_>>()
    };
    let common = a
        .iter()
        .zip(&b)
        .take_while(|(a, b)| normalize(a) == normalize(b))
        .count();
    let token=crate::engine::digest(&serde_json::to_vec(&json!({"left":actions::revision(&left,native.then_some(&right)),"right":actions::revision(&right,None),"choice":req.choice,"target":req.target})).unwrap());
    if !inspection::errors().is_empty() {
        return Err(Error(inspection::errors().join("; ")));
    }
    let mut choices = if native {
        vec!["local", "remote", "merge", "defer"]
    } else {
        vec!["repair", "local", "merge", "defer"]
    };
    let mut l = preview_turns(&a);
    l["session_id"] = json!(left.key());
    l["harness"] = json!(left.harness);
    l["host"] = json!(ctx().host);
    l["folder"] = json!(left.path);
    let mut r = preview_turns(&b);
    r["session_id"] = json!(right.key());
    r["harness"] = json!(right.harness);
    r["host"] = if native {
        right.meta()["host"].clone()
    } else {
        json!(ctx().host)
    };
    let merge_preview = if native {
        let fix = crate::git::fixer_for(right.meta(), &left.path);
        match harness::get(left.harness).other_branch(&left, &right, fix.as_ref()) {
            Ok(delta) => {
                let text = crate::merge::branch_summary(&delta, crate::ctx::jstr(right.meta(), "host"));
                json!({"text":text,"maximum_characters":100000,"preserves_remote_branch":true})
            }
            Err(e) => {
                choices.retain(|c| *c != "merge");
                json!({"available":false,"reason":e.to_string()})
            }
        }
    } else {
        json!(null)
    };
    let data = json!({"merge_preview":merge_preview,"comparison":{"left":l,"right":r,"common_messages":common,"left_messages_after_common":a.len()-common,"right_messages_after_common":b.len()-common,"compacted":left.harness=="codex"&&left.src.as_ref().is_some_and(|p|crate::codex::compaction_key(p).ok().flatten().is_some())},"choices":choices,"operations":[{"kind":"resolve","session_id":left.key(),"title":left.title,"target_harness":req.target,"resolution":req.choice,"preserves_alternative":true}]});
    Ok(Comparison {
        left,
        right,
        native,
        root,
        token,
        data,
    })
}
fn resolve(req: &Request) -> Response<Value> {
    if req.choice.as_deref() == Some("defer") {
        return actions::response("resolve", Status::Cancelled, json!({"applied":false}), vec![]);
    }
    let mut p = match compare(req) {
        Ok(p) => p,
        Err(e) => return error("resolve", "comparison_unavailable", e),
    };
    let Some(choice) = req
        .choice
        .as_deref()
        .filter(|c| p.data["choices"].as_array().is_some_and(|v| v.contains(&json!(c))))
    else {
        return error("resolve", "selection_required", "Choose a listed resolution");
    };
    if let Some(r) = confirmation(req, "resolve", &mut p.data, p.token.clone()) {
        return r;
    }
    let _local = match crate::engine::local_guard() {
        Ok(g) => g,
        Err(e) => return error("resolve", "local_action_busy", e),
    };
    let _shared = match actions::acquire(&p.root) {
        Ok(g) => g,
        Err(e) => return Response::error("resolve", e),
    };
    p = match compare(req) {
        Ok(p) => p,
        Err(e) => return error("resolve", "comparison_unavailable", e),
    };
    if let Some(r) = confirmation(req, "resolve", &mut p.data, p.token.clone()) {
        return r;
    }
    if crate::agents::in_use_by_agent_now(&p.left) || (!p.native && crate::agents::in_use_by_agent_now(&p.right)) {
        return error(
            "resolve",
            "conversation_active",
            "Close the selected agent before resolving",
        );
    }
    let result = (|| -> Result<()> {
        let mut st = state::load();
        if !p.native {
            crate::multisync::resolve_pair(&p.root, &p.left, &p.right, choice, &mut st)?;
        } else {
            let project = p.right.project.as_deref().unwrap_or(store::GENERAL);
            match choice {
                "remote" => {
                    crate::commands::install_one(&p.right, Some(&p.left), &p.left.path, &mut st)?;
                }
                "local" => {
                    let fix = crate::git::fixer_for(p.right.meta(), &p.left.path);
                    let title = tr!(
                        "{title} (branch from {host})",
                        title = p.right.title,
                        host = crate::ctx::jstr(p.right.meta(), "host")
                    );
                    let branch =
                        harness::get(p.right.harness).branch_copy(&p.right, &p.left.path, fix.as_ref(), &title)?;
                    store::push_one(
                        &p.root,
                        project,
                        &branch,
                        crate::git::git_info(&branch.path),
                        &mut st,
                        &HashMap::new(),
                    )?;
                    store::push_one(
                        &p.root,
                        project,
                        &p.left,
                        crate::git::git_info(&p.left.path),
                        &mut st,
                        &HashMap::new(),
                    )?;
                    p.data["preserved_branch"] = json!(branch.key());
                }
                "merge" => crate::merge::merge_one(&p.root, &mut p.left, &p.right, &mut st)?,
                _ => return Err(Error("Unsupported resolution".into())),
            }
        }
        state::save(&mut st)
    })();
    match result {
        Ok(()) => {
            p.data["applied"] = json!(true);
            actions::response("resolve", Status::Ok, p.data, vec![])
        }
        Err(e) => error("resolve", "resolution_failed", e),
    }
}
fn backup_id(b: &state::Backup) -> String {
    format!("backup:{}", crate::engine::digest(b.file.to_string_lossy().as_bytes()))
}
fn entries(req: &Request) -> Value {
    let mut ids: HashSet<_> = req.ids.iter().cloned().collect();
    if !ids.is_empty() {
        crate::multisync::extend_keys(&mut ids);
    }
    let mut all:Vec<_>=state::backups().into_iter().map(|b|json!({"id":backup_id(&b),"kind":"backup","session_id":format!("{}:{}",b.harness,b.id),"title":b.title,"folder":b.path,"at":b.at})).collect();
    all.extend(crate::quarantine::recovery_entries());
    let publication_error = match crate::publication::entries() {
        Ok(entries) => {
            all.extend(entries);
            None
        }
        Err(e) => Some(e.to_string()),
    };
    all.retain(|b| ids.is_empty() || b["session_id"].as_str().is_some_and(|id| ids.contains(id)));
    all.sort_by(|a, b| {
        b["at"]
            .as_f64()
            .partial_cmp(&a["at"].as_f64())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    json!({"entries":all,"publication_error":publication_error})
}
fn restore(req: &Request) -> Response<Value> {
    let Some(id) = req.backup.as_deref() else {
        return error("restore", "selection_required", "Choose a recovery entry");
    };
    if id.starts_with("publication:") {
        return crate::publication::restore(req);
    }
    let backup = state::backups().into_iter().find(|b| backup_id(b) == id);
    let quarantine = id.strip_prefix("quarantine:");
    let (session_id, revision, destination) = if let Some(b) = &backup {
        let content = match std::fs::read(&b.file) {
            Ok(v) => v,
            Err(e) => return error("restore", "backup_unreadable", e),
        };
        (
            format!("{}:{}", b.harness, b.id),
            crate::engine::digest(&content),
            b.path.clone(),
        )
    } else if let Some(id) = quarantine {
        let Some(e) = crate::quarantine::recovery_entries()
            .into_iter()
            .find(|e| e["id"] == req.backup.as_deref().unwrap())
        else {
            return error("restore", "backup_missing", "Recovery entry unavailable");
        };
        let revision = match crate::quarantine::recovery_token(id) {
            Ok(v) => v,
            Err(e) => return error("restore", "backup_unreadable", e),
        };
        (
            e["session_id"].as_str().unwrap_or_default().to_string(),
            revision,
            e["folder"].as_str().unwrap_or_default().to_string(),
        )
    } else {
        return error("restore", "backup_missing", "Recovery entry unavailable");
    };
    let _inspect = inspection::Guard::new();
    let current = harness::local_keys(&HashSet::from([session_id.clone()]))
        .into_iter()
        .find(|s| s.key() == session_id);
    let token = crate::engine::digest(
        &serde_json::to_vec(
            &json!({"id":id,"backup":revision,"current":current.as_ref().map(|s|actions::revision(s,None))}),
        )
        .unwrap(),
    );
    let mut data = json!({"operations":[{"kind":"restore","session_id":session_id,"destination":destination,"backup":id}],"preserves_current":true});
    if let Some(r) = confirmation(req, "restore", &mut data, token) {
        return r;
    }
    let _local = match crate::engine::local_guard() {
        Ok(g) => g,
        Err(e) => return error("restore", "local_action_busy", e),
    };
    if current.as_ref().is_some_and(crate::agents::in_use_by_agent_now) {
        return error(
            "restore",
            "conversation_active",
            "Close the selected agent before restoring",
        );
    }
    drop(_inspect);
    let result = if let Some(b) = &backup {
        crate::commands::restore_one(b, current.as_ref()).map(|_| ())
    } else {
        crate::quarantine::recover(quarantine.unwrap())
    };
    match result {
        Ok(()) => {
            data["applied"] = json!(true);
            actions::response("restore", Status::Ok, data, vec![])
        }
        Err(e) => error("restore", "restore_failed", e),
    }
}
pub fn run(req: &Request) -> Response<Value> {
    let _quiet = ui::Silence::new();
    match req.action {
        Action::Recoveries => {
            let data = entries(req);
            let diagnostics = data["publication_error"]
                .as_str()
                .map(|e| crate::api::Diagnostic::new("shared_recovery_unavailable", e))
                .into_iter()
                .collect::<Vec<_>>();
            actions::response(
                "recoveries",
                if diagnostics.is_empty() {
                    Status::Ok
                } else {
                    Status::Incomplete
                },
                data,
                diagnostics,
            )
        }
        Action::Restore => restore(req),
        Action::Resolve => resolve(req),
        _ => match compare(req) {
            Ok(p) => actions::response("compare", Status::Ok, p.data, vec![]),
            Err(e) => error("compare", "comparison_unavailable", e),
        },
    }
}

pub fn cli(command: &str, id: Option<&str>, target: Option<&str>, choice: Option<&str>, json_output: bool) {
    let key = if command == "recoveries" || command == "restore" {
        id.map(String::from)
    } else if let Some(id) = id {
        Some(id.into())
    } else {
        let catalog = crate::catalog::Catalog::load();
        let rows: Vec<_> = catalog.groups.iter().filter(|g| g.managed).map(|g| g.row()).collect();
        let Some(selected) = ui::fzf(&rows, &tr!("Select a conversation"), false, true)
            .into_iter()
            .next()
        else {
            return;
        };
        catalog
            .groups
            .iter()
            .find(|g| g.selection_key() == selected)
            .map(|g| g.id.clone())
    };
    let mut value =
        json!({"action":command,"ids":key.iter().cloned().collect::<Vec<_>>(),"target":target,"choice":choice});
    if command == "restore" {
        value["ids"] = json!([]);
        value["backup"] = json!(id);
    }
    let req: Request = serde_json::from_value(value.clone()).unwrap();
    if json_output {
        crate::api::print(run(&req));
        return;
    }
    if command == "recoveries" {
        let r = run(&req);
        for diagnostic in &r.diagnostics {
            ui::warn(&diagnostic.message);
        }
        let items = r.data.unwrap_or_default();
        let rows: Vec<_> = items["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| ui::Row {
                key: e["id"].as_str().unwrap_or_default().into(),
                line: format!(
                    "{} · {} · {}{}",
                    e["kind"].as_str().unwrap_or_default(),
                    e["title"].as_str().unwrap_or_default(),
                    ui::ago(e["at"].as_f64().unwrap_or_default()),
                    if e["kind"] == "publication" {
                        format!(
                            " · {}",
                            crate::publication::status_label(e["status"].as_str().unwrap_or_default())
                        )
                    } else {
                        String::new()
                    }
                ),
                preview: format!("{}\n{}", e["session_id"], e["folder"]),
            })
            .collect();
        if let Some(id) = ui::fzf(&rows, &tr!("Restoring the conversation"), false, true)
            .into_iter()
            .next()
        {
            cli("restore", Some(&id), None, None, false);
        }
        return;
    }
    if command == "compare" || command == "resolve" {
        let mut inspection = value.clone();
        inspection["action"] = json!("compare");
        let r = run(&serde_json::from_value(inspection).unwrap());
        if r.status != Status::Ok {
            for d in r.diagnostics {
                ui::warn(&d.message);
            }
            return;
        }
        let data = r.data.unwrap();
        for (side, label) in [("left", tr!("Local")), ("right", tr!("Shared"))] {
            println!(
                "\n{label}: {}",
                data["comparison"][side]["session_id"].as_str().unwrap_or_default()
            );
            for m in data["comparison"][side]["messages"].as_array().into_iter().flatten() {
                println!(
                    "{}: {}",
                    m["role"].as_str().unwrap_or_default(),
                    m["text"].as_str().unwrap_or_default()
                );
            }
        }
        if command == "compare" {
            return;
        }
        if choice.is_none() {
            let rows: Vec<_> = data["choices"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| ui::Row {
                    key: c.as_str().unwrap().into(),
                    line: choice_label(c.as_str().unwrap()),
                    preview: choice_label(c.as_str().unwrap()),
                })
                .collect();
            let Some(c) = ui::fzf(&rows, &tr!("Choose what to do"), false, false)
                .into_iter()
                .next()
            else {
                return;
            };
            value["choice"] = json!(c);
        }
    }
    let req: Request = serde_json::from_value(value.clone()).unwrap();
    let preview = run(&req);
    if preview.status == Status::Cancelled {
        return;
    }
    if preview.status != Status::Ok {
        for d in preview.diagnostics {
            ui::warn(&d.message);
        }
        return;
    }
    if command == "restore"
        && let Some(data) = &preview.data
    {
        if data["replaces_shared"] == true {
            ui::warn(&tr!(
                "The current shared version will be preserved as another recovery entry"
            ));
        }
        for message in data["recovery_preview"]["messages"].as_array().into_iter().flatten() {
            println!(
                "{}: {}",
                message["role"].as_str().unwrap_or_default(),
                message["text"].as_str().unwrap_or_default()
            );
        }
        for op in data["operations"].as_array().into_iter().flatten() {
            println!(
                "{} → {}",
                op["source"].as_str().unwrap_or_default(),
                op["destination"].as_str().unwrap_or_default()
            );
        }
    }
    if !ui::confirm(&tr!("Apply")) {
        return;
    }
    value["apply"] = json!(true);
    value["expected"] = preview.data.unwrap()["expected"].clone();
    let r = run(&serde_json::from_value(value).unwrap());
    if r.status == Status::Ok {
        ui::ok(&tr!("Action finished"));
    } else {
        for d in r.diagnostics {
            ui::warn(&d.message);
        }
    }
}
pub fn choice_label(choice: &str) -> String {
    match choice {
        "local" => tr!("Keep local"),
        "remote" => tr!("Keep shared"),
        "merge" => tr!("Merge both"),
        "repair" => tr!("Repair linked copy"),
        _ => tr!("Deferred"),
    }
}

pub fn commands() -> Vec<clap::Command> {
    use clap::{Arg, ArgAction, Command};
    ["compare", "resolve", "recoveries"]
        .into_iter()
        .map(|name| {
            Command::new(name)
                .about(match name {
                    "compare" => tr!("Compare conversation histories"),
                    "resolve" => tr!("Resolve a conversation conflict"),
                    _ => tr!("Browse recoverable conversations"),
                })
                .arg(Arg::new("conversation").value_name("harness:id"))
                .arg(Arg::new("target").long("target").value_parser(harness::CONVERTS))
                .arg(
                    Arg::new("choice")
                        .long("choice")
                        .value_parser(["local", "remote", "merge", "repair", "defer"]),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help(tr!("Show the profile as JSON")),
                )
        })
        .collect()
}
