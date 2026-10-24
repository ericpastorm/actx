//! Versioned content fingerprints. File stamps only cache reads; writes rehash their inputs.
use crate::{
    ctx::{Result, ctx},
    session::Session,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
    path::Path,
    sync::{Mutex, OnceLock},
};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    pub local: String,
    pub remote: String,
    pub stamp: String,
    pub weights: (u64, u64),
}
#[derive(Clone, Serialize, Deserialize)]
struct Cached {
    stamp: String,
    hash: String,
}
fn cache() -> &'static Mutex<BTreeMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<BTreeMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(crate::inspection::optional_json(&ctx().cache_dir.join("content-v1.json")).unwrap_or_default())
    })
}
pub fn stamp(s: &Session) -> String {
    let paths = if let Some(path) = &s.src {
        vec![path.clone()]
    } else if s.harness == "opencode" {
        vec![ctx().opencode_db.clone()]
    } else {
        Vec::new()
    };
    let mut out = format!("{}:{}:{}", s.key(), s.path, s.weight);
    for path in paths {
        for suffix in ["", "-wal", "-journal"] {
            let path = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
            out.push_str(&file_stamp(&path));
        }
    }
    out
}
fn file_stamp(path: &Path) -> String {
    let Ok(m) = path.metadata() else {
        return ":missing".into();
    };
    let stamp = format!(":{}:{}:{:?}", path.display(), m.len(), m.modified());
    #[cfg(unix)]
    let stamp = {
        use std::os::unix::fs::MetadataExt;
        format!("{stamp}:{}:{}", m.ctime(), m.ctime_nsec())
    };
    stamp
}
fn mix(hash: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *hash = (*hash ^ u64::from(*b)).wrapping_mul(0x100000001b3);
    }
}
pub fn records(harness: &str, reader: impl BufRead) -> Result<String> {
    let mut hash = 0xcbf29ce484222325;
    if harness == "opencode" {
        let value: Value = serde_json::from_reader(reader)?;
        mix(&mut hash, &serde_json::to_vec(value.get("messages").unwrap_or(&value))?);
        return Ok(format!("content-v1:{hash:016x}"));
    }
    for raw in reader.split(b'\n') {
        let raw = raw?;
        if raw.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&raw) else {
            return Err(crate::ctx::Error("Incomplete or invalid conversation record".into()));
        };
        let kind = value["type"].as_str().unwrap_or_default();
        let counts = match harness {
            "claude" => matches!(kind, "user" | "assistant"),
            "codex" => matches!(kind, "response_item" | "compacted"),
            _ => true,
        };
        if counts {
            mix(&mut hash, &serde_json::to_vec(&value)?);
            mix(&mut hash, b"\n");
        }
    }
    Ok(format!("content-v1:{hash:016x}"))
}
pub fn fresh(s: &Session) -> Result<String> {
    if s.harness == "antigravity" {
        let path = s
            .src
            .as_ref()
            .ok_or_else(|| crate::ctx::Error("Missing native conversation".into()))?;
        let mut hash = 0xcbf29ce484222325;
        use std::io::Read;
        for path in [
            path.clone(),
            std::path::PathBuf::from(format!("{}-wal", path.display())),
        ] {
            if !path.exists() {
                continue;
            }
            let mut file = std::fs::File::open(path)?;
            let mut bytes = [0; 65536];
            loop {
                let n = file.read(&mut bytes)?;
                if n == 0 {
                    break;
                }
                mix(&mut hash, &bytes[..n]);
            }
        }
        return Ok(format!("content-v1:{hash:016x}"));
    }
    if s.meta.is_some() {
        if let Some(hash) = s.meta()["content_hash"].as_str() {
            return Ok(hash.into());
        }
        // Legacy publications lack content_hash. Their files are never edited in place (each
        // publication swaps in new ones), so a file stamp safely spares re-downloading them.
        let file = s.stored_file(None);
        let key = format!("stored:{}", file.display());
        let before = format!("{}{}", file_stamp(&file), file_stamp(&s.store_dir().join("meta.json")));
        if let Some(hash) = cached(&key, &before) {
            return Ok(hash);
        }
        let hash = records(s.harness, crate::store::open_read(&file)?)?;
        remember(key, before, &hash);
        return Ok(hash);
    }
    if s.harness == "opencode" {
        let value = crate::opencode::read_export(s)?;
        return records("opencode", serde_json::to_vec(&value)?.as_slice());
    }
    let path = s
        .src
        .as_ref()
        .ok_or_else(|| crate::ctx::Error("Missing native conversation".into()))?;
    records(s.harness, BufReader::new(std::fs::File::open(path)?))
}
fn cached(key: &str, stamp: &str) -> Option<String> {
    cache()
        .lock()
        .unwrap()
        .get(key)
        .filter(|c| c.stamp == stamp)
        .map(|c| c.hash.clone())
}
/// Keep a fingerprint for this process, and on disk unless inspecting (previews write nothing).
fn remember(key: String, stamp: String, hash: &str) {
    cache().lock().unwrap().insert(
        key,
        Cached {
            stamp,
            hash: hash.into(),
        },
    );
    if crate::inspection::enabled() {
        return;
    }
    let _ = (|| -> Result<()> {
        std::fs::create_dir_all(&ctx().cache_dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(&ctx().cache_dir)?;
        serde_json::to_writer(tmp.as_file_mut(), &*cache().lock().unwrap())?;
        tmp.persist(ctx().cache_dir.join("content-v1.json"))
            .map_err(|e| crate::ctx::Error(e.to_string()))?;
        Ok(())
    })();
}
pub fn local(s: &Session) -> Result<String> {
    let before = stamp(s);
    if let Some(hash) = cached(&s.key(), &before) {
        return Ok(hash);
    }
    let hash = fresh(s)?;
    if stamp(s) != before {
        return Err(crate::ctx::Error("Conversation changed while reading".into()));
    }
    remember(s.key(), before, &hash);
    Ok(hash)
}
/// A copy's fingerprint, from the cache when its files haven't changed since it was taken.
pub fn current(s: &Session) -> Result<String> {
    if s.meta.is_some() {
        return fresh(s);
    }
    local(s).or_else(|_| fresh(s))
}
pub fn capture(local: &Session, remote: &Session, weights: (u64, u64)) -> Result<Baseline> {
    Ok(Baseline {
        local: fresh(local)?,
        remote: fresh(remote)?,
        stamp: stamp(local),
        weights,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fingerprints_detect_equal_size_edits_and_ignore_auxiliary_records() {
        let first = b"{\"type\":\"assistant\",\"message\":{\"content\":\"hello\"}}\n";
        let edited = b"{\"type\":\"assistant\",\"message\":{\"content\":\"world\"}}\n";
        assert_eq!(first.len(), edited.len());
        assert_ne!(
            records("claude", &first[..]).unwrap(),
            records("claude", &edited[..]).unwrap()
        );
        let mut auxiliary = first.to_vec();
        auxiliary.extend_from_slice(b"{\"type\":\"progress\",\"bytes\":123}\n");
        assert_eq!(
            records("claude", &first[..]).unwrap(),
            records("claude", auxiliary.as_slice()).unwrap()
        );
        assert!(records("claude", b"{\"type\":\"assistant\"".as_slice()).is_err());
    }
}
