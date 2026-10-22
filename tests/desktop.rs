//! A real Linux window, using temporary homes and the same reviewed API as its controls.
//! Run explicitly under X11 (CI uses Xvfb); ordinary test runs do not open a window.
#![cfg(all(feature = "gui", target_os = "linux"))]
mod common;
use common::*;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::PathBuf,
    process::{Child, Stdio},
    time::{Duration, Instant, SystemTime},
};

struct Window {
    child: Child,
    index: PathBuf,
    log: PathBuf,
}
impl Window {
    fn wait_for(&mut self, description: &str, check: impl Fn(&Value) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "desktop exited while {description}: {}",
                std::fs::read_to_string(&self.log).unwrap_or_default()
            );
            if let Ok(bytes) = std::fs::read(&self.index)
                && let Ok(snapshot) = serde_json::from_slice::<Value>(&bytes)
                && check(&snapshot["catalog"])
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "desktop timed out while {description}: {}",
                std::fs::read_to_string(&self.log).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn api(m: &Machine, req: &Value) -> Value {
    let mut child = m
        .command(ACTX)
        .arg("api")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(req.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let response: Value =
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    assert_eq!(response["status"], "ok", "{response}");
    response
}
fn apply(m: &Machine, mut request: Value) {
    let plan = api(m, &request);
    request["expected"] = if plan["data"]["expected"].is_object() {
        plan["data"]["expected"].clone()
    } else {
        plan["data"]["operations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|op| (op["session_id"].as_str().unwrap().into(), op["revision"].clone()))
            .collect::<serde_json::Map<_, _>>()
            .into()
    };
    request["apply"] = json!(true);
    api(m, &request);
}
fn contains(catalog: &Value, id: &str) -> bool {
    catalog["conversations"].as_array().is_some_and(|rows| {
        rows.iter().any(|row| {
            row["copies"]
                .as_array()
                .is_some_and(|copies| copies.iter().any(|copy| copy["id"] == id))
        })
    })
}

#[test]
#[ignore = "requires a native X11 display; CI runs this under Xvfb"]
fn native_window_tracks_reviewed_changes_and_keeps_its_offline_catalog() {
    assert!(std::env::var_os("DISPLAY").is_some(), "run with xvfb-run -a");
    let w = World::new();
    let m = w.machine("desktop");
    m.actx(&["profile", "--harnesses", "claude"]);
    let work = m.home.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let source = claude_session(
        &m,
        &work,
        &[
            ("user", "GUI conversation 中文"),
            ("assistant", "Keep the original history"),
        ],
        Some("Desktop workflow"),
    );
    std::fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(86400))
        .unwrap();
    let original = std::fs::read(&source).unwrap();
    let original_time = source.metadata().unwrap().modified().unwrap();
    let key = format!("claude:{}", source.file_stem().unwrap().to_string_lossy());
    let log = m.home.join("desktop.log");
    let output = std::fs::File::create(&log).unwrap();
    let child = m
        .command(ACTX)
        .arg("gui")
        .env("DISPLAY", std::env::var_os("DISPLAY").unwrap())
        .env("LIBGL_ALWAYS_SOFTWARE", "1")
        .env_remove("WAYLAND_DISPLAY")
        .stdout(output.try_clone().unwrap())
        .stderr(output)
        .spawn()
        .unwrap();
    let mut window = Window {
        child,
        index: m.home.join(".cache/actx/gui-index-v1.json"),
        log,
    };
    window.wait_for("opening the window", |c| c["complete"] == true);
    assert!(!contains(
        &serde_json::from_slice::<Value>(&std::fs::read(&window.index).unwrap()).unwrap()["catalog"],
        &key
    ));

    apply(&m, json!({"action":"create_project","project":"demo","folder":work}));
    window.wait_for("refreshing projects", |c| {
        c["projects"]
            .as_array()
            .is_some_and(|p| p.iter().any(|p| p["collection"]["id"] == "demo"))
    });
    let candidates = api(&m, &json!({"action":"candidates","project":"demo"}));
    assert!(contains(&candidates["data"], &key));
    apply(&m, json!({"action":"add","project":"demo","ids":[key]}));
    window.wait_for("adding a conversation", |c| contains(c, &key));
    apply(&m, json!({"action":"delete","ids":[key]}));
    window.wait_for("removing the shared copy", |c| !contains(c, &key));
    assert_eq!(std::fs::read(&source).unwrap(), original);

    apply(&m, json!({"action":"add","general":true,"ids":[key]}));
    window.wait_for("adding to General", |c| contains(c, &key));
    let saved = std::fs::read(&window.index).unwrap();
    std::fs::rename(w.store(), w.dir.path().join("offline-store")).unwrap();
    // Simulate the local notification that prompts a running desktop to verify its catalog.
    std::fs::write(
        m.home.join(".cache/actx/gui-index-dirty"),
        uuid::Uuid::new_v4().to_string(),
    )
    .unwrap();
    std::thread::sleep(Duration::from_secs(5));
    assert!(window.child.try_wait().unwrap().is_none());
    assert_eq!(
        std::fs::read(&window.index).unwrap(),
        saved,
        "offline refresh erased the saved catalog"
    );
    assert_eq!(std::fs::read(&source).unwrap(), original);
    assert_eq!(source.metadata().unwrap().modified().unwrap(), original_time);
    let log = std::fs::read_to_string(&window.log).unwrap();
    assert!(!log.contains("panicked"), "{log}");
}
