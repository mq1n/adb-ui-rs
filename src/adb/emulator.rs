use std::path::PathBuf;
use std::process::{Command, Stdio};

use super::device_mgmt::adb_shell;
use super::devices::list_devices;
use super::{
    adb_command, command_available, homebrew_tool_candidates, sdk_root_candidates, CommandExt,
    CREATE_NO_WINDOW,
};

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

    // Homebrew on macOS installs emulator directly into its bin directory.
    for candidate in homebrew_tool_candidates("emulator") {
        if candidate.exists() {
            return Some(candidate);
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

/// Infer the SDK root from the emulator binary path.
/// e.g. `/opt/homebrew/share/android-commandlinetools/emulator/emulator` → parent of `emulator/`.
fn infer_sdk_root(emu_path: &std::path::Path) -> Option<PathBuf> {
    // If the binary lives in <sdk>/emulator/emulator, the SDK root is two levels up.
    let parent = emu_path.parent()?;
    if parent.file_name()?.to_str()? == "emulator" {
        return parent.parent().map(PathBuf::from);
    }
    None
}

/// Start an emulator AVD. Returns immediately (emulator runs in background).
/// Waits briefly to catch early fatal errors from the emulator process.
pub fn start_emulator(avd_name: &str, cold_boot: bool) -> (bool, String) {
    let Some(emu) = emulator_path() else {
        return (false, "Emulator binary not found".into());
    };
    let mut cmd = Command::new(&emu);
    cmd.args(["-avd", avd_name, "-gpu", "auto"]);
    if cold_boot {
        cmd.arg("-no-snapshot-load");
    }

    // Set ANDROID_SDK_ROOT so the emulator can find system images and platform-tools.
    if std::env::var_os("ANDROID_SDK_ROOT").is_none() {
        // Try explicit SDK root candidates first, then infer from the emulator binary path.
        let sdk_root = sdk_root_candidates()
            .into_iter()
            .find(|r| r.join("platform-tools").exists())
            .or_else(|| infer_sdk_root(&emu));
        if let Some(root) = sdk_root {
            cmd.env("ANDROID_SDK_ROOT", &root);
        }
    }

    cmd.stdout(Stdio::null())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);

    match cmd.spawn() {
        Ok(mut child) => {
            // Wait briefly to catch immediate fatal errors (e.g. missing system image).
            std::thread::sleep(std::time::Duration::from_secs(3));
            match child.try_wait() {
                Ok(Some(status)) if !status.success() => {
                    // Process already exited with error — collect stderr.
                    let stderr = child
                        .stderr
                        .take()
                        .map(|mut se| {
                            let mut buf = String::new();
                            std::io::Read::read_to_string(&mut se, &mut buf).ok();
                            buf
                        })
                        .unwrap_or_default();
                    let reason = stderr
                        .lines()
                        .find(|l| l.contains("FATAL") || l.contains("ERROR") || l.contains("error"))
                        .unwrap_or("unknown error")
                        .trim();
                    (false, format!("Emulator exited immediately: {reason}"))
                }
                _ => {
                    // Still running or exited successfully — good.
                    (true, format!("Emulator '{avd_name}' starting..."))
                }
            }
        }
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

    // Check PATH.
    if command_available("avdmanager", "list") {
        return Some(PathBuf::from("avdmanager"));
    }

    // Homebrew on macOS.
    for candidate in homebrew_tool_candidates("avdmanager") {
        if candidate.exists() {
            return Some(candidate);
        }
    }

    None
}

/// List available system images for AVD creation.
pub fn list_system_images() -> Vec<String> {
    // Use sdkmanager to list installed system images.
    let sdkmanager = sdk_root_candidates()
        .into_iter()
        .find_map(|sdk| {
            let windows_path = sdk.join("cmdline-tools/latest/bin/sdkmanager.bat");
            if windows_path.exists() {
                return Some(windows_path);
            }
            let unix_path = sdk.join("cmdline-tools/latest/bin/sdkmanager");
            if unix_path.exists() {
                return Some(unix_path);
            }
            None
        })
        .or_else(|| {
            // Check PATH.
            if command_available("sdkmanager", "--version") {
                return Some(PathBuf::from("sdkmanager"));
            }
            // Homebrew on macOS.
            homebrew_tool_candidates("sdkmanager")
                .into_iter()
                .find(|c| c.exists())
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
