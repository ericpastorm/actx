//! Opt-in project rules. Normal browsing never scans unrelated local conversations.
use crate::{
    actions::{self, Request},
    api::{Response, Status},
    commands::Args,
    ctx::{Error, Result},
    harness, inspection, project,
    session::Session,
    state, store, tr, ui,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

fn yes() -> bool {
    true
}
fn tools() -> Vec<String> {
    harness::NAMES.iter().map(|s| s.to_string()).collect()
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "yes")]
    pub subfolders: bool,
    #[serde(default)]
    pub include_clones: bool,
    #[serde(default = "tools")]
    pub harnesses: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub exclude_titles: Vec<String>,
}
impl Default for Rule {
    fn default() -> Self {
        Self {
            enabled: false,
            subfolders: true,
            include_clones: false,
            harnesses: tools(),
            exclude: vec![],
            exclude_titles: vec![],
        }
    }
}
pub fn rule(root: &Path, name: &str) -> Rule {
    project::metadata(root, name)
        .and_then(|v| v.get("auto_add").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}
fn validate(rule: &Rule) -> Result<()> {
    if rule.harnesses.iter().any(|h| !harness::syncs(h)) || rule.harnesses.is_empty() {
        return Err(Error(
            "Choose Claude, Codex or opencode for native automatic additions".into(),
        ));
    }
    if rule.exclude.len() + rule.exclude_titles.len() > 64 {
        return Err(Error("Use at most 64 exclusion patterns".into()));
    }
    for pattern in rule.exclude.iter().chain(&rule.exclude_titles) {
        if pattern.len() > 256 || pattern.contains('\0') {
            return Err(Error("Exclusion pattern is too long or invalid".into()));
        }
    }
    for pattern in &rule.exclude {
        if Path::new(pattern).is_absolute() || pattern.split(['/', '\\']).any(|c| c == "..") {
            return Err(Error("Exclusions must be relative project paths".into()));
        }
    }
    Ok(())
}
fn matches(pattern: &str, value: &str) -> bool {
    if pattern.strip_suffix("/**").is_some_and(|base| matches(base, value)) {
        return true;
    }
    let pattern = pattern.replace('\\', "/");
    let escaped = regex::escape(&pattern)
        .replace(r"\*\*", ".*")
        .replace(r"\*", "[^/]*")
        .replace(r"\?", "[^/]");
    regex::Regex::new(&format!("(?i)^{escaped}$")).is_ok_and(|r| r.is_match(value))
}
fn ignored(root: &Path) -> Result<HashSet<String>> {
    match std::fs::read(root.join(".auto-add-ignored.json")) {
        Ok(data) => Ok(serde_json::from_slice(&data)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashSet::new()),
        Err(e) => Err(e.into()),
    }
}
/// Called under the store lock. Explicit delete must not be undone by the next automatic run.
pub fn suppress(root: &Path, key: &str) -> Result<()> {
    let mut keys = ignored(root)?;
    if keys.insert(key.into()) {
        save_ignored(root, &keys)?;
    }
    Ok(())
}
pub fn allow(root: &Path, key: &str) -> Result<()> {
    let mut keys = ignored(root)?;
    if keys.remove(key) {
        save_ignored(root, &keys)?;
    }
    Ok(())
}
fn save_ignored(root: &Path, keys: &HashSet<String>) -> Result<()> {
    let mut tmp = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer(tmp.as_file_mut(), keys)?;
    tmp.persist(root.join(".auto-add-ignored.json"))
        .map_err(|e| Error(e.to_string()))?;
    Ok(())
}

#[derive(Clone)]
pub struct Candidate {
    pub session: Session,
    pub project: String,
    pub reason: &'static str,
    pub eligible: bool,
}
pub fn candidates(root: &Path, args: &Args, override_rule: Option<(&str, &Rule)>) -> Result<Vec<Candidate>> {
    if args.keys.is_some() || args.general {
        return Ok(vec![]);
    }
    let st = state::load();
    let mut rules = Vec::new();
    for dir in store::project_dirs(root) {
        let name = dir.file_name().unwrap_or_default().to_string_lossy().to_string();
        let rule = override_rule
            .filter(|(n, _)| n.eq_ignore_ascii_case(&name))
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| rule(root, &name));
        if !rule.enabled {
            continue;
        }
        validate(&rule)?;
        if let Some(folder) = project::folder(root, &name, &st) {
            rules.push((name, folder, rule));
        }
    }
    if rules.is_empty() {
        return Ok(vec![]);
    }
    let mut excluded: HashSet<_> = store::stored_sessions(root).into_iter().map(|s| s.key()).collect();
    excluded.extend(ignored(root)?);
    excluded.extend(crate::multisync::linked_family());
    excluded.extend(crate::multisync::retired_keys());
    excluded.extend(crate::catalog::aliases().into_keys());
    let mut found = Vec::new();
    let locals = harness::NAMES
        .iter()
        .filter(|h| rules.iter().any(|(_, _, r)| r.harnesses.iter().any(|n| n == **h)))
        .flat_map(|h| harness::get(h).local());
    for session in locals {
        if excluded.contains(&session.key())
            || crate::clean::native_agent(&session)
            || session.harness == "claude" && session.id.starts_with("agent-")
        {
            continue;
        }
        let mut matching = Vec::new();
        for (name, folder, rule) in &rules {
            if !rule.harnesses.contains(&session.harness.to_string()) || session.path.is_empty() {
                continue;
            }
            let within = |base: &str| {
                project::contains(base, &session.path)
                    && (rule.subfolders
                        || crate::paths::canonical(Path::new(base)).ok()
                            == crate::paths::canonical(Path::new(&session.path)).ok())
            };
            let direct = within(folder);
            let clone_base = rule
                .include_clones
                .then(|| {
                    let original = crate::git::git_id(folder)?;
                    let other = crate::git::git_id(&session.path)?;
                    if original.root.is_none() || original.root != other.root || original.top == other.top {
                        return None;
                    }
                    let base = crate::paths::canonical(Path::new(folder)).ok()?;
                    let top = crate::paths::canonical(Path::new(&original.top)).ok()?;
                    let relative = base.strip_prefix(top).ok()?;
                    Some(Path::new(&other.top).join(relative).to_string_lossy().to_string())
                })
                .flatten();
            let base = if direct {
                folder.clone()
            } else if let Some(base) = clone_base.filter(|base| within(base)) {
                base
            } else {
                continue;
            };
            let relative = crate::paths::canonical(Path::new(&session.path))
                .ok()
                .and_then(|p| {
                    crate::paths::canonical(Path::new(&base)).ok().and_then(|base| {
                        p.strip_prefix(base)
                            .ok()
                            .map(|p| crate::paths::slashed(&p.to_string_lossy()))
                    })
                })
                .unwrap_or_default();
            if rule.exclude.iter().any(|p| matches(p, &relative))
                || rule.exclude_titles.iter().any(|p| matches(p, &session.title))
            {
                continue;
            }
            matching.push(name.clone());
        }
        if matching.is_empty()
            || args
                .project
                .as_ref()
                .is_some_and(|p| !matching.iter().any(|n| n.eq_ignore_ascii_case(p)))
        {
            continue;
        }
        let reason = if matching.len() > 1 {
            "ambiguous_project"
        } else if crate::agents::in_use_by_agent_now(&session)
            || args.auto && (crate::agents::in_use(&session) || args.busy.as_deref() == Some(session.key().as_str()))
            || session.harness == "codex"
                && session
                    .src
                    .as_ref()
                    .is_some_and(|p| crate::codex::completed_end(p).ok().flatten().is_some())
        {
            "turn_in_progress"
        } else {
            "project_rule_match"
        };
        found.push(Candidate {
            session,
            project: matching.join(", "),
            reason,
            eligible: reason == "project_rule_match",
        });
    }
    found.sort_by(|a, b| a.project.cmp(&b.project).then(a.session.key().cmp(&b.session.key())));
    Ok(found)
}
pub fn operations(candidates: &[Candidate]) -> Vec<crate::sync_plan::Operation> {
    candidates
        .iter()
        .map(|c| {
            let mut op = crate::sync_plan::Operation::new(
                if c.eligible { "adopt" } else { "deferred" },
                &c.session,
                c.reason,
                match c.reason {
                    "ambiguous_project" => tr!("More than one project matches; choose manually"),
                    "turn_in_progress" => tr!("Finish the current agent turn before synchronizing"),
                    _ => tr!("Add a new conversation from the enabled project rule"),
                },
            );
            op.project = c.project.clone();
            op.project_id = c.eligible.then(|| c.project.clone());
            op.destination = Some(c.session.path.clone());
            op
        })
        .collect()
}
pub fn apply(root: &Path, candidates: &[Candidate], st: &mut state::State) -> Result<Vec<Value>> {
    let mut results = Vec::new();
    for c in candidates.iter().filter(|c| c.eligible) {
        let result = store::push_one(
            root,
            &c.project,
            &c.session,
            crate::git::git_info(&c.session.path),
            st,
            &HashMap::new(),
        );
        results.push(
            json!({"session_id":c.session.key(),"success":result.is_ok(),"message":result.err().map(|e|e.to_string())}),
        );
    }
    Ok(results)
}
pub fn run(root: &Path, args: &Args) {
    let _loading = ui::loading(&tr!("Finding new conversations in enabled projects"));
    let _local = match crate::engine::local_guard() {
        Ok(g) => g,
        Err(_) => return,
    };
    let _shared = match actions::acquire(root) {
        Ok(g) => g,
        Err(_) => return,
    };
    let selected = {
        let _inspect = inspection::Guard::new();
        match candidates(root, args, None) {
            Ok(c) => c,
            Err(e) => {
                ui::warn(&e.to_string());
                return;
            }
        }
    };
    let mut st = state::load();
    match apply(root, &selected, &mut st) {
        Ok(results) => {
            for r in results {
                if r["success"] != true {
                    ui::warn(r["message"].as_str().unwrap_or_default());
                }
            }
            if let Err(e) = state::save(&mut st) {
                ui::warn(&e.to_string());
            }
        }
        Err(e) => ui::warn(&e.to_string()),
    }
}

pub fn configure(req: &Request) -> Response<Value> {
    let Some(name) = req.project.as_deref() else {
        return actions::error("adoption", "selection_required", "Choose a project");
    };
    let Some(root) = store::store_root(true) else {
        return actions::error("adoption", "shared_folder_unavailable", "Shared folder unavailable");
    };
    let Some(mut meta) = project::metadata(&root, name) else {
        return actions::error("adoption", "project_not_found", "Bind the project folder first");
    };
    let current = rule(&root, name);
    let proposed = req.adoption.clone().unwrap_or_else(|| current.clone());
    if let Err(e) = validate(&proposed) {
        return actions::error("adoption", "invalid_rule", e.to_string());
    }
    let _inspect = inspection::Guard::new();
    if proposed.enabled && project::folder(&root, name, &state::load()).is_none() {
        return actions::error(
            "adoption",
            "invalid_destination",
            "Choose a work folder before enabling automatic additions",
        );
    }
    let selected = match candidates(
        &root,
        &Args {
            project: Some(name.into()),
            ..Default::default()
        },
        Some((name, &proposed)),
    ) {
        Ok(c) => c,
        Err(e) => return actions::error("adoption", "discovery_failed", e.to_string()),
    };
    let token = crate::engine::digest(
        &serde_json::to_vec(
            &json!({"before":meta,"after":proposed,"folder":project::folder(&root,name,&state::load())}),
        )
        .unwrap(),
    );
    let mut data = json!({"rule":proposed,"previous_rule":current,"operations":[{"kind":"adoption","collection":name,"enabled":proposed.enabled}],"candidate_operations":operations(&selected).iter().map(|op|json!({"session_id":op.conversation_id,"title":op.title,"project":op.project,"reason":op.reason_code,"eligible":op.kind=="adopt"})).collect::<Vec<_>>()});
    if req.adoption.is_none() {
        return actions::response("adoption", Status::Ok, data, vec![]);
    }
    if let Some(r) = crate::recovery::confirmation(req, "adoption", &mut data, token.clone()) {
        return r;
    }
    drop(_inspect);
    let _local = match crate::engine::local_guard() {
        Ok(g) => g,
        Err(e) => return actions::error("adoption", "local_action_busy", e.to_string()),
    };
    let _shared = match actions::acquire(&root) {
        Ok(g) => g,
        Err(d) => return Response::error("adoption", d),
    };
    let now = project::metadata(&root, name);
    let now_token = crate::engine::digest(
        &serde_json::to_vec(
            &json!({"before":now,"after":proposed,"folder":project::folder(&root,name,&state::load())}),
        )
        .unwrap(),
    );
    if now_token != token {
        return actions::error(
            "adoption",
            "preview_required",
            "Project settings changed; preview again",
        );
    }
    if proposed.enabled && project::folder(&root, name, &state::load()).is_none() {
        return actions::error(
            "adoption",
            "invalid_destination",
            "Choose a work folder before enabling automatic additions",
        );
    }
    meta["auto_add"] = json!(proposed);
    let result = (|| -> Result<()> {
        let dir = store::find_project(&root, name).ok_or_else(|| Error("Project disappeared".into()))?;
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
        serde_json::to_writer_pretty(tmp.as_file_mut(), &meta)?;
        tmp.persist(dir.join(".project.json"))
            .map_err(|e| Error(e.to_string()))?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            crate::gui_index::invalidate();
            data["applied"] = json!(true);
            actions::response("adoption", Status::Ok, data, vec![])
        }
        Err(e) => actions::error("adoption", "settings_save_failed", e.to_string()),
    }
}

/// The CLI uses the same reviewed service as the desktop and JSON clients.
pub fn command() -> clap::Command {
    use clap::{Arg, ArgAction, Command};
    Command::new("auto-add")
        .about(tr!("Configure automatic additions for a project"))
        .arg(Arg::new("project"))
        .arg(
            Arg::new("on")
                .long("on")
                .action(ArgAction::SetTrue)
                .conflicts_with("off"),
        )
        .arg(Arg::new("off").long("off").action(ArgAction::SetTrue))
        .arg(Arg::new("exclude").long("exclude").action(ArgAction::Append))
        .arg(
            Arg::new("exclude-title")
                .long("exclude-title")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("clear-exclusions")
                .long("clear-exclusions")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("harnesses")
                .long("harnesses")
                .value_delimiter(',')
                .value_parser(harness::NAMES),
        )
        .arg(
            Arg::new("include-clones")
                .long("include-clones")
                .action(ArgAction::SetTrue)
                .conflicts_with("no-clones"),
        )
        .arg(Arg::new("no-clones").long("no-clones").action(ArgAction::SetTrue))
        .arg(
            Arg::new("no-subfolders")
                .long("no-subfolders")
                .action(ArgAction::SetTrue)
                .conflicts_with("subfolders"),
        )
        .arg(Arg::new("subfolders").long("subfolders").action(ArgAction::SetTrue))
        .arg(Arg::new("json").long("json").action(ArgAction::SetTrue))
}
pub fn cli(m: &clap::ArgMatches) {
    let Some(root) = store::store_root(true) else {
        let response = actions::error("adoption", "shared_folder_unavailable", "Shared folder unavailable");
        if m.get_flag("json") {
            println!("{}", serde_json::to_string(&response).unwrap());
        } else {
            ui::warn("Shared folder unavailable");
        }
        return;
    };
    let name = m.get_one::<String>("project").cloned().or_else(|| {
        std::env::current_dir()
            .ok()
            .and_then(|p| project::current(&root, &state::load(), &p.to_string_lossy()))
    });
    let Some(name) = name else {
        let response = actions::error("adoption", "selection_required", tr!("Choose a project"));
        if m.get_flag("json") {
            println!("{}", serde_json::to_string(&response).unwrap());
        } else {
            ui::warn(&tr!("Choose a project"));
        }
        return;
    };
    let mut next = rule(&root, &name);
    let mut change = false;
    for (key, value) in [("on", true), ("off", false)] {
        if m.get_flag(key) {
            next.enabled = value;
            change = true;
        }
    }
    for (key, value) in [("include-clones", true), ("no-clones", false)] {
        if m.get_flag(key) {
            next.include_clones = value;
            change = true;
        }
    }
    for (key, value) in [("subfolders", true), ("no-subfolders", false)] {
        if m.get_flag(key) {
            next.subfolders = value;
            change = true;
        }
    }
    if m.get_flag("clear-exclusions") {
        next.exclude.clear();
        next.exclude_titles.clear();
        change = true;
    }
    if let Some(h) = m.get_many::<String>("harnesses") {
        next.harnesses = h.cloned().collect();
        change = true;
    }
    for (key, patterns) in [
        ("exclude", &mut next.exclude),
        ("exclude-title", &mut next.exclude_titles),
    ] {
        if let Some(values) = m.get_many::<String>(key) {
            patterns.extend(values.cloned());
            change = true;
        }
    }
    let value = if change {
        json!({"action":"adoption","project":name,"adoption":next})
    } else {
        json!({"action":"adoption","project":name})
    };
    let req: Request = serde_json::from_value(value.clone()).unwrap();
    let preview = {
        let _loading = ui::loading(&tr!("Finding new conversations in enabled projects"));
        configure(&req)
    };
    if m.get_flag("json") {
        println!("{}", serde_json::to_string(&preview).unwrap());
        return;
    }
    if preview.status != Status::Ok {
        for d in preview.diagnostics {
            ui::warn(&d.message);
        }
        return;
    }
    let data = preview.data.unwrap();
    println!("{}", serde_json::to_string_pretty(&data).unwrap());
    if !change || !ui::confirm(&tr!("Apply")) {
        return;
    }
    let mut apply = value;
    apply["apply"] = json!(true);
    apply["expected"] = data["expected"].clone();
    let result = configure(&serde_json::from_value(apply).unwrap());
    if result.status == Status::Ok {
        ui::ok(&tr!("Action finished"));
    } else {
        for d in result.diagnostics {
            ui::warn(&d.message);
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn exclusions_cover_whole_subtrees_and_titles() {
        assert!(super::matches("scratch/**", "scratch"));
        assert!(super::matches("scratch/**", "scratch/child"));
        assert!(!super::matches("scratch/**", "scratchpad"));
        assert!(super::matches("*private*", "A private task"));
        assert!(!super::Rule::default().enabled);
    }
}
