use std::fmt::Write;
use std::path::Path;
use std::process::Stdio;

use super::device_mgmt::run_device_action;
use super::{adb_command, adb_path, CommandExt, CREATE_NO_WINDOW};

pub struct PullLogsSummary {
    pub count: usize,
    pub warnings: Vec<String>,
}

/// Launch an app by activity class: `am start -n <bundle_id>/<activity>`.
pub fn launch_activity(serial: &str, bundle_id: &str, activity: &str) -> (bool, String) {
    let component = match build_activity_component(bundle_id, activity) {
        Ok(component) => component,
        Err(error) => return (false, error),
    };
    run_device_action(serial, &["shell", "am", "start", "-n", &component])
}

/// Launch an app via monkey (fallback when no activity class is configured).
pub fn launch_via_monkey(serial: &str, bundle_id: &str) -> (bool, String) {
    let bundle_id = bundle_id.trim();
    run_device_action(
        serial,
        &[
            "shell",
            "monkey",
            "-p",
            bundle_id,
            "-c",
            "android.intent.category.LAUNCHER",
            "1",
        ],
    )
}

/// Force-stop the app.
pub fn force_stop(serial: &str, bundle_id: &str) -> (bool, String) {
    run_device_action(serial, &["shell", "am", "force-stop", bundle_id.trim()])
}

/// Open the Android Settings page for the app.
pub fn open_app_settings(serial: &str, bundle_id: &str) -> (bool, String) {
    let uri = format!("package:{}", bundle_id.trim());
    run_device_action(
        serial,
        &[
            "shell",
            "am",
            "start",
            "-a",
            "android.settings.APPLICATION_DETAILS_SETTINGS",
            "-d",
            &uri,
        ],
    )
}

/// Purge app: force-stop + uninstall + remove all leftover data.
pub fn purge_app(serial: &str, bundle_id: &str) -> (bool, String) {
    let bundle_id = bundle_id.trim();
    let mut log = String::new();

    // 1. Force-stop
    let (_, msg) = force_stop(serial, bundle_id);
    let _ = writeln!(log, "Force stop: {}", msg.trim());

    // 2. Uninstall
    let (ok, msg) = run_device_action(serial, &["uninstall", bundle_id]);
    if ok {
        log.push_str("Uninstall: OK\n");
    } else {
        let _ = writeln!(log, "Uninstall: {} (may not be installed)", msg.trim());
    }

    // 3. Remove leftover data directories
    let paths = [
        format!("/data/data/{bundle_id}"),
        format!("/sdcard/Android/data/{bundle_id}"),
    ];
    for rpath in &paths {
        let (ok, _) = run_device_action(serial, &["shell", "rm", "-rf", rpath]);
        if ok {
            let _ = writeln!(log, "Removed {rpath}");
        }
    }

    log.push_str("Purge complete");
    (true, log)
}

/// Check if `run-as <bundle_id>` is available (app is debuggable and installed).
pub fn check_run_as(serial: &str, bundle_id: &str) -> bool {
    let output = adb_command().and_then(|mut c| {
        c.args(["-s", serial, "shell", "run-as", bundle_id.trim(), "pwd"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
    });
    output.is_some_and(|o| o.status.success())
}

/// Get the PID of a running app (pidof with ps fallback).
pub fn get_app_pid(serial: &str, bundle_id: &str) -> Option<String> {
    let bundle_id = bundle_id.trim();
    // Try pidof first
    let output = adb_command().and_then(|mut c| {
        c.args(["-s", serial, "shell", "pidof", "-s", bundle_id])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
    });
    if let Some(o) = &output {
        if o.status.success() {
            let pid = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !pid.is_empty() {
                return Some(pid);
            }
        }
    }
    // Fallback: grep ps output
    let output = adb_command().and_then(|mut c| {
        c.args(["-s", serial, "shell", "ps", "-A"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
    });
    if let Some(o) = output {
        let stdout = String::from_utf8_lossy(&o.stdout);
        for line in stdout.lines() {
            if line.contains(bundle_id) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    return Some(parts[1].to_string());
                }
            }
        }
    }
    None
}

/// Push a local directory to a remote path on the device.
pub fn push_directory(serial: &str, local_path: &str, remote_path: &str) -> (bool, String) {
    run_device_action(serial, &["push", local_path, remote_path])
}

/// Fix remote directory permissions (chmod -R 0755) so the app can read pushed files.
pub fn fix_permissions(serial: &str, remote_path: &str) -> (bool, String) {
    run_device_action(serial, &["shell", "chmod", "-R", "0755", remote_path])
}

/// Deploy a local directory into app-internal storage via run-as staging.
/// Stages to /data/local/tmp, then copies into the app's files directory via run-as.
pub fn deploy_via_run_as(
    serial: &str,
    local_path: &str,
    remote_suffix: &str,
    bundle_id: &str,
) -> (bool, String) {
    let Some(remote_suffix) = super::sanitize_relative_remote_path(remote_suffix) else {
        return (
            false,
            "Remote suffix must be a relative path without traversal".into(),
        );
    };
    let stage_root = "/data/local/tmp/_adb_ui_stage";
    let stage_dir = format!("{stage_root}/{remote_suffix}");
    let target_dir = format!("files/{remote_suffix}");
    let mut log = String::new();

    // Clean + prepare staging
    let _ = run_device_action(serial, &["shell", "rm", "-rf", &stage_dir]);
    let stage_parent = stage_dir.rsplit_once('/').map_or(&*stage_dir, |(p, _)| p);
    let (ok, _) = run_device_action(serial, &["shell", "mkdir", "-p", stage_parent]);
    if !ok {
        return (false, "Failed to prepare staging directory".into());
    }

    // Push to staging
    log.push_str("Pushing to staging...\n");
    let (ok, msg) = push_directory(serial, local_path, &stage_dir);
    if !ok {
        return (false, format!("Push to staging failed: {msg}"));
    }
    let _ = writeln!(log, "Staged: {msg}");

    // chmod staging
    let _ = run_device_action(serial, &["shell", "chmod", "-R", "0755", &stage_dir]);

    // Copy into app-internal via run-as
    let copy_cmd = format!(
        "mkdir -p {target} && cp -R {stage}/. {target}",
        target = super::shell_quote(&target_dir),
        stage = super::shell_quote(&stage_dir),
    );
    log.push_str("Copying to app-internal...\n");
    let (ok, msg) = run_device_action(
        serial,
        &["shell", "run-as", bundle_id.trim(), "sh", "-c", &copy_cmd],
    );
    if !ok {
        // Cleanup staging
        let _ = run_device_action(serial, &["shell", "rm", "-rf", &stage_dir]);
        return (false, format!("run-as copy failed: {msg}"));
    }

    // Cleanup staging
    let _ = run_device_action(serial, &["shell", "rm", "-rf", &stage_dir]);
    log.push_str("Deployed to app-internal storage");
    (true, log)
}

/// Fetch crash-only logcat entries. Blocking.
pub fn crash_logcat(serial: &str) -> Result<String, String> {
    let mut cmd = adb_command().ok_or_else(|| adb_path().unwrap_err())?;
    let output = cmd
        .args([
            "-s",
            serial,
            "logcat",
            "-d",
            "AndroidRuntime:E",
            "libc:F",
            "DEBUG:V",
            "*:F",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed to run adb: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if stdout.trim().is_empty() {
        Ok("No crash entries found in current logcat buffer.".into())
    } else {
        // Return last 200 lines
        let lines: Vec<&str> = stdout.lines().collect();
        let start = lines.len().saturating_sub(200);
        Ok(lines[start..].join("\n"))
    }
}

/// Pull app logs (internal + external) to a local directory.
pub fn pull_logs_to_dir(
    serial: &str,
    bundle_id: &str,
    dest_dir: &Path,
) -> Result<PullLogsSummary, String> {
    std::fs::create_dir_all(dest_dir)
        .map_err(|e| format!("Failed to create {}: {e}", dest_dir.display()))?;

    let mut count = 0;
    let mut warnings = Vec::new();
    let bundle_id = bundle_id.trim();

    // Internal logs via run-as cat
    let int_dir = dest_dir.join("internal");
    std::fs::create_dir_all(&int_dir)
        .map_err(|e| format!("Failed to create {}: {e}", int_dir.display()))?;
    match adb_command()
        .ok_or_else(|| "ADB not available".to_string())?
        .args([
            "-s",
            serial,
            "shell",
            "run-as",
            bundle_id,
            "ls",
            "files/logs/",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            for fname in stdout.lines() {
                let fname = fname.trim();
                if fname.is_empty() {
                    continue;
                }
                match adb_command()
                    .ok_or_else(|| "ADB not available".to_string())?
                    .args([
                        "-s",
                        serial,
                        "shell",
                        "run-as",
                        bundle_id,
                        "cat",
                        &format!("files/logs/{fname}"),
                    ])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .creation_flags(CREATE_NO_WINDOW)
                    .output()
                {
                    Ok(co) if co.status.success() && !co.stdout.is_empty() => {
                        let path = int_dir.join(safe_local_filename(fname));
                        std::fs::write(&path, &co.stdout)
                            .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
                        count += 1;
                    }
                    Ok(co) if co.status.success() => {}
                    Ok(co) => warnings.push(format!(
                        "internal {} read failed: {}",
                        fname,
                        describe_command_failure(&co)
                    )),
                    Err(error) => warnings.push(format!(
                        "internal {fname} read failed: failed to spawn adb: {error}"
                    )),
                }
            }
        }
        Ok(o) => warnings.push(format!(
            "internal log listing failed: {}",
            describe_command_failure(&o)
        )),
        Err(error) => warnings.push(format!("internal log listing failed: {error}")),
    }

    // External logs via adb pull
    let ext_dir = dest_dir.join("external");
    std::fs::create_dir_all(&ext_dir)
        .map_err(|e| format!("Failed to create {}: {e}", ext_dir.display()))?;
    let ext_remote = format!("/sdcard/Android/data/{bundle_id}/files/logs/.");
    let (ok, msg) = run_device_action(
        serial,
        &["pull", &ext_remote, &ext_dir.display().to_string()],
    );
    if ok {
        count += count_files_recursive(&ext_dir);
    } else {
        warnings.push(format!("external log pull failed: {msg}"));
    }

    // Clean empty dirs
    for dir in [&int_dir, &ext_dir] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            if entries.count() == 0 {
                let _ = std::fs::remove_dir(dir);
            }
        }
    }

    if count == 0 && !warnings.is_empty() {
        return Err(warnings.join("; "));
    }

    Ok(PullLogsSummary { count, warnings })
}

fn build_activity_component(bundle_id: &str, activity: &str) -> Result<String, String> {
    let bundle_id = bundle_id.trim();
    let activity = activity.trim();
    if bundle_id.is_empty() || activity.is_empty() {
        return Err("Bundle ID and activity must not be empty".into());
    }

    if activity.contains('/') {
        return Ok(activity.to_string());
    }

    Ok(format!("{bundle_id}/{activity}"))
}

fn safe_local_filename(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => ch,
        })
        .collect::<String>();

    if sanitized.trim().is_empty() {
        "log.txt".to_string()
    } else {
        sanitized
    }
}

fn count_files_recursive(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .map(|entry| {
                let path = entry.path();
                if path.is_file() {
                    1
                } else if path.is_dir() {
                    count_files_recursive(&path)
                } else {
                    0
                }
            })
            .sum()
    })
}

fn describe_command_failure(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("exit code: {}", output.status)
    }
}

#[cfg(test)]
mod tests {
    use super::build_activity_component;

    #[test]
    fn build_activity_component_accepts_short_activity_name() {
        let component = build_activity_component("com.example.app", ".MainActivity").unwrap();
        assert_eq!(component, "com.example.app/.MainActivity");
    }

    #[test]
    fn build_activity_component_accepts_full_component() {
        let component =
            build_activity_component("com.example.app", "com.example.app/.MainActivity").unwrap();
        assert_eq!(component, "com.example.app/.MainActivity");
    }
}
