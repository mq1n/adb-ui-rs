use std::io::Read;
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use super::device_mgmt::run_device_action;
use super::{adb_command, shell_quote, CommandExt, RemoteFileEntry, CREATE_NO_WINDOW};

const EXPLORER_CMD_TIMEOUT: Duration = Duration::from_secs(20);

/// List files in a remote directory.
pub fn list_remote_dir(serial: &str, path: &str) -> Result<Vec<RemoteFileEntry>, String> {
    let target = list_target_path(path);
    let output = run_direct_capture(serial, &["ls", "-la", &target], EXPLORER_CMD_TIMEOUT)?;

    Ok(parse_ls_la(&output))
}

/// Pull a remote file to a local path.
pub fn pull_remote_file(serial: &str, remote: &str, local: &str) -> (bool, String) {
    run_device_action(serial, &["pull", remote, local])
}

/// Push a local file to a remote path.
pub fn push_remote_file(serial: &str, local: &str, remote: &str) -> (bool, String) {
    run_device_action(serial, &["push", local, remote])
}

/// Delete a remote file or directory.
pub fn delete_remote(serial: &str, path: &str, is_dir: bool) -> (bool, String) {
    if is_dir {
        run_device_action(serial, &["shell", "rm", "-rf", path])
    } else {
        run_device_action(serial, &["shell", "rm", "-f", path])
    }
}

/// Create a remote directory.
pub fn mkdir_remote(serial: &str, path: &str) -> (bool, String) {
    run_device_action(serial, &["shell", "mkdir", "-p", path])
}

/// Read a small text file from device (for preview).
pub fn cat_remote_file(serial: &str, path: &str) -> Result<String, String> {
    run_direct_capture(serial, &["cat", path], EXPLORER_CMD_TIMEOUT)
}

/// Run an arbitrary shell command inside the given working directory.
pub fn run_explorer_command(serial: &str, cwd: &str, command: &str) -> Result<String, String> {
    let shell_cmd = format!("cd {} && {}", shell_quote(cwd), command.trim());
    run_shell_capture(serial, &shell_cmd, EXPLORER_CMD_TIMEOUT)
}

fn run_shell_capture(serial: &str, shell_cmd: &str, timeout: Duration) -> Result<String, String> {
    let mut child = adb_command()
        .ok_or_else(|| "ADB not available".to_string())?
        .args(["-s", serial, "shell", "sh", "-c", shell_cmd])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|error| format!("Failed to spawn adb shell: {error}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture adb stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to capture adb stderr".to_string())?;

    let stdout_handle = thread::spawn(move || {
        let mut stdout = stdout;
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let stderr_handle = thread::spawn(move || {
        let mut stderr = stderr;
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!(
                    "ADB explorer command timed out after {}s",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("Failed to wait for adb explorer command: {error}"));
            }
        }
    }?;

    let stdout = stdout_handle
        .join()
        .map_err(|_| "Explorer stdout reader thread panicked".to_string())?;
    let stderr = stderr_handle
        .join()
        .map_err(|_| "Explorer stderr reader thread panicked".to_string())?;

    let stdout = String::from_utf8_lossy(&stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&stderr).trim().to_string();

    if status.success() {
        Ok(stdout)
    } else if !stderr.is_empty() {
        Err(stderr)
    } else if !stdout.is_empty() {
        Err(stdout)
    } else {
        Err(format!("adb shell exited with status {status}"))
    }
}

fn run_direct_capture(
    serial: &str,
    shell_args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let mut args = vec!["-s", serial, "shell"];
    args.extend_from_slice(shell_args);
    run_adb_capture(&args, timeout)
}

fn run_adb_capture(args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut child = adb_command()
        .ok_or_else(|| "ADB not available".to_string())?
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|error| format!("Failed to spawn adb shell: {error}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture adb stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to capture adb stderr".to_string())?;

    let stdout_handle = thread::spawn(move || {
        let mut stdout = stdout;
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let stderr_handle = thread::spawn(move || {
        let mut stderr = stderr;
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!(
                    "ADB explorer command timed out after {}s",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("Failed to wait for adb explorer command: {error}"));
            }
        }
    }?;

    let stdout = stdout_handle
        .join()
        .map_err(|_| "Explorer stdout reader thread panicked".to_string())?;
    let stderr = stderr_handle
        .join()
        .map_err(|_| "Explorer stderr reader thread panicked".to_string())?;

    let stdout = String::from_utf8_lossy(&stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&stderr).trim().to_string();

    if status.success() {
        Ok(stdout)
    } else if !stderr.is_empty() {
        Err(stderr)
    } else if !stdout.is_empty() {
        Err(stdout)
    } else {
        Err(format!("adb shell exited with status {status}"))
    }
}

fn list_target_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed == "/" {
        "/".to_string()
    } else {
        format!("{}/", trimmed.trim_end_matches('/'))
    }
}

fn parse_ls_la(output: &str) -> Vec<RemoteFileEntry> {
    let mut entries = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("total") {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 8 {
            continue;
        }

        let permissions = parts[0].to_string();
        let (modified, name_start) = if looks_like_iso_ls_timestamp(&parts) {
            (format!("{} {}", parts[5], parts[6]), 7)
        } else if parts.len() >= 9 {
            (format!("{} {} {}", parts[5], parts[6], parts[7]), 8)
        } else {
            continue;
        };

        let raw_name = parts[name_start..].join(" ");
        let name = raw_name
            .split(" -> ")
            .next()
            .unwrap_or(raw_name.as_str())
            .to_string();

        if name == "." || name == ".." {
            continue;
        }

        let is_dir = permissions.starts_with('d');
        let size = if is_dir {
            0
        } else {
            parts[4].parse::<usize>().unwrap_or(0)
        };

        entries.push(RemoteFileEntry {
            name,
            is_dir,
            size,
            modified,
            permissions,
        });
    }

    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    entries
}

fn looks_like_iso_ls_timestamp(parts: &[&str]) -> bool {
    parts.len() >= 8
        && parts[5].contains('-')
        && parts[5].chars().filter(|ch| *ch == '-').count() == 2
        && parts[6].contains(':')
}

#[cfg(test)]
mod tests {
    use super::parse_ls_la;

    #[test]
    fn parse_ls_la_handles_spaces_and_symlinks() {
        let output = "\
drwxr-xr-x 2 root root 4096 Apr 03 12:00 Documents\n\
-rw-r--r-- 1 root root 12 Apr 03 12:00 read me.txt\n\
lrwxrwxrwx 1 root root 10 Apr 03 12:00 latest -> read me.txt\n";

        let entries = parse_ls_la(output);

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "Documents");
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].name, "latest");
        assert_eq!(entries[2].name, "read me.txt");
    }

    #[test]
    fn parse_ls_la_handles_iso_timestamps_from_android() {
        let output = "\
drwxrws--- 2 u0_a61 media_rw 4096 2026-03-23 12:05 Alarms\n\
lrw-r--r-- 1 root root 21 2009-01-01 02:00 sdcard -> /storage/self/primary\n";

        let entries = parse_ls_la(output);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "Alarms");
        assert_eq!(entries[0].modified, "2026-03-23 12:05");
        assert_eq!(entries[1].name, "sdcard");
        assert_eq!(entries[1].modified, "2009-01-01 02:00");
    }
}
