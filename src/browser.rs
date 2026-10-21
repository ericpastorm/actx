//! Compact conversation browser. Actions always resolve a physical copy after family selection.
use crate::{
    catalog::{Catalog, Conversation},
    commands::{self, Args},
    ctx::{Result, ctx},
    harness, profile, store, tr,
    ui::{self, Row},
};
use serde_json::{Value, json};
use std::collections::HashSet;

fn scoped<'a>(catalog: &'a Catalog, args: &Args) -> Vec<&'a Conversation> {
    if let Some(name) = &args.project
        && let Some(root) = store::store_root(true)
        && store::find_project(&root, name).is_none()
    {
        ui::die(&tr!("Project {name} doesn't exist", name = name));
    }
    catalog.scoped(args)
}
pub fn list(args: &Args) {
    let catalog = Catalog::load();
    let groups = scoped(&catalog, args);
    println!("{}", ui::bold(&crate::catalog::header()));
    for g in &groups {
        println!("{}", g.line());
    }
    if let Some(root) = store::store_root(true) {
        for dir in store::project_dirs(&root) {
            let name = dir.file_name().unwrap_or_default().to_string_lossy();
            if !args.general
                && args.project.as_ref().is_none_or(|p| p.eq_ignore_ascii_case(&name))
                && !groups
                    .iter()
                    .any(|g| g.remote().iter().any(|r| r.project.as_deref() == Some(&name)))
            {
                println!("{} {}", name, tr!("(empty)"));
            }
        }
    }
    if groups.is_empty() {
        println!("{}", tr!("No conversations in this view"));
    }
    if !args.all {
        let hidden = catalog.unmanaged();
        if hidden > 0 {
            println!(
                "{}",
                ui::dim(&tr!(
                    "{n} local conversations actx is not syncing are hidden; use --all to see them",
                    n = hidden
                ))
            );
        }
    }
    if groups.iter().any(|g| {
        g.copies
            .iter()
            .any(|c| ["push", "pull", "new", "conflict", "unknown"].contains(&c.state))
    }) {
        println!("\n{}", tr!("There are pending changes: {cmd}", cmd = "actx sync"));
    }
}

pub fn browse(args: Args) {
    if !ui::interactive() && ctx().pick.is_none() {
        list(&args);
        return;
    }
    loop {
        let catalog = Catalog::load();
        let groups = scoped(&catalog, &args);
        let rows: Vec<_> = groups.iter().map(|g| g.row()).collect();
        if rows.is_empty() {
            list(&args);
            return;
        }
        let Some(pick) = ui::picker(
            &rows,
            &format!(
                "{}\n{}",
                tr!("Conversations · TAB select · Enter open · Alt-H harnesses · Delete remove · Esc back"),
                crate::catalog::header()
            ),
            true,
            true,
            &["enter", "alt-h", "delete"],
            &[],
        ) else {
            return;
        };
        let selected: Vec<_> = pick.keys.iter().filter_map(|k| catalog.find(k)).cloned().collect();
        match pick.action.as_str() {
            "alt-h" => {
                if let Err(e) = edit(&selected) {
                    ui::fail(&e.to_string());
                }
            }
            "delete" => remove(&selected),
            _ if selected.len() == 1 => open(&selected[0], args.print),
            _ => ui::warn(&tr!(
                "Select one conversation to open; multiple selection is available for harnesses and removal"
            )),
        }
        if ctx().pick.is_some() {
            return;
        }
    }
}

pub fn resume(args: Args) {
    let catalog = Catalog::load();
    let rows: Vec<_> = scoped(&catalog, &args).iter().map(|g| g.row()).collect();
    if let Some(key) = ui::fzf(&rows, &tr!("Choose a conversation to open"), false, true).first()
        && let Some(g) = catalog.find(key)
    {
        open(g, args.print);
    }
}

fn choice(group: &Conversation) -> Option<(String, bool)> {
    let config = profile::current();
    if let Some(preferred) = config
        .get("preferred_harnesses")
        .and_then(|v| v.get(&group.id))
        .and_then(Value::as_str)
        && group.copies.iter().any(|c| c.session().harness == preferred)
    {
        return Some((preferred.into(), false));
    }
    let names: std::collections::BTreeSet<_> = group
        .copies
        .iter()
        .map(|c| c.session().harness)
        .chain(profile::harnesses(&group.id))
        .collect();
    if let Some(preferred) = config.get("open_harness").and_then(Value::as_str)
        && names.contains(preferred)
    {
        return Some((preferred.into(), false));
    }
    if names.len() == 1 {
        return Some((names.into_iter().next()?.into(), false));
    }
    let rows: Vec<_> = names
        .into_iter()
        .map(|n| {
            let copies: Vec<_> = group.copies.iter().filter(|c| c.session().harness == n).collect();
            Row {
                key: n.into(),
                line: format!(
                    "{}  {}",
                    harness::get(n).label(),
                    if copies.is_empty() {
                        tr!("pending creation")
                    } else {
                        copies
                            .iter()
                            .map(|c| crate::catalog::state_label(c.state))
                            .collect::<Vec<_>>()
                            .join(" · ")
                    }
                ),
                preview: group.preview(),
            }
        })
        .collect();
    let picked = ui::picker(
        &rows,
        &tr!("Open in which harness? Enter opens · Ctrl-R remembers and opens"),
        false,
        true,
        &["enter", "ctrl-r"],
        &[],
    )?;
    Some((picked.keys.into_iter().next()?, picked.action == "ctrl-r"))
}
fn open(group: &Conversation, print: bool) {
    let Some((name, remember)) = choice(group) else { return };
    let Some(tool) = harness::known(&name) else { return };
    let shared = group.remote();
    if !shared.is_empty() {
        commands::sync(Args {
            all: true,
            skip_retention: true,
            keys: Some(shared.iter().map(|s| s.key()).collect()),
            ..Default::default()
        });
    } else if store::configured_root().is_some()
        && store::store_root(true).is_none()
        && !ui::confirm(&tr!("Shared folder unavailable. Open the local copy without syncing?"))
    {
        return;
    }
    let refreshed = Catalog::load();
    let Some(group) = refreshed.find(&group.id) else {
        ui::warn(&tr!("Conversation no longer available"));
        return;
    };
    if group
        .copies
        .iter()
        .any(|c| ["conflict", "unknown", "push", "pull"].contains(&c.state))
    {
        ui::warn(&tr!(
            "This conversation still has pending changes or a conflict; sync or resolve it before opening"
        ));
        return;
    }
    let copies: Vec<_> = group
        .copies
        .iter()
        .filter(|c| c.session().harness == tool)
        .filter_map(|c| c.local.as_ref())
        .collect();
    let session = match copies.as_slice() {
        [] => {
            ui::warn(&tr!("No local copy in this harness yet; finish syncing first"));
            return;
        }
        [s] => *s,
        _ => {
            let rows: Vec<_> = copies
                .iter()
                .map(|s| Row {
                    key: s.key(),
                    line: format!("{} · {}", s.title, s.id),
                    preview: crate::commands::session_preview(s),
                })
                .collect();
            let Some(key) = ui::fzf(&rows, &tr!("Choose the physical copy to open"), false, true)
                .into_iter()
                .next()
            else {
                return;
            };
            let Some(s) = copies.into_iter().find(|s| s.key() == key) else {
                return;
            };
            s
        }
    };
    if !std::path::Path::new(&session.path).is_dir() {
        ui::warn(&tr!("Choose an existing work folder before opening this conversation"));
        return;
    }
    if remember {
        let mut config = profile::current();
        if !config["preferred_harnesses"].is_object() {
            config["preferred_harnesses"] = json!({});
        }
        config["preferred_harnesses"][&group.id] = json!(tool);
        if let Err(e) = profile::save(&config) {
            ui::warn(&e.to_string());
        }
    }
    if print {
        println!("{}", harness::get(tool).resume(&session.id, &session.path));
        return;
    }
    if crate::agents::in_use_by_agent_now(session) {
        ui::warn(&tr!("This conversation is already open; close its agent first"));
        return;
    }
    let program = if tool == "antigravity" { "agy" } else { tool };
    if !crate::ctx::which(program) {
        ui::warn(&tr!("The selected harness is not installed"));
        return;
    }
    let mut command = crate::ctx::console_command(program);
    command.current_dir(&session.path);
    match tool {
        "claude" => {
            command.args(["--resume", &session.id]);
        }
        "codex" => {
            command.args(["resume", &session.id]);
        }
        "opencode" => {
            command.args(["-s", &session.id]);
        }
        _ => {
            command.args(["--conversation", &session.id]);
        }
    }
    if let Err(e) = command.status() {
        ui::fail(&e.to_string());
    }
}

pub fn edit(groups: &[Conversation]) -> Result<()> {
    if groups.is_empty() {
        return Ok(());
    }
    let current = profile::harnesses(&groups[0].id);
    let modes = vec![
        Row {
            key: "::custom".into(),
            line: tr!("Choose harnesses for this conversation"),
            preview: groups[0].preview(),
        },
        Row {
            key: "::default".into(),
            line: tr!("Use the machine profile"),
            preview: tr!("Future machine profile changes will also apply to this conversation"),
        },
        Row {
            key: "::none".into(),
            line: tr!("No linked harnesses"),
            preview: tr!("Existing local copies are kept"),
        },
    ];
    let Some(mode) = ui::fzf(&modes, &tr!("Harness preferences · Esc cancels"), false, true)
        .into_iter()
        .next()
    else {
        return Ok(());
    };
    let names = if mode == "::custom" {
        let rows: Vec<_> = harness::CONVERTS
            .iter()
            .map(|n| Row {
                key: n.to_string(),
                line: format!(
                    "{}{}",
                    harness::get(n).label(),
                    if harness::get(n).installed() {
                        String::new()
                    } else {
                        format!(" · {}", tr!("not installed"))
                    }
                ),
                preview: tr!(
                    "Select the harnesses where you want to maintain a copy; missing copies will be created during sync"
                ),
            })
            .collect();
        let selected: Vec<_> = current.iter().map(|n| n.to_string()).collect();
        let Some(pick) = ui::picker(
            &rows,
            &tr!("Harnesses · TAB toggles · Enter saves · Esc cancels · existing copies are kept"),
            true,
            true,
            &[],
            &selected,
        ) else {
            return Ok(());
        };
        pick.keys.iter().filter_map(|k| harness::known(k)).collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut config = profile::current();
    if !config["conversation_profiles"].is_object() {
        config["conversation_profiles"] = json!({});
    }
    for g in groups {
        let keys: HashSet<_> = std::iter::once(g.id.clone())
            .chain(g.remote().into_iter().map(|s| s.key()))
            .collect();
        for key in keys {
            if mode == "::default" {
                config["conversation_profiles"].as_object_mut().unwrap().remove(&key);
            } else {
                config["conversation_profiles"][&key] = json!(names);
            }
        }
        if let Some(preferred) = config.get_mut("preferred_harnesses").and_then(Value::as_object_mut) {
            preferred.remove(&g.id);
        }
    }
    profile::save(&config)?;
    ui::ok(&tr!(
        "Harness preferences saved; new copies will be created during sync"
    ));
    Ok(())
}

fn remove(groups: &[Conversation]) {
    let rows = vec![
        Row {
            key: "::harness".into(),
            line: tr!("Change harness preferences"),
            preview: tr!("Stop maintaining a harness copy; keep all existing copies"),
        },
        Row {
            key: "::shared".into(),
            line: tr!("Delete shared copies"),
            preview: tr!("Keep local originals and projects"),
        },
        Row {
            key: "::project".into(),
            line: tr!("Delete a whole project"),
            preview: tr!("Removes every saved conversation in the chosen project; local originals are kept"),
        },
    ];
    match ui::fzf(&rows, &tr!("What do you want to remove? Esc cancels"), false, true)
        .first()
        .map(String::as_str)
    {
        Some("::harness") => {
            if let Err(e) = edit(groups) {
                ui::fail(&e.to_string());
            }
        }
        Some("::shared") => {
            let copies: Vec<_> = groups.iter().flat_map(Conversation::remote).collect();
            if copies.is_empty() {
                ui::warn(&tr!("No saved conversations to delete"));
                return;
            }
            if let Some(root) = store::store_root(true) {
                commands::delete_saved(&root, &copies.iter().collect::<Vec<_>>());
            }
        }
        Some("::project") => {
            let names: std::collections::BTreeSet<_> = groups
                .iter()
                .flat_map(Conversation::remote)
                .filter_map(|s| s.project)
                .filter(|n| n != store::GENERAL)
                .collect();
            let rows: Vec<_> = names
                .into_iter()
                .map(|n| Row {
                    key: n.clone(),
                    line: n,
                    preview: String::new(),
                })
                .collect();
            if let Some(name) = ui::fzf(&rows, &tr!("Choose the project to delete"), false, false)
                .into_iter()
                .next()
            {
                commands::rm(Args {
                    project: Some(name),
                    ..Default::default()
                });
            }
        }
        _ => {}
    }
}
