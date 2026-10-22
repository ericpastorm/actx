//! Subprocess transport with progress and cooperative cancellation for reviewed writes.
use eframe::egui;
use serde_json::{Value, json};
use std::{
    io::{BufRead, Read, Write},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
#[derive(Default)]
pub(super) struct Control {
    cancelled: AtomicBool,
    pub(super) finished: AtomicBool,
    progress: Mutex<Value>,
}
impl Control {
    pub(super) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    pub(super) fn progress(&self) -> Value {
        self.progress.lock().unwrap().clone()
    }
}
pub(super) fn call_api(
    request: &Value,
    lang: &str,
    control: Arc<Control>,
    ctx: &egui::Context,
) -> Result<Value, String> {
    let temp = tempfile::Builder::new()
        .prefix("actx-desktop-job-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let cancel = temp.path().join("cancel");
    let mut cmd = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    crate::ctx::hide_window(&mut cmd);
    let mut child = cmd
        .arg("api")
        .env("ACTX_LANG", lang)
        .env_remove("ACTX_PICK")
        .env_remove("ACTX_ANSWER")
        .env("ACTX_PROGRESS", "1")
        .env("ACTX_CANCEL_FILE", &cancel)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Err(e) = child
        .stdin
        .take()
        .ok_or_else(|| "Cannot open API stdin".to_string())
        .and_then(|mut stdin| {
            stdin
                .write_all(request.to_string().as_bytes())
                .map_err(|e| e.to_string())
        })
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
    }
    let mut stdout = child.stdout.take().unwrap();
    let output = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr = child.stderr.take().unwrap();
    let reader_control = control.clone();
    let ctx = ctx.clone();
    let errors = std::thread::spawn(move || {
        let mut errors = String::new();
        for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Ok(event) = serde_json::from_str::<Value>(&line)
                && event["event"] == "progress"
            {
                *reader_control.progress.lock().unwrap() = event;
                ctx.request_repaint();
            } else if let Ok(event) = serde_json::from_str::<Value>(&line)
                && event["event"] == "discovery_index"
            {
                let _ = crate::discovery::persist(&event);
            } else if errors.len() < 8192 {
                errors.push_str(&line);
                errors.push('\n');
            }
        }
        errors
    });
    let mut cancellation_sent = false;
    loop {
        if control.cancelled.load(Ordering::Relaxed) && !cancellation_sent {
            cancellation_sent = true;
            let _ = std::fs::write(&cancel, b"cancel");
            if request["apply"] != true {
                let _ = child.kill();
            }
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = output.join();
                let _ = errors.join();
                return Err(e.to_string());
            }
        }
    }
    let bytes = output
        .join()
        .map_err(|_| "API reader stopped")?
        .map_err(|e| e.to_string())?;
    let errors = errors.join().unwrap_or_default();
    if cancellation_sent && request["apply"] != true {
        return Ok(
            json!({"schema_version":1,"command":request["action"],"status":"cancelled","data":{"applied":false},"diagnostics":[]}),
        );
    }
    let response: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{e}: {errors}"))?;
    if response["schema_version"] != 1 {
        return Err("Unsupported API version".into());
    }
    Ok(response)
}
