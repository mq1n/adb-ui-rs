use std::process::Stdio;

use super::device_mgmt::run_device_action;
use super::explorer::cat_remote_file;
use super::{adb_command, CommandExt, CREATE_NO_WINDOW};

/// Generate a bugreport and save to a local path. Blocking — run from a background thread.
pub fn bugreport(serial: &str, local_path: &str) -> (bool, String) {
    let output = adb_command().and_then(|mut c| {
        c.args(["-s", serial, "bugreport", local_path])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
    });

    match output {
        Some(o) => {
            let mut text = String::from_utf8_lossy(&o.stdout).to_string();
            let err = String::from_utf8_lossy(&o.stderr);
            if !err.trim().is_empty() {
                text.push_str(&err);
            }
            (o.status.success(), text.trim().to_string())
        }
        None => (false, "Failed to run adb bugreport".into()),
    }
}

/// Run monkey stress test. Blocking — run from a background thread.
/// Returns the full monkey output.
pub fn run_monkey(
    serial: &str,
    bundle_id: &str,
    event_count: u32,
    seed: Option<u32>,
) -> (bool, String) {
    let bundle_id = bundle_id.trim();
    let mut args = vec![
        "-s".to_string(),
        serial.to_string(),
        "shell".to_string(),
        "monkey".to_string(),
        "-p".to_string(),
        bundle_id.to_string(),
        "--throttle".to_string(),
        "50".to_string(),
        "--pct-touch".to_string(),
        "40".to_string(),
        "--pct-motion".to_string(),
        "25".to_string(),
        "--pct-trackball".to_string(),
        "0".to_string(),
        "--pct-nav".to_string(),
        "10".to_string(),
        "--pct-majornav".to_string(),
        "10".to_string(),
        "--pct-syskeys".to_string(),
        "5".to_string(),
        "--pct-appswitch".to_string(),
        "5".to_string(),
        "--pct-flip".to_string(),
        "5".to_string(),
        "-v".to_string(),
        "-v".to_string(),
    ];
    if let Some(seed) = seed {
        args.push("-s".to_string());
        args.push(seed.to_string());
    }
    args.push(event_count.to_string());

    let output = adb_command().and_then(|mut c| {
        c.args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
    });

    match output {
        Some(o) => {
            let mut text = String::from_utf8_lossy(&o.stdout).to_string();
            let err = String::from_utf8_lossy(&o.stderr);
            if !err.trim().is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&err);
            }
            // Monkey returns 0 on success (no crash), non-zero on crash.
            let crashed = !o.status.success()
                || text.contains("ANR")
                || text.contains("CRASH")
                || text.contains("Exception");
            if crashed {
                (false, text)
            } else {
                (true, text)
            }
        }
        None => (false, "Failed to run monkey".into()),
    }
}

/// Dump UI hierarchy via uiautomator. Returns the XML content.
pub fn uiautomator_dump(serial: &str) -> Result<String, String> {
    let remote_path = "/sdcard/ui_dump.xml";

    // Dump.
    let (ok, msg) = run_device_action(serial, &["shell", "uiautomator", "dump", remote_path]);
    if !ok {
        return Err(format!("uiautomator dump failed: {msg}"));
    }

    // Read the file.
    let content = cat_remote_file(serial, remote_path);

    // Cleanup.
    let _ = run_device_action(serial, &["shell", "rm", "-f", remote_path]);

    content
}
