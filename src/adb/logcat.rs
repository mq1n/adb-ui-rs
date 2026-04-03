use std::io::{BufRead, BufReader};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Sender;

use super::{adb_command, adb_path, AdbMsg, CommandExt, CREATE_NO_WINDOW};
use crate::config::AppConfig;

pub fn spawn_logcat(
    serial: &str,
    session: u64,
    tx: Sender<AdbMsg>,
    config: &AppConfig,
) -> Option<Child> {
    let serial_owned = serial.to_string();

    let Some(mut cmd) = adb_command() else {
        let err = adb_path().unwrap_err();
        let _ = tx.send(AdbMsg::LogcatStopped(
            serial_owned,
            session,
            format!("adb not available: {err}"),
        ));
        return None;
    };

    let filter_args = config.logcat_filter_args();
    cmd.args(["-s", serial, "logcat", "-v", "threadtime"]);
    for f in &filter_args {
        cmd.arg(f);
    }
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(AdbMsg::LogcatStopped(
                serial_owned,
                session,
                format!("Failed to spawn adb: {e}"),
            ));
            return None;
        }
    };

    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = tx.send(AdbMsg::LogcatStopped(
            serial_owned,
            session,
            "Failed to capture stdout".into(),
        ));
        return None;
    };
    let tx2 = tx.clone();
    let serial2 = serial_owned.clone();

    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::with_capacity(4096);
        let mut line_count: u64 = 0;
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => {
                    let _ = tx2.send(AdbMsg::LogcatLine(
                        serial2.clone(),
                        session,
                        format!("[adb-ui] logcat EOF after {line_count} lines"),
                    ));
                    break;
                }
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf)
                        .trim_end_matches(['\n', '\r'])
                        .to_string();
                    line_count += 1;
                    if tx2
                        .send(AdbMsg::LogcatLine(serial2.clone(), session, line))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx2.send(AdbMsg::LogcatLine(
                        serial2.clone(),
                        session,
                        format!("[adb-ui] logcat read error after {line_count} lines: {e}"),
                    ));
                    break;
                }
            }
        }
        let _ = tx.send(AdbMsg::LogcatStopped(
            serial_owned,
            session,
            format!("Process ended ({line_count} lines read)"),
        ));
    });

    Some(child)
}

/// Fetch a snapshot log. Returns Ok(lines) or Err(message). Blocking.
pub fn fetch_log_snapshot(serial: &str, source_idx: u8) -> Result<Vec<String>, String> {
    use crate::device::LogSource;

    let source = LogSource::from_index(source_idx).unwrap_or(LogSource::Logcat);

    let shell_cmd = match source {
        LogSource::Logcat => return Err("Logcat is a live stream, not a snapshot".into()),
        LogSource::MainUnfiltered => "logcat -b main -d".to_string(),
        LogSource::Kernel => concat!(
            "KLOG=$(logcat -b kernel -d 2>/dev/null); ",
            "if [ -n \"$KLOG\" ]; then echo \"$KLOG\"; else ",
            "dmesg 2>/dev/null || ",
            "cat /proc/last_kmsg 2>/dev/null || ",
            "cat /sys/fs/pstore/console-ramoops 2>/dev/null || ",
            "echo '--- Kernel log not available without root on this device ---'; ",
            "echo '--- Showing kernel-related logcat entries instead ---'; ",
            "logcat -b system -b main -d | grep -iE 'kernel|panic|oops|watchdog|thermal|lowmem'; ",
            "fi"
        )
        .to_string(),
        LogSource::CrashLog => "logcat -b crash -d".to_string(),
        LogSource::EventLog => "logcat -b events -d".to_string(),
        LogSource::RadioLog => "logcat -b radio -d".to_string(),
        LogSource::SystemLog => "logcat -b system -d".to_string(),
        LogSource::SecurityLog => {
            "logcat -b security -d 2>/dev/null || echo 'Security log buffer not available'"
                .to_string()
        }
        LogSource::StatsLog => {
            "logcat -b stats -d 2>/dev/null || echo 'Stats log buffer not available (API 29+)'"
                .to_string()
        }
        LogSource::AllCombined => "logcat -b all -d".to_string(),
    };

    let mut cmd = adb_command().ok_or_else(|| adb_path().unwrap_err())?;

    let output = cmd
        .args(["-s", serial, "shell", &shell_cmd])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed to run adb: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else if !stdout.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            format!("exit code: {}", output.status)
        };
        return Err(format!("{} failed: {detail}", source.label()));
    }

    let lines: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(std::string::ToString::to_string)
        .collect();

    Ok(lines)
}

/// Spawn a periodic watcher that re-fetches a snapshot log source every `interval`.
/// Returns a stop flag.
pub fn spawn_log_watcher(
    serial: &str,
    source_idx: u8,
    tx: Sender<AdbMsg>,
    interval: Duration,
) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let serial = serial.to_string();

    std::thread::spawn(move || {
        while !stop2.load(Ordering::Relaxed) {
            let result = fetch_log_snapshot(&serial, source_idx);
            let _ = tx.send(AdbMsg::LogBuffer(serial.clone(), source_idx, result));

            // Interruptible sleep.
            let mut waited = Duration::ZERO;
            let tick = Duration::from_millis(200);
            while waited < interval && !stop2.load(Ordering::Relaxed) {
                std::thread::sleep(tick);
                waited += tick;
            }
        }
    });

    stop
}
