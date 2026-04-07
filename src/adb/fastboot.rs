use std::path::PathBuf;
use std::process::{Command, Stdio};

use super::{homebrew_tool_candidates, sdk_root_candidates, CommandExt, CREATE_NO_WINDOW};

pub fn list_fastboot_devices() -> (bool, String) {
    run_fastboot_action(None, &["devices"])
}

pub fn flash_partition(serial: Option<&str>, partition: &str, image_path: &str) -> (bool, String) {
    let partition = partition.trim();
    if partition.is_empty() {
        return (false, "Partition name is required".into());
    }

    let mut args = Vec::new();
    if let Some(serial) = serial.map(str::trim).filter(|serial| !serial.is_empty()) {
        args.push("-s".to_string());
        args.push(serial.to_string());
    }
    args.push("flash".to_string());
    args.push(partition.to_string());
    args.push(image_path.to_string());

    let refs: Vec<&str> = args.iter().map(std::string::String::as_str).collect();
    run_fastboot_action(None, &refs)
}

fn run_fastboot_action(_serial: Option<&str>, args: &[&str]) -> (bool, String) {
    let fastboot = match resolve_fastboot() {
        Ok(path) => path,
        Err(error) => return (false, error),
    };

    match Command::new(fastboot)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let text = match (stdout.is_empty(), stderr.is_empty()) {
                (false, true) => stdout,
                (true, false) => stderr,
                (false, false) => format!("{stdout}\n{stderr}"),
                (true, true) => String::new(),
            };
            (output.status.success(), text)
        }
        Err(error) => (false, format!("Failed to run fastboot: {error}")),
    }
}

fn resolve_fastboot() -> Result<PathBuf, String> {
    if fastboot_available("fastboot") {
        return Ok(PathBuf::from("fastboot"));
    }

    for root in sdk_root_candidates() {
        let candidate = root.join("platform-tools").join(fastboot_binary_name());
        if candidate.exists() && fastboot_available(&candidate) {
            return Ok(candidate);
        }
    }

    // Homebrew on macOS installs fastboot directly into its bin directory.
    for candidate in homebrew_tool_candidates("fastboot") {
        if candidate.exists() && fastboot_available(&candidate) {
            return Ok(candidate);
        }
    }

    Err(format!(
        "fastboot not found on PATH or in common Android SDK locations. Checked: {}",
        sdk_root_candidates()
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn fastboot_available(program: impl AsRef<std::ffi::OsStr>) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .is_ok_and(|status| status.success())
}

const fn fastboot_binary_name() -> &'static str {
    #[cfg(windows)]
    {
        "fastboot.exe"
    }

    #[cfg(not(windows))]
    {
        "fastboot"
    }
}
