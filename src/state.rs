//! What this machine remembers between runs, and backups.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use crate::ctx::{Result, ctx};

#[derive(Default)]
pub struct State {
    /// key → (local weight, stored weight) at the last sync
    pub base: BTreeMap<String, (u64, u64)>,
    /// origin repo ("git:<first commit>") or path ("path:<path>") → folder picked on this machine
    pub folders: BTreeMap<String, String>,
}

fn number(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_f64().map(|f| f as u64))
}

pub fn load() -> State {
    let file = ctx().state_file();
    if !file.exists() {
        let legacy = ctx().state_dir.parent().map(|p| p.join("ctx").join("state.json")); // before the rename (ctx → actx)
        if let Some(legacy) = legacy.filter(|p| p.exists()) {
            let _ = std::fs::create_dir_all(&ctx().state_dir);
            let _ = std::fs::copy(legacy, &file);
        }
    }
    let raw: Value = std::fs::read_to_string(&file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let mut st = State::default();
    if let Some(base) = raw.get("base").and_then(Value::as_object) {
        for (key, pair) in base {
            if let Some([a, b]) = pair.as_array().map(Vec::as_slice)
                && let (Some(a), Some(b)) = (number(a), number(b))
            {
                st.base.insert(key.clone(), (a, b));
            }
        }
    }
    if let Some(folders) = raw.get("folders").and_then(Value::as_object) {
        for (key, folder) in folders {
            if let Some(folder) = folder.as_str() {
                st.folders.insert(key.clone(), folder.to_string());
            }
        }
    }
    st
}

pub fn save(st: &State) -> Result<()> {
    std::fs::create_dir_all(&ctx().state_dir)?;
    let base: serde_json::Map<String, Value> = st.base.iter().map(|(k, (a, b))| (k.clone(), json!([a, b]))).collect();
    let data = json!({"base": base, "folders": st.folders});
    let tmp = ctx().state_dir.join("state.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&data)?)?;
    std::fs::rename(tmp, ctx().state_file())?;
    Ok(())
}

fn backup_dir() -> PathBuf {
    let d = ctx()
        .backups()
        .join(chrono::Local::now().format("%Y%m%d-%H%M%S").to_string());
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn backup_file(src: &Path, name: &str) -> Result<()> {
    let dest = backup_dir().join(name);
    // reflink: on btrfs/xfs the copy is instant and takes no space until something changes
    let reflinked = Command::new("cp")
        .arg("--reflink=auto")
        .arg("-p")
        .arg(src)
        .arg(&dest)
        .status();
    if !reflinked.map(|s| s.success()).unwrap_or(false) {
        std::fs::copy(src, &dest)?;
    }
    Ok(())
}

pub fn backup_bytes(data: &[u8], name: &str) -> Result<()> {
    std::fs::write(backup_dir().join(name), data)?;
    Ok(())
}
