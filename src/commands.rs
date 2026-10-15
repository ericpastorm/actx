//! The commands: init, mk, sync, ls, rm, cv, update.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use crate::ctx::{ctx, jstr};
use crate::git::{ask_folder, check_git, fixer_for, git_id, git_info, git_label, resolve_folder, warn_dirty};
use crate::harness::{self, NAMES, all_local};
use crate::i18n;
use crate::paths::short;
use crate::session::{Session, badge, status};
use crate::state::{self, State};
use crate::store::{
    find_project, install_kind, modified, project_dirs, push_one, remove_stored, shared_binary, store_root,
    stored_sessions, sync_self,
};
use crate::tr;
use crate::turns::{COPY_SUFFIX, fit};
use crate::ui::{
    Row, ago, ask, bold, clip, confirm, die, dim, fail, fzf, harness_tag, interactive, local_time, ok, pad, paint,
    render, warn,
};

const ACTIVE_SECS: f64 = 120.0; // a conversation file touched this recently is probably still open

#[derive(Default)]
pub struct Args {
    pub project: Option<String>,
    pub folder: Option<String>,
    pub lang: Option<String>,
    pub all: bool,
}

fn local_index() -> HashMap<String, Session> {
    all_local().into_iter().map(|s| (s.key(), s)).collect()
}

fn cwd() -> String {
    std::env::current_dir()
        .and_then(|p| p.canonicalize())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn session_preview(s: &Session) -> String {
    let mut lines = vec![
        bold(&s.title),
        dim(&format!("{} · {} · {}", s.harness, s.id, s.size_label())),
    ];
    let body = if let Some(meta) = &s.meta {
        lines.push(dim(&tr!(
            "in {project} · pushed from {host} {ago}",
            project = s.project.clone().unwrap_or_default(),
            host = jstr(meta, "host"),
            ago = ago(meta.get("pushed_at").and_then(Value::as_f64).unwrap_or(0.0)),
        )));
        lines.push(dim(&tr!("folder there: {path}", path = short(jstr(meta, "path")))));
        if let Some(g) = meta.get("git").filter(|g| g.is_object()) {
            lines.push(dim(&format!("{}{}", tr!("git when pushed: "), git_label(g))));
        }
        std::fs::read_to_string(s.store_dir().join("preview.txt")).unwrap_or_else(|_| tr!("(no preview)"))
    } else {
        lines.push(dim(&tr!(
            "{path} · last activity {ago}",
            path = short(&s.path),
            ago = ago(s.updated)
        )));
        render(&harness::get(s.harness).preview(s), s.harness)
    };
    format!("{}\n{}\n\n{body}", lines.join("\n"), dim(&"─".repeat(40)))
}

fn resume_cmd(s: &Session, folder: &str) -> String {
    harness::get(s.harness).resume(&s.id, folder)
}

fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

/// How much a conversation looks like it belongs to project `name`: by folder, path or title.
fn relevance(name: &str, s: &Session) -> u8 {
    let n = norm(name);
    if n.is_empty() {
        return 0;
    }
    if norm(&Path::new(&s.path).file_name().unwrap_or_default().to_string_lossy()) == n {
        return 3;
    }
    if norm(&s.path).contains(&n) {
        return 2;
    }
    u8::from(norm(&s.title).contains(&n))
}

fn save(st: &State) {
    if let Err(e) = state::save(st) {
        fail(&e.0);
    }
}

// ── init ────────────────────────────────────────────────────────────────────

const NET_FS: [&str; 10] = [
    "autofs",
    "cifs",
    "smb3",
    "smbfs",
    "nfs",
    "nfs4",
    "9p",
    "fuse.sshfs",
    "fuse.rclone",
    "fuse.s3fs",
];
const SYNC_DIRS: [&str; 8] = [
    "Sync",
    "Dropbox",
    "Nextcloud",
    "ownCloud",
    "OneDrive",
    "pCloudDrive",
    "MEGA",
    "SeaDrive",
];

fn is_store(d: &Path) -> bool {
    d.join(".actx").exists()
        || d.join("_bin/ctx").exists()
        || std::fs::read_dir(d.join("_bin"))
            .map(|it| {
                it.filter_map(|e| e.ok())
                    .any(|e| e.file_name().to_string_lossy().starts_with("actx"))
            })
            .unwrap_or(false)
}

/// (folders that already hold actx data, suggested new folders), from network mounts and sync apps.
fn store_candidates() -> (Vec<String>, Vec<String>) {
    let mut bases: Vec<String> = std::fs::read_to_string("/proc/mounts")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let (point, fs) = (*parts.get(1)?, *parts.get(2)?);
            let skip = point.starts_with("/proc") || point.starts_with("/sys") || point.starts_with("/run/user");
            (NET_FS.contains(&fs) && !skip).then(|| point.replace("\\040", " "))
        })
        .collect();
    bases.extend(
        SYNC_DIRS
            .iter()
            .map(|d| ctx().home.join(d))
            .filter(|p| p.is_dir())
            .map(|p| p.to_string_lossy().into_owned()),
    );
    let mut seen = HashSet::new();
    bases.retain(|b| seen.insert(b.clone()));
    let mut existing = Vec::new();
    for base in &bases {
        let base = Path::new(base);
        let Ok(children) = std::fs::read_dir(base) else {
            continue;
        };
        let dirs = std::iter::once(base.to_path_buf())
            .chain(children.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()));
        existing.extend(dirs.filter(|d| is_store(d)).map(|d| d.to_string_lossy().into_owned()));
    }
    (
        existing,
        bases
            .iter()
            .map(|b| Path::new(b).join("actx").to_string_lossy().into_owned())
            .collect(),
    )
}

pub fn init(args: Args) {
    if let Some(lang) = &args.lang {
        i18n::set_spanish(lang == "es");
    }
    let folder = match args.folder {
        Some(f) => f,
        None => {
            if !interactive() {
                die(&tr!("Usage: {cmd}", cmd = "actx init <folder>"));
            }
            let (existing, suggested) = store_candidates();
            let mut rows: Vec<Row> = existing
                .iter()
                .map(|p| Row {
                    key: p.clone(),
                    line: format!("{}  {}", short(p), paint("32", &tr!("✓ already used by actx"))),
                    preview: String::new(),
                })
                .collect();
            rows.extend(suggested.iter().filter(|p| !existing.contains(p)).map(|p| Row {
                key: p.clone(),
                line: short(p),
                preview: String::new(),
            }));
            rows.push(Row {
                key: "::other".into(),
                line: tr!("✎ other folder (type the path)"),
                preview: String::new(),
            });
            let header = tr!("Where should actx keep your conversations? A folder all your machines can reach");
            let Some(picked) = fzf(&rows, &header, false, false).into_iter().next() else {
                return;
            };
            if picked == "::other" {
                let typed = ask(&tr!("Path:"), false);
                if typed.is_empty() {
                    return;
                }
                typed
            } else {
                picked
            }
        }
    };
    let root = PathBuf::from(crate::store::shellexpand(&folder));
    let made = std::fs::create_dir_all(&root).and_then(|_| {
        let marker = root.join(".actx");
        if marker.exists() {
            Ok(())
        } else {
            std::fs::write(marker, "{\"store\": 1}\n")
        }
    });
    if let Err(e) = made {
        die(&tr!("Can't create {path}: {err}", path = root.display(), err = e));
    }
    let root = root.canonicalize().unwrap_or(root);
    let mut config = ctx().config.clone();
    config["root"] = json!(root.to_string_lossy());
    if let Some(lang) = &args.lang {
        config["lang"] = json!(lang);
    }
    let written = ctx()
        .config_file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|_| {
            std::fs::write(
                &ctx().config_file,
                serde_json::to_string_pretty(&config).unwrap_or_default() + "\n",
            )
        });
    if let Err(e) = written {
        die(&tr!(
            "Can't create {path}: {err}",
            path = ctx().config_file.display(),
            err = e
        ));
    }
    ok(&tr!("Shared folder: {path}", path = bold(&root.to_string_lossy())));
    let projects = project_dirs(&root);
    if !projects.is_empty() {
        let names: Vec<String> = projects
            .iter()
            .map(|d| d.file_name().unwrap_or_default().to_string_lossy().into_owned())
            .collect();
        println!(
            "{}",
            tr!(
                "It already has {n} project(s): {names}",
                n = projects.len(),
                names = names.join(", ")
            )
        );
    }
    warn(&tr!(
        "Conversations include command output and may contain secrets (API keys, tokens). Keep this folder private."
    ));
    let next = if projects.is_empty() {
        format!("actx mk <{}>", tr!("project"))
    } else {
        "actx sync".to_string()
    };
    println!("{}", tr!("Next: {cmd}", cmd = bold(&next)));
}

// ── mk ──────────────────────────────────────────────────────────────────────

pub fn mk(args: Args) {
    let name = args.project.unwrap_or_else(|| {
        let here = cwd();
        let default = git_id(&here).map(|g| g.top).unwrap_or(here);
        let default = Path::new(&default)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let typed = ask(&tr!("Project name [{default}]:", default = default), false);
        if typed.is_empty() { default } else { typed }
    });
    let valid = name.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_')
        && name.chars().all(|c| c.is_alphanumeric() || "_.-".contains(c));
    if !valid {
        die(&tr!("Invalid name: use letters, numbers, '-', '_' or '.'"));
    }
    let root = store_root(false).expect("a shared folder");
    let name = match find_project(&root, &name) {
        Some(p) => p.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        None => {
            if let Err(e) = std::fs::create_dir(root.join(&name)) {
                die(&e.to_string());
            }
            ok(&tr!("Project {name} created", name = bold(&name)));
            name
        }
    };
    let remote: HashMap<String, Session> = stored_sessions(&root).into_iter().map(|s| (s.key(), s)).collect();
    let mut st = state::load();
    let mut ranked = all_local();
    if ranked.is_empty() {
        die(&tr!(
            "No Claude Code, opencode or Codex conversations found on this machine"
        ));
    }
    ranked.sort_by(|a, b| {
        relevance(&name, b)
            .cmp(&relevance(&name, a))
            .then(b.updated.total_cmp(&a.updated))
    });
    let rows: Vec<Row> = ranked
        .iter()
        .map(|s| {
            let tag = match remote.get(&s.key()) {
                Some(r) if r.project.as_deref() == Some(name.as_str()) => {
                    badge(status(Some(s), Some(r), st.base.get(&s.key()).copied()))
                }
                Some(r) => paint(
                    "35",
                    &clip(
                        &pad(
                            &tr!("in {project}", project = r.project.clone().unwrap_or_default()),
                            12,
                        ),
                        12,
                    ),
                ),
                None => " ".repeat(12),
            };
            let star = if relevance(&name, s) > 0 {
                paint("33", "★")
            } else {
                " ".into()
            };
            let mut details = format!("{} · {} · {}", short(&s.path), s.size_label(), ago(s.updated));
            if s.src.is_some() && crate::ctx::now() - s.updated < ACTIVE_SECS {
                details += &format!(" · {}", tr!("open now"));
            }
            let line = format!(
                "{star} {} {tag} {}  {}",
                harness_tag(s.harness),
                pad(&clip(&s.title, 42), 42),
                dim(&details)
            );
            Row {
                key: s.key(),
                line,
                preview: session_preview(s),
            }
        })
        .collect();
    let header = tr!(
        "{name} · ★ = looks like this project · TAB marks several · Enter saves",
        name = name
    );
    let keys: HashSet<String> = fzf(&rows, &header, true, true).into_iter().collect();
    let chosen: Vec<&Session> = ranked.iter().filter(|s| keys.contains(&s.key())).collect();
    if chosen.is_empty() {
        return;
    }
    let mut todo = Vec::new();
    for s in chosen {
        if let Some(r) = remote.get(&s.key())
            && r.project.as_deref() == Some(name.as_str())
            && matches!(
                status(Some(s), Some(r), st.base.get(&s.key()).copied()),
                "pull" | "conflict"
            )
        {
            let question = tr!(
                "«{title}» has changes in the shared folder (from {host}) that aren't here. Overwrite them?",
                title = s.title,
                host = jstr(r.meta(), "host"),
            );
            if !confirm(&question) {
                continue;
            }
        }
        todo.push(s);
    }
    warn_dirty(&todo.iter().map(|s| s.path.clone()).collect());
    for s in todo {
        match push_one(&root, &name, s, git_info(&s.path), &mut st, &remote) {
            Ok(moved) => {
                let note = moved
                    .map(|p| dim(&format!("  {}", tr!("(moved from {project})", project = p))))
                    .unwrap_or_default();
                ok(&format!("{} {}{note}", harness_tag(s.harness), s.title));
            }
            Err(e) => fail(&format!("{}: {e}", s.title)),
        }
    }
    save(&st);
    println!("\n{}", tr!("On your other machines: {cmd}", cmd = bold("actx sync")));
    sync_self(&root);
}

// ── sync ────────────────────────────────────────────────────────────────────

type Pending = (&'static str, Option<Session>, Session);

/// Which projects to sync: the one you name, all of them (--all, or no terminal), or the ones you pick.
fn pick_sync_projects(pending: &BTreeMap<String, Vec<Pending>>, args: &Args) -> Vec<String> {
    if let Some(wanted) = &args.project {
        return match pending.keys().find(|p| p.to_lowercase() == wanted.to_lowercase()) {
            Some(p) => vec![p.clone()],
            None => {
                ok(&tr!("{project}: nothing to sync", project = wanted));
                Vec::new()
            }
        };
    }
    if args.all || !interactive() || ctx().answer.is_some() {
        return pending.keys().cloned().collect();
    }
    let (here_dir, here) = (cwd(), git_id(&cwd()));
    let is_here = |project: &str| {
        pending[project].iter().any(|(_, l, r)| {
            l.as_ref().is_some_and(|l| l.path == here_dir)
                || here.as_ref().is_some_and(|h| {
                    r.meta().get("git").and_then(|g| g.get("root")).and_then(Value::as_str) == h.root.as_deref()
                        && h.root.is_some()
                })
        })
    };
    let newest = |p: &str| pending[p].iter().map(|(_, _, r)| r.updated).fold(0.0, f64::max);
    let mut order: Vec<&String> = pending.keys().collect();
    order.sort_by(|a, b| is_here(b).cmp(&is_here(a)).then(newest(b).total_cmp(&newest(a))));
    let width = order.iter().map(|p| p.chars().count()).max().unwrap_or(0);
    let rows: Vec<Row> = order
        .iter()
        .map(|p| {
            let states: Vec<&str> = pending[*p].iter().map(|(s, _, _)| *s).collect();
            let count = |names: &[&str]| states.iter().filter(|s| names.contains(s)).count();
            let counts: Vec<String> = [
                ("↑", "33", count(&["push"])),
                ("↓", "36", count(&["pull", "new"])),
                ("⚠", "31", count(&["conflict"])),
            ]
            .iter()
            .filter(|(_, _, n)| *n > 0)
            .map(|(label, code, n)| paint(code, &format!("{label}{n}")))
            .collect();
            let mark = if is_here(p) { paint("32", "●") } else { " ".into() };
            let listing: Vec<String> = pending[*p]
                .iter()
                .map(|(s, _, r)| format!("{} {} {}", badge(s), harness_tag(r.harness), r.title))
                .collect();
            Row {
                key: (*p).clone(),
                line: format!("{mark} {}  {}", pad(p, width), counts.join("  ")),
                preview: format!("{}\n\n{}", bold(p), listing.join("\n")),
            }
        })
        .collect();
    fzf(
        &rows,
        &tr!("Sync · ● = where you are · TAB marks several · Ctrl-A all · Enter confirms"),
        true,
        true,
    )
}

pub fn sync(args: Args) {
    let root = store_root(false).expect("a shared folder");
    let mut remote = stored_sessions(&root);
    if remote.is_empty() {
        println!(
            "{}",
            tr!(
                "No conversations in the shared folder yet. Start with: {cmd}",
                cmd = bold(&format!("actx mk <{}>", tr!("project")))
            )
        );
        return;
    }
    let mut st = state::load();
    let local = local_index();
    remote.sort_by(|a, b| a.project.cmp(&b.project).then(b.updated.total_cmp(&a.updated)));
    let mut pending: BTreeMap<String, Vec<Pending>> = BTreeMap::new();
    let mut in_sync = 0;
    for r in remote {
        let l = local.get(&r.key());
        let state = status(l, Some(&r), st.base.get(&r.key()).copied());
        if state == "synced" {
            in_sync += 1;
            let l = l.expect("synced means it's here");
            st.base.entry(r.key()).or_insert((l.weight, r.weight));
        } else {
            pending
                .entry(r.project.clone().unwrap_or_default())
                .or_default()
                .push((state, l.cloned(), r));
        }
    }
    if pending.is_empty() {
        save(&st);
        ok(&tr!("All in sync ({n} conversations)", n = in_sync));
        return;
    }
    let chosen = pick_sync_projects(&pending, &args);
    if chosen.is_empty() {
        save(&st);
        return;
    }
    let skipped: usize = pending
        .iter()
        .filter(|(p, _)| !chosen.contains(p))
        .map(|(_, v)| v.len())
        .sum();
    let (mut up, mut down, mut conflicts) = (Vec::new(), Vec::new(), Vec::new());
    for project in &chosen {
        for (state, l, r) in pending.remove(project).unwrap_or_default() {
            match state {
                "push" => up.push((l, r)),
                "pull" | "new" => down.push((l, r)),
                _ => conflicts.push((l, r)),
            }
        }
    }
    let mut merges = Vec::new();
    for (l, r) in conflicts {
        let host = jstr(r.meta(), "host").to_string();
        warn(&tr!(
            "{project} · «{title}» changed here and on {host} since the last sync.",
            project = r.project.clone().unwrap_or_default(),
            title = r.title,
            host = host,
        ));
        println!(
            "{}",
            dim(&format!(
                "  {}",
                tr!(
                    "merge = keep this one + a summary of what was done on {host}; {host}'s version is kept separately",
                    host = host
                )
            ))
        );
        let choice = ask(
            &format!(
                "  {}",
                tr!("What now? [m]erge / [l]ocal / [r]emote ({host}) / [s]kip:", host = host)
            ),
            true,
        );
        match choice.chars().next() {
            Some('m') => merges.push((l, r)),
            Some('l' | 'a') => up.push((l, r)),   // local / aquí
            Some('r' | 'o') => down.push((l, r)), // remote / otro
            _ => println!("{}", dim(&format!("  {}", tr!("skipped")))),
        }
    }
    let none = HashMap::new();
    warn_dirty(
        &up.iter()
            .filter_map(|(l, _)| l.as_ref().map(|l| l.path.clone()))
            .collect::<BTreeSet<_>>(),
    );
    for (l, r) in &up {
        let Some(l) = l else { continue };
        let project = r.project.clone().unwrap_or_default();
        match push_one(&root, &project, l, git_info(&l.path), &mut st, &none) {
            Ok(_) => println!(
                "{} {} {} {}",
                paint("33", "↑"),
                pad(&project, 14),
                harness_tag(r.harness),
                r.title
            ),
            Err(e) => fail(&format!("{}: {e}", r.title)),
        }
    }
    let mut checked = HashSet::new();
    let mut ready = Vec::new();
    for (l, r) in &down {
        // if you already have it, it stays where it is; if it's new, find its place
        let folder = match l {
            Some(l) if Path::new(&l.path).is_dir() => Some(l.path.clone()),
            _ => resolve_folder(r.meta(), &st).or_else(|| ask_folder(r.meta(), &mut st)),
        };
        let Some(folder) = folder else {
            warn(&tr!(
                "{project} · «{title}»: I don't know which folder it goes in on this machine; skipping",
                project = r.project.clone().unwrap_or_default(),
                title = r.title,
            ));
            continue;
        };
        if checked.insert(folder.clone()) {
            check_git(r.meta(), &folder);
        }
        let fix = fixer_for(r.meta(), &folder);
        match harness::get(r.harness).install(r, l.as_ref(), &folder, fix.as_ref()) {
            Ok(weight) => {
                st.base.insert(r.key(), (weight, r.weight));
                let project = r.project.clone().unwrap_or_default();
                println!(
                    "{} {} {} {}  {}",
                    paint("36", "↓"),
                    pad(&project, 14),
                    harness_tag(r.harness),
                    r.title,
                    dim(&format!("→ {}", short(&folder)))
                );
                ready.push((r, folder));
            }
            Err(e) => fail(&format!("{}: {e}", r.title)),
        }
    }
    for (l, r) in merges {
        let Some(mut l) = l else { continue };
        if let Err(e) = crate::merge::merge_one(&root, &mut l, &r, &mut st) {
            fail(&tr!(
                "{title}: couldn't merge ({err}); nothing else was touched",
                title = r.title,
                err = e
            ));
        }
    }
    save(&st);
    if in_sync > 0 {
        ok(&tr!("{n} in sync", n = in_sync));
    }
    if skipped > 0 {
        println!("{}", dim(&tr!("{n} pending in projects you didn't pick", n = skipped)));
    }
    if !ready.is_empty() {
        println!("\n{}", tr!("To continue:"));
        for (r, folder) in ready {
            println!(
                "  {}   {}",
                resume_cmd(r, &folder),
                dim(crate::ui::take_chars(&r.title, 50))
            );
        }
    }
    sync_self(&root);
}

// ── ls ──────────────────────────────────────────────────────────────────────

pub fn ls(args: Args) {
    let root = store_root(false).expect("a shared folder");
    let mut dirs = project_dirs(&root);
    if let Some(name) = &args.project {
        match find_project(&root, name) {
            Some(p) => dirs = vec![p],
            None => die(&tr!(
                "Project {name} doesn't exist. Create it with: {cmd}",
                name = name,
                cmd = format!("actx mk {name}")
            )),
        }
    }
    if dirs.is_empty() {
        println!(
            "{}",
            tr!(
                "No projects yet. Create one with: {cmd}",
                cmd = bold(&format!("actx mk <{}>", tr!("project")))
            )
        );
        return;
    }
    let st = state::load();
    let local = local_index();
    let mut by_project: HashMap<String, Vec<Session>> = HashMap::new();
    for r in stored_sessions(&root) {
        by_project
            .entry(r.project.clone().unwrap_or_default())
            .or_default()
            .push(r);
    }
    let name_of = |d: &PathBuf| d.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let newest = |d: &PathBuf| {
        by_project
            .get(&name_of(d))
            .map(|v| v.iter().map(|r| r.updated).fold(0.0, f64::max))
            .unwrap_or(0.0)
    };
    dirs.sort_by(|a, b| newest(b).total_cmp(&newest(a)));
    let mut pending = 0;
    for d in &dirs {
        let mut ss = by_project.get(&name_of(d)).cloned().unwrap_or_default();
        ss.sort_by(|a, b| b.updated.total_cmp(&a.updated));
        let count = if ss.is_empty() {
            tr!("(empty)")
        } else {
            tr!("{n} conversation(s)", n = ss.len())
        };
        println!("{}{}", bold(&name_of(d)), dim(&format!("  {count}")));
        for r in &ss {
            let l = local.get(&r.key());
            let state = status(l, Some(r), st.base.get(&r.key()).copied());
            if state != "synced" {
                pending += 1;
            }
            let meta = r.meta();
            let place = match l {
                Some(l) => short(&l.path),
                None => tr!(
                    "{path} on {host}",
                    path = short(jstr(meta, "path")),
                    host = jstr(meta, "host")
                ),
            };
            println!(
                "  {} {} {}  {}",
                badge(state),
                harness_tag(r.harness),
                r.title,
                dim(&format!("· {place} · {}", ago(r.updated)))
            );
        }
        println!();
    }
    if pending > 0 {
        println!("{}", tr!("There are pending changes: {cmd}", cmd = bold("actx sync")));
    }
}

// ── rm ──────────────────────────────────────────────────────────────────────

pub fn rm(args: Args) {
    let root = store_root(false).expect("a shared folder");
    let wanted = args.project.unwrap_or_default();
    let Some(pdir) = find_project(&root, &wanted) else {
        die(&tr!("Project {name} doesn't exist", name = wanted))
    };
    let pname = pdir.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let mut ss: Vec<Session> = stored_sessions(&root)
        .into_iter()
        .filter(|r| r.project.as_deref() == Some(pname.as_str()))
        .collect();
    ss.sort_by(|a, b| b.updated.total_cmp(&a.updated));
    if ss.is_empty() {
        if confirm(&tr!("{name} is empty. Delete it?", name = pname)) {
            let _ = std::fs::remove_dir_all(&pdir);
            ok(&tr!("Project {name} deleted", name = pname));
        }
        return;
    }
    let rows: Vec<Row> = ss
        .iter()
        .map(|r| Row {
            key: r.key(),
            line: format!(
                "{} {}  {}",
                harness_tag(r.harness),
                local_time(r.updated, "%d %b %H:%M"),
                r.title
            ),
            preview: session_preview(r),
        })
        .collect();
    let header = tr!(
        "Remove from {name} (nothing is deleted on your machines) · TAB marks several",
        name = pname
    );
    let keys: HashSet<String> = fzf(&rows, &header, true, true).into_iter().collect();
    let chosen: Vec<&Session> = ss.iter().filter(|r| keys.contains(&r.key())).collect();
    if chosen.is_empty() {
        return;
    }
    let everything = chosen.len() == ss.len();
    let question = if everything {
        tr!("Delete the whole project {name}?", name = pname)
    } else {
        tr!("Remove {n} from {name}?", n = chosen.len(), name = pname)
    };
    if !confirm(&question) {
        return;
    }
    let mut st = state::load();
    for r in &chosen {
        if let Err(e) = remove_stored(r) {
            fail(&format!("{}: {e}", r.title));
        }
        st.base.remove(&r.key());
    }
    if everything {
        let _ = std::fs::remove_dir_all(&pdir);
    }
    save(&st);
    ok(&if everything {
        tr!("Project {name} deleted", name = pname)
    } else {
        tr!("Removed {n}", n = chosen.len())
    });
}

// ── cv ──────────────────────────────────────────────────────────────────────

/// The tool to copy the chosen conversations into: asked only if there's more than one option.
fn pick_target(chosen: &[&Session]) -> Option<&'static str> {
    let sources: HashSet<&str> = chosen.iter().map(|s| s.harness).collect();
    let options: Vec<&'static str> = NAMES
        .iter()
        .copied()
        .filter(|n| harness::get(n).installed() && !(sources.len() == 1 && sources.contains(n)))
        .collect();
    match options.len() {
        0 => die(&tr!(
            "There's no other tool to convert to: install Claude Code, opencode or Codex"
        )),
        1 => Some(options[0]),
        _ => {
            let rows: Vec<Row> = options
                .iter()
                .map(|n| Row {
                    key: n.to_string(),
                    line: format!("→ {} {}", harness_tag(n), harness::get(n).label()),
                    preview: String::new(),
                })
                .collect();
            let picked = fzf(&rows, &tr!("Convert to which tool?"), false, false)
                .into_iter()
                .next()?;
            harness::known(&picked)
        }
    }
}

/// Create a copy of s in another tool and return it. The original is left untouched.
fn convert_one(s: &Session, target: &str, st: &mut State) -> crate::ctx::Result<Session> {
    use crate::ctx::Error;
    let source = harness::get(s.harness);
    let (folder, (turns, model)) = if let Some(meta) = &s.meta {
        let folder = resolve_folder(meta, st)
            .or_else(|| ask_folder(meta, st))
            .ok_or_else(|| Error(tr!("I don't know which folder it goes in on this machine")))?;
        let fix = fixer_for(meta, &folder);
        let turns = source.stored_turns(s, fix.as_ref())?;
        (folder, turns)
    } else {
        (s.path.clone(), source.local_turns(s)?)
    };
    if turns.is_empty() {
        return Err(Error(tr!("it has no text messages to convert")));
    }
    if !Path::new(&folder).is_dir() {
        return Err(Error(tr!(
            "{path} doesn't exist on this machine",
            path = short(&folder)
        )));
    }
    let turns = fit(turns, source.label(), model.as_deref());
    let title = tr!(
        "{title} (from {source})",
        title = COPY_SUFFIX.replace(&s.title, ""),
        source = source.label()
    );
    harness::get(target).write(&turns, &folder, &title, model.as_deref())
}

pub fn cv(args: Args) {
    let root = store_root(true);
    let remote: HashMap<String, Session> = root
        .as_ref()
        .map(|r| stored_sessions(r).into_iter().map(|s| (s.key(), s)).collect())
        .unwrap_or_default();
    let local = all_local();
    let have: HashSet<String> = local.iter().map(Session::key).collect();
    let mut candidates: Vec<Session> = local;
    candidates.extend(
        remote
            .iter()
            .filter(|(k, _)| !have.contains(*k))
            .map(|(_, r)| r.clone()),
    );
    if let Some(name) = &args.project {
        let name = name.to_lowercase();
        candidates.retain(|s| {
            remote
                .get(&s.key())
                .is_some_and(|r| r.project.as_deref().map(str::to_lowercase) == Some(name.clone()))
        });
        if candidates.is_empty() {
            die(&tr!(
                "No conversations in project {name}",
                name = args.project.clone().unwrap_or_default()
            ));
        }
    }
    if candidates.is_empty() {
        die(&tr!("No Claude Code, opencode or Codex conversations found"));
    }
    candidates.sort_by(|a, b| b.updated.total_cmp(&a.updated));
    let rows: Vec<Row> = candidates
        .iter()
        .map(|s| {
            let place = match &s.meta {
                Some(meta) => tr!(
                    "{path} on {host}",
                    path = short(jstr(meta, "path")),
                    host = jstr(meta, "host")
                ),
                None => short(&s.path),
            };
            let mut details = format!("{place} · {} · {}", s.size_label(), ago(s.updated));
            if let Some(r) = remote.get(&s.key()) {
                details += &format!(
                    " · {}",
                    tr!("in {project}", project = r.project.clone().unwrap_or_default())
                );
            }
            Row {
                key: s.key(),
                line: format!(
                    "{} {}  {}",
                    harness_tag(s.harness),
                    pad(&clip(&s.title, 42), 42),
                    dim(&details)
                ),
                preview: session_preview(s),
            }
        })
        .collect();
    let header = tr!("Convert · creates a copy in another tool, the original is left untouched · TAB marks several");
    let keys: HashSet<String> = fzf(&rows, &header, true, true).into_iter().collect();
    let chosen: Vec<&Session> = candidates.iter().filter(|s| keys.contains(&s.key())).collect();
    if chosen.is_empty() {
        return;
    }
    let Some(target) = pick_target(&chosen) else { return };
    let mut st = state::load();
    let mut ready = Vec::new();
    for s in chosen {
        if s.harness == target {
            println!(
                "{}",
                dim(&tr!(
                    "{title}: it's already a {tool} conversation",
                    title = s.title,
                    tool = harness::get(target).label()
                ))
            );
            continue;
        }
        if s.src.is_some() && s.weight > 50 << 20 {
            println!(
                "{}",
                dim(&tr!(
                    "Reading «{title}» ({size})…",
                    title = s.title,
                    size = s.size_label()
                ))
            );
        }
        let new = match convert_one(s, target, &mut st) {
            Ok(new) => new,
            Err(e) => {
                fail(&format!("{}: {e}", s.title));
                continue;
            }
        };
        let mut note = String::new();
        if let (Some(root), Some(r)) = (&root, remote.get(&s.key())) {
            // the copy travels with its project
            let project = r.project.clone().unwrap_or_default();
            note = match push_one(root, &project, &new, git_info(&new.path), &mut st, &HashMap::new()) {
                Ok(_) => dim(&format!("  {}", tr!("(also saved in {project})", project = project))),
                Err(e) => paint(
                    "31",
                    &format!(
                        "  {}",
                        tr!("(couldn't save it in {project}: {err})", project = project, err = e)
                    ),
                ),
            };
        }
        ok(&format!(
            "{} → {} {}{note}",
            harness_tag(s.harness),
            harness_tag(new.harness),
            new.title
        ));
        ready.push(new);
    }
    save(&st);
    if !ready.is_empty() {
        println!("\n{}", tr!("To continue:"));
        for new in &ready {
            println!("  {}", resume_cmd(new, &new.path));
        }
        if let Some(root) = &root {
            sync_self(root);
        }
    }
}

// ── update ──────────────────────────────────────────────────────────────────

pub fn update() {
    let me = ctx().self_path.clone();
    match install_kind() {
        "package" => {
            println!(
                "{}",
                tr!(
                    "actx was installed with your package manager: update it there (e.g. {cmd}).",
                    cmd = "makepkg -si"
                )
            );
            return;
        }
        "dev" => {
            let repo = me
                .ancestors()
                .find(|d| d.join("Cargo.toml").exists())
                .unwrap_or(&me)
                .to_path_buf();
            let cmd = format!(
                "git -C {} pull && cargo build --release",
                crate::ui::shell_quote(&repo.to_string_lossy())
            );
            println!("{}", tr!("This is a git checkout: update it with {cmd}.", cmd = cmd));
            return;
        }
        _ => {}
    }
    let root = store_root(false).expect("a shared folder");
    let shared = shared_binary(&root);
    if !shared.exists() {
        sync_self(&root);
        ok(&tr!(
            "There was no actx in the shared folder: uploaded this machine's copy"
        ));
        return;
    }
    let new = std::fs::read(&shared).unwrap_or_default();
    if std::fs::read(&me).ok().as_deref() == Some(new.as_slice()) {
        ok(&tr!("You already have the latest version"));
        return;
    }
    if modified(&me) > modified(&shared) + 2.0 {
        sync_self(&root);
        ok(&tr!("This machine's version is newer than the shared one: uploaded it"));
        return;
    }
    let tmp = me.with_file_name(".actx.new");
    let installed = (|| -> std::io::Result<String> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&tmp, &new)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        let out = Command::new(&tmp).arg("--version").output()?;
        let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !version.starts_with("actx ") {
            return Err(std::io::Error::other(format!("{} {version}", out.status)));
        }
        let when = std::fs::metadata(&shared)?.modified()?;
        std::fs::File::options().write(true).open(&tmp)?.set_modified(when)?; // so it doesn't look "newer" than the shared one
        std::fs::rename(&tmp, &me)?;
        Ok(version)
    })();
    if let Err(e) = installed {
        let _ = std::fs::remove_file(&tmp);
        die(&tr!("The shared copy is broken ({err}); not installing it", err = e));
    }
    ok(&tr!(
        "actx updated from the shared folder (saved {ago})",
        ago = ago(modified(&shared))
    ));
}
