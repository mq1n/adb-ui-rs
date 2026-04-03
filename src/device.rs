// This module centralizes per-device UI state, including fields wired in gradually across tabs.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use eframe::egui;

use crate::adb::{DeviceInfo, FileEntry, RemoteFileEntry};

const MAX_LOGCAT_LINES: usize = 50_000;
const MAX_SHELL_LINES: usize = 10_000;
const MAX_LOG_BUFFER_LINES: usize = 100_000;
const MAX_ACTION_LOG_LINES: usize = 2_000;
const MAX_EXPLORER_COMMAND_BYTES: usize = 2 * 1024 * 1024;
const MAX_EXPLORER_LOG_LINES: usize = 1_000;
/// Lines rendered per page in the log viewer.
pub const LOG_PAGE_SIZE: usize = 5_000;

/// Available log sources in the Logs tab sidebar.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogSource {
    Logcat,
    MainUnfiltered,
    Kernel,
    CrashLog,
    EventLog,
    RadioLog,
    SystemLog,
    SecurityLog,
    StatsLog,
    AllCombined,
}

impl LogSource {
    pub const ALL: &[Self] = &[
        Self::Logcat,
        Self::MainUnfiltered,
        Self::Kernel,
        Self::CrashLog,
        Self::EventLog,
        Self::RadioLog,
        Self::SystemLog,
        Self::SecurityLog,
        Self::StatsLog,
        Self::AllCombined,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Logcat => "Logcat",
            Self::MainUnfiltered => "Main (all)",
            Self::Kernel => "Kernel (dmesg)",
            Self::CrashLog => "Crash Log",
            Self::EventLog => "Event Log",
            Self::RadioLog => "Radio Log",
            Self::SystemLog => "System Log",
            Self::SecurityLog => "Security Log",
            Self::StatsLog => "Stats Log",
            Self::AllCombined => "All Combined",
        }
    }

    pub const fn is_live(self) -> bool {
        matches!(self, Self::Logcat)
    }

    pub const fn index(self) -> u8 {
        self as u8
    }

    pub fn from_index(idx: u8) -> Option<Self> {
        Self::ALL.get(usize::from(idx)).copied()
    }
}

pub const MAX_SCREEN_CAPTURES: usize = 50;
const MAX_DEBUG_OUTPUT_BYTES: usize = 2 * 1024 * 1024; // 2 MB per category

/// Debug & profiling categories in the Debug tab sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugRunKind {
    Atrace,
    Simpleperf,
    Strace,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DebugCategory {
    ActivityManager,
    DumpsysServices,
    SystemTrace,
    Simpleperf,
    Strace,
    MemoryAnalysis,
    GpuGraphics,
    NetworkDiag,
}

impl DebugCategory {
    pub const ALL: &[Self] = &[
        Self::ActivityManager,
        Self::DumpsysServices,
        Self::SystemTrace,
        Self::Simpleperf,
        Self::Strace,
        Self::MemoryAnalysis,
        Self::GpuGraphics,
        Self::NetworkDiag,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::ActivityManager => "Activity Manager",
            Self::DumpsysServices => "Dumpsys Services",
            Self::SystemTrace => "System Trace",
            Self::Simpleperf => "Simpleperf",
            Self::Strace => "Strace",
            Self::MemoryAnalysis => "Memory",
            Self::GpuGraphics => "GPU / Graphics",
            Self::NetworkDiag => "Network",
        }
    }

    pub const fn index(self) -> u8 {
        self as u8
    }

    pub fn from_index(idx: u8) -> Option<Self> {
        Self::ALL.get(usize::from(idx)).copied()
    }

    pub const fn run_kind(self) -> Option<DebugRunKind> {
        match self {
            Self::SystemTrace => Some(DebugRunKind::Atrace),
            Self::Simpleperf => Some(DebugRunKind::Simpleperf),
            Self::Strace => Some(DebugRunKind::Strace),
            Self::ActivityManager
            | Self::DumpsysServices
            | Self::MemoryAnalysis
            | Self::GpuGraphics
            | Self::NetworkDiag => None,
        }
    }
}

/// System monitor categories in the Monitor tab sidebar.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MonitorCategory {
    Processes,
    Top,
    SystemInfo,
    Storage,
    BatteryPower,
    Thermal,
    IoStats,
    Services,
}

impl MonitorCategory {
    pub const ALL: &[Self] = &[
        Self::Processes,
        Self::Top,
        Self::SystemInfo,
        Self::Storage,
        Self::BatteryPower,
        Self::Thermal,
        Self::IoStats,
        Self::Services,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Processes => "Processes",
            Self::Top => "Top / Load",
            Self::SystemInfo => "System Info",
            Self::Storage => "Storage",
            Self::BatteryPower => "Battery & Power",
            Self::Thermal => "Thermal",
            Self::IoStats => "I/O Stats",
            Self::Services => "Services",
        }
    }

    pub const fn index(self) -> u8 {
        self as u8
    }

    pub fn from_index(idx: u8) -> Option<Self> {
        Self::ALL.get(usize::from(idx)).copied()
    }
}

/// A captured screenshot with metadata.
pub struct ScreenCapture {
    pub timestamp: String,
    pub png_bytes: Arc<Vec<u8>>,
    pub texture: Option<egui::TextureHandle>,
    pub width: u32,
    pub height: u32,
}

/// Sort criteria for the file list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileSortBy {
    Name,
    Size,
    #[default]
    Modified,
    Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeployMethod {
    #[default]
    External,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CapabilityStatus {
    #[default]
    Unknown,
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogcatUiState {
    pub running: bool,
    pub auto_scroll: bool,
}

impl Default for LogcatUiState {
    fn default() -> Self {
        Self {
            running: false,
            auto_scroll: true,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FileActivityState {
    pub pulling: bool,
    pub watching: bool,
    pub dirty: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ShellUiState {
    pub running: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LoadingState {
    pub props: bool,
    pub explorer: bool,
    pub uiautomator: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplorerPreviewSkipReason {
    ListingInProgress,
    DirectorySelected,
    FileTooLarge(usize),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScreenUiState {
    pub capturing: bool,
    pub recording: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TestRunState {
    pub monkey: bool,
    pub bugreport: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DebugRunState {
    pub atrace: bool,
    pub simpleperf: bool,
    pub strace: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSortState {
    pub by: FileSortBy,
    pub ascending: bool,
}

impl Default for FileSortState {
    fn default() -> Self {
        Self {
            by: FileSortBy::Modified,
            ascending: false,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DeployState {
    pub running: bool,
    pub status: String,
    pub method: DeployMethod,
    pub run_as: CapabilityStatus,
    pub crash_log: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PackageToolsState {
    pub package_name: String,
    pub package_filter: String,
    pub permission_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionToolsState {
    pub tcpip_port: String,
    pub forward_local: String,
    pub forward_remote: String,
    pub reverse_local: String,
    pub reverse_remote: String,
}

impl Default for ConnectionToolsState {
    fn default() -> Self {
        Self {
            tcpip_port: "5555".to_string(),
            forward_local: "tcp:8080".to_string(),
            forward_remote: "tcp:8080".to_string(),
            reverse_local: "tcp:8081".to_string(),
            reverse_remote: "tcp:8081".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandToolsState {
    pub instrument_runner: String,
    pub am_command: String,
    pub pm_command: String,
    pub cmd_service: String,
    pub cmd_args: String,
    pub wm_size: String,
    pub wm_density: String,
    pub settings_namespace: String,
    pub settings_key: String,
    pub settings_value: String,
    pub sqlite_path: String,
    pub sqlite_query: String,
    pub sqlite_run_as_package: String,
    pub content_command: String,
}

impl Default for CommandToolsState {
    fn default() -> Self {
        Self {
            instrument_runner: String::new(),
            am_command: String::new(),
            pm_command: String::new(),
            cmd_service: String::new(),
            cmd_args: String::new(),
            wm_size: String::new(),
            wm_density: String::new(),
            settings_namespace: "global".to_string(),
            settings_key: String::new(),
            settings_value: String::new(),
            sqlite_path: "databases/app.db".to_string(),
            sqlite_query: "SELECT name FROM sqlite_master;".to_string(),
            sqlite_run_as_package: String::new(),
            content_command: String::new(),
        }
    }
}

/// State for a single connected device tab.
pub struct DeviceState {
    pub info: DeviceInfo,
    /// Accumulated logcat lines.
    pub logcat_lines: Vec<String>,
    /// Logcat UI state.
    pub logcat_ui: LogcatUiState,
    /// Logcat error/status message.
    pub logcat_status: String,
    /// Current live-logcat session, used to ignore stale worker output after restarts.
    pub logcat_session: u64,
    /// Logcat text filter (case-insensitive substring).
    pub logcat_filter: String,
    /// Optional live logcat tag filter (case-insensitive substring).
    pub logcat_tag_filter: String,
    /// Optional live logcat PID filter.
    pub logcat_pid_filter: String,
    /// Log level filter: 0=All, 1=V, 2=D, 3=I, 4=W, 5=E, 6=F
    pub level_filter: usize,
    /// Pulled file logs keyed by key (e.g. "internal/client.1.log").
    pub file_logs: HashMap<String, FileEntry>,
    /// Sorted keys cache (rebuilt on change).
    pub sorted_keys: Vec<String>,
    /// File transfer/watch state.
    pub file_activity: FileActivityState,
    /// File pull/watcher status message.
    pub file_status: String,
    /// Current live file-watcher session, used to ignore stale watcher output after restarts.
    pub file_watch_session: u64,
    /// Active sub-tab: 0=Logs, 1=File Logs, 2=Shell, 3=Screen, 4=Explorer, 5=Device, 6=Debug, 7=Monitor, 8=Deploy, 9=App Log
    pub active_sub_tab: usize,
    /// Currently selected log source in the Logs tab sidebar.
    pub active_log_source: LogSource,
    /// Snapshot log buffers for non-live sources.
    pub log_buffers: HashMap<LogSource, Vec<String>>,
    /// Which log sources are currently being fetched.
    pub log_loading: HashSet<LogSource>,
    /// Current page index for the log viewer (0-based, resets on source switch).
    pub log_page: usize,
    /// Running log watchers per source (stop flags). Logcat excluded (has its own stream).
    pub log_watchers: HashMap<LogSource, Arc<AtomicBool>>,
    /// Selected file key for viewing.
    pub selected_file: Option<String>,
    /// Search filter for file log content.
    pub file_content_filter: String,
    /// Stop flag for the watcher thread.
    pub watcher_stop: Option<Arc<AtomicBool>>,
    /// Sort settings for the file list.
    pub file_sort: FileSortState,
    /// Shell output history.
    pub shell_output: Vec<String>,
    /// Interactive shell state.
    pub shell: ShellUiState,
    /// Current command input.
    pub shell_input: String,
    /// Command history for up/down arrow navigation.
    pub shell_history: Vec<String>,
    /// Current position in history (`shell_history.len()` = at end / new input).
    pub shell_history_pos: usize,
    /// Cached device properties (key, value).
    pub device_props: Vec<(String, String)>,
    /// Shared loading state for async UI tools.
    pub loading: LoadingState,
    /// Status messages from device actions.
    pub action_log: Vec<String>,
    /// File explorer: current remote path.
    pub explorer_path: String,
    /// File explorer: whether at least one listing attempt completed.
    pub explorer_loaded_once: bool,
    /// File explorer: directory listing.
    pub explorer_entries: Vec<RemoteFileEntry>,
    /// File explorer: error message.
    pub explorer_error: String,
    /// File explorer: path currently being loaded, if any.
    pub explorer_loading_path: Option<String>,
    /// File explorer: editable path input shown in the toolbar.
    pub explorer_path_input: String,
    /// File explorer: selected entry name.
    pub explorer_selected: Option<String>,
    /// File explorer: preview content for selected file.
    pub explorer_preview: Option<String>,
    /// File explorer: preview error message for the selected entry.
    pub explorer_preview_error: String,
    /// File explorer: navigation history (back stack).
    pub explorer_history: Vec<String>,
    /// File explorer: generation counter to discard stale async results.
    pub explorer_gen: u64,
    /// File explorer: generation counter to discard stale preview results.
    pub explorer_preview_gen: u64,
    /// File explorer: whether preview content is currently loading.
    pub explorer_preview_loading: bool,
    /// File explorer: active right-side tab (0 = Preview, 1 = Commands).
    pub explorer_right_tab: usize,
    /// File explorer: command input.
    pub explorer_command_input: String,
    /// File explorer: command output.
    pub explorer_command_output: String,
    /// File explorer: command status line.
    pub explorer_command_status: String,
    /// File explorer: command history.
    pub explorer_command_history: Vec<String>,
    /// File explorer: history cursor.
    pub explorer_command_history_pos: usize,
    /// File explorer: command session token.
    pub explorer_command_session: u64,
    /// File explorer: whether a command is currently running.
    pub explorer_command_running: bool,
    /// File explorer: stop flag for live follow mode.
    pub explorer_follow_stop: Option<Arc<AtomicBool>>,
    /// File explorer: lifecycle and response log.
    pub explorer_log_lines: Vec<String>,
    /// Screen: capture history (timestamp, `texture_id`, width, height, `png_bytes`).
    pub screen_captures: Vec<ScreenCapture>,
    /// Screen: index of currently viewed capture.
    pub screen_view_idx: Option<usize>,
    /// Screen capture/record state.
    pub screen: ScreenUiState,
    /// Screen: auto-capture interval (None = disabled).
    pub screen_auto_interval: Option<f64>,
    /// Screen: last auto-capture time.
    pub screen_last_auto: f64,
    /// Screen: status message.
    pub screen_status: String,
    /// Monkey: event count input string.
    pub monkey_event_count: String,
    /// Test run state.
    pub test_runs: TestRunState,
    /// `UIAutomator`: cached XML dump content.
    pub uiautomator_dump: Option<String>,
    // ── Debug tab ───────────────────────────────────────────────────────
    /// Currently selected debug category.
    pub active_debug_category: DebugCategory,
    /// Cached output per category.
    pub debug_outputs: HashMap<DebugCategory, String>,
    /// Which categories are currently being fetched.
    pub debug_loading: HashSet<DebugCategory>,
    /// Text filter for debug output.
    pub debug_filter: String,
    /// Dumpsys: selected service name.
    pub dumpsys_service: String,
    /// Dumpsys: available service list (cached).
    pub dumpsys_services_list: Vec<String>,
    /// Atrace: selected categories for system trace.
    pub atrace_categories: Vec<String>,
    /// Atrace: available category list (cached).
    pub atrace_available_cats: Vec<String>,
    /// Atrace: duration in seconds (input string).
    pub atrace_duration: String,
    /// Debug profiler run state.
    pub debug_runs: DebugRunState,
    /// Simpleperf: duration in seconds (input string).
    pub simpleperf_duration: String,
    /// Simpleperf: event to record.
    pub simpleperf_event: String,
    /// Strace: target PID (input string).
    pub strace_pid: String,
    /// Strace: duration in seconds (input string).
    pub strace_duration: String,
    /// Memory analysis: heap watch threshold in megabytes.
    pub memory_watch_limit_mb: String,
    // ── Monitor tab ────────────────────────────────────────────────────
    /// Currently selected monitor category.
    pub active_monitor_category: MonitorCategory,
    /// Cached output per category.
    pub monitor_outputs: HashMap<MonitorCategory, String>,
    /// Which categories are currently being fetched.
    pub monitor_loading: HashSet<MonitorCategory>,
    /// Text filter for monitor output.
    pub monitor_filter: String,
    /// Process search input (for ps grep).
    pub monitor_ps_search: String,
    /// PID input for kill/signal.
    pub monitor_kill_pid: String,
    // ── Deploy tab ─────────────────────────────────────────────────
    /// Deploy workflow state.
    pub deploy: DeployState,
    /// Package manager helpers and inputs.
    pub package_tools: PackageToolsState,
    /// Connection and forwarding helpers.
    pub connection_tools: ConnectionToolsState,
    /// Advanced shell-backed command helpers.
    pub command_tools: CommandToolsState,
}

impl DeviceState {
    pub fn new(info: DeviceInfo) -> Self {
        Self {
            info,
            logcat_lines: Vec::with_capacity(4096),
            logcat_ui: LogcatUiState::default(),
            logcat_status: String::new(),
            logcat_session: 0,
            logcat_filter: String::new(),
            logcat_tag_filter: String::new(),
            logcat_pid_filter: String::new(),
            level_filter: 0,
            file_logs: HashMap::new(),
            sorted_keys: Vec::new(),
            file_activity: FileActivityState::default(),
            file_status: String::new(),
            file_watch_session: 0,
            active_sub_tab: 0,
            active_log_source: LogSource::Logcat,
            log_buffers: HashMap::new(),
            log_loading: HashSet::new(),
            log_page: 0, // page 0 = newest lines
            log_watchers: HashMap::new(),
            selected_file: None,
            file_content_filter: String::new(),
            watcher_stop: None,
            file_sort: FileSortState::default(),
            shell_output: Vec::with_capacity(1024),
            shell: ShellUiState::default(),
            shell_input: String::new(),
            shell_history: Vec::new(),
            shell_history_pos: 0,
            device_props: Vec::new(),
            loading: LoadingState::default(),
            action_log: Vec::new(),
            explorer_path: "/sdcard".into(),
            explorer_loaded_once: false,
            explorer_entries: Vec::new(),
            explorer_error: String::new(),
            explorer_loading_path: None,
            explorer_path_input: "/sdcard".into(),
            explorer_selected: None,
            explorer_preview: None,
            explorer_preview_error: String::new(),
            explorer_history: Vec::new(),
            explorer_gen: 0,
            explorer_preview_gen: 0,
            explorer_preview_loading: false,
            explorer_right_tab: 0,
            explorer_command_input: String::new(),
            explorer_command_output: String::new(),
            explorer_command_status: String::new(),
            explorer_command_history: Vec::new(),
            explorer_command_history_pos: 0,
            explorer_command_session: 0,
            explorer_command_running: false,
            explorer_follow_stop: None,
            explorer_log_lines: Vec::new(),
            screen_captures: Vec::new(),
            screen_view_idx: None,
            screen: ScreenUiState::default(),
            screen_auto_interval: None,
            screen_last_auto: 0.0,
            screen_status: String::new(),
            monkey_event_count: "1000".into(),
            test_runs: TestRunState::default(),
            uiautomator_dump: None,
            active_debug_category: DebugCategory::ActivityManager,
            debug_outputs: HashMap::new(),
            debug_loading: HashSet::new(),
            debug_filter: String::new(),
            dumpsys_service: String::new(),
            dumpsys_services_list: Vec::new(),
            atrace_categories: Vec::new(),
            atrace_available_cats: Vec::new(),
            atrace_duration: "5".into(),
            debug_runs: DebugRunState::default(),
            simpleperf_duration: "5".into(),
            simpleperf_event: "cpu-cycles".into(),
            strace_pid: String::new(),
            strace_duration: "5".into(),
            memory_watch_limit_mb: "256".into(),
            active_monitor_category: MonitorCategory::Processes,
            monitor_outputs: HashMap::new(),
            monitor_loading: HashSet::new(),
            monitor_filter: String::new(),
            monitor_ps_search: String::new(),
            monitor_kill_pid: String::new(),
            deploy: DeployState::default(),
            package_tools: PackageToolsState::default(),
            connection_tools: ConnectionToolsState::default(),
            command_tools: CommandToolsState::default(),
        }
    }

    pub fn push_logcat_line(&mut self, line: String) {
        self.logcat_lines.push(line);
        if self.logcat_lines.len() > MAX_LOGCAT_LINES {
            let drain = MAX_LOGCAT_LINES / 5;
            self.logcat_lines.drain(..drain);
        }
        if self.logcat_ui.auto_scroll {
            self.log_page = 0;
        }
    }

    /// Set a snapshot log buffer (replaces previous content).
    pub fn set_log_buffer(&mut self, source: LogSource, lines: Vec<String>) {
        let mut buf = lines;
        if buf.len() > MAX_LOG_BUFFER_LINES {
            buf.drain(..buf.len() - MAX_LOG_BUFFER_LINES);
        }
        self.log_buffers.insert(source, buf);
        self.log_loading.remove(&source);
    }

    pub fn push_shell_output(&mut self, line: String) {
        self.shell_output.push(line);
        if self.shell_output.len() > MAX_SHELL_LINES {
            let drain = MAX_SHELL_LINES / 5;
            self.shell_output.drain(..drain);
        }
    }

    pub fn push_action_log(&mut self, line: String) {
        self.action_log.push(line);
        if self.action_log.len() > MAX_ACTION_LOG_LINES {
            let drain = MAX_ACTION_LOG_LINES / 5;
            self.action_log.drain(..drain);
        }
    }

    /// Set debug output for a category (capped to `MAX_DEBUG_OUTPUT_BYTES`).
    pub fn set_debug_output(&mut self, cat: DebugCategory, mut text: String) {
        text.truncate(MAX_DEBUG_OUTPUT_BYTES);
        self.debug_outputs.insert(cat, text);
        self.debug_loading.remove(&cat);
    }

    /// Set monitor output for a category (capped to `MAX_DEBUG_OUTPUT_BYTES`).
    pub fn set_monitor_output(&mut self, cat: MonitorCategory, mut text: String) {
        text.truncate(MAX_DEBUG_OUTPUT_BYTES);
        self.monitor_outputs.insert(cat, text);
        self.monitor_loading.remove(&cat);
    }

    /// Insert or update a file entry. Marks dirty for deferred sort rebuild.
    pub fn upsert_file(&mut self, entry: FileEntry) {
        self.file_logs.insert(entry.key.clone(), entry);
        self.file_activity.dirty = true;
    }

    /// Rebuild sorted keys if dirty. Call once per frame after draining messages.
    pub fn flush_if_dirty(&mut self) {
        if self.file_activity.dirty {
            self.rebuild_sorted_keys();
            self.file_activity.dirty = false;
        }
    }

    /// Rebuild the sorted key list.
    pub fn rebuild_sorted_keys(&mut self) {
        let mut keys: Vec<String> = self.file_logs.keys().cloned().collect();
        let logs = &self.file_logs;
        let asc = self.file_sort.ascending;

        keys.sort_by(|a, b| {
            let ea = &logs[a];
            let eb = &logs[b];
            let ord = match self.file_sort.by {
                FileSortBy::Name => ea.name.to_lowercase().cmp(&eb.name.to_lowercase()),
                FileSortBy::Size => ea.size.cmp(&eb.size),
                FileSortBy::Modified => ea.modified.cmp(&eb.modified),
                FileSortBy::Source => ea.source.cmp(&eb.source).then(ea.name.cmp(&eb.name)),
            };
            if asc {
                ord
            } else {
                ord.reverse()
            }
        });

        self.sorted_keys = keys;
    }

    /// Stop the file watcher if running.
    pub fn stop_watcher(&mut self) {
        if let Some(ref stop) = self.watcher_stop {
            stop.store(true, Ordering::Relaxed);
        }
        self.watcher_stop = None;
        self.file_activity.watching = false;
    }

    /// Advance the live logcat session and return the new session ID.
    pub const fn start_next_logcat_session(&mut self) -> u64 {
        self.logcat_session = next_session_id(self.logcat_session);
        self.logcat_session
    }

    /// Advance the live file-watcher session and return the new session ID.
    pub const fn start_next_file_watch_session(&mut self) -> u64 {
        self.file_watch_session = next_session_id(self.file_watch_session);
        self.file_watch_session
    }

    /// Stop a log source watcher.
    pub fn stop_log_watcher(&mut self, source: LogSource) {
        if let Some(stop) = self.log_watchers.remove(&source) {
            stop.store(true, Ordering::Relaxed);
        }
    }

    /// Stop all log source watchers.
    pub fn stop_all_log_watchers(&mut self) {
        for (_, stop) in self.log_watchers.drain() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    /// Explorer path shown in the toolbar. While a request is in flight, this is the pending path.
    pub fn explorer_visible_path(&self) -> &str {
        self.explorer_loading_path
            .as_deref()
            .unwrap_or(&self.explorer_path)
    }

    /// Start navigation to a target path and optionally push the current visible path onto history.
    pub fn start_explorer_navigation(
        &mut self,
        target: &str,
        push_history: bool,
    ) -> Option<(u64, String)> {
        let target = normalize_remote_dir_path(target);
        let current_visible = self.explorer_visible_path().to_string();

        if target == current_visible {
            return None;
        }

        if push_history && self.explorer_history.last() != Some(&current_visible) {
            self.explorer_history.push(current_visible);
        }

        Some(self.begin_explorer_request(target))
    }

    /// Start a refresh of the current visible path.
    pub fn start_explorer_refresh(&mut self) -> (u64, String) {
        self.begin_explorer_request(self.explorer_visible_path().to_string())
    }

    /// Navigate back to the previous path, skipping duplicate entries.
    pub fn start_explorer_back_navigation(&mut self) -> Option<(u64, String)> {
        let current_visible = self.explorer_visible_path().to_string();
        while let Some(path) = self.explorer_history.pop() {
            let normalized = normalize_remote_dir_path(&path);
            if normalized != current_visible {
                return Some(self.begin_explorer_request(normalized));
            }
        }
        None
    }

    /// Apply a successful directory listing if it belongs to the current request.
    pub fn finish_explorer_listing(
        &mut self,
        request_gen: u64,
        path: &str,
        entries: Vec<RemoteFileEntry>,
    ) -> bool {
        if request_gen != self.explorer_gen {
            return false;
        }

        self.explorer_path = normalize_remote_dir_path(path);
        self.explorer_loaded_once = true;
        self.explorer_entries = entries;
        self.loading.explorer = false;
        self.explorer_loaded_once = true;
        self.explorer_loading_path = None;
        self.explorer_path_input = self.explorer_path.clone();
        self.explorer_error.clear();
        self.explorer_selected = None;
        self.explorer_preview = None;
        self.explorer_preview_error.clear();
        self.explorer_preview_loading = false;
        true
    }

    /// Apply a failed directory listing if it belongs to the current request.
    pub fn fail_explorer_listing(&mut self, request_gen: u64, error: String) -> bool {
        if request_gen != self.explorer_gen {
            return false;
        }

        self.loading.explorer = false;
        self.explorer_loading_path = None;
        self.explorer_error = error;
        self.explorer_path_input = self.explorer_visible_path().to_string();
        self.explorer_selected = None;
        self.explorer_preview = None;
        self.explorer_preview_error.clear();
        self.explorer_preview_loading = false;
        true
    }

    /// Start preview loading for an entry from the currently displayed listing.
    pub fn start_explorer_preview_request(
        &mut self,
        entry: &RemoteFileEntry,
    ) -> Result<(u64, u64, String), ExplorerPreviewSkipReason> {
        self.explorer_selected = Some(entry.name.clone());
        self.explorer_preview = None;
        self.explorer_preview_error.clear();

        if self.loading.explorer {
            self.explorer_preview_loading = false;
            return Err(ExplorerPreviewSkipReason::ListingInProgress);
        }
        if entry.is_dir {
            self.explorer_preview_loading = false;
            return Err(ExplorerPreviewSkipReason::DirectorySelected);
        }
        if entry.size >= 512_000 {
            self.explorer_preview_loading = false;
            return Err(ExplorerPreviewSkipReason::FileTooLarge(entry.size));
        }

        self.explorer_preview_gen = next_session_id(self.explorer_preview_gen);
        self.explorer_preview_loading = true;

        Ok((
            self.explorer_gen,
            self.explorer_preview_gen,
            join_remote_path(&self.explorer_path, &entry.name),
        ))
    }

    /// Apply a preview result if it belongs to the current listing and preview request.
    pub fn finish_explorer_preview(
        &mut self,
        listing_gen: u64,
        preview_gen: u64,
        result: Result<String, String>,
    ) -> bool {
        if listing_gen != self.explorer_gen || preview_gen != self.explorer_preview_gen {
            return false;
        }

        self.explorer_preview_loading = false;
        match result {
            Ok(content) => {
                self.explorer_preview = Some(content);
                self.explorer_preview_error.clear();
            }
            Err(error) => {
                self.explorer_preview = None;
                self.explorer_preview_error = error;
            }
        }
        true
    }

    pub fn set_explorer_command_output(&mut self, mut output: String) {
        output.truncate(MAX_EXPLORER_COMMAND_BYTES);
        self.explorer_command_output = output;
    }

    pub fn start_next_explorer_command_session(&mut self) -> u64 {
        self.explorer_command_session = next_session_id(self.explorer_command_session);
        self.explorer_command_running = true;
        self.explorer_command_status = "Running...".to_string();
        self.explorer_right_tab = 1;
        self.explorer_command_session
    }

    pub fn finish_explorer_command_session(
        &mut self,
        session: u64,
        status: impl Into<String>,
    ) -> bool {
        if self.explorer_command_session != session {
            return false;
        }
        self.explorer_command_running = false;
        self.explorer_command_status = status.into();
        self.explorer_follow_stop = None;
        true
    }

    pub fn stop_explorer_follow(&mut self) {
        if let Some(stop) = self.explorer_follow_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self.explorer_command_running = false;
    }

    pub fn push_explorer_log(&mut self, line: String) {
        self.explorer_log_lines.push(line);
        if self.explorer_log_lines.len() > MAX_EXPLORER_LOG_LINES {
            let drain = MAX_EXPLORER_LOG_LINES / 5;
            self.explorer_log_lines.drain(..drain);
        }
    }

    /// Display label for the tab.
    pub fn label(&self) -> String {
        if self.info.model == "unknown" {
            self.info.serial.clone()
        } else {
            format!("{} ({})", self.info.model, self.info.serial)
        }
    }

    fn begin_explorer_request(&mut self, target: String) -> (u64, String) {
        self.explorer_gen = next_session_id(self.explorer_gen);
        self.explorer_preview_gen = next_session_id(self.explorer_preview_gen);
        self.loading.explorer = true;
        self.explorer_loading_path = Some(target.clone());
        self.explorer_path_input.clone_from(&target);
        self.explorer_error.clear();
        self.explorer_selected = None;
        self.explorer_preview = None;
        self.explorer_preview_error.clear();
        self.explorer_preview_loading = false;
        (self.explorer_gen, target)
    }
}

/// Log level names for the dropdown.
pub const LEVEL_NAMES: &[&str] = &["All", "Verbose", "Debug", "Info", "Warn", "Error", "Fatal"];
pub const LEVEL_CHARS: &[char] = &[' ', 'V', 'D', 'I', 'W', 'E', 'F'];

/// Check if a logcat line passes the level filter.
pub fn line_passes_level(line: &str, min_level: usize) -> bool {
    if min_level == 0 {
        return true;
    }
    let bytes = line.as_bytes();
    if bytes.len() < 20 {
        return true;
    }

    for i in 18..bytes.len().min(40) {
        if i + 2 < bytes.len()
            && bytes[i] == b' '
            && bytes[i + 1].is_ascii_uppercase()
            && bytes[i + 2] == b' '
        {
            let level_char = bytes[i + 1] as char;
            let level_idx = LEVEL_CHARS
                .iter()
                .position(|&c| c == level_char)
                .unwrap_or(0);
            return level_idx >= min_level;
        }
    }

    true
}

pub fn line_passes_tag(line: &str, tag_filter: &str) -> bool {
    let tag_filter = tag_filter.trim();
    if tag_filter.is_empty() {
        return true;
    }

    parse_threadtime_tag(line).is_some_and(|tag| tag.to_lowercase().contains(tag_filter))
}

pub fn line_passes_pid(line: &str, pid_filter: &str) -> bool {
    let pid_filter = pid_filter.trim();
    if pid_filter.is_empty() {
        return true;
    }

    parse_threadtime_pid(line) == Some(pid_filter)
}

fn parse_threadtime_pid(line: &str) -> Option<&str> {
    line.split_whitespace().nth(2)
}

fn parse_threadtime_tag(line: &str) -> Option<&str> {
    line.split_whitespace()
        .nth(5)
        .map(|tag| tag.trim_end_matches(':'))
}

const fn next_session_id(current: u64) -> u64 {
    if current == u64::MAX {
        1
    } else {
        current + 1
    }
}

pub fn normalize_remote_dir_path(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.trim().split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            let _ = parts.pop();
        } else {
            parts.push(part);
        }
    }

    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

pub fn join_remote_path(base: &str, name: &str) -> String {
    let mut path = normalize_remote_dir_path(base);
    if path != "/" {
        path.push('/');
    }
    path.push_str(name.trim_matches('/'));
    normalize_remote_dir_path(&path)
}

pub fn parent_remote_dir(path: &str) -> Option<String> {
    let normalized = normalize_remote_dir_path(path);
    if normalized == "/" {
        None
    } else {
        normalized
            .rsplit_once('/')
            .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
            .map(std::string::ToString::to_string)
    }
}

pub fn resolve_remote_path(cwd: &str, path: &str) -> String {
    let path = path.trim();
    if path.starts_with('/') {
        normalize_remote_dir_path(path)
    } else {
        normalize_remote_dir_path(&format!("{}/{}", normalize_remote_dir_path(cwd), path))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        join_remote_path, normalize_remote_dir_path, parent_remote_dir, resolve_remote_path,
        DebugCategory, DebugRunKind, DeviceState, ExplorerPreviewSkipReason, LogSource,
        MonitorCategory, RemoteFileEntry,
    };
    use crate::adb::DeviceInfo;

    #[test]
    fn log_source_indices_round_trip_through_all() {
        for (index, source) in LogSource::ALL.iter().copied().enumerate() {
            let index = u8::try_from(index).expect("test index must fit in u8");
            assert_eq!(source.index(), index);
            assert_eq!(LogSource::from_index(index), Some(source));
        }
    }

    #[test]
    fn debug_category_indices_and_run_kinds_are_stable() {
        for (index, category) in DebugCategory::ALL.iter().copied().enumerate() {
            let index = u8::try_from(index).expect("test index must fit in u8");
            assert_eq!(category.index(), index);
            assert_eq!(DebugCategory::from_index(index), Some(category));
        }

        assert_eq!(
            DebugCategory::SystemTrace.run_kind(),
            Some(DebugRunKind::Atrace)
        );
        assert_eq!(
            DebugCategory::Simpleperf.run_kind(),
            Some(DebugRunKind::Simpleperf)
        );
        assert_eq!(DebugCategory::Strace.run_kind(), Some(DebugRunKind::Strace));
        assert_eq!(DebugCategory::NetworkDiag.run_kind(), None);
    }

    #[test]
    fn monitor_category_indices_round_trip_through_all() {
        for (index, category) in MonitorCategory::ALL.iter().copied().enumerate() {
            let index = u8::try_from(index).expect("test index must fit in u8");
            assert_eq!(category.index(), index);
            assert_eq!(MonitorCategory::from_index(index), Some(category));
        }
    }

    #[test]
    fn remote_path_helpers_normalize_and_join_consistently() {
        assert_eq!(normalize_remote_dir_path(""), "/");
        assert_eq!(
            normalize_remote_dir_path("/sdcard//Download/"),
            "/sdcard/Download"
        );
        assert_eq!(normalize_remote_dir_path("/sdcard/../data"), "/data");
        assert_eq!(join_remote_path("/sdcard/", "logs"), "/sdcard/logs");
        assert_eq!(
            parent_remote_dir("/sdcard/logs"),
            Some("/sdcard".to_string())
        );
        assert_eq!(parent_remote_dir("/"), None);
        assert_eq!(
            resolve_remote_path("/sdcard/logs", "../cache"),
            "/sdcard/cache"
        );
    }

    #[test]
    fn explorer_navigation_tracks_visible_path_and_rejects_stale_preview() {
        let mut device = DeviceState::new(DeviceInfo {
            serial: "serial".to_string(),
            state: "device".to_string(),
            model: "model".to_string(),
        });

        let (first_gen, first_path) = device
            .start_explorer_navigation("/sdcard/Download", true)
            .expect("navigation should start");
        assert_eq!(first_gen, 1);
        assert_eq!(first_path, "/sdcard/Download");
        assert_eq!(device.explorer_history, vec!["/sdcard".to_string()]);
        assert_eq!(device.explorer_visible_path(), "/sdcard/Download");

        let (second_gen, second_path) = device
            .start_explorer_navigation("/sdcard/Documents", true)
            .expect("second navigation should start");
        assert_eq!(second_gen, 2);
        assert_eq!(second_path, "/sdcard/Documents");
        assert_eq!(
            device.explorer_history,
            vec!["/sdcard".to_string(), "/sdcard/Download".to_string()]
        );

        assert!(
            !device.finish_explorer_listing(first_gen, &first_path, Vec::new()),
            "stale listing must be ignored"
        );
        assert!(device.finish_explorer_listing(second_gen, &second_path, Vec::new()));

        let entry = RemoteFileEntry {
            name: "test.txt".to_string(),
            is_dir: false,
            size: 12,
            modified: String::new(),
            permissions: "-rw-r--r--".to_string(),
        };
        let (listing_gen, preview_gen, remote_path) = device
            .start_explorer_preview_request(&entry)
            .expect("preview should start");
        assert_eq!(listing_gen, second_gen);
        assert_eq!(remote_path, "/sdcard/Documents/test.txt");

        let stale_preview_gen = preview_gen;
        let _ = device.start_explorer_preview_request(&entry);
        assert!(
            !device.finish_explorer_preview(
                listing_gen,
                stale_preview_gen,
                Ok("stale".to_string())
            ),
            "stale preview must be ignored"
        );
    }

    #[test]
    fn explorer_preview_request_reports_skip_reason() {
        let mut device = DeviceState::new(DeviceInfo {
            serial: "serial".to_string(),
            state: "device".to_string(),
            model: "model".to_string(),
        });
        device.loading.explorer = true;

        let entry = RemoteFileEntry {
            name: "test.txt".to_string(),
            is_dir: false,
            size: 12,
            modified: String::new(),
            permissions: "-rw-r--r--".to_string(),
        };

        assert_eq!(
            device.start_explorer_preview_request(&entry),
            Err(ExplorerPreviewSkipReason::ListingInProgress)
        );
    }

    #[test]
    fn push_logcat_line_resets_to_newest_page_when_auto_scroll_is_enabled() {
        let mut device = DeviceState::new(DeviceInfo {
            serial: "serial".to_string(),
            state: "device".to_string(),
            model: "model".to_string(),
        });
        device.log_page = 3;
        device.logcat_ui.auto_scroll = true;

        device.push_logcat_line("line".to_string());

        assert_eq!(device.log_page, 0);
    }

    #[test]
    fn push_logcat_line_keeps_current_page_when_auto_scroll_is_disabled() {
        let mut device = DeviceState::new(DeviceInfo {
            serial: "serial".to_string(),
            state: "device".to_string(),
            model: "model".to_string(),
        });
        device.log_page = 3;
        device.logcat_ui.auto_scroll = false;

        device.push_logcat_line("line".to_string());

        assert_eq!(device.log_page, 3);
    }
}
