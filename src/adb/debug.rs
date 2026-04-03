use super::{adb_command, adb_path, CommandExt, CREATE_NO_WINDOW};
use std::process::Stdio;

/// Run a debug shell command and return stdout. Blocking.
pub fn run_debug_shell(serial: &str, shell_cmd: &str) -> Result<String, String> {
    let mut cmd = adb_command().ok_or_else(|| adb_path().unwrap_err())?;
    let output = cmd
        .args(["-s", serial, "shell", shell_cmd])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed to run adb: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() && stdout.trim().is_empty() {
        let detail = if stderr.trim().is_empty() {
            format!("exit code: {}", output.status)
        } else {
            stderr.trim().to_string()
        };
        return Err(detail);
    }

    // Merge stderr if present.
    let mut result = stdout;
    if !stderr.trim().is_empty() {
        result.push_str("\n--- stderr ---\n");
        result.push_str(stderr.trim());
    }
    Ok(result)
}

/// List available dumpsys services. Blocking.
pub fn list_dumpsys_services(serial: &str) -> Result<Vec<String>, String> {
    let output = run_debug_shell(serial, "dumpsys -l")?;
    Ok(output
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.contains("Currently running services:"))
        .collect())
}

/// List available atrace categories. Blocking.
pub fn list_atrace_categories(serial: &str) -> Result<Vec<String>, String> {
    let output = run_debug_shell(
        serial,
        "atrace --list_categories 2>/dev/null || echo 'atrace not available'",
    )?;
    Ok(output
        .lines()
        .map(|l| {
            // Format: "  gfx - Graphics"
            l.split_whitespace().next().unwrap_or("").to_string()
        })
        .filter(|l| !l.is_empty() && !l.contains("atrace"))
        .collect())
}

/// Run atrace capture. Blocking — returns trace text output.
pub fn run_atrace(
    serial: &str,
    categories: &[String],
    duration_secs: u32,
) -> Result<String, String> {
    let cats = if categories.is_empty() {
        "sched freq idle am wm gfx view".to_string()
    } else {
        categories.join(" ")
    };
    let cats = super::sanitize_shell_arg(&cats);
    let cmd = format!("atrace -t {duration_secs} {cats} 2>&1");
    run_debug_shell(serial, &cmd)
}

/// Run simpleperf stat. Blocking.
pub fn run_simpleperf_stat(
    serial: &str,
    pid: Option<&str>,
    duration_secs: u32,
    event: &str,
) -> Result<String, String> {
    let target = match pid {
        Some(p) if !p.is_empty() => {
            let p = super::sanitize_shell_arg(p);
            format!("-p {p}")
        }
        _ => "-a".to_string(),
    };
    let event = super::sanitize_shell_arg(event);
    let cmd = format!(
        "simpleperf stat {target} -e {event} --duration {duration_secs} 2>&1 || \
         echo 'simpleperf not available (needs API 28+)'"
    );
    run_debug_shell(serial, &cmd)
}

/// Run simpleperf record + report. Blocking.
pub fn run_simpleperf_record(
    serial: &str,
    pid: Option<&str>,
    duration_secs: u32,
    event: &str,
) -> Result<String, String> {
    let target = match pid {
        Some(p) if !p.is_empty() => {
            let p = super::sanitize_shell_arg(p);
            format!("-p {p}")
        }
        _ => "-a".to_string(),
    };
    let event = super::sanitize_shell_arg(event);
    let cmd = format!(
        "cd /data/local/tmp && \
         simpleperf record {target} -e {event} --duration {duration_secs} -o perf.data 2>&1 && \
         simpleperf report -i perf.data --sort comm,dso,symbol 2>&1; \
         rm -f perf.data"
    );
    run_debug_shell(serial, &cmd)
}

/// Run strace on a PID. Blocking.
pub fn run_strace(serial: &str, pid: &str, duration_secs: u32) -> Result<String, String> {
    let pid = super::sanitize_shell_arg(pid);
    let cmd = format!(
        "timeout {duration_secs} strace -p {pid} -c 2>&1 || \
         echo 'strace ended or not available (needs root)'"
    );
    run_debug_shell(serial, &cmd)
}
