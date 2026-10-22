//! Optional desktop progress/cancellation transport; stdout remains the JSON response only.
use crate::ctx::{Error, Result};
use std::{path::PathBuf, sync::OnceLock};
fn cancel_file() -> Option<&'static PathBuf> {
    static FILE: OnceLock<Option<PathBuf>> = OnceLock::new();
    FILE.get_or_init(|| std::env::var_os("ACTX_CANCEL_FILE").map(PathBuf::from))
        .as_ref()
}
pub fn cancelled() -> bool {
    cancel_file().is_some_and(|p| p.is_file())
}
pub fn check() -> Result<()> {
    if cancelled() {
        Err(Error("Operation cancelled".into()))
    } else {
        Ok(())
    }
}
pub fn progress(done: usize, total: usize, label: &str) {
    if std::env::var("ACTX_PROGRESS").as_deref() == Ok("1") {
        eprintln!(
            "{}",
            serde_json::json!({"event":"progress","done":done,"total":total,"label":label})
        );
    }
}
