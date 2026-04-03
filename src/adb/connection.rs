use std::process::Stdio;

use super::{adb_command, CommandExt, CREATE_NO_WINDOW};

/// Connect to a device over WiFi/TCP.
pub fn adb_connect(addr: &str) -> (bool, String) {
    let output = adb_command().and_then(|mut c| {
        c.args(["connect", addr])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
    });

    match output {
        Some(o) => {
            let text = format_output(&o.stdout, &o.stderr);
            let ok = text.contains("connected") && !text.contains("cannot");
            (ok, text)
        }
        None => (false, "Failed to run adb".into()),
    }
}

/// Pair with a device for Android 11+ wireless debugging.
pub fn adb_pair(addr: &str, code: &str) -> (bool, String) {
    let output = adb_command().and_then(|mut c| {
        c.args(["pair", addr, code])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
    });

    match output {
        Some(o) => {
            let text = format_output(&o.stdout, &o.stderr);
            (o.status.success(), text)
        }
        None => (false, "Failed to run adb".into()),
    }
}

/// Disconnect a TCP device.
pub fn adb_disconnect(addr: &str) -> (bool, String) {
    let output = adb_command().and_then(|mut c| {
        c.args(["disconnect", addr])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
    });

    match output {
        Some(o) => {
            let text = format_output(&o.stdout, &o.stderr);
            (o.status.success(), text)
        }
        None => (false, "Failed to run adb".into()),
    }
}

/// Disconnect all TCP devices.
pub fn adb_disconnect_all() -> (bool, String) {
    let output = adb_command().and_then(|mut c| {
        c.args(["disconnect"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
    });

    match output {
        Some(o) => {
            let text = format_output(&o.stdout, &o.stderr);
            (o.status.success(), text)
        }
        None => (false, "Failed to run adb".into()),
    }
}

/// Show the current `adb devices -l` output.
pub fn adb_devices_long() -> (bool, String) {
    run_global_adb_action(&["devices", "-l"])
}

/// Restart the global ADB server.
pub fn restart_adb_server() -> (bool, String) {
    let (kill_ok, kill_msg) = run_global_adb_action(&["kill-server"]);
    let (start_ok, start_msg) = run_global_adb_action(&["start-server"]);
    let ok = kill_ok && start_ok;

    let mut parts = Vec::new();
    if !kill_msg.trim().is_empty() {
        parts.push(format!("kill-server: {kill_msg}"));
    }
    if !start_msg.trim().is_empty() {
        parts.push(format!("start-server: {start_msg}"));
    }

    let message = if parts.is_empty() {
        "ADB server restarted".to_string()
    } else {
        parts.join(" | ")
    };
    (ok, message)
}

/// Check if a serial is an emulator.
pub fn is_emulator_serial(serial: &str) -> bool {
    serial.starts_with("emulator-")
}

/// Check if a serial looks like a WSA device.
pub fn is_wsa_serial(serial: &str) -> bool {
    // WSA is typically 127.0.0.1:58526 or localhost:58526
    serial.contains(":58526") || serial.contains(":58527")
}

/// Check if a serial is a TCP/WiFi connection.
pub fn is_tcp_device(serial: &str) -> bool {
    serial.contains(':')
}

/// Open WSA Settings on Windows.
#[cfg(windows)]
pub fn open_wsa_settings() -> bool {
    use std::os::windows::process::CommandExt as WinCmdExt;
    let mut cmd = std::process::Command::new("cmd.exe");
    cmd.args(["/c", "start", "wsa-settings:"]);
    WinCmdExt::creation_flags(&mut cmd, CREATE_NO_WINDOW);
    cmd.status()
        .map(|s: std::process::ExitStatus| s.success())
        .unwrap_or(false)
}

#[cfg(not(windows))]
pub fn open_wsa_settings() -> bool {
    false
}

/// Launch WSA (open the WSA app, which starts the subsystem).
#[cfg(windows)]
pub fn launch_wsa() -> bool {
    use std::os::windows::process::CommandExt as WinCmdExt;
    let mut cmd = std::process::Command::new("cmd.exe");
    cmd.args(["/c", "start", "wsa://"]);
    WinCmdExt::creation_flags(&mut cmd, CREATE_NO_WINDOW);
    cmd.status()
        .map(|s: std::process::ExitStatus| s.success())
        .unwrap_or(false)
}

#[cfg(not(windows))]
pub fn launch_wsa() -> bool {
    false
}

fn format_output(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();

    match (stdout.is_empty(), stderr.is_empty()) {
        (false, true) => stdout,
        (true, false) => stderr,
        (false, false) => format!("{stdout}\n{stderr}"),
        (true, true) => String::new(),
    }
}

fn run_global_adb_action(args: &[&str]) -> (bool, String) {
    let output = adb_command().and_then(|mut c| {
        c.args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
    });

    match output {
        Some(o) => {
            let text = format_output(&o.stdout, &o.stderr);
            (o.status.success(), text)
        }
        None => (false, "Failed to run adb".into()),
    }
}
