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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resume: Option<Resume>,
}
/// Where to go on reading a native history that grew: just after a complete line, with the
/// fingerprint so far and two digests of every byte before it. Only if both still match is the
/// rest read alone; anything else (an edit, a truncation, another file) reads it all again.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
struct Resume {
    offset: u64,
    state: u64,
    raw: u64,
    crc: u32,
}
const FNV: u64 = 0xcbf29ce484222325;
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
    let mut hash = FNV;
    if harness == "opencode" {
        let value: Value = serde_json::from_reader(reader)?;
        mix(&mut hash, &serde_json::to_vec(value.get("messages").unwrap_or(&value))?);
        return Ok(format!("content-v1:{hash:016x}"));
    }
    for raw in reader.split(b'\n') {
        record(harness, &raw?, &mut hash)?;
    }
    Ok(format!("content-v1:{hash:016x}"))
}
/// One line of a line-based history, into its fingerprint.
fn record(harness: &str, raw: &[u8], hash: &mut u64) -> Result<()> {
    if raw.is_empty() {
        return Ok(());
    }
    let Ok(value) = serde_json::from_slice::<Value>(raw) else {
        return Err(crate::ctx::Error("Incomplete or invalid conversation record".into()));
    };
    let kind = value["type"].as_str().unwrap_or_default();
    let counts = match harness {
        "claude" => matches!(kind, "user" | "assistant"),
        "codex" => matches!(kind, "response_item" | "compacted"),
        _ => true,
    };
    if counts {
        mix(hash, &serde_json::to_vec(&value)?);
        mix(hash, b"\n");
    }
    Ok(())
}
/// `records` of a native history file, reading only what follows `from` when every byte before
/// it is unchanged. Same fingerprint as reading it all, and where the next read can go on from.
fn grown(harness: &str, path: &Path, from: Option<Resume>) -> Result<(String, Resume)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let mut at = Resume {
        offset: 0,
        state: FNV,
        raw: FNV,
        crc: 0,
    };
    if let Some(from) = from.filter(|f| f.offset <= size) {
        let (mut raw, mut crc, mut read) = (FNV, crc32fast::Hasher::new(), 0);
        let mut prefix = (&mut file).take(from.offset);
        let mut buf = vec![0; 1 << 20];
        loop {
            crate::work::check()?;
            let n = prefix.read(&mut buf)?;
            if n == 0 {
                break;
            }
            mix(&mut raw, &buf[..n]);
            crc.update(&buf[..n]);
            read += n as u64;
        }
        if read == from.offset && raw == from.raw && crc.finalize() == from.crc {
            at = from;
        } else {
            file.seek(SeekFrom::Start(0))?;
        }
    }
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let (mut state, mut crc) = (at.state, crc32fast::Hasher::new_with_initial(at.crc));
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        match line.strip_suffix(b"\n") {
            Some(body) => {
                record(harness, body, &mut state)?;
                mix(&mut at.raw, &line);
                crc.update(&line);
                at.offset += line.len() as u64;
                at.state = state;
            }
            // being written: counted as a full read counts it, but never resumed from
            None => record(harness, &line, &mut state)?,
        }
    }
    at.crc = crc.finalize();
    Ok((format!("content-v1:{state:016x}"), at))
}
/// A native history made of JSON lines (Claude Code, Codex), as opposed to a database.
fn lines_file(s: &Session) -> Option<&std::path::PathBuf> {
    if s.meta.is_some() || matches!(s.harness, "opencode" | "antigravity") {
        return None;
    }
    s.src.as_ref()
}
pub fn fresh(s: &Session) -> Result<String> {
    if s.harness == "antigravity" {
        let path = s
            .src
            .as_ref()
            .ok_or_else(|| crate::ctx::Error("Missing native conversation".into()))?;
        let mut hash = FNV;
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
        remember(
            key,
            Cached {
                stamp: before,
                hash: hash.clone(),
                resume: None,
            },
        );
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
fn remember(key: String, entry: Cached) {
    cache().lock().unwrap().insert(key.clone(), entry.clone());
    if crate::inspection::enabled() {
        if let Ok(value) = serde_json::to_value(&entry) {
            crate::discovery::keep("content-v1.json", key, value);
        }
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
    let known = cache().lock().unwrap().get(&s.key()).cloned();
    if let Some(entry) = known.as_ref().filter(|c| c.stamp == before) {
        return Ok(entry.hash.clone());
    }
    let (hash, resume) = match lines_file(s) {
        Some(path) => {
            let (hash, resume) = grown(s.harness, path, known.and_then(|c| c.resume))?;
            (hash, Some(resume))
        }
        None => (fresh(s)?, None),
    };
    if stamp(s) != before {
        // What was read before the change is still a valid place to go on from.
        if resume.is_some() {
            remember(
                s.key(),
                Cached {
                    stamp: String::new(),
                    hash: String::new(),
                    resume,
                },
            );
        }
        return Err(crate::ctx::Error("Conversation changed while reading".into()));
    }
    remember(
        s.key(),
        Cached {
            stamp: before,
            hash: hash.clone(),
            resume,
        },
    );
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
    #[test]
    fn resuming_a_grown_history_always_matches_reading_it_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        for harness in ["claude", "codex"] {
            let kinds = ["user", "assistant", "progress", "response_item", "compacted", "system"];
            let (mut content, mut pending, mut resume) = (Vec::<u8>::new(), Vec::<u8>::new(), None::<Resume>);
            let (mut read, mut resumed) = (Vec::new(), 0);
            for _ in 0..400 {
                match next(20) {
                    // an agent appends lines (finishing a half-written one), not always compact JSON
                    0..=12 => {
                        content.append(&mut pending);
                        for _ in 0..=next(3) {
                            let (kind, text) = (kinds[next(kinds.len())], "abcdefgh".repeat(1 + next(4)));
                            let gap = if next(3) == 0 { " " } else { "" };
                            let line = format!("{{\"type\":{gap}\"{kind}\",\"message\":{{\"content\":\"{text}\"}}}}\n");
                            content.extend_from_slice(line.as_bytes());
                        }
                    }
                    // half a line, still being written
                    13 | 14 if pending.is_empty() && content.last().is_none_or(|b| *b == b'\n') => {
                        let line = b"{\"type\":\"assistant\",\"message\":{\"content\":\"partial\"}}\n";
                        let cut = 1 + next(line.len() - 2);
                        content.extend_from_slice(&line[..cut]);
                        pending = line[cut..].to_vec();
                    }
                    // an equal-size edit of earlier text
                    15 | 16 => {
                        if let Some(i) = (0..content.len()).filter(|i| content[*i] == b'c').nth(next(64)) {
                            content[i] = b'C';
                        }
                    }
                    // cut back to an earlier line, or rarely mid-line (broken until cut again)
                    17 => {
                        let ends: Vec<_> = std::iter::once(0)
                            .chain((0..content.len()).filter(|i| content[*i] == b'\n').map(|i| i + 1))
                            .collect();
                        content.truncate(ends[next(ends.len())]);
                        pending.clear();
                    }
                    18 if next(10) == 0 => {
                        content.truncate(next(content.len() + 1));
                        pending.clear();
                    }
                    _ if pending.is_empty() => content.push(b'\n'),
                    _ => {}
                }
                std::fs::write(&path, &content).unwrap();
                let offset = resume.map_or(0, |r| r.offset as usize);
                if offset > 0 && content.get(..offset) == read.get(..offset) {
                    resumed += 1;
                }
                let expected = records(harness, content.as_slice()).map_err(|e| e.to_string());
                let got = grown(harness, &path, resume).map_err(|e| e.to_string());
                assert_eq!(got.as_ref().map(|g| &g.0), expected.as_ref(), "{harness}");
                if let Ok((_, next_resume)) = got {
                    assert!(
                        content[..next_resume.offset as usize]
                            .last()
                            .is_none_or(|b| *b == b'\n')
                    );
                    resume = Some(next_resume);
                    read = content.clone();
                }
            }
            assert!(
                resumed > 100,
                "{harness}: only {resumed} steps went on from a resume point"
            );
        }
    }
    #[test]
    fn a_grown_history_reads_only_what_was_added() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let line = |text: &str| format!("{{\"type\":\"user\",\"message\":{{\"content\":\"{text}\"}}}}\n");
        std::fs::write(&path, line("first")).unwrap();
        let (_, mut resume) = grown("claude", &path, None).unwrap();
        std::fs::write(&path, line("first") + &line("second")).unwrap();
        // A wrong fingerprint at the resume point shows up only if that point is trusted.
        resume.state ^= 1;
        let trusted = grown("claude", &path, Some(resume)).unwrap().0;
        assert_ne!(
            trusted,
            records("claude", std::fs::read(&path).unwrap().as_slice()).unwrap()
        );
        // Once a byte before it changed, everything is read again.
        std::fs::write(&path, line("First") + &line("second")).unwrap();
        let reread = grown("claude", &path, Some(resume)).unwrap().0;
        assert_eq!(
            reread,
            records("claude", std::fs::read(&path).unwrap().as_slice()).unwrap()
        );
    }
}
