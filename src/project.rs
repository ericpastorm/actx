//! A named actx project has a work folder and a Git identity, independent of any session's cwd.
use crate::{
    ctx::{Error, Result, ctx},
    git, paths,
    state::State,
    store,
};
use serde_json::{Value, json};
use std::path::Path;

pub fn key(name: &str) -> String {
    format!("project:{}", name.to_lowercase())
}

pub fn metadata(root: &Path, name: &str) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(store::find_project(root, name)?.join(".project.json")).ok()?).ok()
}

/// Called under the store lock. Existing projects retain their portable identity.
pub fn bind(root: &Path, name: &str, folder: &str, st: &mut State) -> Result<()> {
    let dir = store::find_project(root, name).ok_or_else(|| Error("Missing project".into()))?;
    if let Some(meta) = metadata(root, name) {
        let expected = meta["git"]["root"].as_str();
        if let Some(expected) = expected
            && git::git_id(folder).and_then(|g| g.root).as_deref() != Some(expected)
        {
            return Err(Error(crate::tr!(
                "This project belongs to another repository; run this command in its work folder"
            )));
        }
    } else {
        let meta = json!({"v":1,"path":folder,"rel":paths::rel_home(folder),"home":ctx().home_str(),"host":ctx().host,"git":git::git_info(folder)});
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
        serde_json::to_writer_pretty(tmp.as_file_mut(), &meta)?;
        tmp.persist(dir.join(".project.json"))
            .map_err(|e| Error(e.to_string()))?;
    }
    st.folders.insert(key(name), folder.to_string());
    Ok(())
}

pub fn folder(root: &Path, name: &str, st: &State) -> Option<String> {
    if let Some(folder) = st.folders.get(&key(name)).filter(|p| Path::new(p).is_dir()) {
        return Some(folder.clone());
    }
    let meta = metadata(root, name)?;
    if !meta["git"].is_object() && meta["path"] == meta["home"] {
        return None;
    }
    git::resolve_folder(&meta, st)
}

pub fn contains(folder: &str, path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let canonical = |p: &str| paths::canonical(Path::new(p)).unwrap_or_else(|_| p.into());
    canonical(path).starts_with(canonical(folder))
}

/// Discovery may suggest a session started outside the repo when it mentions its real path.
/// The picker still requires an explicit selection; a title is never enough to link histories.
pub fn candidate(folder: &str, session: &crate::session::Session) -> bool {
    if contains(folder, &session.path) {
        return true;
    }
    if let (Some(project), Some(session)) = (git::git_id(folder), git::git_id(&session.path))
        && project.root.is_some()
        && project.root == session.root
    {
        return true;
    }
    let source = session.src.as_deref().unwrap_or(&crate::ctx::ctx().opencode_db);
    crate::discovery::memo(
        &format!("project-match:{}:{folder}", session.key()),
        source,
        &Value::Null,
        || Ok(candidate_mentions(folder, session)),
    )
    .unwrap_or(false)
}
fn candidate_mentions(folder: &str, session: &crate::session::Session) -> bool {
    let mentions = |text: &[u8]| {
        [folder.to_string(), paths::slashed(folder), folder.replace('\\', "\\\\")]
            .iter()
            .any(|path| {
                memchr::memmem::find_iter(text, path.as_bytes()).any(|start| {
                    text.get(start + path.len())
                        .is_none_or(|c| c.is_ascii_whitespace() || b"/\\\"')]}".contains(c))
                })
            })
    };
    match &session.src {
        Some(path) if session.harness != "antigravity" => {
            crate::claude::chunks(path).is_ok_and(|(head, tail)| mentions(&head) || mentions(&tail))
        }
        _ => crate::harness::get(session.harness)
            .preview(session)
            .iter()
            .any(|(_, text)| mentions(text.as_bytes())),
    }
}

pub fn current(root: &Path, st: &State, cwd: &str) -> Option<String> {
    let here = git::git_id(cwd);
    let mut matches: Vec<_> = store::project_dirs(root)
        .into_iter()
        .filter_map(|dir| {
            let name = dir.file_name()?.to_string_lossy().into_owned();
            // Only consult bindings and Git metadata here, without scanning other repositories.
            if let Some(folder) = st.folders.get(&key(&name)).filter(|folder| contains(folder, cwd)) {
                return Some((folder.len(), name));
            }
            let here = here.as_ref()?;
            let meta = metadata(root, &name)?;
            (here.root.is_some() && meta["git"]["root"].as_str() == here.root.as_deref())
                .then_some((here.top.len(), name))
        })
        .collect();
    matches.sort_by(|a, b| b.cmp(a));
    match matches.as_slice() {
        [(n, name), rest @ ..] if rest.first().is_none_or(|(other, _)| other < n) => Some(name.clone()),
        _ => None,
    }
}

/// Additional project context is used for discovery/placement, never to rewrite unrelated paths.
pub fn placement(root: &Path, r: &crate::session::Session, st: &mut State, prompt: bool) -> Option<String> {
    let name = r.project.as_deref()?;
    let meta = metadata(root, name)?;
    let resolved = folder(root, name, st).or_else(|| {
        if !prompt {
            return None;
        }
        let mut meta = meta.clone();
        meta["title"] = json!(name);
        git::ask_folder(&meta, st)
    })?;
    st.folders.insert(key(name), resolved.clone());
    Some(resolved)
}

pub fn has_binding(root: &Path, name: &str) -> bool {
    metadata(root, name).is_some()
}
