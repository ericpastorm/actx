//! The commands: init, mk, sync, ls, rm, cv, update.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

use crate::agents::{ACTIVE_SECS, in_use, in_use_by_agent};
use crate::ctx::{ctx, jstr};
use crate::git::{
    ask_folder, ask_local_folder, check_git, fixer_for, git_id, git_info, git_label, resolve_folder, warn_dirty,
};
use crate::harness::{self, all_local};
use crate::i18n;
use crate::lock::{Holder, LockError, SharedLock};
use crate::paths::short;
use crate::session::{Session, badge, status};
use crate::state::{self, Backup, State};
use crate::store::{
    GENERAL, collection_label, find_project, install_kind, modified, project_dirs, push_one, pushed_at, remove_stored,
    shared_binary, store_root, stored_sessions, sync_self,
};
use crate::tr;
use crate::turns::{COPY_SUFFIX, fit};
use crate::ui::{
    Row, ago, ask, bold, clip, confirm, die, dim, fail, fzf, harness_tag, interactive, is_auto, local_time, ok, pad,
    paint, render, say, set_auto, warn,
};

#[derive(Default, Clone)]
pub struct Args {
    pub dry_run: bool,
    pub project: Option<String>,
    pub folder: Option<String>,
    pub lang: Option<String>,
    pub harnesses: Option<String>,
    pub retention: Option<String>,
    pub conversation: Option<String>,
    pub keys: Option<HashSet<String>>,
    pub print: bool,
    pub wizard: bool,
    pub json: bool,
    pub skip_retention: bool,
    pub all: bool,
    pub general: bool,
    /// sync: an automatic run (no questions, output to a log)
    pub auto: bool,
    /// sync --auto: a process to wait for (the agent that just closed), seconds to wait, and a
    /// conversation that's opening right now
    pub after: Option<u32>,
    pub delay: u64,
    pub busy: Option<String>,
}

fn local_index() -> HashMap<String, Session> {
    all_local().into_iter().map(|s| (s.key(), s)).collect()
}

fn cwd() -> String {
    std::env::current_dir()
        .and_then(|p| crate::paths::canonical(&p))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(crate) fn session_preview(s: &Session) -> String {
    let mut lines = vec![
        bold(&s.title),
        dim(&format!("{} · {} · {}", s.harness, s.id, s.size_label())),
    ];
    let body = if let Some(meta) = &s.meta {
        lines.push(dim(&tr!(
            "in {project} · pushed from {host} {ago}",
            project = s.project_label(),
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

fn save(st: &mut State) {
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
const SYNC_DIRS: [&str; 9] = [
    "Sync",
    "Dropbox",
    "Nextcloud",
    "ownCloud",
    "OneDrive",
    "pCloudDrive",
    "MEGA",
    "SeaDrive",
    "SynologyDrive",
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

/// Network shares mounted on this machine.
fn network_mounts() -> Vec<String> {
    if cfg!(windows) {
        // mapped network drives (Z:\ → \\nas\share), from PowerShell
        let out = crate::ctx::quiet("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-PSDrive -PSProvider FileSystem | Where-Object { $_.DisplayRoot } | ForEach-Object { $_.Root }",
            ])
            .stdin(Stdio::null())
            .output()
            .map(|o| o.stdout)
            .unwrap_or_default();
        return String::from_utf8_lossy(&out)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
    }
    if let Ok(mounts) = std::fs::read_to_string("/proc/mounts") {
        return mounts
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                let (point, fs) = (*parts.get(1)?, *parts.get(2)?);
                let skip = point.starts_with("/proc") || point.starts_with("/sys") || point.starts_with("/run/user");
                (NET_FS.contains(&fs) && !skip).then(|| point.replace("\\040", " "))
            })
            .collect();
    }
    // macOS and the BSDs: "//me@nas/share on /Volumes/share (smbfs, nodev, nosuid, mounted by me)"
    let out = Command::new("mount").output().map(|o| o.stdout).unwrap_or_default();
    String::from_utf8_lossy(&out)
        .lines()
        .filter_map(|line| {
            let (_, rest) = line.split_once(" on ")?;
            let (point, opts) = rest.rsplit_once(" (")?;
            let fs = opts.split([',', ')']).next()?.trim();
            (NET_FS.contains(&fs) || ["afpfs", "webdav"].contains(&fs)).then(|| point.to_string())
        })
        .collect()
}

/// (folders that already hold actx data, suggested new folders), from network mounts and sync apps.
fn store_candidates() -> (Vec<String>, Vec<String>) {
    let mut bases = network_mounts();
    let home = &ctx().home;
    let mut synced: Vec<PathBuf> = SYNC_DIRS.iter().map(|d| home.join(d)).collect();
    // macOS: Dropbox, Google Drive, OneDrive… live in ~/Library/CloudStorage; iCloud Drive elsewhere
    synced.extend(
        std::fs::read_dir(home.join("Library/CloudStorage"))
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.path()),
    );
    synced.push(home.join("Library/Mobile Documents/com~apple~CloudDocs"));
    bases.extend(
        synced
            .into_iter()
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
            let (mut existing, suggested) = store_candidates();
            if let Some(root) = crate::store::configured_root().filter(|p| p.is_dir()) {
                let path = root.to_string_lossy().into_owned();
                existing.retain(|p| p != &path);
                existing.insert(0, path);
            }
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
    let root = crate::paths::canonical(&root).unwrap_or(root);
    let mut config = ctx().config.clone();
    config["root"] = json!(root.to_string_lossy());
    if let Some(lang) = &args.lang {
        config["lang"] = json!(lang);
    }
    if let Err(e) = crate::profile::configure(
        &mut config,
        args.harnesses.as_deref(),
        args.retention.as_deref(),
        args.wizard && interactive(),
        None,
    ) {
        die(&e.to_string());
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
    if args.wizard {
        crate::profile::show(&config, false);
    }
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
    let valid = name.chars().next().is_some_and(|c| c.is_alphanumeric())
        && name.chars().all(|c| c.is_alphanumeric() || "_.-".contains(c));
    if !valid {
        die(&tr!("Invalid name: use letters, numbers, '-', '_' or '.'"));
    }
    let root = store_root(false).expect("a shared folder");
    let Some(_lock) = lock_store(&root) else { return };
    if let Some(p) = find_project(&root, &name) {
        println!(
            "{}",
            tr!(
                "Project {name} already exists",
                name = p.file_name().unwrap_or_default().to_string_lossy()
            )
        );
    } else {
        if let Err(e) = std::fs::create_dir(root.join(&name)) {
            die(&e.to_string());
        }
        ok(&tr!("Project {name} created", name = bold(&name)));
    }
    let here = cwd();
    let folder = git_id(&here).map(|g| g.top).unwrap_or(here);
    let mut st = state::load();
    let bound = folder != ctx().home_str();
    if bound {
        if let Err(e) = crate::project::bind(&root, &name, &folder, &mut st) {
            die(&e.to_string());
        }
        save(&mut st);
        ok(&tr!("Work folder: {path}", path = short(&folder)));
    }
    drop(_lock);
    if bound && (interactive() || ctx().pick.is_some()) {
        add(Args {
            project: Some(name),
            ..Default::default()
        });
    } else {
        println!("{}", tr!("Next: {cmd}", cmd = bold(&format!("actx add {name}"))));
    }
}

/// Add local conversations to an existing project, or to General when no project is named.
pub fn add(args: Args) {
    let root = store_root(false).expect("a shared folder");
    let mut st = state::load();
    let requested = args.project.or_else(|| {
        (!args.general)
            .then(|| crate::project::current(&root, &st, &cwd()))
            .flatten()
    });
    let name = match requested {
        Some(wanted) => find_project(&root, &wanted)
            .map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                die(&tr!(
                    "Project {name} doesn't exist. Create it with: {cmd}",
                    name = wanted,
                    cmd = format!("actx mk {wanted}")
                ))
            }),
        None => GENERAL.to_string(),
    };
    let remote: HashMap<String, Session> = stored_sessions(&root).into_iter().map(|s| (s.key(), s)).collect();
    let folder = if name == GENERAL {
        None
    } else {
        crate::project::folder(&root, &name, &st)
    };
    let linked = crate::multisync::linked_keys();
    let mut ranked = all_local();
    ranked.retain(|s| !linked.contains(&s.key()));
    if !args.all
        && let Some(folder) = &folder
    {
        ranked.retain(|s| crate::project::candidate(folder, s));
    }
    if ranked.is_empty() && folder.is_some() && !args.all {
        println!(
            "{}",
            tr!(
                "No conversations found in this project folder; use actx add {name} --all to choose from other folders",
                name = name
            )
        );
        return;
    }
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
                    &clip(&pad(&tr!("in {project}", project = r.project_label()), 12), 12),
                ),
                None => " ".repeat(12),
            };
            let star = if name != GENERAL && relevance(&name, s) > 0 {
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
    let (rows, group_keys) = crate::catalog::Catalog::load_all().compact(rows);
    let header = tr!(
        "{name} · TAB marks several · Enter saves",
        name = collection_label(&name)
    );
    let keys: HashSet<String> = fzf(&rows, &header, true, true)
        .into_iter()
        .filter_map(|key| group_keys.get(&key).cloned())
        .collect();
    let chosen: Vec<&Session> = ranked.iter().filter(|s| keys.contains(&s.key())).collect();
    if chosen.is_empty() {
        return;
    }
    let mut todo = Vec::new();
    for s in chosen {
        if let Some(r) = remote.get(&s.key())
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
    let Some(_lock) = lock_store(&root) else { return };
    if name != GENERAL && find_project(&root, &name).is_none() {
        warn(&tr!("Project {name} doesn't exist", name = name));
        return;
    }
    if name != GENERAL && !crate::project::has_binding(&root, &name) {
        let folders: std::collections::BTreeSet<_> = todo
            .iter()
            .map(|s| git_id(&s.path).map(|g| g.top).unwrap_or_else(|| s.path.clone()))
            .filter(|p| !p.is_empty() && p != &ctx().home_str())
            .collect();
        if folders.len() == 1
            && let Some(folder) = folders.first()
            && let Err(e) = crate::project::bind(&root, &name, folder, &mut st)
        {
            fail(&e.to_string());
            return;
        }
    }
    let now = crate::store::pushed_now(&root);
    for s in todo {
        if moved_on(&now, &s.key(), remote.get(&s.key()).map(pushed_at), &s.title) {
            continue;
        }
        match push_one(&root, &name, s, git_info(&s.path), &mut st, &remote) {
            Ok(moved) => {
                let note = moved
                    .map(|p| {
                        dim(&format!(
                            "  {}",
                            tr!("(moved from {project})", project = collection_label(&p))
                        ))
                    })
                    .unwrap_or_default();
                ok(&format!("{} {}{note}", harness_tag(s.harness), s.title));
            }
            Err(e) => fail(&format!("{}: {e}", s.title)),
        }
    }
    save(&mut st);
    drop(_lock);
    crate::multisync::run(&root, &Args::default());
    println!("\n{}", tr!("On your other machines: {cmd}", cmd = bold("actx sync")));
    sync_self(&root);
}

// ── sync ────────────────────────────────────────────────────────────────────

type Pending = (&'static str, Option<Session>, Session);

/// Which projects to sync: the one you name, all of them (--all, or no terminal), or the ones you pick.
fn pick_sync_projects(pending: &BTreeMap<String, Vec<Pending>>, args: &Args) -> Vec<String> {
    let wanted = args
        .general
        .then_some(GENERAL.to_string())
        .or_else(|| args.project.clone());
    if let Some(wanted) = &wanted {
        return match pending.keys().find(|p| p.to_lowercase() == wanted.to_lowercase()) {
            Some(p) => vec![p.clone()],
            None => {
                ok(&tr!("{project}: nothing to sync", project = collection_label(wanted)));
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
    let width = order
        .iter()
        .map(|p| collection_label(p).chars().count())
        .max()
        .unwrap_or(0);
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
                line: format!("{mark} {}  {}", pad(&collection_label(p), width), counts.join("  ")),
                preview: format!("{}\n\n{}", bold(&collection_label(p)), listing.join("\n")),
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

/// Take the shared folder's lock before writing to it. None: busy (automatic runs give up quietly).
pub(crate) fn lock_store(root: &Path) -> Option<SharedLock> {
    let auto = is_auto();
    let wait = Duration::from_secs(ctx().lock_wait.unwrap_or(if auto { 120 } else { 60 }));
    let waiting = |h: &Holder| {
        if !auto {
            println!(
                "{}",
                dim(&tr!(
                    "{host} is writing to the shared folder (since {time}): waiting…",
                    host = h.host,
                    time = local_time(h.since, "%H:%M")
                ))
            );
        }
    };
    match SharedLock::acquire(root, wait, waiting) {
        Ok(lock) => Some(lock),
        Err(LockError::Busy(h)) => {
            let msg = tr!(
                "The shared folder is still busy ({host}, since {time}). Try again in a while; if nothing is running there, delete {path}",
                host = h.host,
                time = local_time(h.since, "%H:%M"),
                path = root.join(".lock").display()
            );
            if auto {
                say(&msg)
            } else {
                die(&msg)
            }
            None
        }
        Err(LockError::Io(e)) => {
            let msg = tr!("Can't write to the shared folder: {err}", err = e);
            if auto {
                say(&msg)
            } else {
                die(&msg)
            }
            None
        }
    }
}

/// Did someone push `key` since we looked (`was`: when it had been pushed then, None if it wasn't there)?
fn moved_on(now: &HashMap<String, f64>, key: &str, was: Option<f64>, title: &str) -> bool {
    let changed = now.get(key).copied() != was;
    if changed && is_auto() {
        say(&dim(&tr!(
            "«{title}» changed in the shared folder meanwhile: next time",
            title = title
        )));
    } else if changed {
        warn(&tr!(
            "«{title}» changed in the shared folder meanwhile: run actx sync again",
            title = title
        ));
    }
    changed
}

/// An open conversation can't be replaced. Automatic runs leave it for later; otherwise ask.
fn may_replace(l: &Session, args: &Args) -> bool {
    if is_auto() {
        return !(in_use(l) || args.busy.as_deref() == Some(l.key().as_str()));
    }
    if !in_use_by_agent(l) {
        return true;
    }
    confirm(&tr!(
        "«{title}» is open in {tool} right now: what you do there would be lost. Replace it anyway?",
        title = l.title,
        tool = harness::get(l.harness).label()
    ))
}

/// General conversations have an explicit destination per conversation on this machine.
/// Never infer it from the sender's cwd, Git repo, or another conversation's chosen folder.
pub(crate) fn session_folder(r: &Session, st: &mut State, prompt: bool) -> Option<String> {
    if !r.is_general()
        && let Some(root) = crate::store::store_root(true)
        && let Some(name) = r.project.as_deref()
        && crate::project::has_binding(&root, name)
    {
        return crate::project::placement(&root, r, st, prompt);
    }
    let key = format!("session:{}", r.key());
    if let Some(folder) = st.folders.get(&key).filter(|p| Path::new(p).is_dir()) {
        return Some(folder.clone());
    }
    let meta = r.meta();
    // A project label does not make a conversation started in $HOME belong to that repo.
    // Ask for its work folder rather than silently installing every harness in $HOME.
    let home_project =
        !r.is_general() && !meta.get("git").is_some_and(Value::is_object) && jstr(meta, "path") == jstr(meta, "home");
    if !r.is_general() && !home_project {
        return resolve_folder(meta, st).or_else(|| prompt.then(|| ask_folder(meta, st)).flatten());
    }
    if !prompt {
        return None;
    }
    let mut scratch = st.clone();
    let mut meta = r.meta().clone();
    meta["git"] = Value::Null;
    let folder = ask_folder(&meta, &mut scratch)?;
    st.folders.insert(key, folder.clone());
    Some(folder)
}

pub fn sync(mut args: Args) {
    if args.dry_run {
        crate::sync_plan::preview(&args);
        return;
    }
    let auto = args.auto;
    let root = if auto {
        set_auto();
        crate::auto::rotate_log();
        if let Some(pid) = args.after {
            crate::agents::wait_for(pid);
        }
        std::thread::sleep(Duration::from_secs(args.delay));
        match crate::store::reachable_root(Duration::from_secs(10)) {
            Some(root) => root,
            None => return, // away from home, say: next time
        }
    } else {
        store_root(false).expect("a shared folder")
    };
    if !auto && !args.all && !args.general && args.project.is_none() {
        args.project = crate::project::current(&root, &state::load(), &cwd());
    }
    let explicit = auto || args.all || args.general || args.project.is_some();
    if explicit {
        crate::multisync::run(&root, &args);
    }
    let mut maintenance = SyncMaintenance {
        root: &root,
        scopes: if explicit { vec![args.clone()] } else { Vec::new() },
    };
    let mut remote = stored_sessions(&root);
    remote.retain(|r| {
        (!args.general || r.is_general())
            && args
                .project
                .as_ref()
                .is_none_or(|p| r.project.as_ref().is_some_and(|name| name.eq_ignore_ascii_case(p)))
    });
    if let Some(keys) = &args.keys {
        remote.retain(|r| keys.contains(&r.key()));
    }
    if remote.is_empty() {
        if !auto {
            println!(
                "{}",
                tr!(
                    "No conversations in the shared folder yet. Start with: {cmd}",
                    cmd = bold("actx add")
                )
            );
        }
        return;
    }
    let mut st = state::load();
    let local = {
        let _loading = crate::ui::loading(&tr!("Loading synced conversations"));
        let mut keys: HashSet<_> = remote.iter().map(Session::key).collect();
        crate::multisync::extend_keys(&mut keys);
        harness::local_keys(&keys)
            .into_iter()
            .map(|s| (s.key(), s))
            .collect::<HashMap<_, _>>()
    };
    remote.sort_by(|a, b| a.project.cmp(&b.project).then(b.updated.total_cmp(&a.updated)));
    let mut pending: BTreeMap<String, Vec<Pending>> = BTreeMap::new();
    let mut in_sync = 0;
    for (state, r) in crate::sync_plan::native(&local, remote, &st) {
        let l = local.get(&r.key());
        if state == "synced" {
            in_sync += 1;
            let l = l.expect("synced means it's here");
            st.base.entry(r.key()).or_insert((l.weight, r.weight)).0 = l.weight; // as this version measures it
        } else {
            pending
                .entry(r.project.clone().unwrap_or_default())
                .or_default()
                .push((state, l.cloned(), r));
        }
    }
    if pending.is_empty() {
        if maintenance.scopes.is_empty() {
            maintenance.scopes.push(args.clone());
        }
        save(&mut st);
        if !auto {
            ok(&tr!("All in sync ({n} conversations)", n = in_sync));
        }
        sync_self(&root);
        return;
    }
    let chosen = if auto {
        pending
            .keys()
            .filter(|p| {
                (!args.general || p.as_str() == GENERAL)
                    && args
                        .project
                        .as_ref()
                        .is_none_or(|name| p.to_lowercase() == name.to_lowercase())
            })
            .cloned()
            .collect()
    } else {
        pick_sync_projects(&pending, &args)
    };
    if chosen.is_empty() {
        save(&mut st);
        return;
    }
    if maintenance.scopes.is_empty() {
        maintenance.scopes = chosen
            .iter()
            .map(|name| Args {
                project: Some(name.clone()),
                ..args.clone()
            })
            .collect();
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
        if auto {
            warn(&tr!(
                "{project} · «{title}» changed here and on {host}: run actx sync to decide",
                project = r.project_label(),
                title = r.title,
                host = host,
            ));
            continue;
        }
        warn(&tr!(
            "{project} · «{title}» changed here and on {host} since the last sync.",
            project = r.project_label(),
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
    // where each pull goes: decided (and asked) before taking the lock
    let mut placed = Vec::new();
    for (l, r) in down {
        if let Some(l) = &l
            && !may_replace(l, &args)
        {
            if auto {
                say(&dim(&tr!(
                    "«{title}» is open in {tool}: it'll be pulled later",
                    title = r.title,
                    tool = harness::get(r.harness).label()
                )));
            } else {
                println!("{}", dim(&format!("  {}", tr!("skipped"))));
            }
            continue;
        }
        // if you already have it, it stays where it is; if it's new, find its place
        let folder = match &l {
            Some(l) if Path::new(&l.path).is_dir() => Some(l.path.clone()),
            _ => session_folder(&r, &mut st, !auto),
        };
        match folder {
            Some(folder) => placed.push((l, r, folder)),
            None if !auto => warn(&tr!(
                "{project} · «{title}»: I don't know which folder it goes in on this machine; skipping",
                project = r.project_label(),
                title = r.title,
            )),
            None if r.is_general() => say(&tr!(
                "«{title}» needs a destination folder: run actx sync --general",
                title = r.title
            )),
            None => {}
        }
    }
    if !auto {
        warn_dirty(
            &up.iter()
                .filter_map(|(l, _)| l.as_ref().map(|l| l.path.clone()))
                .collect::<BTreeSet<_>>(),
        );
    }
    let mut ready = Vec::new();
    if !(up.is_empty() && placed.is_empty() && merges.is_empty())
        && let Some(_lock) = lock_store(&root)
    {
        let now = crate::store::pushed_now(&root);
        let none = HashMap::new();
        for (l, r) in &up {
            let Some(l) = l else { continue };
            if moved_on(&now, &r.key(), Some(pushed_at(r)), &r.title) {
                continue;
            }
            let project = r.project.clone().unwrap_or_default();
            match push_one(&root, &project, l, git_info(&l.path), &mut st, &none) {
                Ok(_) => say(&format!(
                    "{} {} {} {}",
                    paint("33", "↑"),
                    pad(&collection_label(&project), 14),
                    harness_tag(r.harness),
                    r.title
                )),
                Err(e) => fail(&format!("{}: {e}", r.title)),
            }
        }
        let mut checked = HashSet::new();
        for (l, r, folder) in &placed {
            if moved_on(&now, &r.key(), Some(pushed_at(r)), &r.title) {
                continue;
            }
            if !auto && checked.insert(folder.clone()) {
                check_git(r.meta(), folder);
            }
            let fix = fixer_for(r.meta(), folder);
            match harness::get(r.harness).install(r, l.as_ref(), folder, fix.as_ref()) {
                Ok(weight) => {
                    st.base.insert(r.key(), (weight, r.weight));
                    let project = r.project.clone().unwrap_or_default();
                    say(&format!(
                        "{} {} {} {}  {}",
                        paint("36", "↓"),
                        pad(&collection_label(&project), 14),
                        harness_tag(r.harness),
                        r.title,
                        dim(&format!("→ {}", short(folder)))
                    ));
                    ready.push((r, folder));
                }
                Err(e) => fail(&format!("{}: {e}", r.title)),
            }
        }
        for (l, r) in merges {
            let Some(mut l) = l else { continue };
            if moved_on(&now, &r.key(), Some(pushed_at(&r)), &r.title) {
                continue;
            }
            if let Err(e) = crate::merge::merge_one(&root, &mut l, &r, &mut st) {
                fail(&tr!(
                    "{title}: couldn't merge ({err}); nothing else was touched",
                    title = r.title,
                    err = e
                ));
            }
        }
    }
    save(&mut st);
    if auto {
        sync_self(&root);
        return;
    }
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
                resume_cmd(r, folder),
                dim(crate::ui::take_chars(&r.title, 50))
            );
        }
    }
    sync_self(&root);
}

// ── ls ──────────────────────────────────────────────────────────────────────

pub fn ls(args: Args) {
    crate::browser::list(&args);
}

// ── rm ──────────────────────────────────────────────────────────────────────

/// Remove a whole project. Conversation selection belongs to `delete`.
pub fn rm(args: Args) {
    let root = store_root(false).expect("a shared folder");
    let wanted = args.project.unwrap_or_default();
    let Some(pdir) = find_project(&root, &wanted) else {
        die(&tr!("Project {name} doesn't exist", name = wanted))
    };
    let pname = pdir.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let snapshot = |sessions: Vec<Session>| -> BTreeMap<String, u64> {
        sessions
            .into_iter()
            .filter(|r| r.project.as_deref() == Some(&pname))
            .map(|r| (r.key(), pushed_at(&r).to_bits()))
            .collect()
    };
    let before = snapshot(stored_sessions(&root));
    let logical_count = crate::catalog::Catalog::load()
        .groups
        .iter()
        .filter(|g| g.remote().iter().any(|r| r.project.as_deref() == Some(&pname)))
        .count();
    if !confirm(&tr!(
        "Delete project {name} and all its saved conversations ({n})? Local originals are kept.",
        name = pname,
        n = logical_count
    )) {
        return;
    }
    let Some(_lock) = lock_store(&root) else { return };
    if !pdir.is_dir() || snapshot(stored_sessions(&root)) != before {
        warn(&tr!(
            "Project {name} changed meanwhile: run actx rm again",
            name = pname
        ));
        return;
    }
    if let Err(e) = std::fs::remove_dir_all(&pdir) {
        fail(&e.to_string());
        return;
    }
    let mut st = state::load();
    for key in before.keys() {
        st.base.remove(key);
        st.folders.remove(&format!("session:{key}"));
    }
    st.folders.remove(&crate::project::key(&pname));
    save(&mut st);
    ok(&tr!("Project {name} deleted", name = pname));
}

/// Remove selected shared copies, leaving both projects and local originals in place.
pub fn delete(args: Args) {
    let root = store_root(false).expect("a shared folder");
    let scope = if args.general {
        Some(GENERAL.to_string())
    } else {
        args.project.map(|name| {
            find_project(&root, &name)
                .map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned())
                .unwrap_or_else(|| die(&tr!("Project {name} doesn't exist", name = name)))
        })
    };
    let mut sessions = stored_sessions(&root);
    sessions.retain(|r| scope.as_ref().is_none_or(|p| r.project.as_ref() == Some(p)));
    sessions.sort_by(|a, b| b.updated.total_cmp(&a.updated));
    if sessions.is_empty() {
        println!("{}", tr!("No saved conversations to delete"));
        return;
    }
    let rows: Vec<Row> = sessions
        .iter()
        .map(|r| Row {
            key: r.key(),
            line: format!(
                "{} {}  {}  {}",
                harness_tag(r.harness),
                local_time(r.updated, "%d %b %H:%M"),
                r.title,
                dim(&r.project_label())
            ),
            preview: session_preview(r),
        })
        .collect();
    let catalog = crate::catalog::Catalog::load();
    let (rows, _) = catalog.compact(rows);
    let keys: HashSet<String> = fzf(
        &rows,
        &tr!("Delete saved conversations · local originals are kept · TAB marks several"),
        true,
        true,
    )
    .into_iter()
    .collect();
    let selected: HashSet<_> = keys
        .iter()
        .filter_map(|key| catalog.find(key))
        .flat_map(|g| g.remote().into_iter().map(|s| s.key()))
        .collect();
    let chosen: Vec<&Session> = sessions.iter().filter(|r| selected.contains(&r.key())).collect();
    delete_saved(&root, &chosen);
}

pub(crate) fn delete_saved(root: &Path, chosen: &[&Session]) {
    let count = chosen
        .iter()
        .map(|r| {
            r.meta()
                .get("conversation_id")
                .and_then(Value::as_str)
                .map(String::from)
                .unwrap_or_else(|| crate::catalog::identity(&r.key()))
        })
        .collect::<HashSet<_>>()
        .len();
    if chosen.is_empty()
        || !confirm(&tr!(
            "Delete {n} saved conversation(s)? Local originals and projects are kept.",
            n = count
        ))
    {
        return;
    }
    let Some(_lock) = lock_store(root) else { return };
    let current: HashMap<String, Session> = stored_sessions(root).into_iter().map(|r| (r.key(), r)).collect();
    let now = current.iter().map(|(key, r)| (key.clone(), pushed_at(r))).collect();
    let mut st = state::load();
    let mut removed = 0;
    for &r in chosen {
        if moved_on(&now, &r.key(), Some(pushed_at(r)), &r.title)
            || current.get(&r.key()).is_none_or(|c| c.project != r.project)
        {
            continue;
        }
        match remove_stored(r) {
            Ok(()) => {
                st.base.remove(&r.key());
                st.folders.remove(&format!("session:{}", r.key()));
                removed += 1;
            }
            Err(e) => fail(&format!("{}: {e}", r.title)),
        }
    }
    save(&mut st);
    ok(&tr!("Removed {n} shared copy/copies", n = removed));
}

// ── cv ──────────────────────────────────────────────────────────────────────

/// The tool to copy the chosen conversations into: asked only if there's more than one option.
fn pick_target(chosen: &[&Session]) -> Option<&'static str> {
    let sources: HashSet<&str> = chosen.iter().map(|s| s.harness).collect();
    let options: Vec<&'static str> = harness::CONVERTS
        .iter()
        .copied()
        .filter(|n| harness::get(n).installed() && !(sources.len() == 1 && sources.contains(n)))
        .collect();
    match options.len() {
        0 => die(&tr!(
            "There's no other tool to convert to: install Claude Code, opencode, Codex or Antigravity"
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
    let _loading = crate::ui::loading(&tr!("Syncing {title} to {tool}", title = s.title, tool = target));
    use crate::ctx::Error;
    let source = harness::get(s.harness);
    let (folder, (turns, model)) = if let Some(meta) = &s.meta {
        let folder = session_folder(s, st, true)
            .ok_or_else(|| Error(tr!("I don't know which folder it goes in on this machine")))?;
        let fix = fixer_for(meta, &folder);
        let turns = source.stored_turns(s, fix.as_ref())?;
        (folder, turns)
    } else {
        let (mut turns, model) = source.local_turns(s)?;
        if turns.is_empty() {
            return Err(Error(tr!("it has no text messages to convert")));
        }
        let folder = if !s.path.is_empty() && Path::new(&s.path).is_dir() {
            s.path.clone()
        } else {
            ask_local_folder(&s.title, &s.path, st).ok_or_else(|| {
                if s.path.is_empty() {
                    Error(tr!("I don't know which folder it goes in on this machine"))
                } else {
                    Error(tr!("{path} doesn't exist on this machine", path = short(&s.path)))
                }
            })?
        };
        if !s.path.is_empty()
            && s.path != folder
            && let Some(fix) = crate::paths::PathFixer::new(&[(&s.path, &folder)])
        {
            for t in &mut turns {
                let json = serde_json::to_vec(&t.text)?;
                t.text = serde_json::from_slice(&fix.fix(&json))?;
            }
        }
        (folder, (turns, model))
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
    let mut local = all_local();
    local.extend(harness::get("antigravity").local()); // it converts, though it doesn't sync
    let have: HashSet<String> = local.iter().map(Session::key).collect();
    let mut candidates: Vec<Session> = local;
    candidates.extend(
        remote
            .iter()
            .filter(|(k, _)| !have.contains(*k))
            .map(|(_, r)| r.clone()),
    );
    let catalog = crate::catalog::Catalog::load_all();
    let scope = args.general.then_some(GENERAL.to_string()).or(args.project);
    if let Some(name) = &scope {
        let name = name.to_lowercase();
        candidates.retain(|s| {
            catalog.find(&s.key()).is_some_and(|g| {
                g.remote()
                    .iter()
                    .any(|r| r.project.as_deref().map(str::to_lowercase) == Some(name.clone()))
            })
        });
        if candidates.is_empty() {
            die(&tr!(
                "No conversations in project {name}",
                name = collection_label(&name)
            ));
        }
    }
    if candidates.is_empty() {
        die(&tr!(
            "No Claude Code, opencode, Codex or Antigravity conversations found"
        ));
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
            match remote.get(&s.key()) {
                Some(r) => details += &format!(" · {}", tr!("in {project}", project = r.project_label())),
                None if root.is_some() => details += &format!(" · {}", tr!("no project")),
                None => {}
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
    let (rows, _) = catalog.compact(rows);
    let header = tr!("Convert · creates a copy in another tool, the original is left untouched · TAB marks several");
    let keys: HashSet<String> = fzf(&rows, &header, true, true).into_iter().collect();
    let mut chosen = Vec::new();
    for key in keys {
        let Some(group) = catalog.find(&key) else { continue };
        let sources: Vec<_> = candidates
            .iter()
            .filter(|s| group.copies.iter().any(|c| c.session().key() == s.key()))
            .collect();
        if sources.len() == 1 {
            chosen.push(sources[0]);
        } else {
            let rows: Vec<Row> = sources
                .iter()
                .map(|s| Row {
                    key: s.id.clone(),
                    line: format!("{} {}", harness_tag(s.harness), s.title),
                    preview: session_preview(s),
                })
                .collect();
            if let Some(key) = fzf(&rows, &tr!("Convert from which harness?"), false, true).first()
                && let Some(s) = sources.into_iter().find(|s| s.id == *key)
            {
                chosen.push(s);
            }
        }
    }
    if chosen.is_empty() {
        return;
    }
    let Some(target) = pick_target(&chosen) else { return };
    let mut st = state::load();
    let mut ready = Vec::new();
    for s in chosen {
        if let Some(group) = catalog.find(&s.key())
            && let Some(existing) = group.copies.iter().find(|c| c.session().harness == target)
        {
            if let Some(local) = &existing.local {
                ok(&tr!(
                    "Using the existing {tool} copy of {title}",
                    tool = harness::get(target).label(),
                    title = group.title
                ));
                ready.push(local.clone());
            } else {
                warn(&tr!("This harness copy is shared only: sync it first"));
            }
            continue;
        }

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
        let identity = catalog.find(&s.key()).map(|g| g.id.clone()).unwrap_or_else(|| s.key());
        if let Err(e) = crate::catalog::remember(&identity, &new.key()) {
            warn(&e.to_string());
        }
        let mut note = String::new();
        if let (Some(root), Some(r)) = (&root, remote.get(&s.key()))
            && harness::syncs(new.harness)
        {
            // the copy travels with its project
            let project = r.project.clone().unwrap_or_default();
            if let Some(_lock) = lock_store(root) {
                if crate::store::find_stored(root, r.harness, &r.id).is_some_and(|current| current.project == r.project)
                {
                    note = match push_one(root, &project, &new, git_info(&new.path), &mut st, &HashMap::new()) {
                        Ok(_) => dim(&format!(
                            "  {}",
                            tr!("(also saved in {project})", project = collection_label(&project))
                        )),
                        Err(e) => paint(
                            "31",
                            &format!(
                                "  {}",
                                tr!(
                                    "(couldn't save it in {project}: {err})",
                                    project = collection_label(&project),
                                    err = e
                                )
                            ),
                        ),
                    };
                } else {
                    warn(&tr!(
                        "«{title}» changed in the shared folder meanwhile: run actx sync again",
                        title = r.title
                    ));
                }
            }
        }
        ok(&format!(
            "{} → {} {}{note}",
            harness_tag(s.harness),
            harness_tag(new.harness),
            new.title
        ));
        ready.push(new);
    }
    save(&mut st);
    if !ready.is_empty() {
        println!(
            "{}",
            dim(&tr!(
                "Note: tool calls become text, and reasoning, images, and messages before a compaction are left out."
            ))
        );
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
    // Windows only runs files that end in .exe
    let tmp = me.with_file_name(if cfg!(windows) { ".actx.new.exe" } else { ".actx.new" });
    let installed = (|| -> std::io::Result<String> {
        std::fs::write(&tmp, &new)?;
        crate::paths::make_executable(&tmp)?;
        let out = crate::ctx::quiet(&tmp).arg("--version").output()?;
        let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !version.starts_with("actx ") {
            return Err(std::io::Error::other(format!("{} {version}", out.status)));
        }
        let when = std::fs::metadata(&shared)?.modified()?;
        std::fs::File::options().write(true).open(&tmp)?.set_modified(when)?; // so it doesn't look "newer" than the shared one
        if cfg!(windows) {
            // a running .exe can be renamed but not replaced: move it out of the way first
            let old = me.with_extension("old.exe");
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&me, &old)?;
            if let Err(e) = std::fs::rename(&tmp, &me) {
                let _ = std::fs::rename(&old, &me);
                return Err(e);
            }
        } else {
            std::fs::rename(&tmp, &me)?;
        }
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

// ── restore ─────────────────────────────────────────────────────────────────

fn backup_title(b: &Backup, local: &HashMap<String, Session>) -> String {
    if !b.title.is_empty() {
        return b.title.clone();
    }
    local
        .get(&format!("{}:{}", b.harness, b.id))
        .map(|l| l.title.clone())
        .unwrap_or_else(|| b.id.clone())
}

/// Put a backed-up conversation back. Returns its folder.
fn restore_one(b: &Backup, current: Option<&Session>) -> crate::ctx::Result<String> {
    let _loading = crate::ui::loading(&tr!("Restoring the conversation"));
    use crate::ctx::Error;
    use crate::state::{Copy, backup};
    if let Some(c) = current
        && in_use_by_agent(c)
    {
        return Err(Error(tr!(
            "it's open in {tool} right now: close it first",
            tool = harness::get(c.harness).label()
        )));
    }
    let folder = current
        .map(|c| c.path.clone())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| b.path.clone());
    if b.harness == "opencode" {
        if folder.is_empty() || !Path::new(&folder).is_dir() {
            return Err(Error(tr!("I don't know which folder it goes in on this machine")));
        }
        let data = std::fs::read(&b.file)?;
        if let Some(c) = current {
            backup(c, Copy::Bytes(&crate::opencode::export(&c.id, &c.path)?), "restore")?;
            crate::opencode::delete(&c.id, &c.path)?;
        }
        crate::opencode::import(&data, &folder)?;
        return Ok(folder);
    }
    let dest = current
        .and_then(|c| c.src.clone())
        .or_else(|| b.src.clone())
        .ok_or_else(|| Error(tr!("I don't know which folder it goes in on this machine")))?;
    if let Some(c) = current
        && let Some(src) = &c.src
    {
        backup(c, Copy::File(src), "restore")?;
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = dest.with_extension("actx-tmp");
    std::fs::copy(&b.file, &tmp)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(folder)
}

pub fn restore() {
    let all = state::backups();
    if all.is_empty() {
        println!(
            "{}",
            tr!("No backups yet: actx makes one before replacing a conversation on this machine.")
        );
        return;
    }
    let local = local_index();
    let rows: Vec<Row> = all
        .iter()
        .map(|b| {
            let title = backup_title(b, &local);
            let size = b.file.metadata().map(|m| m.len()).unwrap_or(0);
            let msgs = if b.harness == "opencode" {
                crate::opencode::file_preview(&b.file)
            } else {
                let mut s = Session::local(b.harness, b.id.clone(), title.clone(), b.path.clone(), b.at, size);
                s.src = Some(b.file.clone());
                harness::get(b.harness).preview(&s)
            };
            Row {
                key: b.file.to_string_lossy().into_owned(),
                line: format!(
                    "{}  {} {}  {}",
                    local_time(b.at, "%d %b %H:%M"),
                    harness_tag(b.harness),
                    pad(&clip(&title, 42), 42),
                    dim(&format!("{} · {}", short(&b.path), crate::ui::human(size)))
                ),
                preview: format!(
                    "{}\n{}\n\n{}",
                    bold(&title),
                    dim(&tr!("backed up {ago}", ago = ago(b.at))),
                    render(&msgs, b.harness)
                ),
            }
        })
        .collect();
    let header = tr!(
        "Backups, newest first (kept {n} days) · Enter restores one",
        n = state::KEEP_BACKUP_DAYS
    );
    let Some(picked) = fzf(&rows, &header, false, true).into_iter().next() else {
        return;
    };
    let Some(b) = all.iter().find(|b| b.file.to_string_lossy() == picked) else {
        return;
    };
    let title = backup_title(b, &local);
    let current = local.get(&format!("{}:{}", b.harness, b.id));
    let when = local_time(b.at, "%d %b %H:%M");
    let question = if current.is_some() {
        tr!(
            "Replace «{title}» on this machine with the copy from {when}? (the current one is backed up first)",
            title = title,
            when = when
        )
    } else {
        tr!("Restore «{title}» from {when}?", title = title, when = when)
    };
    if !confirm(&question) {
        return;
    }
    match restore_one(b, current) {
        Ok(folder) => {
            ok(&tr!("Restored «{title}»", title = title));
            println!(
                "{}",
                dim(&tr!(
                    "If it's in a project, the next sync pushes it (or asks, if it changed elsewhere too)."
                ))
            );
            println!(
                "\n{}\n  {}",
                tr!("To continue:"),
                harness::get(b.harness).resume(&b.id, &folder)
            );
        }
        Err(e) => fail(&format!("{title}: {e}")),
    }
}

// ── doctor ──────────────────────────────────────────────────────────────────

fn first_line(program: &str, args: &[&str]) -> Option<String> {
    let out = crate::ctx::quiet(program).args(args).output().ok()?;
    let text = String::from_utf8_lossy(&[out.stdout, out.stderr].concat()).into_owned();
    text.lines()
        .next()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
}

pub fn doctor() {
    let line = |mark: &str, what: &str, detail: &str| println!("  {mark} {} {detail}", pad(what, 18));
    let good = |what: &str, detail: &str| line(&paint("32", "✓"), what, detail);
    let bad = |what: &str, detail: &str| line(&paint("31", "✗"), what, detail);
    let meh = |what: &str, detail: &str| line(&paint("33", "⚠"), what, detail);
    let none = |what: &str, detail: &str| line(&dim("·"), what, &dim(detail));
    let mut problems = 0;

    println!(
        "{} {}",
        bold(&format!("actx {}", crate::ctx::VERSION)),
        dim(&format!(
            "· {} · {}",
            install_kind(),
            short(&ctx().self_path.to_string_lossy())
        ))
    );
    let config = short(&ctx().config_file.to_string_lossy());
    match crate::store::configured_root() {
        None => {
            problems += 1;
            bad(
                &tr!("shared folder"),
                &tr!("not configured: run {cmd}", cmd = bold("actx init")),
            );
        }
        Some(root) => match crate::store::reachable_root(Duration::from_secs(5)) {
            None => {
                problems += 1;
                bad(
                    &tr!("shared folder"),
                    &tr!(
                        "{path}: can't reach it (if it's a network share: is it mounted?)",
                        path = short(&root.to_string_lossy())
                    ),
                );
            }
            Some(root) => {
                let writable = tempfile::tempfile_in(&root).is_ok();
                let stored = stored_sessions(&root);
                let detail = tr!(
                    "{path} · {p} project(s), {n} conversation(s)",
                    path = short(&root.to_string_lossy()),
                    p = project_dirs(&root).len(),
                    n = stored.len()
                );
                if writable {
                    good(&tr!("shared folder"), &detail);
                } else {
                    problems += 1;
                    bad(
                        &tr!("shared folder"),
                        &format!("{detail} · {}", tr!("can't write to it")),
                    );
                }
                let lock = root.join(".lock");
                if let Some(v) = std::fs::read_to_string(&lock)
                    .ok()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                {
                    let since = v.get("since").and_then(Value::as_f64).unwrap_or(0.0);
                    meh(
                        &tr!("lock"),
                        &tr!(
                            "{host} is writing to it (since {ago}); if nothing is running there, delete {path}",
                            host = jstr(&v, "host"),
                            ago = ago(since),
                            path = lock.display()
                        ),
                    );
                }
            }
        },
    }
    println!("  {}", dim(&tr!("config: {path}", path = config)));

    println!("\n{}", bold(&tr!("Tools")));
    for (program, what) in [("git", "git"), ("fzf", "fzf")] {
        match first_line(program, &["--version"]) {
            Some(v) => good(what, &v),
            None => {
                problems += 1;
                bad(what, &tr!("not found: actx needs it"));
            }
        }
    }
    for name in harness::CONVERTS {
        let h = harness::get(name);
        if !h.installed() {
            none(h.label(), &tr!("not installed"));
            continue;
        }
        let count = h.local().len();
        let version = match name {
            "claude" => crate::claude::version().to_string(),
            "codex" => crate::codex::version().to_string(),
            "antigravity" => first_line("agy", &["--version"]).unwrap_or_default(),
            _ => first_line("opencode", &["--version"])
                .and_then(|v| {
                    v.split_whitespace()
                        .last()
                        .map(|v| v.trim_start_matches('v').to_string())
                })
                .unwrap_or_default(),
        };
        let mut detail = if version.is_empty() {
            tr!("{n} conversation(s)", n = count)
        } else {
            tr!("{version} · {n} conversation(s)", version = version, n = count)
        };
        if !harness::syncs(name) {
            detail += &format!(" · {}", tr!("converts, doesn't sync yet"));
        }
        if name == "opencode" {
            let cli = version.split('.').next().and_then(|v| v.parse::<u8>().ok());
            let db = crate::opencode::version();
            if let (Some(cli), Some(db)) = (cli, db)
                && cli != db
            {
                problems += 1;
                meh(
                    h.label(),
                    &format!(
                        "{detail} · {}",
                        tr!("the CLI is {a}.x but its database is from {b}.x", a = cli, b = db)
                    ),
                );
                continue;
            }
        }
        good(h.label(), &detail);
    }

    println!("\n{}", bold(&tr!("Automatic sync")));
    let parts = crate::auto::parts();
    if parts.iter().any(|(_, on)| *on) {
        for (what, on) in &parts {
            if *on {
                good(what, "");
            } else {
                none(what, "");
            }
        }
        if let Some(last) = crate::auto::last_log(1).pop() {
            println!("  {}", dim(&tr!("last: {line}", line = last)));
        }
    } else {
        none(&tr!("off"), &tr!("turn it on with actx auto on"));
    }
    let running = crate::agents::summary();
    if !running.is_empty() {
        let mut seen = BTreeSet::new();
        let mut open: Vec<String> = running
            .iter()
            .filter(|(_, cwd)| seen.insert(cwd.clone()))
            .map(|(tool, cwd)| format!("{tool} {}", short(cwd)))
            .collect();
        if open.len() > 4 {
            let more = open.len() - 3;
            open.truncate(3);
            open.push(format!("+{more}"));
        }
        println!("  {}", dim(&tr!("agents running now: {list}", list = open.join(", "))));
    }

    println!("\n{}", bold(&tr!("This machine")));
    let st = state::load();
    let backups = state::backups();
    let size: u64 = backups
        .iter()
        .filter_map(|b| b.file.metadata().ok())
        .map(|m| m.len())
        .sum();
    good(
        &tr!("state"),
        &tr!(
            "{n} conversation(s) tracked · {path}",
            n = st.base.len(),
            path = short(&ctx().state_file().to_string_lossy())
        ),
    );
    good(
        &tr!("backups"),
        &tr!(
            "{n} · {size} · kept {days} days (actx restore)",
            n = backups.len(),
            size = crate::ui::human(size),
            days = state::KEEP_BACKUP_DAYS
        ),
    );
    println!();
    if problems == 0 {
        ok(&tr!("All good"));
    } else {
        warn(&tr!("{n} problem(s) above", n = problems));
    }
}

struct SyncMaintenance<'a> {
    root: &'a Path,
    scopes: Vec<Args>,
}
impl Drop for SyncMaintenance<'_> {
    fn drop(&mut self) {
        for args in &self.scopes {
            crate::multisync::run(self.root, args);
            if !args.skip_retention {
                crate::multisync::prune(self.root, args);
            }
        }
    }
}
pub fn profile(mut args: Args) {
    if args.wizard {
        if !interactive() && ctx().pick.is_none() {
            die(&tr!(
                "Use an interactive terminal, or configure the profile with --harnesses and --retention"
            ));
        }
        let mut config = crate::profile::current();
        if let Err(e) =
            crate::profile::configure(&mut config, None, None, true, None).and_then(|_| crate::profile::save(&config))
        {
            die(&e.to_string());
        }
        crate::profile::show(&config, false);
        return;
    }

    if args.conversation.as_deref() == Some("::pick") {
        let catalog = crate::catalog::Catalog::load();
        let rows: Vec<_> = catalog
            .groups
            .iter()
            .filter(|g| !g.remote().is_empty())
            .map(|g| g.row())
            .collect();
        args.conversation = fzf(&rows, &tr!("Choose a conversation profile"), false, true)
            .first()
            .and_then(|key| catalog.find(key))
            .map(|g| g.id.clone());
        if args.conversation.is_none() {
            return;
        }
    }
    let family = args
        .conversation
        .as_ref()
        .and_then(|key| crate::catalog::Catalog::load().find(key).cloned());
    if let Some(group) = &family {
        args.conversation = Some(group.id.clone());
    }
    if args.conversation.is_some() && args.harnesses.is_none() && args.retention.is_none() {
        if let Some(group) = family
            && let Err(e) = crate::browser::edit(&[group])
        {
            fail(&e.to_string());
        }
        return;
    }
    let mut config = crate::profile::current();
    let changing = args.harnesses.is_some() || args.retention.is_some();
    if changing
        && let Err(e) = crate::profile::configure(
            &mut config,
            args.harnesses.as_deref(),
            args.retention.as_deref(),
            false,
            args.conversation.as_deref(),
        )
        .and_then(|_| {
            if let Some(group) = &family
                && let Some(names) = config
                    .get("conversation_profiles")
                    .and_then(|p| p.get(&group.id))
                    .cloned()
            {
                for copy in group.remote() {
                    config["conversation_profiles"][&copy.key()] = names.clone();
                }
            }
            crate::profile::save(&config)
        })
    {
        die(&e.to_string());
    }
    crate::profile::show(&config, args.json);
}
