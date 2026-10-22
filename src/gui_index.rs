//! Disposable local desktop catalog. Action planning always uses the live services.
use crate::ctx::ctx;

/// CLI/API changes notify a running desktop without rescanning shared storage.
/// Keep the last snapshot: it remains useful while the shared folder is unavailable.
pub fn invalidate() {
    let dir = &ctx().cache_dir;
    if !dir.is_dir() {
        return;
    }
    if let Ok(mut tmp) = tempfile::NamedTempFile::new_in(dir) {
        use std::io::Write;
        if tmp.write_all(uuid::Uuid::new_v4().to_string().as_bytes()).is_ok() {
            let _ = tmp.persist(dir.join("gui-index-dirty"));
        }
    }
}

#[cfg(feature = "gui")]
mod index {
    use crate::{
        ctx::{Result, ctx},
        engine,
    };
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};
    use std::{
        io::Read,
        path::{Path, PathBuf},
    };
    const MAX_BYTES: u64 = 32 * 1024 * 1024;
    #[derive(Serialize, Deserialize)]
    pub struct Snapshot {
        version: u32,
        context: String,
        pub checked_at: u64,
        pub catalog: Value,
    }
    #[derive(Clone)]
    pub struct Index {
        dir: PathBuf,
        context: String,
        epoch: String,
    }
    fn stamp(path: &Path) -> Value {
        std::fs::metadata(path)
            .ok()
            .map(|m| {
                json!([
                    m.len(),
                    m.modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_nanos().to_string())
                ])
            })
            .unwrap_or(Value::Null)
    }
    /// Only local files and configured path strings; never stat/canonicalize a NAS path here.
    fn context() -> String {
        let c = ctx();
        engine::digest(&serde_json::to_vec(&json!({"home":c.home,"host":c.host,"config":std::fs::read(&c.config_file).ok(),"root_override":std::env::var_os("ACTX_ROOT").map(|s|s.to_string_lossy().into_owned()),"claude":c.claude_projects,"codex":c.codex_home,"opencode":c.opencode_db})).unwrap())
    }
    pub fn epoch() -> String {
        let c = ctx();
        engine::digest(
            &serde_json::to_vec(&json!([
                stamp(&c.state_file()),
                stamp(&c.config_file),
                stamp(&c.state_dir.join("harness-links.json")),
                stamp(&c.state_dir.join("conversation-groups.json")),
                stamp(&c.cache_dir.join("gui-index-dirty"))
            ]))
            .unwrap(),
        )
    }
    pub fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
    pub fn normal(request: &Value) -> bool {
        request["action"] == "list"
            && request["all"] != true
            && request["general"] != true
            && request["project"].is_null()
            && request["ids"].as_array().is_none_or(Vec::is_empty)
    }
    fn valid(data: &Value) -> bool {
        data["complete"] == true
            && data["projects"].is_array()
            && data["conversations"].is_array()
            && data["scope"]["all"] == false
            && data["scope"]["general"] == false
            && data["scope"]["project"].is_null()
            && data["conversations"].as_array().is_some_and(|rows| {
                rows.iter()
                    .all(|r| r["id"].is_string() && r["managed"] == true && r["copies"].is_array())
            })
    }
    impl Index {
        pub fn current() -> Self {
            Self {
                dir: ctx().cache_dir.clone(),
                context: context(),
                epoch: epoch(),
            }
        }
        pub fn unchanged(&self) -> bool {
            self.context == context() && self.epoch == epoch()
        }
        pub fn load(&self) -> Option<Snapshot> {
            let mut input = std::fs::File::open(self.dir.join("gui-index-v1.json"))
                .ok()?
                .take(MAX_BYTES + 1);
            let mut bytes = Vec::new();
            input.read_to_end(&mut bytes).ok()?;
            if bytes.len() as u64 > MAX_BYTES {
                return None;
            }
            let snapshot: Snapshot = serde_json::from_slice(&bytes).ok()?;
            (snapshot.version == 1 && snapshot.context == self.context && valid(&snapshot.catalog)).then_some(snapshot)
        }
        pub fn store(&self, response: &Value) -> Result<bool> {
            if !self.unchanged()
                || response["schema_version"] != 1
                || !matches!(response["status"].as_str(), Some("ok" | "conflict"))
                || !valid(&response["data"])
            {
                return Ok(false);
            }
            self.write(&response["data"], now())?;
            Ok(true)
        }
        fn write(&self, catalog: &Value, checked_at: u64) -> Result<()> {
            let snapshot = Snapshot {
                version: 1,
                context: self.context.clone(),
                checked_at,
                catalog: catalog.clone(),
            };
            let bytes = serde_json::to_vec(&snapshot)?;
            if bytes.len() as u64 > MAX_BYTES {
                return Ok(());
            }
            std::fs::create_dir_all(&self.dir)?;
            let mut tmp = tempfile::NamedTempFile::new_in(&self.dir)?;
            use std::io::Write;
            tmp.write_all(&bytes)?;
            tmp.persist(self.dir.join("gui-index-v1.json"))
                .map_err(|e| crate::ctx::Error(e.to_string()))?;
            Ok(())
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        fn catalog() -> Value {
            json!({"complete":true,"scope":{"all":false,"general":false,"project":null},"projects":[{"collection":{"kind":"project","id":"Work"},"local_folder":"/different/on/each/pc"}],"conversations":[{"id":"claude:one","managed":true,"title":"Saved","copies":[]}]})
        }
        #[test]
        fn snapshot_is_local_disposable_and_partitioned_by_configuration() {
            let dir = tempfile::tempdir().unwrap();
            let i = Index {
                dir: dir.path().into(),
                context: "offline-NAS-a".into(),
                epoch: epoch(),
            };
            i.write(&catalog(), 42).unwrap();
            let loaded = i.load().unwrap();
            assert_eq!(loaded.checked_at, 42);
            assert_eq!(loaded.catalog, catalog());
            assert!(
                Index {
                    context: "other-NAS-or-home".into(),
                    ..i.clone()
                }
                .load()
                .is_none()
            );
            let path = dir.path().join("gui-index-v1.json");
            std::fs::write(&path, b"incomplete {").unwrap();
            assert!(i.load().is_none());
            i.write(&catalog(), 43).unwrap();
            let mut wrong: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            wrong["version"] = json!(2);
            std::fs::write(&path, wrong.to_string()).unwrap();
            assert!(i.load().is_none());
        }
        #[test]
        fn refresh_never_replaces_a_good_snapshot_with_partial_inventory_or_old_context() {
            let dir = tempfile::tempdir().unwrap();
            let i = Index {
                dir: dir.path().into(),
                context: context(),
                epoch: epoch(),
            };
            i.write(&catalog(), 42).unwrap();
            let before = std::fs::read(dir.path().join("gui-index-v1.json")).unwrap();
            let mut r = json!({"schema_version":1,"status":"incomplete","data":catalog()});
            assert!(!i.store(&r).unwrap());
            r["status"] = json!("ok");
            r["data"]["scope"]["all"] = json!(true);
            assert!(!i.store(&r).unwrap());
            r["data"] = catalog();
            assert!(
                !Index {
                    epoch: "changed-during-read".into(),
                    ..i.clone()
                }
                .store(&r)
                .unwrap()
            );
            assert_eq!(before, std::fs::read(dir.path().join("gui-index-v1.json")).unwrap());
            assert!(i.store(&r).unwrap());
            assert_eq!(i.load().unwrap().catalog, catalog());
            assert!(!normal(&json!({"action":"candidates"})));
            assert!(!normal(&json!({"action":"list","all":true})));
        }
    }
}
#[cfg(feature = "gui")]
pub use index::*;
