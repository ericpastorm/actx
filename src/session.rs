//! A conversation, on this machine or in the shared folder, and what to do with it.

use std::path::PathBuf;

use serde_json::Value;

use crate::tr;
use crate::ui::{human, pad, paint};

#[derive(Clone, Debug)]
pub struct Session {
    pub harness: &'static str,
    pub id: String,
    pub title: String,
    /// folder the conversation was started in
    pub path: String,
    /// last activity (epoch seconds)
    pub updated: f64,
    /// to tell whether it changed: bytes (claude, codex) or number of messages (opencode)
    pub weight: u64,
    /// Claude Code: the file's size. Lines are only ever added to the end, so a weight saved at the
    /// last sync between `weight` and `size` means nothing but lines that don't count came since
    /// (and weights saved by older versions, which counted more, still match)
    pub size: Option<u64>,
    /// file-based tools: the conversation file
    pub src: Option<PathBuf>,
    /// stored ones: their meta.json, folder and project
    pub meta: Option<Value>,
    pub store_dir: Option<PathBuf>,
    pub project: Option<String>,
}

impl Session {
    pub fn local(harness: &'static str, id: String, title: String, path: String, updated: f64, weight: u64) -> Session {
        Session {
            harness,
            id,
            title,
            path,
            updated,
            weight,
            size: None,
            src: None,
            meta: None,
            store_dir: None,
            project: None,
        }
    }

    /// Is this (local) conversation still the one that weighed `weight`?
    pub fn same(&self, weight: u64) -> bool {
        match self.size {
            Some(size) => self.weight <= weight && weight <= size,
            None => self.weight == weight,
        }
    }

    pub fn key(&self) -> String {
        format!("{}:{}", self.harness, self.id)
    }

    pub fn is_general(&self) -> bool {
        self.project.as_deref() == Some(crate::store::GENERAL)
    }

    pub fn project_label(&self) -> String {
        crate::store::collection_label(self.project.as_deref().unwrap_or_default())
    }

    pub fn size_label(&self) -> String {
        if self.harness == "opencode" {
            format!("{} msgs", self.weight)
        } else {
            human(self.weight)
        }
    }

    pub fn meta(&self) -> &Value {
        self.meta.as_ref().expect("a stored session")
    }

    pub fn store_dir(&self) -> &PathBuf {
        self.store_dir.as_ref().expect("a stored session")
    }

    /// A file inside its folder in the shared folder (by default, the conversation itself).
    pub fn stored_file(&self, name: Option<&str>) -> PathBuf {
        let name = name.unwrap_or_else(|| crate::ctx::jstr(&self.meta()["files"], "session"));
        self.store_dir().join(name)
    }
}

/// What to do with a conversation. base = (local weight, stored weight) at the last sync.
pub fn status(local: Option<&Session>, remote: Option<&Session>, base: Option<(u64, u64)>) -> &'static str {
    let (Some(local), Some(remote)) = (local, remote) else {
        return if remote.is_none() { "local" } else { "new" };
    };
    if let Some((was_here, was_there)) = base {
        let (changed_here, changed_there) = (!local.same(was_here), remote.weight != was_there);
        return match (changed_here, changed_there) {
            (true, true) => "conflict",
            (true, false) => "push",
            (false, true) => "pull",
            (false, false) => "synced",
        };
    }
    if local.same(remote.weight) {
        "synced"
    } else if local.weight > remote.weight {
        "push"
    } else {
        "pull"
    }
}

pub fn badge(st: &str) -> String {
    let (text, code) = match st {
        "new" => (tr!("↓ new"), "36"),
        "synced" => (tr!("✓ synced"), "32"),
        "push" => (tr!("↑ push"), "33"),
        "pull" => (tr!("↓ pull"), "33"),
        "conflict" => (tr!("⚠ conflict"), "31"),
        _ => (String::new(), "0"),
    };
    paint(code, &pad(&text, 12))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(weight: u64) -> Session {
        Session::local("claude", "x".into(), "t".into(), "/p".into(), 0.0, weight)
    }

    #[test]
    fn status_uses_the_last_sync_as_base() {
        assert_eq!(status(None, Some(&s(100)), None), "new");
        assert_eq!(status(Some(&s(100)), None, None), "local");
        assert_eq!(status(Some(&s(100)), Some(&s(100)), Some((100, 100))), "synced");
        assert_eq!(status(Some(&s(150)), Some(&s(100)), Some((100, 100))), "push");
        assert_eq!(status(Some(&s(100)), Some(&s(150)), Some((100, 100))), "pull");
        assert_eq!(status(Some(&s(120)), Some(&s(150)), Some((100, 100))), "conflict");
        // sizes differ but nothing changed since the last sync (e.g. paths were rewritten)
        assert_eq!(status(Some(&s(90)), Some(&s(100)), Some((90, 100))), "synced");
        assert_eq!(status(Some(&s(150)), Some(&s(100)), None), "push"); // no base: longer wins
        // Claude Code: lines that don't count after the last message (size 130 > weight 100), and
        // bases saved by versions that counted more (up to 120)
        let mut c = s(100);
        c.size = Some(130);
        assert_eq!(status(Some(&c), Some(&s(150)), Some((120, 100))), "pull");
        assert_eq!(status(Some(&c), Some(&s(100)), Some((100, 100))), "synced");
        assert_eq!(status(Some(&c), Some(&s(120)), None), "synced");
        c.weight = 125; // a message after the base
        assert_eq!(status(Some(&c), Some(&s(150)), Some((120, 100))), "conflict");
        c.weight = 50; // replaced by a shorter one
        c.size = Some(60);
        assert_eq!(status(Some(&c), Some(&s(100)), Some((100, 100))), "push");
    }

    #[test]
    fn sizes_are_shown_in_the_right_unit() {
        let mut x = s(1310820);
        x.harness = "codex";
        assert_eq!(x.size_label(), "1MB");
        assert_eq!(s(2048).size_label(), "2KB");
        let mut o = s(13);
        o.harness = "opencode";
        assert_eq!(o.size_label(), "13 msgs");
    }
}
