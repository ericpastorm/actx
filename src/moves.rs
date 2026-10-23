//! Reorganize saved histories without republishing or editing native conversations.
use crate::{
    catalog::{self, Catalog},
    ctx::{Error, Result},
    project,
    session::Session,
    state::{self, State},
    store::{self, GENERAL},
    tr,
    ui::{self, Row},
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    time::SystemTime,
};
pub(crate) mod api;

pub fn command() -> Command {
    Command::new("mv")
        .about(tr!("Move saved conversations or rename an actx project"))
        .arg(Arg::new("destination").help(tr!(
            "Destination project; otherwise choose it after selecting conversations"
        )))
        .arg(
            Arg::new("general")
                .long("general")
                .action(ArgAction::SetTrue)
                .conflicts_with("destination")
                .help(tr!("Move the selected conversations to General")),
        )
        .arg(
            Arg::new("all")
                .long("all")
                .action(ArgAction::SetTrue)
                .help(tr!("Choose from all saved conversations, beyond the current project")),
        )
        .arg(
            Arg::new("project")
                .long("project")
                .num_args(2)
                .value_names(["OLD", "NEW"])
                .conflicts_with_all(["destination", "general", "all"])
                .help(tr!("Rename an actx project; keep its work folder")),
        )
}

fn named_project(root: &Path, name: &str) -> Result<String> {
    store::find_project(root, name)
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .ok_or_else(|| Error(tr!("Project {name} doesn't exist", name = name)))
}
fn validate_name(name: &str) -> Result<()> {
    if project::valid_name(name) {
        Ok(())
    } else {
        Err(Error(tr!("Invalid name: use letters, numbers, '-', '_' or '.'")))
    }
}
fn optional_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
type Stamp = (bool, u64, SystemTime);
fn stamp(path: &Path) -> Result<Stamp> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(Error(tr!("Cannot move a shared entry that is a symbolic link")));
    }
    Ok((meta.is_dir(), meta.len(), meta.modified()?))
}
type Snapshot = BTreeMap<PathBuf, (Stamp, Option<Vec<u8>>)>;
fn snapshot(path: &Path) -> Result<Snapshot> {
    fn walk(path: &Path, out: &mut Snapshot) -> Result<()> {
        crate::work::check()?;
        let value = stamp(path)?;
        let metadata = matches!(
            path.file_name().and_then(|s| s.to_str()),
            Some("meta.json" | ".project.json" | project::RENAMED_FROM)
        );
        let bytes = if !value.0 && metadata {
            Some(std::fs::read(path)?)
        } else {
            None
        };
        out.insert(path.to_path_buf(), (value, bytes));
        if value.0 {
            for entry in std::fs::read_dir(path)? {
                walk(&entry?.path(), out)?;
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(path, &mut out)?;
    Ok(out)
}
fn require_lock(root: &Path) -> Result<()> {
    if crate::lock::owned(root) {
        Ok(())
    } else {
        Err(Error(tr!(
            "The shared folder changed or its lock was lost; run actx mv again"
        )))
    }
}
fn changed() -> Error {
    Error(tr!("The shared folder changed while confirming; run actx mv again"))
}
fn identity(s: &Session, aliases: &HashMap<String, String>) -> String {
    catalog::root(
        aliases,
        s.meta()
            .get("conversation_id")
            .and_then(|v| v.as_str())
            .unwrap_or(&s.key()),
    )
}

/// A reviewed move can later be used by the desktop. Applying it requires the shared lock.
pub(crate) struct MovePlan {
    root: PathBuf,
    pub copies: Vec<Session>,
    pub destination: String,
    create: bool,
    before: Vec<Snapshot>,
    target: Option<(Stamp, Option<Vec<u8>>)>,
    identities: HashSet<String>,
}
impl MovePlan {
    pub fn prepare(root: &Path, mut copies: Vec<Session>, destination: &str, create: bool) -> Result<Self> {
        let _loading = ui::loading(&tr!("Preparing move"));
        copies.sort_by_key(Session::key);
        let destination = if destination == GENERAL {
            GENERAL.into()
        } else if create {
            validate_name(destination)?;
            if store::find_project(root, destination).is_some() || exists(&root.join(destination))? {
                return Err(Error(tr!("Project {name} already exists", name = destination)));
            }
            destination.into()
        } else {
            named_project(root, destination)?
        };
        let target_dir = root.join(&destination);
        let target = if exists(&target_dir)? {
            Some((stamp(&target_dir)?, optional_bytes(&target_dir.join(".project.json"))?))
        } else {
            None
        };
        let before = copies.iter().map(|s| snapshot(s.store_dir())).collect::<Result<_>>()?;
        let aliases = catalog::aliases();
        let identities = copies.iter().map(|s| identity(s, &aliases)).collect();
        Ok(Self {
            root: root.into(),
            copies,
            destination,
            create,
            before,
            target,
            identities,
        })
    }

    fn revision(&self) -> String {
        crate::engine::digest(
            format!(
                "{:?}",
                (&self.root, &self.destination, self.create, &self.before, &self.target)
            )
            .as_bytes(),
        )
    }

    pub fn apply(&self, st: &mut State, local_folders: &HashMap<String, String>) -> Result<usize> {
        require_lock(&self.root)?;
        let aliases = catalog::aliases();
        let current: Vec<_> = store::stored_sessions(&self.root)
            .into_iter()
            .filter(|s| self.identities.contains(&identity(s, &aliases)))
            .collect();
        if current.len() != self.copies.len()
            || current.iter().map(Session::key).collect::<HashSet<_>>()
                != self.copies.iter().map(Session::key).collect()
        {
            return Err(changed());
        }
        for (s, before) in self.copies.iter().zip(&self.before) {
            if snapshot(s.store_dir()).ok().as_ref() != Some(before) {
                return Err(changed());
            }
        }
        let target = self.root.join(&self.destination);
        let now = if exists(&target)? {
            Some((stamp(&target)?, optional_bytes(&target.join(".project.json"))?))
        } else {
            None
        };
        if now != self.target || (self.create && store::find_project(&self.root, &self.destination).is_some()) {
            return Err(changed());
        }
        let paths: Vec<_> = self
            .copies
            .iter()
            .filter(|s| s.project.as_deref() != Some(&self.destination))
            .map(|s| (s.store_dir(), target.join(s.harness).join(&s.id)))
            .collect();
        for (_, dest) in &paths {
            if exists(dest)? {
                return Err(Error(tr!(
                    "A shared copy already exists at {path}; nothing was moved",
                    path = dest.display()
                )));
            }
            if let Some(parent) = dest.parent().filter(|p| p.exists()) {
                stamp(parent)?;
            }
        }
        let mut moved: Vec<(&PathBuf, &PathBuf)> = Vec::new();
        crate::work::check()?;
        let result = (|| -> Result<()> {
            for (src, dest) in &paths {
                require_lock(&self.root)?;
                std::fs::create_dir_all(dest.parent().unwrap())?;
                std::fs::rename(src, dest)?;
                moved.push((src, dest));
            }
            Ok(())
        })();
        if let Err(error) = result {
            let mut failures = Vec::new();
            for (src, dest) in moved.into_iter().rev() {
                if let Err(e) = std::fs::rename(dest, src) {
                    failures.push(format!("{} → {}: {e}", dest.display(), src.display()));
                }
            }
            // Remove only newly created empty containers; saved histories are never deleted.
            for (_, dest) in &paths {
                let _ = std::fs::remove_dir(dest.parent().unwrap());
            }
            if self.target.is_none() {
                let _ = std::fs::remove_dir(&target);
            }
            if failures.is_empty() {
                return Err(error);
            }
            return Err(Error(tr!(
                "Move interrupted: {err}. Copies remain at: {paths}",
                err = error,
                paths = failures.join("; ")
            )));
        }
        if self.destination == GENERAL {
            for s in &self.copies {
                if let Some(folder) = local_folders.get(&s.key()).filter(|f| Path::new(f).is_dir()) {
                    st.folders.insert(format!("session:{}", s.key()), folder.clone());
                }
            }
        }
        crate::gui_index::invalidate();
        Ok(self.identities.len())
    }
}

pub(crate) struct RenamePlan {
    root: PathBuf,
    source: PathBuf,
    pub old: String,
    pub new: String,
    before: Snapshot,
}
impl RenamePlan {
    pub fn prepare(root: &Path, old: &str, new: &str) -> Result<Self> {
        let _loading = ui::loading(&tr!("Preparing move"));
        validate_name(new)?;
        let old = named_project(root, old)?;
        if store::find_project(root, new).is_some() || exists(&root.join(new))? {
            return Err(Error(tr!("Project {name} already exists", name = new)));
        }
        let source = root.join(&old);
        let before = snapshot(&source)?;
        Ok(Self {
            root: root.into(),
            source,
            old,
            new: new.into(),
            before,
        })
    }
    pub fn apply(&self, st: &mut State) -> Result<()> {
        require_lock(&self.root)?;
        if snapshot(&self.source).ok().as_ref() != Some(&self.before)
            || store::find_project(&self.root, &self.new).is_some()
            || exists(&self.root.join(&self.new))?
        {
            return Err(changed());
        }
        let file = self.source.join(project::RENAMED_FROM);
        let original = optional_bytes(&file)?;
        let mut names: Vec<String> = original
            .as_deref()
            .map(serde_json::from_slice)
            .transpose()?
            .unwrap_or_default();
        names.push(self.old.clone());
        require_lock(&self.root)?;
        let write = |bytes: &[u8]| -> Result<()> {
            use std::io::Write;
            let mut tmp = tempfile::NamedTempFile::new_in(&self.source)?;
            tmp.write_all(bytes)?;
            tmp.persist(&file).map_err(|e| Error(e.to_string()))?;
            Ok(())
        };
        write(&serde_json::to_vec(&names)?)?;
        if let Err(error) = std::fs::rename(&self.source, self.root.join(&self.new)) {
            match original {
                Some(bytes) => write(&bytes)?,
                None => std::fs::remove_file(&file)?,
            }
            return Err(error.into());
        }
        if let Some(folder) = st.folders.remove(&project::key(&self.old)) {
            st.folders.insert(project::key(&self.new), folder);
        }
        crate::gui_index::invalidate();
        Ok(())
    }
    fn revision(&self) -> String {
        crate::engine::digest(format!("{:?}", (&self.root, &self.old, &self.new, &self.before)).as_bytes())
    }
}

fn destination(root: &Path) -> Option<(String, bool)> {
    let mut rows: Vec<_> = store::project_dirs(root)
        .iter()
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy().into_owned();
            Some(Row {
                key: name.clone(),
                line: name,
                preview: String::new(),
            })
        })
        .collect();
    rows.sort_by_key(|row| row.line.to_lowercase());
    rows.push(Row {
        key: GENERAL.into(),
        line: store::collection_label(GENERAL),
        preview: String::new(),
    });
    rows.push(Row {
        key: "::new".into(),
        line: tr!("Create project"),
        preview: String::new(),
    });
    let key = ui::fzf(&rows, &tr!("Move to which project or General?"), false, false)
        .into_iter()
        .next()?;
    if key == "::new" {
        let name = ui::ask(&tr!("New project name:"), false);
        (!name.is_empty()).then_some((name, true))
    } else {
        Some((key, false))
    }
}

pub fn cli(m: &ArgMatches) {
    if let Err(e) = run(m) {
        ui::die(&e.to_string());
    }
}
fn run(m: &ArgMatches) -> Result<()> {
    let root = store::store_root(false).expect("shared folder");
    let st = state::load();
    if let Some(names) = m.get_many::<String>("project") {
        let names: Vec<_> = names.collect();
        let plan = RenamePlan::prepare(&root, names[0], names[1])?;
        if !ui::confirm(&tr!(
            "Rename actx project {old} → {new}? Work folders and histories are kept.",
            old = plan.old,
            new = plan.new
        )) {
            return Ok(());
        }
        let Some(_lock) = crate::commands::lock_store(&root) else {
            return Ok(());
        };
        let mut st = state::load();
        let _loading = ui::loading(&tr!("Renaming project"));
        plan.apply(&mut st)?;
        state::save(&mut st)?;
        ui::ok(&tr!("Project renamed: {old} → {new}", old = plan.old, new = plan.new));
        return Ok(());
    }
    let explicit = if m.get_flag("general") {
        Some(GENERAL.into())
    } else {
        m.get_one::<String>("destination")
            .map(|name| named_project(&root, name))
            .transpose()?
    };
    let here = std::env::current_dir()?;
    let scope = (!m.get_flag("all"))
        .then(|| project::current(&root, &st, &here.to_string_lossy()))
        .flatten();
    let catalog = Catalog::load();
    let rows: Vec<_> = catalog
        .groups
        .iter()
        .filter(|g| {
            g.copies
                .iter()
                .filter_map(|c| c.remote.as_ref())
                .any(|r| scope.as_ref().is_none_or(|p| r.project.as_ref() == Some(p)))
        })
        .map(|g| g.row())
        .collect();
    if rows.is_empty() {
        println!(
            "{}",
            tr!("No saved conversations in this view; use actx mv --all to choose from other projects")
        );
        return Ok(());
    }
    let header = match &scope {
        Some(name) => tr!(
            "Move conversations from {name} · --all shows other projects · TAB marks several",
            name = name
        ),
        None => tr!("Move saved conversations · all linked harness copies move together · TAB marks several"),
    };
    let keys = ui::fzf(&rows, &header, true, true);
    let chosen: Vec<_> = keys.iter().filter_map(|key| catalog.find(key)).collect();
    if chosen.is_empty() {
        return Ok(());
    }
    let Some((target, create)) = explicit.map(|name| (name, false)).or_else(|| destination(&root)) else {
        return Ok(());
    };
    let copies: Vec<_> = chosen
        .iter()
        .filter(|g| {
            g.copies
                .iter()
                .any(|c| c.remote.as_ref().is_some_and(|r| r.project.as_ref() != Some(&target)))
        })
        .flat_map(|g| g.remote())
        .collect();
    if copies.is_empty() {
        println!(
            "{}",
            tr!(
                "The selected conversations are already in {name}",
                name = store::collection_label(&target)
            )
        );
        return Ok(());
    }
    let local_folders = chosen
        .iter()
        .flat_map(|g| &g.copies)
        .filter_map(|c| c.local.as_ref())
        .map(|s| (s.key(), s.path.clone()))
        .collect();
    let plan = MovePlan::prepare(&root, copies, &target, create)?;
    let mut reviewed = HashSet::new();
    for s in &plan.copies {
        if reviewed.insert((s.title.clone(), s.project.clone())) {
            println!(
                "  {}: {} → {}",
                s.title,
                s.project_label(),
                store::collection_label(&target)
            );
        }
    }
    if !ui::confirm(&tr!(
        "Move {n} conversation(s) to {name}? Native histories and work folders are kept.",
        n = plan.identities.len(),
        name = store::collection_label(&target)
    )) {
        return Ok(());
    }
    let Some(_lock) = crate::commands::lock_store(&root) else {
        return Ok(());
    };
    let mut st = state::load();
    let _loading = ui::loading(&tr!("Moving saved conversations"));
    let count = plan.apply(&mut st, &local_folders)?;
    state::save(&mut st)?;
    ui::ok(&tr!(
        "Moved {n} conversation(s) to {name}",
        n = count,
        name = store::collection_label(&target)
    ));
    Ok(())
}
