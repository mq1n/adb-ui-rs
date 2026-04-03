mod connection;
mod debug;
mod deploy;
mod device_mgmt;
mod devices;
mod emulator;
mod explorer;
mod fastboot;
mod file_logs;
mod logcat;
mod screen;
mod shell;
mod tools;

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;

pub use connection::{
    adb_connect, adb_devices_long, adb_disconnect, adb_disconnect_all, adb_pair,
    is_emulator_serial, is_tcp_device, is_wsa_serial, launch_wsa, open_wsa_settings,
    restart_adb_server,
};
pub use debug::{
    clear_heap_watch_limit, dump_heap_to_file, launch_with_allocation_tracking,
    list_atrace_categories, list_dumpsys_services, run_atrace, run_debug_shell,
    run_simpleperf_record, run_simpleperf_stat, run_strace, set_heap_watch_limit,
};
pub use deploy::{
    check_run_as, crash_logcat, deploy_via_run_as, fix_permissions, get_app_pid, launch_activity,
    launch_app, launch_via_monkey, open_app_settings, pull_logs_to_dir, purge_app, push_directory,
    resolve_launchable_activity,
};
pub use device_mgmt::{get_device_props, run_device_action};
pub use devices::list_devices;
pub use emulator::{
    create_avd, delete_avd, get_running_emulator_map, kill_emulator, list_avds, list_system_images,
    start_emulator,
};
pub use explorer::{
    cat_remote_file, delete_remote, list_remote_dir, mkdir_remote, pull_remote_file,
    push_remote_file, run_explorer_command,
};
pub use fastboot::{flash_partition, list_fastboot_devices};
pub use file_logs::{pull_file_logs, spawn_file_watcher};
pub use logcat::{fetch_log_snapshot, spawn_log_watcher, spawn_logcat};
pub use screen::{capture_screenshot_bytes, start_screen_record};
pub use shell::{spawn_shell, ShellHandle};
pub use tools::{bugreport, run_monkey, uiautomator_dump};

/// Windows `CREATE_NO_WINDOW` flag — prevents console popups from spawned processes.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Resolved ADB path (can be re-set by user).
static ADB_PATH: Mutex<Option<Result<PathBuf, String>>> = Mutex::new(None);

fn resolve_adb() -> Result<PathBuf, String> {
    if command_available("adb", "version") {
        return Ok(PathBuf::from("adb"));
    }

    for root in sdk_root_candidates() {
        let candidate = root.join("platform-tools").join(adb_binary_name());
        if candidate.exists() && command_available(&candidate, "version") {
            return Ok(candidate);
        }
    }

    Err(format!(
        "adb not found on PATH or in common Android SDK locations. Checked: {}",
        sdk_root_candidates()
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

pub fn adb_path() -> Result<PathBuf, String> {
    let mut guard = ADB_PATH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if guard.is_none() {
        *guard = Some(resolve_adb());
    }
    (*guard).as_ref().map_or_else(
        || Err("ADB path cache was not initialized".to_string()),
        Clone::clone,
    )
}

pub fn set_adb_path(path: &str) -> Result<(), String> {
    let p = PathBuf::from(path);
    if !p.exists() {
        return Err(format!("File does not exist: {path}"));
    }

    let probe = Command::new(&p)
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();

    match probe {
        Ok(s) if s.success() => {
            *ADB_PATH
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Ok(p));
            Ok(())
        }
        Ok(_) => Err(format!(
            "'{path}' exited with error — is it a valid adb binary?",
        )),
        Err(e) => Err(format!("Failed to run '{path}': {e}")),
    }
}

fn adb_command() -> Option<Command> {
    adb_path().ok().map(Command::new)
}

const fn adb_binary_name() -> &'static str {
    #[cfg(windows)]
    {
        "adb.exe"
    }

    #[cfg(not(windows))]
    {
        "adb"
    }
}

fn command_available(program: impl AsRef<std::ffi::OsStr>, version_arg: &str) -> bool {
    Command::new(program)
        .arg(version_arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .is_ok_and(|status| status.success())
}

pub fn sdk_root_candidates() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();

    for env_var in ["ANDROID_SDK_ROOT", "ANDROID_HOME"] {
        if let Some(path) = std::env::var_os(env_var).filter(|value| !value.is_empty()) {
            push_unique_path(&mut roots, &mut seen, PathBuf::from(path));
        }
    }

    #[cfg(windows)]
    {
        if let Some(path) = std::env::var_os("LOCALAPPDATA") {
            push_unique_path(
                &mut roots,
                &mut seen,
                PathBuf::from(path).join("Android/Sdk"),
            );
        }
        if let Some(path) = std::env::var_os("USERPROFILE") {
            push_unique_path(
                &mut roots,
                &mut seen,
                PathBuf::from(path).join("AppData/Local/Android/Sdk"),
            );
        }
        push_unique_path(&mut roots, &mut seen, PathBuf::from("C:/Android/Sdk"));
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(path) = std::env::var_os("HOME") {
            push_unique_path(
                &mut roots,
                &mut seen,
                PathBuf::from(path).join("Library/Android/sdk"),
            );
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(path) = std::env::var_os("HOME") {
            let home = PathBuf::from(path);
            push_unique_path(&mut roots, &mut seen, home.join("Android/Sdk"));
            push_unique_path(&mut roots, &mut seen, home.join("Android/sdk"));
        }
    }

    roots
}

fn push_unique_path(paths: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>, candidate: PathBuf) {
    if seen.insert(candidate.clone()) {
        paths.push(candidate);
    }
}

// ─── Shell safety ───────────────────────────────────────────────────────────

/// Sanitize a string for safe embedding in an `adb shell` command.
/// Allows: alphanumeric, `.`, `_`, `-`, `/`, `+`, `:`, `@`
/// Strips everything else to prevent shell injection.
pub fn sanitize_shell_arg(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | '+' | ':' | '@'))
        .collect()
}

/// Shell-quote a value for safe embedding in a POSIX shell command.
pub fn shell_quote(s: &str) -> String {
    let escaped = s.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

/// Normalize a remote relative path and reject traversal or absolute paths.
pub fn sanitize_relative_remote_path(path: &str) -> Option<String> {
    let path = path.trim().replace('\\', "/");
    if path.is_empty() || path.starts_with('/') {
        return None;
    }

    let mut parts = Vec::new();
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        if matches!(segment, "." | "..") {
            return None;
        }
        let sanitized = sanitize_shell_arg(segment);
        if sanitized != segment {
            return None;
        }
        parts.push(sanitized);
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Validate that a string looks like a numeric PID.
pub fn validate_pid(s: &str) -> Result<&str, &'static str> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        Err("PID is required")
    } else if trimmed.chars().all(|c| c.is_ascii_digit()) {
        Ok(trimmed)
    } else {
        Err("PID must contain only digits")
    }
}

// ─── Messages ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum AdbMsg {
    LogcatLine(String, u64, String),
    LogcatStopped(String, u64, String),
    DevicesUpdated(Vec<DeviceInfo>),
    AdbNotFound(String),
    FileLog(String, FileEntry),
    FileLogsDone(String, usize),
    FileWatchLog(String, u64, FileEntry),
    FileWatchCycle(String, u64, usize),
    FileWatchStopped(String, u64, String),
    ShellOutput(String, String), // (serial, line)
    ShellExited(String, String), // (serial, reason)
    DeviceProps(String, Vec<(String, String)>),
    DeviceActionResult(String, String),
    ResolvedLaunchActivity(String, String),
    ExplorerListing(String, u64, String, Vec<RemoteFileEntry>),
    ExplorerError(String, u64, String),
    ExplorerPreview(String, u64, u64, Result<String, String>),
    ExplorerReloadIfCurrent(String, String),
    ExplorerCommandResult(String, u64, ExplorerCommandReport),
    ExplorerCommandStopped(String, u64, String),
    ScreenshotReady(String, Vec<u8>, String),
    ScreenshotError(String, String),
    AvdList(Vec<String>),
    SystemImageList(Vec<String>),
    /// Monkey test finished: (serial, success, output).
    MonkeyDone(String, bool, String),
    /// `UIAutomator` dump result: (serial, `Ok(xml)` or `Err(msg)`).
    UiDump(String, Result<String, String>),
    /// Bugreport finished: (serial, success, message).
    BugreportDone(String, bool, String),
    /// Snapshot log buffer fetched: (serial, `source_index`, result).
    LogBuffer(String, u8, Result<Vec<String>, String>),
    /// Maps running emulator serial -> AVD name.
    RunningEmuMap(HashMap<String, String>),
    /// Debug command result: (serial, `category_index`, `Ok(output)` or `Err(msg)`).
    DebugOutput(String, u8, Result<String, String>),
    /// Dumpsys service list: (serial, services).
    DumpsysServiceList(String, Vec<String>),
    /// Atrace available categories: (serial, categories).
    AtraceCategories(String, Vec<String>),
    /// Per-device run-as availability check: (serial, `bundle_id`, available).
    RunAsAvailability(String, String, bool),
    /// Monitor command result: (serial, `category_index`, `Ok(output)` or `Err(msg)`).
    MonitorOutput(String, u8, Result<String, String>),
    /// Deploy progress/result: (serial, label, `Ok(msg)` or `Err(msg)`).
    DeployResult(String, String, Result<String, String>),
    /// Crash logcat result: (serial, `Ok(text)` or `Err(msg)`).
    CrashLogcat(String, Result<String, String>),
    /// Pull logs to folder result: (serial, `Ok(count)` or `Err(msg)`).
    PullLogsResult(String, Result<usize, String>),
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub key: String,
    pub name: String,
    pub source: String,
    pub size: usize,
    pub modified: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub serial: String,
    pub state: String,
    pub model: String,
}

/// A single entry from `ls -la` on the device.
#[derive(Debug, Clone)]
pub struct RemoteFileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: usize,
    pub modified: String,
    pub permissions: String,
}

#[derive(Debug, Clone)]
pub struct ExplorerCommandReport {
    pub cwd: String,
    pub command: String,
    pub duration_ms: u64,
    pub output: String,
    pub success: bool,
    pub timed_out: bool,
    pub follow_poll: bool,
}

// ─── Platform ────────────────────────────────────────────────────────────────

trait CommandExt {
    fn creation_flags(&mut self, flags: u32) -> &mut Self;
}

#[cfg(windows)]
impl CommandExt for Command {
    fn creation_flags(&mut self, flags: u32) -> &mut Self {
        use std::os::windows::process::CommandExt as WinCmdExt;
        WinCmdExt::creation_flags(self, flags);
        self
    }
}

#[cfg(not(windows))]
impl CommandExt for Command {
    fn creation_flags(&mut self, _flags: u32) -> &mut Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{sanitize_relative_remote_path, shell_quote};

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("ab'cd"), "'ab'\"'\"'cd'");
    }

    #[test]
    fn relative_remote_path_rejects_traversal() {
        assert_eq!(sanitize_relative_remote_path("../pack"), None);
        assert_eq!(sanitize_relative_remote_path("/pack"), None);
        assert_eq!(
            sanitize_relative_remote_path("pack/assets"),
            Some("pack/assets".to_string())
        );
    }
}
