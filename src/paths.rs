//! Paths: showing them, Claude Code's folder names, and rewriting another machine's paths.

use std::path::Path;

use crate::ctx::ctx;

pub fn short(path: &str) -> String {
    let home = ctx().home_str();
    if path == home || path.starts_with(&format!("{home}/")) {
        format!("~{}", &path[home.len()..])
    } else {
        path.to_string()
    }
}

/// The path relative to $HOME ("." for $HOME itself), if it's inside it.
pub fn rel_home(path: &str) -> Option<String> {
    let rel = Path::new(path).strip_prefix(&ctx().home).ok()?;
    let rel = rel.to_string_lossy();
    Some(if rel.is_empty() {
        ".".to_string()
    } else {
        rel.into_owned()
    })
}

/// $HOME/rel, the way it was stored.
pub fn under_home(rel: &str) -> String {
    if rel == "." {
        ctx().home_str()
    } else {
        ctx().home.join(rel).to_string_lossy().into_owned()
    }
}

/// How Claude Code names a project's folder in ~/.claude/projects.
pub fn encode_claude(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Undo encode_claude by looking at the disk ('-' may come from '/', '-', '_', '.', ' '...).
pub fn resolve_encoded(name: &str) -> Option<String> {
    fn walk(base: &Path, rest: &str) -> Option<String> {
        if rest.is_empty() {
            return Some(base.to_string_lossy().into_owned());
        }
        let mut entries: Vec<String> = std::fs::read_dir(base)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        entries.sort();
        for e in entries {
            let enc = format!("-{}", encode_claude(&e));
            if (rest == enc || rest.starts_with(&format!("{enc}-")))
                && let Some(found) = walk(&base.join(&e), &rest[enc.len()..])
            {
                return Some(found);
            }
        }
        None
    }
    if name.starts_with('-') {
        walk(Path::new("/"), name)
    } else {
        None
    }
}

/// Swaps another machine's paths for this one's inside a conversation. A path only matches as a
/// whole name: /home/a/shop matches /home/a/shop and /home/a/shop/x, not /home/a/shop-old.
pub struct PathFixer {
    pairs: Vec<(Vec<u8>, Vec<u8>)>,
}

impl PathFixer {
    pub fn new(pairs: &[(&str, &str)]) -> Option<PathFixer> {
        let pairs: Vec<_> = pairs
            .iter()
            .filter(|(old, new)| !old.is_empty() && !new.is_empty() && old != new)
            .map(|(old, new)| (old.as_bytes().to_vec(), new.as_bytes().to_vec()))
            .collect();
        if pairs.is_empty() {
            None
        } else {
            Some(PathFixer { pairs })
        }
    }

    pub fn fix(&self, data: &[u8]) -> Vec<u8> {
        let mut data = data.to_vec();
        for (old, new) in &self.pairs {
            let finder = memchr::memmem::Finder::new(old);
            if finder.find(&data).is_none() {
                continue;
            }
            let mut out = Vec::with_capacity(data.len());
            let mut pos = 0;
            for at in finder.find_iter(&data) {
                if at < pos {
                    continue;
                }
                let next = data.get(at + old.len()).copied();
                let whole =
                    !matches!(next, Some(c) if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'-');
                if whole {
                    out.extend_from_slice(&data[pos..at]);
                    out.extend_from_slice(new);
                    pos = at + old.len();
                }
            }
            out.extend_from_slice(&data[pos..]);
            data = out;
        }
        data
    }
}

pub type Fix<'a> = Option<&'a PathFixer>;

pub fn apply(fix: Fix, data: &[u8]) -> Vec<u8> {
    match fix {
        Some(f) => f.fix(data),
        None => data.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_fixer_respects_name_boundaries() {
        let fix = PathFixer::new(&[("/home/a/shop", "/home/b/work/shop2")]).unwrap();
        let data = b"\"cwd\":\"/home/a/shop\" edited /home/a/shop/cart.ts not /home/a/shop-old or /home/a/shopping";
        assert_eq!(
            fix.fix(data),
            b"\"cwd\":\"/home/b/work/shop2\" edited /home/b/work/shop2/cart.ts not /home/a/shop-old or /home/a/shopping"
        );
        assert!(PathFixer::new(&[("/same", "/same")]).is_none());
    }

    #[test]
    fn encode_and_resolve_claude_folder_names() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Work").join("my-app_v2.0").join("src");
        std::fs::create_dir_all(&folder).unwrap();
        let folder = folder.to_string_lossy().into_owned();
        let name = encode_claude(&folder);
        assert!(!name.contains('/') && !name.contains('_') && !name.contains('.'));
        assert_eq!(resolve_encoded(&name), Some(folder));
    }
}
