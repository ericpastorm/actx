//! init and update.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::*;

#[test]
fn init_saves_the_shared_folder() {
    let w = World::new();
    let m = w.machine("pc");
    let target = w.dir.path().join("nas/actx");
    let mut c = m.command(ACTX);
    c.env_remove("ACTX_ROOT")
        .env_remove("ACTX_LANG")
        .args(["init", target.to_str().unwrap(), "--lang", "es"]);
    let out = String::from_utf8_lossy(&c.output().unwrap().stdout).into_owned();
    assert!(out.contains("Carpeta compartida"), "{out}");
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(m.home.join(".config/actx/config.json")).unwrap()).unwrap();
    assert_eq!(config, serde_json::json!({"root": canon(&target), "lang": "es"}));
    assert!(target.join(".actx").exists());
    // later runs use it, in Spanish
    let mut c = m.command(ACTX);
    c.env_remove("ACTX_ROOT").env_remove("ACTX_LANG");
    assert!(String::from_utf8_lossy(&c.output().unwrap().stdout).contains("Aún no hay proyectos"));
}

/// A copy of actx installed as a plain file, as `curl` would leave it.
fn installed_copy(w: &World) -> (Machine, PathBuf, PathBuf) {
    let m = w.machine("pc");
    let bin = w.dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let mine = bin.join(if cfg!(windows) { "actx.exe" } else { "actx" });
    std::fs::copy(ACTX, &mine).unwrap();
    let shared = w
        .store()
        .join("_bin")
        .join(format!("actx-{}-{}", std::env::consts::ARCH, std::env::consts::OS));
    std::fs::create_dir_all(shared.parent().unwrap()).unwrap();
    (m, mine, shared)
}

fn run(m: &Machine, program: &Path, args: &[&str]) -> String {
    let out = m.command(program.to_str().unwrap()).args(args).output().unwrap();
    String::from_utf8_lossy(&[out.stdout, out.stderr].concat()).into_owned()
}

/// Stand in for another build of actx in the shared folder: one that prints `version` (or fails
/// when None), saved `age` seconds from now. On Windows there are no scripts to run: a copy of
/// actx with a tail added is another (working) build, and garbage is a broken one.
fn write_script(path: &Path, version: Option<&str>, age: i64) {
    if cfg!(windows) {
        let mut body = std::fs::read(ACTX).unwrap();
        match version {
            Some(v) => body.extend_from_slice(format!("\0{v}").as_bytes()),
            None => body = b"not a program".to_vec(),
        }
        std::fs::write(path, body).unwrap();
    } else {
        let script = match version {
            Some(v) => format!("echo 'actx {v}'"),
            None => "exit 1".to_string(),
        };
        std::fs::write(path, format!("#!/bin/sh\n{script}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let when = if age >= 0 {
        SystemTime::now() + Duration::from_secs(age as u64)
    } else {
        SystemTime::now() - Duration::from_secs((-age) as u64)
    };
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

#[test]
fn update_installs_a_newer_shared_copy() {
    let w = World::new();
    let (m, mine, shared) = installed_copy(&w);
    write_script(&shared, Some("9.9.9"), 60);
    assert!(run(&m, &mine, &["update"]).contains("updated"));
    assert!(String::from_utf8_lossy(&std::fs::read(&mine).unwrap()).contains("9.9.9"));
    assert_eq!(std::fs::read(&mine).unwrap(), std::fs::read(&shared).unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert!(mine.metadata().unwrap().permissions().mode() & 0o111 != 0);
    }
    // Windows: the old actx was moved aside to make room, not left in the way
    assert!(!mine.with_file_name(".actx.new.exe").exists());
}

#[test]
fn update_uploads_when_this_copy_is_newer() {
    let w = World::new();
    let (m, mine, shared) = installed_copy(&w);
    write_script(&shared, Some("0.0.1"), -600);
    assert!(run(&m, &mine, &["update"]).contains("uploaded"));
    assert_eq!(std::fs::read(&shared).unwrap(), std::fs::read(&mine).unwrap());
}

#[test]
fn update_refuses_a_broken_shared_copy() {
    let w = World::new();
    let (m, mine, shared) = installed_copy(&w);
    write_script(&shared, None, 60);
    assert!(run(&m, &mine, &["update"]).contains("broken"));
    assert_eq!(std::fs::read(&mine).unwrap(), std::fs::read(ACTX).unwrap());
}

#[test]
fn update_defers_to_git_and_packages() {
    let w = World::new();
    let m = w.machine("pc");
    if PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".git").exists() {
        assert!(m.actx(&["update"]).contains("git checkout"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let locked = w.dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        let mine = locked.join("actx");
        std::fs::copy(ACTX, &mine).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap(); // like /usr/bin for a user
        let out = run(&m, &mine, &["update"]);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(out.contains("package manager"), "{out}");
    }
}

#[cfg(unix)]
#[test]
fn machines_on_the_python_version_move_to_this_one() {
    use std::os::unix::fs::PermissionsExt;
    needs!("python3");
    let w = World::new();
    let (m, mine, shared) = installed_copy(&w);
    run(&m, &mine, &["update"]);
    assert_eq!(std::fs::read(&shared).unwrap(), std::fs::read(ACTX).unwrap());
    // the Python version updates from _bin/actx, which it checks is Python before installing it
    let bridge = w.store().join("_bin/actx");
    let check = m
        .command("python3")
        .args([
            "-c",
            "import sys; compile(open(sys.argv[1]).read(), sys.argv[1], 'exec')",
        ])
        .arg(&bridge)
        .status()
        .unwrap();
    assert!(check.success());
    let old = w.dir.path().join("bin/old-actx");
    std::fs::copy(&bridge, &old).unwrap();
    std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o755)).unwrap();
    // its first run puts this program in its place, and runs it
    assert!(run(&m, &old, &["--version"]).starts_with("actx "));
    assert_eq!(std::fs::read(&old).unwrap(), std::fs::read(ACTX).unwrap());
    assert!(run(&m, &old, &["--version"]).starts_with("actx "));
    // and it's left there only once
    let when = bridge.metadata().unwrap().modified().unwrap();
    std::fs::remove_file(&shared).unwrap();
    run(&m, &mine, &["update"]);
    assert!(shared.exists());
    assert_eq!(bridge.metadata().unwrap().modified().unwrap(), when);
}
