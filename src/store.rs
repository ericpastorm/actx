//! The shared folder: <project>/<tool>/<id>/{meta.json, preview.txt, the conversation, compressed}.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::ctx::{Result, ctx, jstr, now};
use crate::session::Session;
use crate::state::State;
use crate::tr;
use crate::ui::{bold, die, warn};

pub const COMP: &str = "zst";

/// A stored file, decompressed (zstd, or gzip from actx versions running on Python < 3.14).
pub fn open_read(path: &Path) -> Result<Box<dyn BufRead>> {
    let file = File::open(path)?;
    Ok(match path.extension().and_then(|e| e.to_str()) {
        Some("zst") => Box::new(BufReader::new(zstd::Decoder::new(file)?)),
        Some("gz") => Box::new(BufReader::new(flate2::read::MultiGzDecoder::new(file))),
        _ => Box::new(BufReader::new(file)),
    })
}

pub fn read_all(path: &Path) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    open_read(path)?.read_to_end(&mut data)?;
    Ok(data)
}

/// Compress `src` into `dest`; returns how many bytes were read.
pub fn write_compressed(dest: &Path, mut src: impl Read) -> Result<u64> {
    let mut enc = zstd::Encoder::new(File::create(dest)?, 3)?;
    let n = io::copy(&mut src, &mut enc)?;
    enc.finish()?;
    Ok(n)
}

pub fn tar_dir(dest: &Path, dir: &Path, name: &str) -> Result<()> {
    let mut tar = tar::Builder::new(zstd::Encoder::new(File::create(dest)?, 3)?);
    tar.follow_symlinks(false);
    tar.append_dir_all(name, dir)?;
    tar.into_inner()?.finish()?;
    Ok(())
}

pub fn untar(archive: &Path, dest: &Path) -> Result<()> {
    tar::Archive::new(open_read(archive)?).unpack(dest)?;
    Ok(())
}

/// Each line of a stored file, decompressed and with this machine's paths.
pub fn stored_lines(path: &Path, fix: crate::paths::Fix) -> Result<Vec<Vec<u8>>> {
    let mut lines = Vec::new();
    for line in open_read(path)?.split(b'\n') {
        lines.push(crate::paths::apply(fix, &line?));
    }
    Ok(lines)
}

pub fn store_root(optional: bool) -> Option<PathBuf> {
    let setting = std::env::var("ACTX_ROOT")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| ctx().config.get("root").and_then(Value::as_str).map(String::from));
    let Some(setting) = setting else {
        if optional {
            return None;
        }
        die(&tr!(
            "No shared folder configured yet. Run: {cmd}",
            cmd = bold("actx init")
        ));
    };
    let root = PathBuf::from(shellexpand(&setting));
    if let Err(e) = std::fs::create_dir(&root)
        && e.kind() != io::ErrorKind::AlreadyExists
    {
        if optional {
            warn(&tr!(
                "Can't reach the shared folder ({err}): using only what's on this machine",
                err = e
            ));
            return None;
        }
        die(&format!(
            "{}\n  {}",
            tr!(
                "Can't reach the shared folder ({root}): {err}.",
                root = root.display(),
                err = e
            ),
            tr!("If it's a network share, check that it's mounted (away from home you may need a VPN).")
        ));
    }
    Some(root)
}

pub fn shellexpand(path: &str) -> String {
    match path.strip_prefix("~") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{}{rest}", ctx().home_str()),
        _ => path.to_string(),
    }
}

fn names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

pub fn project_dirs(root: &Path) -> Vec<PathBuf> {
    names(root)
        .into_iter()
        .filter(|n| !n.starts_with('.') && !n.starts_with('_'))
        .map(|n| root.join(n))
        .filter(|p| p.is_dir())
        .collect()
}

pub fn find_project(root: &Path, name: &str) -> Option<PathBuf> {
    project_dirs(root)
        .into_iter()
        .find(|d| d.file_name().map(|n| n.to_string_lossy().to_lowercase()) == Some(name.to_lowercase()))
}

pub fn stored_sessions(root: &Path) -> Vec<Session> {
    let mut out = Vec::new();
    for project in project_dirs(root) {
        let pname = project.file_name().unwrap_or_default().to_string_lossy().into_owned();
        for tool in names(&project).into_iter().filter(|n| !n.starts_with('.')) {
            for id in names(&project.join(&tool)).into_iter().filter(|n| !n.starts_with('.')) {
                let dir = project.join(&tool).join(&id);
                let Some(meta) = std::fs::read_to_string(dir.join("meta.json"))
                    .ok()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                else {
                    continue; // half-written uploads
                };
                let Some(harness) = crate::harness::known(jstr(&meta, "harness")) else {
                    continue; // stored by a newer actx that knows more tools
                };
                let path = match meta.get("rel").and_then(Value::as_str) {
                    Some(rel) => crate::paths::under_home(rel),
                    None => jstr(&meta, "path").to_string(),
                };
                let mut s = Session::local(
                    harness,
                    jstr(&meta, "id").to_string(),
                    jstr(&meta, "title").to_string(),
                    path,
                    meta.get("updated").and_then(Value::as_f64).unwrap_or(0.0),
                    meta.get("weight").and_then(Value::as_u64).unwrap_or(0),
                );
                s.meta = Some(meta);
                s.store_dir = Some(dir);
                s.project = Some(pname.clone());
                out.push(s);
            }
        }
    }
    out
}

/// What a tool's push leaves in the upload folder.
pub struct Pushed {
    pub files: Value,
    pub updated: f64,
    pub weight: u64,
    pub preview: String,
}

/// Store `s` in `project`. If it was in another project, move it and return that project's name.
pub fn push_one(
    root: &Path,
    project: &str,
    s: &Session,
    git: Option<Value>,
    st: &mut State,
    remote: &HashMap<String, Session>,
) -> Result<Option<String>> {
    let parent = root.join(project).join(s.harness);
    let dest = parent.join(&s.id);
    let pid = std::process::id();
    let tmp = parent.join(format!(".tmp-{}-{pid}", s.id));
    let old = parent.join(format!(".old-{}-{pid}", s.id));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let result = (|| -> Result<u64> {
        let pushed = crate::harness::get(s.harness).push(s, &tmp)?;
        let meta = json!({
            "v": 2, "harness": s.harness, "id": s.id, "title": s.title,
            "path": s.path, "rel": crate::paths::rel_home(&s.path), "home": ctx().home_str(),
            "host": ctx().host, "pushed_at": now(), "updated": pushed.updated, "weight": pushed.weight,
            "files": pushed.files, "git": git,
        });
        std::fs::write(tmp.join("meta.json"), serde_json::to_string_pretty(&meta)?)?;
        std::fs::write(tmp.join("preview.txt"), &pushed.preview)?;
        if dest.exists() {
            std::fs::rename(&dest, &old)?;
        }
        std::fs::rename(&tmp, &dest)?;
        let _ = std::fs::remove_dir_all(&old);
        Ok(pushed.weight)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    let weight = result?;
    st.base.insert(
        s.key(),
        (if s.harness == "opencode" { s.weight } else { weight }, weight),
    );
    if let Some(prev) = remote.get(&s.key())
        && prev.project.as_deref() != Some(project)
    {
        remove_stored(prev)?;
        return Ok(prev.project.clone());
    }
    Ok(None)
}

pub fn remove_stored(r: &Session) -> Result<()> {
    std::fs::remove_dir_all(r.store_dir())?;
    if let Some(parent) = r.store_dir().parent() {
        let _ = std::fs::remove_dir(parent); // the tool's folder, if it's now empty
    }
    Ok(())
}

/// How this copy of actx was installed: a "dev" build, a system "package", or a "user" file.
pub fn install_kind() -> &'static str {
    let me = &ctx().self_path;
    let dev = me.to_string_lossy().contains("/target/")
        && me
            .ancestors()
            .any(|d| d.join("Cargo.toml").exists() && d.join(".git").exists());
    if dev {
        return "dev";
    }
    let writable = me.parent().map(|d| tempfile::tempfile_in(d).is_ok()).unwrap_or(false);
    if me.starts_with("/usr") || me.starts_with("/opt") || !writable {
        "package"
    } else {
        "user"
    }
}

/// Where machines without a package find actx in the shared folder (binaries differ per platform).
pub fn shared_binary(root: &Path) -> PathBuf {
    root.join("_bin")
        .join(format!("actx-{}-{}", std::env::consts::ARCH, std::env::consts::OS))
}

fn mtime(p: &Path) -> f64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn modified(p: &Path) -> f64 {
    mtime(p)
}

/// Leave a copy of actx in the shared folder, to install it on machines without a package.
pub fn sync_self(root: &Path) {
    let kind = install_kind();
    if kind == "package" {
        return;
    }
    let shared = shared_binary(root);
    let me = &ctx().self_path;
    let same = std::fs::read(&shared)
        .ok()
        .zip(std::fs::read(me).ok())
        .map(|(a, b)| a == b);
    if same == Some(true) {
        return;
    }
    if shared.exists() && mtime(&shared) > mtime(me) + 2.0 {
        if kind == "user" {
            warn(&tr!(
                "There's a newer actx in the shared folder: {cmd}",
                cmd = bold("actx update")
            ));
        }
        return;
    }
    let _ = std::fs::create_dir_all(shared.parent().unwrap_or(root));
    let tmp = shared.with_extension("tmp");
    if std::fs::copy(me, &tmp).is_ok() {
        let _ = File::open(me)
            .and_then(|f| f.metadata())
            .and_then(|m| File::options().write(true).open(&tmp)?.set_modified(m.modified()?));
        let _ = std::fs::rename(&tmp, &shared);
    }
}
