use std::process::{Child, Stdio};

use super::{adb_command, CommandExt, CREATE_NO_WINDOW};

/// Capture a screenshot and return raw PNG bytes.
pub fn capture_screenshot_bytes(serial: &str) -> Result<Vec<u8>, String> {
    let mut cmd = adb_command().ok_or("ADB not available")?;
    let output = cmd
        .args(["-s", serial, "exec-out", "screencap", "-p"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed: {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    if output.stdout.is_empty() {
        return Err("Empty screenshot data".into());
    }
    Ok(output.stdout)
}

/// Start screen recording. Returns the child process.
pub fn start_screen_record(
    serial: &str,
    remote_path: &str,
    time_limit: u32,
) -> Result<Child, String> {
    let mut cmd = adb_command().ok_or_else(|| "ADB not available".to_string())?;
    cmd.args([
        "-s",
        serial,
        "shell",
        "screenrecord",
        "--time-limit",
        &time_limit.to_string(),
        remote_path,
    ])
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .creation_flags(CREATE_NO_WINDOW);

    cmd.spawn()
        .map_err(|error| format!("Failed to start screenrecord: {error}"))
}
