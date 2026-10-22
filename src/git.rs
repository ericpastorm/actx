//! Git: identifying a repo (by its first commit) and finding its folder on this machine.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};

use crate::ctx::{ctx, jstr};
use crate::paths::{PathFixer, base_name, short, slashed, under_home};
use crate::state::State;
use crate::tr;
use crate::ui::{Row, ask, fzf, interactive, paint, warn};

const SCAN_DEPTH: usize = 5;
const SKIP_DIRS: [&str; 9] = [
    "node_modules",
    "build",
    "dist",
    "target",
    "vendor",
    "venv",
    "Android",
    "go",
    "snap",
];

#[derive(Clone, Debug)]
pub struct GitId {
    pub top: String,
    pub root: Option<String>,
    pub branch: Option<String>,
    pub head: Option<String>,
}

fn git(path: &str, args: &[&str]) -> Option<String> {
    let out = crate::ctx::command("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A repo's identity: its first commit is the same in every clone and worktree.
pub fn git_id(path: &str) -> Option<GitId> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<GitId>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(found) = cache.lock().unwrap().get(path) {
        return found.clone();
    }
    let found = (|| {
        if !Path::new(path).is_dir() {
            return None;
        }
        let top = crate::paths::native(&git(path, &["rev-parse", "--show-toplevel"])?);
        let root = git(path, &["rev-list", "--max-parents=0", "HEAD"])
            .and_then(|r| r.split_whitespace().min().map(String::from));
        Some(GitId {
            top,
            root,
            branch: git(path, &["rev-parse", "--abbrev-ref", "HEAD"]),
            head: git(path, &["rev-parse", "--short", "HEAD"]),
        })
    })();
    cache.lock().unwrap().insert(path.to_string(), found.clone());
    found
}

/// The repo's state, as stored with a conversation.
pub fn git_info(path: &str) -> Option<Value> {
    let id = git_id(path)?;
    let unpushed = git(path, &["rev-list", "--count", "@{u}..HEAD"])
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0);
    Some(json!({
        "top": id.top, "root": id.root, "branch": id.branch, "head": id.head,
        "remote": git(path, &["remote", "get-url", "origin"]),
        "dirty": git(path, &["status", "--porcelain"]).is_some_and(|s| !s.is_empty()),
        "unpushed": unpushed,
    }))
}

pub fn git_label(g: &Value) -> String {
    let mut label = format!("{} @ {}", jstr(g, "branch"), jstr(g, "head"));
    if g.get("dirty").and_then(Value::as_bool).unwrap_or(false) {
        label += &format!(" · {}", tr!("uncommitted changes"));
    }
    let unpushed = g.get("unpushed").and_then(Value::as_u64).unwrap_or(0);
    if unpushed > 0 {
        label += &format!(" · {}", tr!("{n} unpushed commit(s)", n = unpushed));
    }
    label
}

/// The repos on this machine: in your home (or ACTX_SCAN), and next to the folders it has
/// conversations in or was told to put them in (projects kept on another disk, say).
pub fn local_repos(st: &State) -> &'static Vec<GitId> {
    static REPOS: OnceLock<Vec<GitId>> = OnceLock::new();
    REPOS.get_or_init(|| {
        let _loading = crate::ui::loading(&tr!("Finding the project on this machine"));
        fn walk(d: &Path, depth: usize, found: &mut Vec<GitId>, seen: &mut std::collections::HashSet<PathBuf>) {
            let Ok(real) = crate::paths::canonical(d) else { return };
            if !seen.insert(real) {
                return;
            }
            let Ok(entries) = std::fs::read_dir(d) else { return };
            let entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
            if entries.iter().any(|e| e.file_name() == ".git") {
                // a folder, or a file in worktrees
                if let Some(id) = git_id(&d.to_string_lossy()) {
                    found.push(id);
                }
                return;
            }
            if depth == 0 {
                return;
            }
            for e in entries {
                let name = e.file_name().to_string_lossy().into_owned();
                let is_dir = e.path().is_dir();
                if is_dir && !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str()) {
                    walk(&e.path(), depth - 1, found, seen);
                }
            }
        }
        let mut found = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for root in &ctx().scan_roots {
            walk(root, SCAN_DEPTH, &mut found, &mut seen);
        }
        // where this machine has conversations, and where it was told to put them
        let mut used: Vec<String> = crate::harness::all_local().into_iter().map(|s| s.path).collect();
        used.extend(st.folders.values().cloned());
        used.sort();
        used.dedup();
        let mut near: Vec<PathBuf> = used
            .iter()
            .filter(|f| !f.is_empty() && !ctx().scan_roots.iter().any(|r| Path::new(f).starts_with(r)))
            .filter_map(|f| {
                let top = git_id(f).map_or_else(|| f.clone(), |g| g.top);
                Path::new(&top).parent().map(Path::to_path_buf)
            })
            .filter(|p| p.parent().is_some()) // not /
            .collect();
        near.sort();
        near.dedup();
        for d in near {
            walk(&d, 1, &mut found, &mut seen);
        }
        found.sort_by(|a, b| a.top.cmp(&b.top));
        found.dedup_by(|a, b| a.top == b.top);
        found
    })
}

/// Like Python's os.path.relpath for a path inside `base` ("." for base itself, ".." if outside).
/// Works on paths from any machine (`/home/me/shop` or `C:\Users\me\shop`) and gives it with `/`.
pub fn relpath(path: &str, base: &str) -> String {
    let (path, base) = (slashed(path), slashed(base));
    let base = base.trim_end_matches('/');
    let same = |a: &str, b: &str| {
        if cfg!(windows) {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    if same(&path, base) {
        return ".".to_string();
    }
    match path.get(..base.len() + 1) {
        Some(head) if same(head, &format!("{base}/")) => path[base.len() + 1..].to_string(),
        _ => "..".to_string(),
    }
}

fn join_sub(top: &str, sub: &str) -> String {
    if sub.is_empty() || sub == "." || sub.starts_with("..") {
        return top.to_string();
    }
    let mut p = PathBuf::from(top);
    p.extend(sub.split('/').filter(|c| !c.is_empty()));
    if p.is_dir() {
        p.to_string_lossy().into_owned()
    } else {
        top.to_string()
    }
}

fn best_repo<'a>(matches: &[&'a GitId], g: &Value) -> Option<&'a GitId> {
    if matches.len() == 1 {
        return Some(matches[0]);
    }
    let same_top = |m: &&GitId| m.top == jstr(g, "top");
    let same_branch = |m: &&GitId| m.branch.as_deref() == g.get("branch").and_then(Value::as_str);
    let same_name = |m: &&GitId| base_name(&m.top) == base_name(jstr(g, "top"));
    for test in [&same_top as &dyn Fn(&&GitId) -> bool, &same_branch, &same_name] {
        let hits: Vec<_> = matches.iter().filter(|m| test(m)).collect();
        if hits.len() == 1 {
            return Some(hits[0]);
        }
    }
    None
}

fn stored_git(meta: &Value) -> Option<(&Value, &str)> {
    let g = meta.get("git").filter(|g| g.is_object())?;
    Some((g, g.get("root").and_then(Value::as_str)?))
}

fn same_path(meta: &Value) -> String {
    match meta.get("rel").and_then(Value::as_str) {
        Some(rel) => under_home(rel),
        None => jstr(meta, "path").to_string(),
    }
}

fn cwd() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The folder on this machine where a stored conversation goes, or None if it isn't clear.
pub fn resolve_folder(meta: &Value, st: &State) -> Option<String> {
    let same = same_path(meta);
    if let Some((g, root)) = stored_git(meta) {
        let sub = relpath(jstr(meta, "path"), jstr(g, "top"));
        if let Some(remembered) = st.folders.get(&format!("git:{root}")).filter(|p| Path::new(p).is_dir()) {
            return Some(match git_id(remembered) {
                Some(here) if here.root.as_deref() == Some(root) => join_sub(&here.top, &sub),
                _ => remembered.clone(),
            });
        }
        if git_id(&same).is_some_and(|there| there.root.as_deref() == Some(root)) {
            return Some(same);
        }
        if let Some(here) = git_id(&cwd()).filter(|h| h.root.as_deref() == Some(root)) {
            return Some(join_sub(&here.top, &sub)); // you're inside the project (e.g. its terminal in your IDE)
        }
        let matches: Vec<&GitId> = local_repos(st)
            .iter()
            .filter(|r| r.root.as_deref() == Some(root))
            .collect();
        return best_repo(&matches, g).map(|best| join_sub(&best.top, &sub));
    }
    if jstr(meta, "path").is_empty() {
        return None;
    }
    if let Some(remembered) = st
        .folders
        .get(&format!("path:{}", jstr(meta, "path")))
        .filter(|p| Path::new(p).is_dir())
    {
        return Some(remembered.clone());
    }
    Path::new(&same).is_dir().then_some(same)
}

fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

fn pick_folder(origin: &str, git_root: Option<&str>, header: &str, st: &mut State) -> Option<String> {
    if !interactive() && ctx().pick.is_none() {
        return None;
    }
    let name = norm(base_name(origin));
    let likeness = |top: &str| {
        // 0 = same name, 1 = similar, 2 = unrelated
        let other = norm(base_name(top));
        if !name.is_empty() && other == name {
            0
        } else if !name.is_empty() && !other.is_empty() && (name.contains(&other) || other.contains(&name)) {
            1
        } else {
            2
        }
    };
    let cwd = crate::paths::canonical(Path::new(&cwd()))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| cwd());
    let mut options = vec![cwd.clone()];
    if let Some(root) = git_root {
        options.extend(
            local_repos(st)
                .iter()
                .filter(|r| r.root.as_deref() == Some(root))
                .map(|r| r.top.clone()),
        );
    }
    let mut others: Vec<String> = local_repos(st).iter().map(|r| r.top.clone()).collect();
    others.sort_by_key(|t| (likeness(t), t.clone()));
    options.extend(others);
    options.push(ctx().home_str());
    let mut seen = std::collections::HashSet::new();
    options.retain(|o| !o.is_empty() && Path::new(o).is_dir() && seen.insert(o.clone()));
    let mut rows: Vec<Row> = options
        .iter()
        .map(|p| Row {
            key: p.clone(),
            line: format!(
                "{}{}",
                if *p == cwd {
                    paint("32", &tr!("● here: "))
                } else {
                    "  ".into()
                },
                short(p)
            ),
            preview: String::new(),
        })
        .collect();
    rows.push(Row {
        key: "::other".into(),
        line: format!("  {}", tr!("✎ other folder (type the path)")),
        preview: String::new(),
    });
    let mut folder = fzf(&rows, header, false, false).into_iter().next()?;
    if folder == "::other" {
        let typed = crate::store::shellexpand(&ask(&tr!("Path:"), false));
        folder = crate::paths::canonical(Path::new(&typed))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(typed);
        if !Path::new(&folder).is_dir() {
            warn(&tr!("{path} doesn't exist", path = short(&folder)));
            return None;
        }
    }
    Some(folder)
}

pub fn ask_folder(meta: &Value, st: &mut State) -> Option<String> {
    let git = stored_git(meta);
    let origin = git.map(|(g, _)| jstr(g, "top")).unwrap_or_else(|| jstr(meta, "path"));
    let header = tr!(
        "Where does «{title}» go? On {host} it was in {path}",
        title = crate::ui::take_chars(jstr(meta, "title"), 60),
        host = jstr(meta, "host"),
        path = short(jstr(meta, "path")),
    );
    let folder = pick_folder(origin, git.map(|(_, root)| root), &header, st)?;
    let key = match git {
        Some((_, root)) => Some(format!("git:{root}")),
        None if !jstr(meta, "path").is_empty() => Some(format!("path:{}", jstr(meta, "path"))),
        None => None,
    };
    if let Some(key) = key {
        st.folders.insert(key, folder.clone());
    }
    if let Some((g, root)) = git
        && let Some(here) = git_id(&folder).filter(|h| h.root.as_deref() == Some(root))
    {
        return Some(join_sub(&here.top, &relpath(jstr(meta, "path"), jstr(g, "top"))));
    }
    Some(folder)
}

/// Ask where to place a converted local conversation whose original folder is missing or absent.
pub fn ask_local_folder(title: &str, path: &str, st: &mut State) -> Option<String> {
    let header = if path.is_empty() {
        tr!(
            "Where should «{title}» work? Pick a folder",
            title = crate::ui::take_chars(title, 60)
        )
    } else {
        tr!(
            "Where should «{title}» go? ({path} doesn't exist here)",
            title = crate::ui::take_chars(title, 60),
            path = short(path)
        )
    };
    pick_folder(if path.is_empty() { title } else { path }, None, &header, st)
}

/// Swaps the other machine's paths (its repo, or its folder, and its home) for this one's.
pub fn fixer_for(meta: &Value, folder: &str) -> Option<PathFixer> {
    if let Some(project) = meta.get("project_folder").filter(|p| p.is_object()) {
        return fixer_for(project, folder);
    }
    let home = ctx().home_str();
    match stored_git(meta).zip(git_id(folder)) {
        Some(((g, root), here)) if here.root.as_deref() == Some(root) => {
            PathFixer::new(&[(jstr(g, "top"), &here.top), (jstr(meta, "home"), &home)])
        }
        _ => PathFixer::new(&[(jstr(meta, "path"), folder), (jstr(meta, "home"), &home)]),
    }
}

pub fn check_git(meta: &Value, folder: &str) {
    let Some(there) = meta.get("git").filter(|g| g.is_object()) else {
        return;
    };
    let Some(here) = git_id(folder) else { return };
    if there.get("root").and_then(Value::as_str) != here.root.as_deref() {
        return;
    }
    let host = jstr(meta, "host");
    if there.get("dirty").and_then(Value::as_bool).unwrap_or(false) {
        warn(&tr!(
            "When it was pushed, {host} had uncommitted changes: they're not here.",
            host = host
        ));
    }
    let unpushed = there.get("unpushed").and_then(Value::as_u64).unwrap_or(0);
    if unpushed > 0 {
        warn(&tr!(
            "When it was pushed, {host} had {n} unpushed commit(s).",
            host = host,
            n = unpushed
        ));
    }
    let (b1, h1) = (jstr(there, "branch"), jstr(there, "head"));
    let (b2, h2) = (here.branch.unwrap_or_default(), here.head.unwrap_or_default());
    if (b1, h1) != (b2.as_str(), h2.as_str()) {
        warn(&tr!(
            "{repo}: {host} was on {b1} @ {h1}, you're on {b2} @ {h2}. Sync git before continuing.",
            repo = short(&here.top),
            host = host,
            b1 = b1,
            h1 = h1,
            b2 = b2,
            h2 = h2,
        ));
    }
}

pub fn warn_dirty(folders: &std::collections::BTreeSet<String>) {
    for folder in folders {
        if let Some(g) = git_info(folder) {
            let dirty = g.get("dirty").and_then(Value::as_bool).unwrap_or(false);
            if dirty || g.get("unpushed").and_then(Value::as_u64).unwrap_or(0) > 0 {
                warn(&tr!(
                    "{repo}: {state}. The conversation travels, the code doesn't: commit and push.",
                    repo = short(jstr(&g, "top")),
                    state = git_label(&g),
                ));
            }
        }
    }
}
