use std::process::Stdio;

use super::{adb_command, adb_path, CommandExt, DeviceInfo, CREATE_NO_WINDOW};

pub fn list_devices() -> Result<Vec<DeviceInfo>, String> {
    let mut cmd = adb_command().ok_or_else(|| adb_path().unwrap_err())?;

    let output = cmd
        .args(["devices", "-l"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed to run adb: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut devices = Vec::new();

    for line in stdout.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let serial = parts[0].to_string();
            let state = parts[1].to_string();
            let model = parts
                .iter()
                .find_map(|p| p.strip_prefix("model:"))
                .unwrap_or("unknown")
                .to_string();
            devices.push(DeviceInfo {
                serial,
                state,
                model,
            });
        }
    }

    Ok(devices)
}
