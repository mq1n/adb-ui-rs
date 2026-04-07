use std::path::PathBuf;
use std::process::{Command, Stdio};

use super::device_mgmt::adb_shell;
use super::devices::list_devices;
use super::{adb_command, sdk_root_candidates, CommandExt, CREATE_NO_WINDOW};

/// Find the Android SDK emulator binary.
fn emulator_path() -> Option<PathBuf> {
    for sdk in sdk_root_candidates() {
        let windows_path = sdk.join("emulator/emulator.exe");
        if windows_path.exists() {
            return Some(windows_path);
        }
        let unix_path = sdk.join("emulator/emulator");
        if unix_path.exists() {
            return Some(unix_path);
        }
    }

    let probe = Command::new("emulator")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
    if let Ok(s) = probe {
        if s.success() {
            return Some(PathBuf::from("emulator"));
        }
    }
    None
}

/// List available AVDs.
pub fn list_avds() -> Vec<String> {
    // Try the emulator binary first.
    if let Some(emu) = emulator_path() {
        let output = Command::new(&emu)
            .arg("-list-avds")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .output();

        if let Ok(o) = output {
            // Some emulator versions print AVD names to stderr instead of stdout.
            let text = if o.status.success() && !o.stdout.is_empty() {
                String::from_utf8_lossy(&o.stdout).to_string()
            } else {
                String::from_utf8_lossy(&o.stderr).to_string()
            };
            let names: Vec<String> = text
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            if !names.is_empty() {
                return names;
            }
        }
    }

    // Fallback: scan ~/.android/avd/ for *.ini files.
    list_avds_from_directory()
}

/// Scan the default AVD directory for `.ini` files and derive AVD names.
fn list_avds_from_directory() -> Vec<String> {
    let avd_dir = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| PathBuf::from(h).join(".android").join("avd"));

    let Some(dir) = avd_dir else {
        return Vec::new();
    };

    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.strip_suffix(".ini").map(|n| n.to_string())
        })
        .collect()
}

/// Get running emulator serials.
pub fn get_running_emulators() -> Vec<String> {
    list_devices().map_or_else(
        |_| Vec::new(),
        |devs| {
            devs.into_iter()
                .filter(|d| d.serial.starts_with("emulator-") && d.state == "device")
                .map(|d| d.serial)
                .collect()
        },
    )
}

/// Get the AVD name for a running emulator serial.
pub fn get_emulator_avd_name(serial: &str) -> Option<String> {
    let out = adb_shell(serial, "getprop ro.kernel.qemu.avd_name");
    if !out.is_empty() {
        return Some(out);
    }
    // Fallback: emu avd name command.
    let output = adb_command()?
        .args(["-s", serial, "emu", "avd", "name"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .next()
        .map(|l| l.trim().to_string())
        .filter(|s| !s.is_empty() && s != "OK")
}

/// Build a map of running emulator serial -> AVD name (blocking; call from background thread).
pub fn get_running_emulator_map() -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for serial in get_running_emulators() {
        if let Some(name) = get_emulator_avd_name(&serial) {
            map.insert(serial, name);
        }
    }
    map
}

/// Start an emulator AVD. Returns immediately (emulator runs in background).
pub fn start_emulator(avd_name: &str, cold_boot: bool) -> (bool, String) {
    let Some(emu) = emulator_path() else {
        return (false, "Emulator binary not found".into());
    };
    let mut cmd = Command::new(&emu);
    cmd.args(["-avd", avd_name, "-gpu", "auto"]);
    if cold_boot {
        cmd.arg("-no-snapshot-load");
    }
    cmd.stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);

    match cmd.spawn() {
        Ok(_) => (true, format!("Emulator '{avd_name}' starting...")),
        Err(e) => (false, format!("Failed to start emulator: {e}")),
    }
}

/// Kill an emulator via adb emu kill.
pub fn kill_emulator(serial: &str) -> (bool, String) {
    super::device_mgmt::run_device_action(serial, &["emu", "kill"])
}

/// Find avdmanager binary.
fn avdmanager_path() -> Option<PathBuf> {
    for sdk in sdk_root_candidates() {
        let windows_path = sdk.join("cmdline-tools/latest/bin/avdmanager.bat");
        if windows_path.exists() {
            return Some(windows_path);
        }
        let unix_path = sdk.join("cmdline-tools/latest/bin/avdmanager");
        if unix_path.exists() {
            return Some(unix_path);
        }
    }

    None
}

/// List available system images for AVD creation.
pub fn list_system_images() -> Vec<String> {
    // Use sdkmanager to list installed system images.
    let sdkmanager = sdk_root_candidates().into_iter().find_map(|sdk| {
        let windows_path = sdk.join("cmdline-tools/latest/bin/sdkmanager.bat");
        if windows_path.exists() {
            return Some(windows_path);
        }
        let unix_path = sdk.join("cmdline-tools/latest/bin/sdkmanager");
        if unix_path.exists() {
            return Some(unix_path);
        }
        None
    });

    let Some(sdk_path) = sdkmanager else {
        return Vec::new();
    };

    let output = Command::new(&sdk_path)
        .args(["--list_installed"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    let Ok(output) = output else {
        return Vec::new();
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with("system-images;") {
                Some(
                    trimmed
                        .split('|')
                        .next()
                        .unwrap_or(trimmed)
                        .trim()
                        .to_string(),
                )
            } else {
                None
            }
        })
        .collect()
}

/// Create a new AVD.
pub fn create_avd(name: &str, system_image: &str, device_type: &str) -> (bool, String) {
    let Some(avdmgr) = avdmanager_path() else {
        return (false, "avdmanager not found".into());
    };

    let mut args = vec![
        "create".to_string(),
        "avd".to_string(),
        "-n".to_string(),
        name.to_string(),
        "-k".to_string(),
        system_image.to_string(),
        "--force".to_string(),
    ];
    if !device_type.is_empty() {
        args.push("-d".to_string());
        args.push(device_type.to_string());
    }

    let output = Command::new(&avdmgr)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    match output {
        Ok(o) => {
            let mut text = String::from_utf8_lossy(&o.stdout).to_string();
            let err = String::from_utf8_lossy(&o.stderr);
            if !err.trim().is_empty() {
                text.push_str(&err);
            }
            (o.status.success(), text.trim().to_string())
        }
        Err(e) => (false, format!("Failed to run avdmanager: {e}")),
    }
}

/// Delete an AVD.
pub fn delete_avd(name: &str) -> (bool, String) {
    let Some(avdmgr) = avdmanager_path() else {
        return (false, "avdmanager not found".into());
    };

    let output = Command::new(&avdmgr)
        .args(["delete", "avd", "-n", name])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    match output {
        Ok(o) => {
            let mut text = String::from_utf8_lossy(&o.stdout).to_string();
            let err = String::from_utf8_lossy(&o.stderr);
            if !err.trim().is_empty() {
                text.push_str(&err);
            }
            (o.status.success(), text.trim().to_string())
        }
        Err(e) => (false, format!("Failed to run avdmanager: {e}")),
    }
}
