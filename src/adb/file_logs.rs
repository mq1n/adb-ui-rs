use std::collections::HashMap;
use std::fmt::Write as _;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Sender;

use super::{adb_command, AdbMsg, CommandExt, FileEntry, CREATE_NO_WINDOW};

pub fn pull_file_logs(serial: &str, tx: Sender<AdbMsg>, bundle_id: String) {
    let serial = serial.to_string();

    std::thread::spawn(move || {
        let summary = pull_all_files(&serial, &tx, &mut HashMap::new(), &bundle_id);
        if !summary.warnings.is_empty() {
            let _ = tx.send(AdbMsg::DeviceActionResult(
                serial.clone(),
                format!(
                    "File log warnings: {}",
                    summarize_warnings(&summary.warnings)
                ),
            ));
        }
        let _ = tx.send(AdbMsg::FileLogsDone(serial, summary.count));
    });
}

pub fn spawn_file_watcher(
    serial: &str,
    session: u64,
    tx: Sender<AdbMsg>,
    interval: Duration,
    bundle_id: String,
) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let serial = serial.to_string();

    std::thread::spawn(move || {
        let mut known: HashMap<String, KnownFileState> = HashMap::new();
        let mut last_warning = None;

        while !stop2.load(Ordering::Relaxed) {
            let summary = pull_changed_files(&serial, session, &tx, &mut known, &bundle_id);
            let warning = if summary.warnings.is_empty() {
                None
            } else {
                Some(summarize_warnings(&summary.warnings))
            };
            if warning != last_warning {
                if let Some(warning) = &warning {
                    let _ = tx.send(AdbMsg::DeviceActionResult(
                        serial.clone(),
                        format!("File watcher warnings: {warning}"),
                    ));
                }
                last_warning = warning;
            }
            let _ = tx.send(AdbMsg::FileWatchCycle(serial.clone(), session, known.len()));

            let mut waited = Duration::ZERO;
            let tick = Duration::from_millis(200);
            while waited < interval && !stop2.load(Ordering::Relaxed) {
                std::thread::sleep(tick);
                waited += tick;
            }
        }

        let _ = tx.send(AdbMsg::FileWatchStopped(serial, session, "Stopped".into()));
    });

    stop
}

fn pull_all_files(
    serial: &str,
    tx: &Sender<AdbMsg>,
    known: &mut HashMap<String, KnownFileState>,
    bundle_id: &str,
) -> FilePullSummary {
    let metadata = list_all_file_metadata(serial, bundle_id);
    let mut count = 0;
    let mut warnings = metadata.warnings;

    for (key, (entry, source)) in &metadata.entries {
        let content = if source == "internal" {
            cat_internal_log(serial, &entry.name, bundle_id)
        } else {
            cat_external_log(serial, &entry.name, bundle_id)
        };

        match content {
            Ok(content) => {
                known.insert(key.clone(), KnownFileState::from(entry));
                let _ = tx.send(AdbMsg::FileLog(
                    serial.to_string(),
                    FileEntry {
                        key: key.clone(),
                        name: entry.name.clone(),
                        source: source.clone(),
                        size: content.len(),
                        modified: entry.modified.clone(),
                        content,
                    },
                ));
                count += 1;
            }
            Err(error) => {
                warnings.push(format!("{} {} read failed: {error}", source, entry.name));
            }
        }
    }

    FilePullSummary { count, warnings }
}

/// Pull only new or changed files (ls -l size changed). Returns count of updated files.
fn pull_changed_files(
    serial: &str,
    session: u64,
    tx: &Sender<AdbMsg>,
    known: &mut HashMap<String, KnownFileState>,
    bundle_id: &str,
) -> FilePullSummary {
    let metadata = list_all_file_metadata(serial, bundle_id);
    let mut updated = 0;
    let mut warnings = metadata.warnings;

    for (key, (entry, source)) in &metadata.entries {
        let previous = known.get(key);
        if previous == Some(&KnownFileState::from(entry)) {
            continue;
        }

        let content = if source == "internal" {
            cat_internal_log(serial, &entry.name, bundle_id)
        } else {
            cat_external_log(serial, &entry.name, bundle_id)
        };

        match content {
            Ok(content) => {
                known.insert(key.clone(), KnownFileState::from(entry));
                let _ = tx.send(AdbMsg::FileWatchLog(
                    serial.to_string(),
                    session,
                    FileEntry {
                        key: key.clone(),
                        name: entry.name.clone(),
                        source: source.clone(),
                        size: content.len(),
                        modified: entry.modified.clone(),
                        content,
                    },
                ));
                updated += 1;
            }
            Err(error) => {
                warnings.push(format!("{} {} read failed: {error}", source, entry.name));
            }
        }
    }

    known.retain(|k, _| metadata.entries.contains_key(k));
    FilePullSummary {
        count: updated,
        warnings,
    }
}

fn list_all_file_metadata(serial: &str, bundle_id: &str) -> FileMetadataScan {
    let mut result = HashMap::new();
    let mut warnings = Vec::new();

    match list_logs_metadata(serial, "internal", bundle_id) {
        Ok(entries) => {
            for entry in entries {
                let key = format!("internal/{}", entry.name);
                result.insert(key, (entry, "internal".to_string()));
            }
        }
        Err(error) => {
            warnings.push(format!("internal log listing failed: {error}"));
        }
    }

    match list_logs_metadata(serial, "external", bundle_id) {
        Ok(entries) => {
            for entry in entries {
                let key = format!("external/{}", entry.name);
                result.insert(key, (entry, "external".to_string()));
            }
        }
        Err(error) => {
            warnings.push(format!("external log listing failed: {error}"));
        }
    }

    FileMetadataScan {
        entries: result,
        warnings,
    }
}

fn list_logs_metadata(serial: &str, source: &str, bundle_id: &str) -> Result<Vec<LsEntry>, String> {
    let mut cmd = adb_command().ok_or_else(|| "ADB not available".to_string())?;
    let output = if source == "internal" {
        cmd.args([
            "-s",
            serial,
            "shell",
            "run-as",
            bundle_id,
            "ls",
            "-l",
            "files/logs/",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("failed to spawn adb: {error}"))?
    } else {
        let ext_dir = format!("/sdcard/Android/data/{bundle_id}/files/logs");
        cmd.args(["-s", serial, "shell", "ls", "-l", &ext_dir])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|error| format!("failed to spawn adb: {error}"))?
    };

    if !output.status.success() {
        return Err(describe_command_failure(&output));
    }

    Ok(parse_ls_l(&String::from_utf8_lossy(&output.stdout)))
}

#[derive(Debug, Clone)]
struct LsEntry {
    name: String,
    size: usize,
    modified: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KnownFileState {
    size: usize,
    modified: String,
}

impl From<&LsEntry> for KnownFileState {
    fn from(entry: &LsEntry) -> Self {
        Self {
            size: entry.size,
            modified: entry.modified.clone(),
        }
    }
}

fn parse_ls_l(output: &str) -> Vec<LsEntry> {
    let mut results = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("total") || line.contains("No such file") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 9 {
            let fname = parts[8..].join(" ");
            let size = parts[4].parse::<usize>().unwrap_or(0);
            let modified = format!("{} {} {}", parts[5], parts[6], parts[7]);
            results.push(LsEntry {
                name: fname,
                size,
                modified,
            });
        } else if parts.len() >= 2 {
            let fname = parts[parts.len() - 1].to_string();
            results.push(LsEntry {
                name: fname,
                size: 0,
                modified: String::new(),
            });
        }
    }
    results
}

fn cat_internal_log(serial: &str, fname: &str, bundle_id: &str) -> Result<String, String> {
    let path = format!("files/logs/{fname}");
    let output = adb_command()
        .ok_or_else(|| "ADB not available".to_string())?
        .args(["-s", serial, "shell", "run-as", bundle_id, "cat", &path])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("failed to spawn adb: {error}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(describe_command_failure(&output))
    }
}

fn cat_external_log(serial: &str, fname: &str, bundle_id: &str) -> Result<String, String> {
    let path = format!("/sdcard/Android/data/{bundle_id}/files/logs/{fname}");
    let output = adb_command()
        .ok_or_else(|| "ADB not available".to_string())?
        .args(["-s", serial, "shell", "cat", &path])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("failed to spawn adb: {error}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(describe_command_failure(&output))
    }
}

#[derive(Debug, Default)]
struct FilePullSummary {
    count: usize,
    warnings: Vec<String>,
}

#[derive(Debug, Default)]
struct FileMetadataScan {
    entries: HashMap<String, (LsEntry, String)>,
    warnings: Vec<String>,
}

fn summarize_warnings(warnings: &[String]) -> String {
    const MAX_WARNING_LINES: usize = 4;

    let mut summary = warnings
        .iter()
        .take(MAX_WARNING_LINES)
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    if warnings.len() > MAX_WARNING_LINES {
        let _ = write!(
            summary,
            "; ... and {} more",
            warnings.len() - MAX_WARNING_LINES
        );
    }
    summary
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
    use super::parse_ls_l;

    #[test]
    fn parse_ls_output_keeps_names_with_spaces() {
        let entries = parse_ls_l("-rw-rw---- 1 u0_a123 u0_a123 42 Apr 03 12:00 app log.txt");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "app log.txt");
        assert_eq!(entries[0].size, 42);
        assert_eq!(entries[0].modified, "Apr 03 12:00");
    }
}
