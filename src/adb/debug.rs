use super::{adb_command, adb_path, CommandExt, CREATE_NO_WINDOW};
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

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

/// Launch an app with allocation tracking enabled.
pub fn launch_with_allocation_tracking(
    serial: &str,
    bundle_id: &str,
    activity: &str,
) -> Result<String, String> {
    let bundle_id = bundle_id.trim();
    if bundle_id.is_empty() {
        return Err("Bundle ID is required".into());
    }

    let command = if activity.trim().is_empty() {
        let component = super::resolve_launchable_activity(serial, bundle_id)?;
        format!(
            "am start -S -W --track-allocation -n {}",
            super::shell_quote(&component)
        )
    } else {
        let component = build_activity_component(bundle_id, activity)?;
        format!(
            "am start -S -W --track-allocation -n {}",
            super::shell_quote(&component)
        )
    };

    run_debug_shell(serial, &command)
}

/// Set the heap watch limit for a process or package.
pub fn set_heap_watch_limit(
    serial: &str,
    process: &str,
    limit_bytes: u64,
) -> Result<String, String> {
    let process = process.trim();
    if process.is_empty() {
        return Err("Process or package name is required".into());
    }

    let command = format!(
        "am set-watch-heap {} {}",
        super::shell_quote(process),
        limit_bytes
    );
    run_debug_shell(serial, &command)
}

/// Clear a previously configured heap watch.
pub fn clear_heap_watch_limit(serial: &str, process: &str) -> Result<String, String> {
    let process = process.trim();
    if process.is_empty() {
        return Err("Process or package name is required".into());
    }

    let command = format!(
        "am clear-watch-heap {} 2>/dev/null || am clear-watch-heap",
        super::shell_quote(process)
    );
    run_debug_shell(serial, &command)
}

/// Dump a managed or native heap and pull it to a local path.
pub fn dump_heap_to_file(
    serial: &str,
    process: &str,
    local_path: &str,
    native: bool,
) -> Result<String, String> {
    let process = process.trim();
    if process.is_empty() {
        return Err("Process or package name is required".into());
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let safe_process = super::sanitize_shell_arg(process);
    let remote_path = format!("/data/local/tmp/{safe_process}_{timestamp}_heap.hprof");

    let mut args = vec![
        "-s".to_string(),
        serial.to_string(),
        "shell".to_string(),
        "am".to_string(),
        "dumpheap".to_string(),
    ];
    if native {
        args.push("-n".to_string());
    }
    args.push(process.to_string());
    args.push(remote_path.clone());

    let dump_output = adb_command()
        .ok_or_else(|| adb_path().unwrap_err())?
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("Failed to run adb dumpheap: {error}"))?;

    if !dump_output.status.success() {
        let stderr = String::from_utf8_lossy(&dump_output.stderr)
            .trim()
            .to_string();
        let stdout = String::from_utf8_lossy(&dump_output.stdout)
            .trim()
            .to_string();
        return Err(if stderr.is_empty() { stdout } else { stderr });
    }

    let pull_output = adb_command()
        .ok_or_else(|| adb_path().unwrap_err())?
        .args(["-s", serial, "pull", &remote_path, local_path])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("Failed to pull heap dump: {error}"))?;

    let _ = adb_command()
        .ok_or_else(|| adb_path().unwrap_err())?
        .args(["-s", serial, "shell", "rm", "-f", &remote_path])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();

    if !pull_output.status.success() {
        let stderr = String::from_utf8_lossy(&pull_output.stderr)
            .trim()
            .to_string();
        let stdout = String::from_utf8_lossy(&pull_output.stdout)
            .trim()
            .to_string();
        return Err(if stderr.is_empty() { stdout } else { stderr });
    }

    let heap_kind = if native { "Native" } else { "Managed" };
    Ok(format!(
        "{heap_kind} heap dump saved to {local_path}\nSource process: {process}"
    ))
}

fn build_activity_component(bundle_id: &str, activity: &str) -> Result<String, String> {
    let bundle_id = bundle_id.trim();
    let activity = activity.trim();
    if bundle_id.is_empty() || activity.is_empty() {
        return Err("Bundle ID and activity must not be empty".into());
    }

    if activity.contains('/') {
        Ok(activity.to_string())
    } else {
        Ok(format!("{bundle_id}/{activity}"))
    }
}
