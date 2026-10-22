//! Double-clickable launcher. The sibling actx contains the shared CLI and desktop services.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
fn main() {
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let launcher = std::env::current_exe()?;
        let binary = launcher.with_file_name(if cfg!(windows) { "actx.exe" } else { "actx" });
        let mut command = std::process::Command::new(binary);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let output = command.arg("gui").stdin(std::process::Stdio::null()).output()?;
        if !output.status.success() {
            return Err(format!("actx: {}\n{}", output.status, String::from_utf8_lossy(&output.stderr)).into());
        }
        Ok(())
    })();
    if let Err(error) = result {
        rfd::MessageDialog::new()
            .set_title("actx")
            .set_description(error.to_string())
            .set_level(rfd::MessageLevel::Error)
            .show();
        std::process::exit(1);
    }
}
