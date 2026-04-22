mod app_log;
mod debug;
mod deploy;
mod device_tab;
mod device_tools;
mod explorer;
mod file_logs;
mod helpers;
mod logs;
mod mirror;
mod monitor;
mod privacy;
mod screen;
mod settings;
mod shell;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Child;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use eframe::egui;

use crate::adb::{self, AdbMsg, DeviceInfo};
use crate::config::AppConfig;
use crate::device::{CapabilityStatus, DebugCategory, DeviceState, LogSource, MonitorCategory};

pub use helpers::now_str;

const DEVICE_POLL_INTERVAL: f64 = 3.0;
const STREAMER_MODE_POLL_INTERVAL: f64 = 2.0;
const FILE_WATCH_INTERVAL: Duration = Duration::from_secs(3);
const MAX_APP_LOG_LINES: usize = 5_000;
const ACTIVE_REPAINT_INTERVAL: Duration = Duration::from_millis(100);
const IDLE_REPAINT_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub struct AppLogEntry {
    pub timestamp: String,
    pub level: AppLogLevel,
    pub message: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AppLogLevel {
    Info,
    Warn,
    Error,
}

impl AppLogLevel {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERR",
        }
    }

    pub const fn color(self) -> egui::Color32 {
        match self {
            Self::Info => egui::Color32::from_rgb(100, 220, 100),
            Self::Warn => egui::Color32::from_rgb(255, 200, 50),
            Self::Error => egui::Color32::from_rgb(255, 80, 80),
        }
    }
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "UI toggle flags are naturally booleans"
)]
pub struct App {
    pub rx: Receiver<AdbMsg>,
    pub tx: Sender<AdbMsg>,
    pub devices: HashMap<String, DeviceState>,
    pub device_order: Vec<String>,
    pub active_device: Option<String>,
    pub logcat_procs: HashMap<String, Child>,
    pub last_device_poll: f64,
    pub last_streamer_mode_poll: f64,
    pub status: String,
    pub fatal_error: Option<String>,
    pub adb_path_candidate: String,
    pub adb_override_message: String,
    pub config: AppConfig,
    pub config_path: PathBuf,
    pub show_settings: bool,
    pub show_devices: bool,
    pub bundle_id_input: String,
    pub log_tags_input: String,
    pub activity_class_input: String,
    /// Internal application log entries.
    pub log_entries: Vec<AppLogEntry>,
    pub log_filter: String,
    pub log_auto_scroll: bool,
    /// Interactive shell handles per device serial.
    pub shell_handles: HashMap<String, adb::ShellHandle>,
    /// Screen recording child processes per device serial.
    pub recording_procs: HashMap<String, Child>,
    /// `WiFi` connect address input.
    pub wifi_connect_addr: String,
    /// WSA port input.
    pub wsa_port: String,
    /// Wireless debugging pairing address input.
    pub pair_address_input: String,
    /// Wireless debugging pairing code input.
    pub pair_code_input: String,
    /// Fastboot serial input.
    pub fastboot_serial_input: String,
    /// Fastboot partition input.
    pub fastboot_partition_input: String,
    /// Devices the user explicitly closed (won't auto-reappear).
    pub hidden_devices: std::collections::HashSet<String>,
    /// Cached AVD list for emulator management.
    pub available_avds: Vec<String>,
    /// Whether AVD list is loading.
    pub avds_loading: bool,
    /// Create AVD: name input.
    pub new_avd_name: String,
    /// Create AVD: selected system image.
    pub new_avd_image: String,
    /// Create AVD: device type (e.g. "`pixel_6`").
    pub new_avd_device: String,
    /// Available system images.
    pub available_system_images: Vec<String>,
    /// Cached running emulator serial -> AVD name (refreshed async, not in UI thread).
    pub running_emu_map: HashMap<String, String>,
    /// Whether the App Log panel is visible.
    pub show_app_log: bool,
    /// Whether automatic streamer mode is currently active.
    pub streamer_mode: bool,
    /// Stable redacted device aliases keyed by serial.
    pub streamer_device_aliases: HashMap<String, usize>,
    /// Last observed device model keyed by serial for redacting stale log lines.
    pub streamer_device_models: HashMap<String, String>,
    /// Next alias ordinal to assign to a newly seen device.
    pub next_streamer_device_alias: usize,
}

impl App {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let loaded_config = AppConfig::load();
        let crate::config::LoadedConfig {
            config,
            path: config_path,
            warnings: config_warnings,
        } = loaded_config;

        let tx2 = tx.clone();
        std::thread::spawn(move || match adb::list_devices() {
            Ok(devs) => {
                let _ = tx2.send(AdbMsg::DevicesUpdated(devs));
            }
            Err(e) => {
                let _ = tx2.send(AdbMsg::AdbNotFound(e));
            }
        });

        let bundle_id_input = config.bundle_id.clone();
        let log_tags_input = config.logcat_tags.join("\n");
        let activity_class_input = config.activity_class.clone();

        let mut app = Self {
            rx,
            tx,
            devices: HashMap::new(),
            device_order: Vec::new(),
            active_device: None,
            logcat_procs: HashMap::new(),
            last_device_poll: 0.0,
            last_streamer_mode_poll: 0.0,
            status: "Scanning for devices...".into(),
            fatal_error: None,
            adb_path_candidate: String::new(),
            adb_override_message: String::new(),
            config,
            config_path,
            show_settings: false,
            show_devices: false,
            bundle_id_input,
            log_tags_input,
            activity_class_input,
            log_entries: vec![AppLogEntry {
                timestamp: now_str(),
                level: AppLogLevel::Info,
                message: "ADB UI started".into(),
            }],
            log_filter: String::new(),
            log_auto_scroll: true,
            shell_handles: HashMap::new(),
            recording_procs: HashMap::new(),
            wifi_connect_addr: String::new(),
            wsa_port: "58526".into(),
            pair_address_input: String::new(),
            pair_code_input: String::new(),
            fastboot_serial_input: String::new(),
            fastboot_partition_input: "boot".into(),
            hidden_devices: std::collections::HashSet::new(),
            available_avds: Vec::new(),
            avds_loading: false,
            new_avd_name: String::new(),
            new_avd_image: String::new(),
            new_avd_device: "pixel_6".into(),
            available_system_images: Vec::new(),
            running_emu_map: HashMap::new(),
            show_app_log: false,
            streamer_mode: false,
            streamer_device_aliases: HashMap::new(),
            streamer_device_models: HashMap::new(),
            next_streamer_device_alias: 1,
        };

        for warning in config_warnings {
            app.log(AppLogLevel::Warn, warning);
        }

        app
    }

    pub fn log(&mut self, level: AppLogLevel, msg: impl Into<String>) {
        self.log_entries.push(AppLogEntry {
            timestamp: now_str(),
            level,
            message: msg.into(),
        });
        if self.log_entries.len() > MAX_APP_LOG_LINES {
            self.log_entries.drain(..MAX_APP_LOG_LINES / 5);
        }
    }

    fn log_skipped(&mut self, serial: &str, action: &str, reason: &str) {
        self.log(
            AppLogLevel::Warn,
            format!("[{serial}] {action} skipped: {reason}"),
        );
    }

    fn log_cancelled(&mut self, serial: &str, action: &str) {
        self.log(AppLogLevel::Info, format!("[{serial}] {action} cancelled"));
    }

    fn log_missing_device_state(&mut self, serial: &str, action: &str) {
        self.log_skipped(serial, action, "device state is no longer available");
    }

    fn log_stale_message(&mut self, serial: &str, action: &str) {
        self.log(
            AppLogLevel::Warn,
            format!("[{serial}] Ignored stale {action}"),
        );
    }

    fn log_mirror(&mut self, level: AppLogLevel, serial: &str, msg: impl Into<String>) {
        self.log(level, format!("[{serial}] [mirror] {}", msg.into()));
    }

    fn log_mirror_info(&mut self, serial: &str, msg: impl Into<String>) {
        self.log_mirror(AppLogLevel::Info, serial, msg);
    }

    fn log_mirror_warn(&mut self, serial: &str, msg: impl Into<String>) {
        self.log_mirror(AppLogLevel::Warn, serial, msg);
    }

    fn log_mirror_error(&mut self, serial: &str, msg: impl Into<String>) {
        self.log_mirror(AppLogLevel::Error, serial, msg);
    }

    fn log_mirror_status(&mut self, serial: &str, msg: &str) {
        let lower = msg.to_lowercase();
        let level = if lower.contains("failed") || lower.contains("error") {
            AppLogLevel::Error
        } else if lower.contains("stopped") || lower.contains("fallback") {
            AppLogLevel::Warn
        } else {
            AppLogLevel::Info
        };
        self.log_mirror(level, serial, msg);
    }

    fn log_defaulted_u32_input(&mut self, serial: &str, field: &str, raw: &str, default: u32) {
        self.log(
            AppLogLevel::Warn,
            format!("[{serial}] Invalid {field} '{raw}', using default {default}"),
        );
    }

    fn parse_u32_input_or_log(
        &mut self,
        serial: &str,
        field: &str,
        raw: &str,
        default: u32,
    ) -> u32 {
        raw.trim().parse::<u32>().unwrap_or_else(|_| {
            self.log_defaulted_u32_input(serial, field, raw, default);
            default
        })
    }

    fn require_bundle_id(&mut self, serial: &str, action: &str) -> Option<String> {
        let bundle_id = self.config.bundle_id.trim();
        if bundle_id.is_empty() {
            self.log_skipped(serial, action, "bundle ID is not configured");
            None
        } else {
            Some(bundle_id.to_string())
        }
    }

    fn close_device_tab(&mut self, serial: &str) {
        self.log(AppLogLevel::Info, format!("Tab closed: {serial}"));
        self.hidden_devices.insert(serial.to_string());

        // Stop watchers.
        if let Some(ds) = self.devices.get_mut(serial) {
            ds.stop_watcher();
            ds.stop_all_log_watchers();
            ds.stop_explorer_follow();
        }
        // Kill logcat.
        if let Some(mut child) = self.logcat_procs.remove(serial) {
            let _ = child.kill();
        }
        // Kill shell.
        if let Some(mut handle) = self.shell_handles.remove(serial) {
            handle.kill();
        }
        // Kill recording.
        if let Some(mut proc) = self.recording_procs.remove(serial) {
            let _ = proc.kill();
        }
        // Stop mirror.
        if let Some(ds) = self.devices.get_mut(serial) {
            ds.cancel_mirror_session("Stopped");
        }
        // Remove device state.
        self.devices.remove(serial);
        self.device_order.retain(|s| s != serial);

        // Fix active device.
        if self.active_device.as_deref() == Some(serial) {
            self.active_device = self.device_order.first().cloned();
        }
    }

    #[allow(clippy::too_many_lines)]
    fn drain_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                AdbMsg::DevicesUpdated(devs) => {
                    self.fatal_error = None;
                    self.handle_devices_updated(&devs);
                }
                AdbMsg::AdbNotFound(err) => {
                    self.log(AppLogLevel::Error, format!("ADB not found: {err}"));
                    self.fatal_error = Some(err);
                    self.status = "ADB not found".into();
                }
                AdbMsg::LogcatLine(serial, session, line) => {
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        if ds.logcat_session != session {
                            continue;
                        }
                        ds.push_logcat_line(line);
                    }
                }
                AdbMsg::LogcatStopped(serial, session, reason) => {
                    let is_current = self
                        .devices
                        .get(&serial)
                        .is_some_and(|ds| ds.logcat_session == session);
                    if !is_current {
                        self.log_stale_message(&serial, "logcat stop event");
                        continue;
                    }
                    self.log(
                        AppLogLevel::Warn,
                        format!("[{serial}] Logcat stopped: {reason}"),
                    );
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.logcat_ui.running = false;
                        ds.logcat_status = format!("Stopped: {reason}");
                    }
                    self.logcat_procs.remove(&serial);
                }
                AdbMsg::FileLog(serial, entry) => {
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.upsert_file(entry);
                        if ds.file_activity.pulling {
                            ds.file_status = format!("Pulling... {} file(s)", ds.file_logs.len());
                        }
                    }
                }
                AdbMsg::FileLogsDone(serial, count) => {
                    self.log(
                        AppLogLevel::Info,
                        format!("[{serial}] File pull done: {count} file(s)"),
                    );
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.file_activity.pulling = false;
                        ds.file_status = format!("Pulled {count} file(s)");
                    }
                }
                AdbMsg::FileWatchLog(serial, session, entry) => {
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        if ds.file_watch_session != session {
                            continue;
                        }
                        ds.upsert_file(entry);
                        ds.file_status = format!("Watching: {} file(s)", ds.file_logs.len());
                    }
                }
                AdbMsg::FileWatchCycle(serial, session, total) => {
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        if ds.file_watch_session != session {
                            continue;
                        }
                        ds.file_status = format!("Watching: {total} file(s)");
                    }
                }
                AdbMsg::FileWatchStopped(serial, session, reason) => {
                    let is_current = self
                        .devices
                        .get(&serial)
                        .is_some_and(|ds| ds.file_watch_session == session);
                    if !is_current {
                        self.log_stale_message(&serial, "file watcher stop event");
                        continue;
                    }
                    self.log(
                        AppLogLevel::Info,
                        format!("[{serial}] File watcher stopped: {reason}"),
                    );
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.file_activity.watching = false;
                        ds.file_status = format!("Watcher stopped: {reason}");
                    }
                }
                AdbMsg::ShellOutput(serial, line) => {
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.push_shell_output(line);
                    }
                }
                AdbMsg::ShellExited(serial, reason) => {
                    self.log(
                        AppLogLevel::Info,
                        format!("[{serial}] Shell exited: {reason}"),
                    );
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.shell.running = false;
                        ds.push_shell_output(format!("--- Shell exited: {reason} ---"));
                    }
                    self.shell_handles.remove(&serial);
                }
                AdbMsg::DeviceProps(serial, props) => {
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.device_props = props;
                        ds.loading.props = false;
                    } else {
                        self.log_missing_device_state(&serial, "device props update");
                    }
                }
                AdbMsg::DeviceActionResult(serial, msg) => {
                    let level = if msg.contains("FAILED")
                        || msg.contains("failed")
                        || msg.contains("Error")
                        || msg.contains("error:")
                    {
                        AppLogLevel::Error
                    } else {
                        AppLogLevel::Info
                    };
                    self.log(level, format!("[{serial}] {msg}"));
                    if level == AppLogLevel::Error {
                        self.show_app_log = true;
                    }
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.push_action_log(format!("{} {msg}", now_str()));
                    }
                }
                AdbMsg::ResolvedLaunchActivity(serial, component) => {
                    self.persist_resolved_activity(&serial, &component);
                }
                AdbMsg::ExplorerListing(serial, gen, path, entries) => {
                    let entry_count = entries.len();
                    let mut applied = false;
                    let mut missing_device = false;
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        applied = ds.finish_explorer_listing(gen, &path, entries);
                        if applied {
                            ds.push_explorer_log(format!(
                                "{} [LIST] OK path={} entries={}",
                                now_str(),
                                path,
                                entry_count
                            ));
                        }
                    } else {
                        missing_device = true;
                    }
                    if applied {
                        self.log(
                            AppLogLevel::Info,
                            format!(
                                "[{serial}] Explorer listing OK: {path} ({entry_count} entries)"
                            ),
                        );
                    } else if missing_device {
                        self.log_missing_device_state(&serial, "explorer listing result");
                    } else {
                        self.log_stale_message(&serial, "explorer listing result");
                    }
                }
                AdbMsg::ExplorerError(serial, gen, err) => {
                    let mut applied = false;
                    let mut missing_device = false;
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        applied = ds.fail_explorer_listing(gen, err.clone());
                        if applied {
                            ds.push_explorer_log(format!(
                                "{} [LIST] FAIL error={}",
                                now_str(),
                                truncate_inline_log(&err, 220)
                            ));
                        }
                    } else {
                        missing_device = true;
                    }
                    if applied {
                        self.log(
                            AppLogLevel::Error,
                            format!("[{serial}] Explorer listing failed: {err}"),
                        );
                    } else if missing_device {
                        self.log_missing_device_state(&serial, "explorer listing error");
                    } else {
                        self.log_stale_message(&serial, "explorer listing error");
                    }
                }
                AdbMsg::AvdList(avds) => {
                    self.available_avds = avds;
                    self.avds_loading = false;
                    // Also refresh running emulator map.
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        let map = adb::get_running_emulator_map();
                        let _ = tx.send(AdbMsg::RunningEmuMap(map));
                    });
                }
                AdbMsg::SystemImageList(images) => {
                    self.available_system_images = images;
                }
                AdbMsg::LogBuffer(serial, source_idx, result) => {
                    let Some(source) = LogSource::from_index(source_idx) else {
                        self.log(
                            AppLogLevel::Error,
                            format!("[{serial}] Ignored log buffer update with unknown source index {source_idx}"),
                        );
                        continue;
                    };

                    match result {
                        Ok(lines) => {
                            let count = lines.len();
                            let level = if count == 0 {
                                AppLogLevel::Warn
                            } else {
                                AppLogLevel::Info
                            };
                            self.log(
                                level,
                                format!("[{serial}] {} fetched: {count} lines", source.label()),
                            );
                            if let Some(ds) = self.devices.get_mut(&serial) {
                                ds.set_log_buffer(source, lines);
                            } else {
                                self.log_missing_device_state(&serial, "log buffer update");
                            }
                        }
                        Err(e) => {
                            self.log(
                                AppLogLevel::Error,
                                format!("[{serial}] {} error: {e}", source.label()),
                            );
                            if let Some(ds) = self.devices.get_mut(&serial) {
                                ds.log_loading.remove(&source);
                            } else {
                                self.log_missing_device_state(&serial, "log buffer error");
                            }
                        }
                    }
                }
                AdbMsg::RunningEmuMap(map) => {
                    self.running_emu_map = map;
                }
                AdbMsg::MonkeyDone(serial, ok, output) => {
                    let status = if ok { "PASSED" } else { "CRASHED/ANR" };
                    self.log(
                        if ok {
                            AppLogLevel::Info
                        } else {
                            AppLogLevel::Error
                        },
                        format!("[{serial}] Monkey {status}"),
                    );
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.test_runs.monkey = false;
                        // Show summary in action log.
                        let lines: Vec<&str> = output.lines().collect();
                        let summary = if lines.len() > 20 {
                            // Show last 20 lines for brevity.
                            format!(
                                "Monkey {status} ({} lines total)\n...last 20 lines...\n{}",
                                lines.len(),
                                lines[lines.len() - 20..].join("\n")
                            )
                        } else {
                            format!("Monkey {status}:\n{output}")
                        };
                        ds.push_action_log(format!("{} {summary}", now_str()));
                    }
                }
                AdbMsg::UiDump(serial, result) => {
                    if let Err(ref e) = result {
                        self.log(
                            AppLogLevel::Error,
                            format!("[{serial}] UI dump failed: {e}"),
                        );
                    }
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.loading.uiautomator = false;
                        match result {
                            Ok(xml) => {
                                ds.uiautomator_dump = Some(xml);
                            }
                            Err(e) => {
                                ds.push_action_log(format!("{} UI dump failed: {e}", now_str()));
                            }
                        }
                    }
                }
                AdbMsg::BugreportDone(serial, ok, msg) => {
                    let status = if ok { "OK" } else { "FAILED" };
                    self.log(AppLogLevel::Info, format!("[{serial}] Bugreport {status}"));
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.test_runs.bugreport = false;
                        ds.push_action_log(format!("{} Bugreport {status}: {msg}", now_str()));
                    }
                }
                AdbMsg::ScreenshotReady(serial, png_bytes, timestamp) => {
                    let mut decode_error = None;
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        match image::load_from_memory_with_format(
                            &png_bytes,
                            image::ImageFormat::Png,
                        ) {
                            Ok(img) => {
                                let w = img.width();
                                let h = img.height();
                                ds.screen_captures.push(crate::device::ScreenCapture {
                                    timestamp,
                                    png_bytes: std::sync::Arc::new(png_bytes),
                                    texture: None,
                                    width: w,
                                    height: h,
                                });
                                ds.screen_view_idx = Some(ds.screen_captures.len() - 1);
                                // Trim history.
                                if ds.screen_captures.len() > crate::device::MAX_SCREEN_CAPTURES {
                                    ds.screen_captures.remove(0);
                                    ds.screen_view_idx = Some(ds.screen_captures.len() - 1);
                                }
                                ds.screen_status =
                                    format!("{} capture(s)", ds.screen_captures.len());
                            }
                            Err(error) => {
                                decode_error = Some(error.to_string());
                                ds.screen_status = format!("Decode error: {error}");
                            }
                        }
                        ds.screen.capturing = false;
                    } else {
                        self.log_missing_device_state(&serial, "screenshot result");
                    }
                    if let Some(error) = decode_error {
                        self.log(
                            AppLogLevel::Error,
                            format!("[{serial}] Screenshot decode failed: {error}"),
                        );
                    }
                }
                AdbMsg::ScreenshotError(serial, err) => {
                    self.log(AppLogLevel::Error, format!("[{serial}] Screenshot: {err}"));
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.screen.capturing = false;
                        ds.screen_status = format!("Error: {err}");
                    }
                }
                AdbMsg::ExplorerPreview(serial, listing_gen, preview_gen, result) => {
                    let preview_error = result.as_ref().err().cloned();
                    #[allow(clippy::option_if_let_else)]
                    let preview_applied = if let Some(ds) = self.devices.get_mut(&serial) {
                        Some(ds.finish_explorer_preview(listing_gen, preview_gen, result))
                    } else {
                        self.log_missing_device_state(&serial, "explorer preview result");
                        None
                    };

                    match preview_applied {
                        Some(false) => {
                            self.log_stale_message(&serial, "explorer preview result");
                        }
                        Some(true) => {
                            if let Some(error) = preview_error {
                                self.log(
                                    AppLogLevel::Error,
                                    format!("[{serial}] Explorer preview failed: {error}"),
                                );
                            }
                        }
                        None => {}
                    }
                }
                AdbMsg::ExplorerReloadIfCurrent(serial, path) => {
                    let should_reload = self
                        .devices
                        .get(&serial)
                        .is_some_and(|ds| ds.explorer_visible_path() == path);
                    if should_reload {
                        self.explorer_navigate_no_history(&serial);
                    }
                }
                AdbMsg::ExplorerCommandResult(serial, session, result) => {
                    self.handle_explorer_command_report(&serial, session, &result);
                }
                AdbMsg::ExplorerCommandStopped(serial, session, status) => {
                    let mut is_current = false;
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        if ds.explorer_command_session == session {
                            ds.push_explorer_log(format!("{} [FOLLOW] {status}", now_str()));
                            is_current = ds.finish_explorer_command_session(session, status);
                        }
                    }
                    if is_current {
                        self.log(
                            AppLogLevel::Info,
                            format!("[{serial}] Explorer command stopped"),
                        );
                    } else {
                        self.log_stale_message(&serial, "explorer follow stop");
                    }
                }
                AdbMsg::DebugOutput(serial, cat_idx, result) => {
                    let Some(cat) = DebugCategory::from_index(cat_idx) else {
                        self.log(
                            AppLogLevel::Error,
                            format!("[{serial}] Ignored debug output with unknown category index {cat_idx}"),
                        );
                        continue;
                    };

                    match result {
                        Ok(text) => {
                            let lines = text.lines().count();
                            self.log(
                                AppLogLevel::Info,
                                format!("[{serial}] {} done: {lines} lines", cat.label()),
                            );
                            if let Some(ds) = self.devices.get_mut(&serial) {
                                ds.set_debug_output(cat, text);
                            } else {
                                self.log_missing_device_state(&serial, "debug output");
                            }
                        }
                        Err(e) => {
                            self.log(
                                AppLogLevel::Error,
                                format!("[{serial}] {} error: {e}", cat.label()),
                            );
                            if let Some(ds) = self.devices.get_mut(&serial) {
                                ds.debug_loading.remove(&cat);
                            } else {
                                self.log_missing_device_state(&serial, "debug error");
                            }
                        }
                    }
                }
                AdbMsg::DumpsysServiceList(serial, services) => {
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.dumpsys_services_list = services;
                    } else {
                        self.log_missing_device_state(&serial, "dumpsys service list update");
                    }
                }
                AdbMsg::AtraceCategories(serial, cats) => {
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.atrace_available_cats = cats;
                    } else {
                        self.log_missing_device_state(&serial, "atrace categories update");
                    }
                }
                AdbMsg::RunAsAvailability(serial, bundle_id, available) => {
                    if bundle_id != self.config.bundle_id {
                        self.log_stale_message(&serial, "run-as availability result");
                        continue;
                    }
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.deploy.run_as = if available {
                            CapabilityStatus::Available
                        } else {
                            CapabilityStatus::Unavailable
                        };
                    } else {
                        self.log_missing_device_state(&serial, "run-as availability update");
                    }
                }
                AdbMsg::DeployResult(serial, label, result) => match result {
                    Ok(msg) => {
                        self.log(
                            AppLogLevel::Info,
                            format!("[{serial}] Deploy {label}: {msg}"),
                        );
                        if let Some(ds) = self.devices.get_mut(&serial) {
                            ds.deploy.running = false;
                            ds.deploy.status = format!("{label}: OK");
                        }
                    }
                    Err(msg) => {
                        self.log(
                            AppLogLevel::Error,
                            format!("[{serial}] Deploy {label}: {msg}"),
                        );
                        if let Some(ds) = self.devices.get_mut(&serial) {
                            ds.deploy.running = false;
                            ds.deploy.status = format!("{label}: FAILED - {msg}");
                        }
                    }
                },
                AdbMsg::CrashLogcat(serial, result) => match result {
                    Ok(text) => {
                        let lines = text.lines().count();
                        self.log(
                            AppLogLevel::Info,
                            format!("[{serial}] Crash logcat: {lines} lines"),
                        );
                        if let Some(ds) = self.devices.get_mut(&serial) {
                            ds.deploy.crash_log = text;
                        }
                    }
                    Err(e) => {
                        self.log(
                            AppLogLevel::Error,
                            format!("[{serial}] Crash logcat error: {e}"),
                        );
                    }
                },
                AdbMsg::PullLogsResult(serial, result) => match result {
                    Ok(count) => {
                        self.log(
                            AppLogLevel::Info,
                            format!("[{serial}] Pulled {count} log file(s) to local folder"),
                        );
                    }
                    Err(e) => {
                        self.log(
                            AppLogLevel::Error,
                            format!("[{serial}] Pull logs failed: {e}"),
                        );
                    }
                },
                AdbMsg::MonitorOutput(serial, cat_idx, result) => {
                    let Some(cat) = MonitorCategory::from_index(cat_idx) else {
                        self.log(
                            AppLogLevel::Error,
                            format!("[{serial}] Ignored monitor output with unknown category index {cat_idx}"),
                        );
                        continue;
                    };

                    match result {
                        Ok(text) => {
                            let lines = text.lines().count();
                            self.log(
                                AppLogLevel::Info,
                                format!("[{serial}] {} done: {lines} lines", cat.label()),
                            );
                            if let Some(ds) = self.devices.get_mut(&serial) {
                                ds.set_monitor_output(cat, text);
                            } else {
                                self.log_missing_device_state(&serial, "monitor output");
                            }
                        }
                        Err(e) => {
                            self.log(
                                AppLogLevel::Error,
                                format!("[{serial}] {} error: {e}", cat.label()),
                            );
                            if let Some(ds) = self.devices.get_mut(&serial) {
                                ds.monitor_loading.remove(&cat);
                            } else {
                                self.log_missing_device_state(&serial, "monitor error");
                            }
                        }
                    }
                }
                AdbMsg::MirrorStopped(serial, session, reason) => {
                    let is_current = self
                        .devices
                        .get(&serial)
                        .is_some_and(|ds| ds.mirror_session == session);
                    if !is_current {
                        self.log_mirror_warn(&serial, "Ignored stale stop event");
                        continue;
                    }

                    let was_active = self.devices.get(&serial).is_some_and(|ds| ds.mirror.active);
                    if was_active {
                        self.log_mirror_warn(&serial, format!("Stopped: {reason}"));
                    }
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        // Full cleanup — matches stop_mirroring().
                        ds.finish_mirror_session(format!("Stopped: {reason}"));
                    }
                }
                AdbMsg::MirrorDisplaySize(serial, session, w, h) => {
                    let is_current = self
                        .devices
                        .get(&serial)
                        .is_some_and(|ds| ds.mirror_session == session);
                    if !is_current {
                        self.log_mirror_warn(&serial, "Ignored stale display-size event");
                        continue;
                    }
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.mirror.device_width = w;
                        ds.mirror.device_height = h;
                    }
                    self.log_mirror_info(&serial, format!("Device display size: {w}x{h}"));
                }
                AdbMsg::MirrorDisplayState(serial, session, w, h, rotation, mode) => {
                    let is_current = self
                        .devices
                        .get(&serial)
                        .is_some_and(|ds| ds.mirror_session == session);
                    if !is_current {
                        self.log_mirror_warn(&serial, "Ignored stale display-state event");
                        continue;
                    }

                    let mut rotation_changed = false;
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        rotation_changed = ds
                            .mirror
                            .current_rotation
                            .is_some_and(|prev| prev != rotation);
                        ds.mirror.current_rotation = Some(rotation);
                        ds.mirror.device_width = w;
                        ds.mirror.device_height = h;
                        ds.mirror_rotation_mode = mode;
                    }

                    if rotation_changed {
                        self.log_mirror_info(
                            &serial,
                            format!(
                                "Display rotated to {} ({}x{}), restarting mirror",
                                rotation.label(),
                                w,
                                h
                            ),
                        );
                        if let Some(ds) = self.devices.get_mut(&serial) {
                            ds.cancel_mirror_session("Restarting after display rotation");
                        }
                        self.start_mirroring(&serial);
                    }
                }
                AdbMsg::MirrorRotationResult(serial, mode, result) => match result {
                    Ok(()) => {
                        if let Some(ds) = self.devices.get_mut(&serial) {
                            ds.mirror_rotation_mode = mode;
                        }
                        self.log_mirror_info(&serial, format!("Rotation set to {}", mode.label()));
                    }
                    Err(error) => {
                        self.log_mirror_error(
                            &serial,
                            format!("Failed to set rotation to {}: {error}", mode.label()),
                        );
                    }
                },
                AdbMsg::MirrorServerStatus(serial, installed, running, msg) => {
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.mirror_server.installed = installed;
                        ds.mirror_server.running = running;
                        ds.mirror_server.busy = false;
                        ds.mirror_server.status = msg;
                    }
                    if let Some(status) = self
                        .devices
                        .get(&serial)
                        .map(|ds| ds.mirror_server.status.clone())
                    {
                        self.log_mirror_status(&serial, &status);
                    } else {
                        self.log_missing_device_state(&serial, "mirror server status");
                    }
                }
                AdbMsg::MirrorLog(serial, level, msg) => match level {
                    adb::AdbLogLevel::Info => self.log_mirror_info(&serial, msg),
                    adb::AdbLogLevel::Warn => self.log_mirror_warn(&serial, msg),
                    adb::AdbLogLevel::Error => self.log_mirror_error(&serial, msg),
                },
            }
        }

        // Rebuild sorted file lists once per frame (not per message).
        for ds in self.devices.values_mut() {
            ds.flush_if_dirty();
        }
    }

    fn handle_devices_updated(&mut self, devs: &[DeviceInfo]) {
        let new_serials: Vec<String> = devs.iter().map(|d| d.serial.clone()).collect();
        self.device_order.retain(|s| new_serials.contains(s));

        // Clear hidden status for devices that disconnected and reconnected.
        self.hidden_devices.retain(|s| new_serials.contains(s));

        for dev in devs {
            self.ensure_streamer_device_alias(&dev.serial);
            self.streamer_device_models
                .insert(dev.serial.clone(), dev.model.clone());
            if self.hidden_devices.contains(&dev.serial) {
                continue; // User closed this tab — don't re-add.
            }
            if !self.devices.contains_key(&dev.serial) {
                let label = if dev.model == "unknown" {
                    dev.serial.clone()
                } else {
                    format!("{} ({})", dev.model, dev.serial)
                };
                self.log(
                    AppLogLevel::Info,
                    format!("Device connected: {label} [{}]", dev.state),
                );
                self.device_order.push(dev.serial.clone());
                let mut ds = DeviceState::new(dev.clone());
                if dev.state == "device" {
                    let session = ds.start_next_logcat_session();
                    self.log(
                        AppLogLevel::Info,
                        format!("[{}] Logcat starting...", dev.serial),
                    );
                    ds.logcat_ui.running = true;
                    ds.logcat_status = "Starting...".into();
                    if let Some(child) =
                        adb::spawn_logcat(&dev.serial, session, self.tx.clone(), &self.config)
                    {
                        self.logcat_procs.insert(dev.serial.clone(), child);
                        ds.logcat_status = "Running".into();
                        self.log(
                            AppLogLevel::Info,
                            format!("[{}] Logcat process spawned", dev.serial),
                        );
                    } else {
                        ds.logcat_ui.running = false;
                        ds.logcat_status = "Failed to start".into();
                        self.log(
                            AppLogLevel::Error,
                            format!("[{}] spawn_logcat returned None", dev.serial),
                        );
                    }
                }
                self.devices.insert(dev.serial.clone(), ds);

                // Auto-check run-as availability for the configured app.
                if dev.state == "device" && !self.config.bundle_id.is_empty() {
                    let serial = dev.serial.clone();
                    let bid = self.config.bundle_id.clone();
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        let ok = adb::check_run_as(&serial, &bid);
                        let _ = tx.send(AdbMsg::RunAsAvailability(serial, bid, ok));
                    });
                }
            }
        }

        // Log disconnected devices.
        let disconnected: Vec<String> = self
            .devices
            .keys()
            .filter(|s| !new_serials.contains(s))
            .cloned()
            .collect();
        for serial in &disconnected {
            self.log(AppLogLevel::Warn, format!("Device disconnected: {serial}"));
        }

        for (serial, ds) in &mut self.devices {
            if !new_serials.contains(serial) {
                ds.stop_watcher();
                ds.stop_all_log_watchers();
                ds.stop_explorer_follow();
                ds.cancel_mirror_session("Device disconnected");
            }
        }
        self.devices.retain(|s, _| new_serials.contains(s));
        self.shell_handles.retain(|s, handle| {
            if new_serials.contains(s) {
                true
            } else {
                let _ = handle.child.kill();
                false
            }
        });
        self.recording_procs.retain(|s, proc| {
            if new_serials.contains(s) {
                true
            } else {
                let _ = proc.kill();
                false
            }
        });
        self.logcat_procs.retain(|s, child| {
            if new_serials.contains(s) {
                true
            } else {
                let _ = child.kill();
                false
            }
        });

        if self.active_device.is_none()
            || !self
                .device_order
                .contains(self.active_device.as_ref().unwrap_or(&String::new()))
        {
            self.active_device = self.device_order.first().cloned();
        }

        let count = self.device_order.len();
        self.status = format!("{count} device(s) connected");
    }

    fn maybe_poll_devices(&mut self, now: f64) {
        if self.fatal_error.is_some() {
            return;
        }
        if now - self.last_device_poll > DEVICE_POLL_INTERVAL {
            self.last_device_poll = now;
            let tx = self.tx.clone();
            std::thread::spawn(move || match adb::list_devices() {
                Ok(devs) => {
                    let _ = tx.send(AdbMsg::DevicesUpdated(devs));
                }
                Err(e) => {
                    let _ = tx.send(AdbMsg::AdbNotFound(e));
                }
            });
        }
    }

    fn repaint_interval(&self) -> Duration {
        let has_active_work = self.devices.values().any(|device| {
            device.logcat_ui.running
                || device.file_activity.pulling
                || device.file_activity.watching
                || device.shell.running
                || device.screen.capturing
                || device.screen.recording
                || device.screen_auto_interval.is_some()
                || device.test_runs.monkey
                || device.test_runs.bugreport
                || device.deploy.running
                || device.loading.props
                || device.loading.explorer
                || device.loading.uiautomator
                || !device.log_loading.is_empty()
                || !device.log_watchers.is_empty()
                || !device.debug_loading.is_empty()
                || !device.monitor_loading.is_empty()
                || device.mirror.active
                || device.mirror_server.busy
        });

        if has_active_work {
            ACTIVE_REPAINT_INTERVAL
        } else {
            IDLE_REPAINT_INTERVAL
        }
    }

    fn shutdown(&mut self) {
        for device in self.devices.values_mut() {
            device.stop_watcher();
            device.stop_all_log_watchers();
            device.stop_explorer_follow();
            device.cancel_mirror_session("Stopped");
            device.screen_auto_interval = None;
            device.screen.capturing = false;
            device.screen.recording = false;
        }

        for (_, mut child) in self.logcat_procs.drain() {
            let _ = child.kill();
        }
        for (_, mut handle) in self.shell_handles.drain() {
            handle.kill();
        }
        for (_, mut process) in self.recording_procs.drain() {
            let _ = process.kill();
        }
    }
}

fn truncate_inline_log(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let shortened: String = text.chars().take(max_chars).collect();
        format!("{shortened}...")
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = ctx.input(|i| i.time);
        self.drain_messages();
        self.maybe_poll_streamer_mode(now);
        self.maybe_poll_devices(now);
        ctx.request_repaint_after(self.repaint_interval());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Top bar: action row + device tabs.
        egui::Panel::top("top_bar").show_inside(ui, |ui| {
            // Row 1: status + action buttons.
            ui.horizontal(|ui| {
                ui.label(self.display_text(&self.status));
                if let Some(badge) = self.streamer_mode_badge() {
                    ui.separator();
                    ui.label(badge);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let settings_label = if self.show_settings {
                        "Close Settings"
                    } else {
                        "Settings"
                    };
                    if ui.button(settings_label).clicked() {
                        self.show_settings = !self.show_settings;
                        if self.show_settings {
                            self.bundle_id_input.clone_from(&self.config.bundle_id);
                            self.activity_class_input
                                .clone_from(&self.config.activity_class);
                            self.log_tags_input = self.config.logcat_tags.join("\n");
                        }
                    }

                    let log_label = if self.show_app_log {
                        "Close Log"
                    } else {
                        "App Log"
                    };
                    if ui.button(log_label).clicked() {
                        self.show_app_log = !self.show_app_log;
                    }

                    let devices_label = if self.show_devices {
                        "Close Devices"
                    } else {
                        "Devices"
                    };
                    if ui.button(devices_label).clicked() {
                        self.show_devices = !self.show_devices;
                        if self.show_devices && self.available_avds.is_empty() && !self.avds_loading
                        {
                            self.avds_loading = true;
                            let tx = self.tx.clone();
                            std::thread::spawn(move || {
                                let avds = adb::list_avds();
                                let _ = tx.send(AdbMsg::AvdList(avds));
                            });
                        }
                    }

                    if ui.button("Refresh Devices").clicked() {
                        self.fatal_error = None;
                        self.last_device_poll = 0.0;
                        self.hidden_devices.clear();
                    }
                });
            });

            // Row 2: device tabs.
            if !self.device_order.is_empty() {
                ui.horizontal(|ui| {
                    let order = self.device_order.clone();
                    let mut close_serial: Option<String> = None;
                    for serial in &order {
                        if let Some(ds) = self.devices.get(serial) {
                            let label = self.display_device_label(serial, &ds.info.model);
                            let selected = self.active_device.as_ref() == Some(serial);
                            let color = if ds.info.state == "device" {
                                egui::Color32::from_rgb(100, 200, 100)
                            } else {
                                egui::Color32::from_rgb(200, 100, 100)
                            };

                            // Measure tab content size.
                            let font = egui::FontId::default();
                            let label_galley =
                                ui.painter()
                                    .layout_no_wrap(label.clone(), font.clone(), color);
                            let label_w = label_galley.size().x;
                            let x_w = 14.0;
                            let padding_x = 8.0;
                            let padding_y = 4.0;
                            let tab_w = label_w + x_w + padding_x * 2.0 + 4.0;
                            let tab_h = label_galley.size().y + padding_y * 2.0;

                            let (tab_rect, tab_response) = ui.allocate_exact_size(
                                egui::vec2(tab_w, tab_h),
                                egui::Sense::click(),
                            );

                            // Background.
                            if selected {
                                ui.painter().rect_filled(
                                    tab_rect,
                                    3.0,
                                    egui::Color32::from_rgb(50, 50, 60),
                                );
                            } else if tab_response.hovered() {
                                ui.painter().rect_filled(
                                    tab_rect,
                                    3.0,
                                    egui::Color32::from_rgb(40, 40, 45),
                                );
                            }

                            // Label text.
                            ui.painter().galley(
                                egui::pos2(tab_rect.min.x + padding_x, tab_rect.min.y + padding_y),
                                label_galley,
                                color,
                            );

                            // X button area (right side of tab).
                            let x_rect = egui::Rect::from_min_size(
                                egui::pos2(tab_rect.max.x - padding_x - x_w, tab_rect.min.y),
                                egui::vec2(x_w + padding_x, tab_h),
                            );
                            let x_resp = ui.allocate_rect(x_rect, egui::Sense::click());
                            let x_color = if x_resp.hovered() {
                                egui::Color32::from_rgb(255, 100, 100)
                            } else {
                                egui::Color32::from_rgb(140, 140, 140)
                            };
                            ui.painter().text(
                                x_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "x",
                                egui::FontId::proportional(11.0),
                                x_color,
                            );

                            // Left click on tab = select.
                            if tab_response.clicked() {
                                self.active_device = Some(serial.clone());
                            }

                            // Middle click on tab OR click X = close.
                            if tab_response.middle_clicked() || x_resp.clicked() {
                                close_serial = Some(serial.clone());
                            }
                        }
                    }

                    // Process close outside the loop.
                    if let Some(serial) = close_serial {
                        self.close_device_tab(&serial);
                    }
                });
            }
        });

        // Settings panel (right side).
        if self.show_settings {
            egui::Panel::right("settings_panel")
                .default_size(350.0)
                .show_inside(ui, |ui| {
                    self.draw_settings(ui);
                });
        }

        // Devices panel (left side).
        if self.show_devices {
            egui::Panel::left("devices_panel")
                .default_size(380.0)
                .show_inside(ui, |ui| {
                    self.draw_devices_panel(ui);
                });
        }

        // App Log panel (bottom).
        if self.show_app_log {
            egui::Panel::bottom("app_log_panel")
                .default_size(200.0)
                .show_inside(ui, |ui| {
                    self.draw_app_log_tab(ui);
                });
        }

        // Central panel.
        egui::CentralPanel::default().show_inside(ui, |ui| {
            if let Some(ref err) = self.fatal_error.clone() {
                self.draw_adb_not_found(ui, err);
            } else {
                let active = self.active_device.clone();
                if let Some(serial) = active {
                    self.draw_device_panel(ui, &serial);
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label("No devices connected. Waiting for ADB devices...");
                    });
                }
            }
        });
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl App {
    // ─── Devices panel (sidebar) ─────────────────────────────────────────────

    fn draw_devices_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Devices");
        ui.separator();

        egui::ScrollArea::vertical()
            .id_salt("devices_panel_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // ── Connect ──────────────────────────────────────
                ui.label(egui::RichText::new("Connect").strong());
                ui.add_space(2.0);

                ui.horizontal(|ui| {
                    ui.label("WiFi/TCP:");
                    if self.streamer_mode_active() {
                        let mut masked = self.display_text(&self.wifi_connect_addr);
                        ui.add_enabled(
                            false,
                            egui::TextEdit::singleline(&mut masked).hint_text("<address>"),
                        );
                    } else {
                        ui.add_sized(
                            [160.0, 18.0],
                            egui::TextEdit::singleline(&mut self.wifi_connect_addr)
                                .hint_text("192.168.1.x:5555"),
                        );
                    }
                    if ui.button("Connect").clicked() && !self.wifi_connect_addr.is_empty() {
                        let addr = self.wifi_connect_addr.clone();
                        let tx = self.tx.clone();
                        self.log(AppLogLevel::Info, format!("Connecting to {addr}..."));
                        std::thread::spawn(move || {
                            let (ok, msg) = adb::adb_connect(&addr);
                            let _ = tx
                                .send(AdbMsg::DeviceActionResult(addr, format!("Connect: {msg}")));
                            let _ = ok;
                        });
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("WSA Port:");
                    ui.add_sized(
                        [50.0, 18.0],
                        egui::TextEdit::singleline(&mut self.wsa_port).hint_text("58526"),
                    );
                    if ui.button("Connect WSA").clicked() {
                        let addr = format!("127.0.0.1:{}", self.wsa_port.trim());
                        let tx = self.tx.clone();
                        self.log(AppLogLevel::Info, format!("Connecting to WSA ({addr})..."));
                        std::thread::spawn(move || {
                            let (ok, msg) = adb::adb_connect(&addr);
                            let status = if ok {
                                "WSA connected"
                            } else {
                                "WSA connect failed"
                            };
                            let _ = tx
                                .send(AdbMsg::DeviceActionResult(addr, format!("{status}: {msg}")));
                        });
                    }
                });

                ui.horizontal(|ui| {
                    if ui.button("WSA Settings").clicked() && !adb::open_wsa_settings() {
                        self.log(
                            AppLogLevel::Error,
                            "Failed to open WSA Settings - is WSA installed?",
                        );
                    }
                    if ui.button("Launch WSA").clicked() && !adb::launch_wsa() {
                        self.log(AppLogLevel::Error, "Failed to launch WSA");
                    }
                    if ui.button("Disconnect All TCP").clicked() {
                        let tx = self.tx.clone();
                        std::thread::spawn(move || {
                            let (_, msg) = adb::adb_disconnect_all();
                            let _ = tx.send(AdbMsg::DeviceActionResult(
                                "all".into(),
                                format!("Disconnect all: {msg}"),
                            ));
                        });
                    }
                });

                ui.add_space(4.0);

                // Wireless pairing.
                ui.horizontal(|ui| {
                    ui.label("Pair:");
                    if self.streamer_mode_active() {
                        let mut masked_addr = self.display_text(&self.pair_address_input);
                        let mut masked_code = self.display_text(&self.pair_code_input);
                        ui.add_enabled(
                            false,
                            egui::TextEdit::singleline(&mut masked_addr).hint_text("<address>"),
                        );
                        ui.add_enabled(
                            false,
                            egui::TextEdit::singleline(&mut masked_code).hint_text("<pair-code>"),
                        );
                    } else {
                        ui.add_sized(
                            [120.0, 18.0],
                            egui::TextEdit::singleline(&mut self.pair_address_input)
                                .hint_text("host:port"),
                        );
                        ui.add_sized(
                            [70.0, 18.0],
                            egui::TextEdit::singleline(&mut self.pair_code_input).hint_text("code"),
                        );
                    }
                    let can_pair = !self.pair_address_input.trim().is_empty()
                        && !self.pair_code_input.trim().is_empty();
                    if ui
                        .add_enabled(can_pair, egui::Button::new("Pair"))
                        .clicked()
                    {
                        let addr = self.pair_address_input.trim().to_string();
                        let code = self.pair_code_input.trim().to_string();
                        let tx = self.tx.clone();
                        self.log(AppLogLevel::Info, format!("Pairing with {addr}..."));
                        std::thread::spawn(move || {
                            let (ok, msg) = adb::adb_pair(&addr, &code);
                            let s = if ok { "Paired" } else { "Pair failed" };
                            let _ =
                                tx.send(AdbMsg::DeviceActionResult(addr, format!("{s}: {msg}")));
                        });
                    }
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                // ── Connected Devices ────────────────────────────
                ui.label(egui::RichText::new("Connected Devices").strong());
                ui.add_space(2.0);

                if self.device_order.is_empty() {
                    ui.colored_label(
                        egui::Color32::from_rgb(140, 140, 140),
                        "No devices connected.",
                    );
                } else {
                    let order = self.device_order.clone();
                    for serial in &order {
                        if let Some(ds) = self.devices.get(serial) {
                            let is_emu = adb::is_emulator_serial(serial);
                            let state_color = if ds.info.state == "device" {
                                egui::Color32::from_rgb(100, 200, 100)
                            } else {
                                egui::Color32::from_rgb(200, 100, 100)
                            };
                            ui.horizontal(|ui| {
                                ui.colored_label(state_color, &ds.info.state);
                                ui.label(
                                    egui::RichText::new(self.display_model(serial, &ds.info.model))
                                        .monospace()
                                        .small(),
                                );
                                ui.colored_label(
                                    egui::Color32::from_rgb(120, 120, 120),
                                    self.display_serial(serial),
                                );
                                if is_emu {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(180, 130, 255),
                                        "[emu]",
                                    );
                                }
                                if adb::is_tcp_device(serial)
                                    && ui.small_button("Disconnect").clicked()
                                {
                                    let addr = serial.clone();
                                    let tx = self.tx.clone();
                                    std::thread::spawn(move || {
                                        let (_, msg) = adb::adb_disconnect(&addr);
                                        let _ = tx.send(AdbMsg::DeviceActionResult(
                                            addr,
                                            format!("Disconnect: {msg}"),
                                        ));
                                    });
                                }
                                if is_emu && ui.small_button("Kill").clicked() {
                                    let es = serial.clone();
                                    let tx = self.tx.clone();
                                    std::thread::spawn(move || {
                                        let (ok, msg) = adb::kill_emulator(&es);
                                        let s = if ok { "Killed" } else { "Kill failed" };
                                        let _ = tx.send(AdbMsg::DeviceActionResult(
                                            es,
                                            format!("{s}: {msg}"),
                                        ));
                                    });
                                }
                            });
                        }
                    }
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                // ── Emulator Management ──────────────────────────
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Emulators (AVDs)").strong());
                    if ui.small_button("Refresh").clicked() {
                        self.avds_loading = true;
                        let tx = self.tx.clone();
                        std::thread::spawn(move || {
                            let avds = adb::list_avds();
                            let _ = tx.send(AdbMsg::AvdList(avds));
                        });
                    }
                    if self.avds_loading {
                        ui.spinner();
                    }
                });
                ui.add_space(2.0);

                if !self.available_avds.is_empty() {
                    for avd in self.available_avds.clone() {
                        let is_running = self.running_emu_map.values().any(|name| name == &avd);
                        ui.horizontal(|ui| {
                            let status_color = if is_running {
                                egui::Color32::from_rgb(100, 200, 100)
                            } else {
                                egui::Color32::from_rgb(160, 160, 160)
                            };
                            ui.colored_label(status_color, if is_running { "ON " } else { "OFF" });
                            ui.label(egui::RichText::new(&avd).monospace());

                            if !is_running {
                                if ui.small_button("Start").clicked() {
                                    let avd = avd.clone();
                                    let tx = self.tx.clone();
                                    self.log(
                                        AppLogLevel::Info,
                                        format!("Starting emulator: {avd}"),
                                    );
                                    std::thread::spawn(move || {
                                        let (ok, msg) = adb::start_emulator(&avd, false);
                                        let s = if ok { "OK" } else { "FAILED" };
                                        let _ = tx.send(AdbMsg::DeviceActionResult(
                                            "emulator".into(),
                                            format!("Start {avd}: {s} - {msg}"),
                                        ));
                                    });
                                }
                                if ui
                                    .small_button("Cold Boot")
                                    .on_hover_text("Start without snapshot")
                                    .clicked()
                                {
                                    let avd = avd.clone();
                                    let tx = self.tx.clone();
                                    self.log(
                                        AppLogLevel::Info,
                                        format!("Cold-booting emulator: {avd}"),
                                    );
                                    std::thread::spawn(move || {
                                        let (ok, msg) = adb::start_emulator(&avd, true);
                                        let s = if ok { "OK" } else { "FAILED" };
                                        let _ = tx.send(AdbMsg::DeviceActionResult(
                                            "emulator".into(),
                                            format!("Cold boot {avd}: {s} - {msg}"),
                                        ));
                                    });
                                }
                            } else if ui.small_button("Kill").clicked() {
                                let emu_serial = self
                                    .running_emu_map
                                    .iter()
                                    .find(|(_, name)| name.as_str() == avd.as_str())
                                    .map(|(serial, _)| serial.clone());
                                if let Some(es) = emu_serial {
                                    let tx = self.tx.clone();
                                    std::thread::spawn(move || {
                                        let (ok, msg) = adb::kill_emulator(&es);
                                        let s = if ok { "Killed" } else { "Kill failed" };
                                        let _ = tx.send(AdbMsg::DeviceActionResult(
                                            es,
                                            format!("{s}: {msg}"),
                                        ));
                                    });
                                }
                            }

                            if !is_running && ui.small_button("Delete").clicked() {
                                let avd = avd.clone();
                                let tx = self.tx.clone();
                                self.log(AppLogLevel::Warn, format!("Deleting AVD: {avd}"));
                                std::thread::spawn(move || {
                                    let (ok, msg) = adb::delete_avd(&avd);
                                    let s = if ok { "Deleted" } else { "Delete failed" };
                                    let _ = tx.send(AdbMsg::DeviceActionResult(
                                        "emulator".into(),
                                        format!("AVD {avd}: {s} - {msg}"),
                                    ));
                                    let avds = adb::list_avds();
                                    let _ = tx.send(AdbMsg::AvdList(avds));
                                });
                            }
                        });
                    }
                } else if !self.avds_loading {
                    ui.colored_label(
                        egui::Color32::from_rgb(140, 140, 140),
                        "No AVDs found. Click Refresh.",
                    );
                }

                ui.add_space(8.0);

                // Create AVD form.
                ui.label(egui::RichText::new("Create AVD").strong());
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.add_sized(
                        [100.0, 18.0],
                        egui::TextEdit::singleline(&mut self.new_avd_name).hint_text("my_avd"),
                    );
                    ui.label("Device:");
                    ui.add_sized(
                        [80.0, 18.0],
                        egui::TextEdit::singleline(&mut self.new_avd_device).hint_text("pixel_6"),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("Image:");
                    if self.available_system_images.is_empty() {
                        ui.add_sized(
                            [200.0, 18.0],
                            egui::TextEdit::singleline(&mut self.new_avd_image)
                                .hint_text("system-images;android-34;..."),
                        );
                        if ui.small_button("Scan Images").clicked() {
                            let tx = self.tx.clone();
                            std::thread::spawn(move || {
                                let images = adb::list_system_images();
                                let _ = tx.send(AdbMsg::SystemImageList(images));
                            });
                        }
                    } else {
                        egui::ComboBox::from_id_salt("devpanel_avd_image")
                            .selected_text(if self.new_avd_image.is_empty() {
                                "Select image..."
                            } else {
                                &self.new_avd_image
                            })
                            .width(280.0)
                            .show_ui(ui, |ui| {
                                for img in self.available_system_images.clone() {
                                    ui.selectable_value(&mut self.new_avd_image, img.clone(), &img);
                                }
                            });
                    }
                });
                ui.horizontal(|ui| {
                    let can_create = !self.new_avd_name.trim().is_empty()
                        && !self.new_avd_image.trim().is_empty();
                    if ui
                        .add_enabled(can_create, egui::Button::new("Create AVD"))
                        .clicked()
                    {
                        let name = self.new_avd_name.trim().to_string();
                        let image = self.new_avd_image.trim().to_string();
                        let device = self.new_avd_device.trim().to_string();
                        let tx = self.tx.clone();
                        self.log(AppLogLevel::Info, format!("Creating AVD: {name} ({image})"));
                        std::thread::spawn(move || {
                            let (ok, msg) = adb::create_avd(&name, &image, &device);
                            let s = if ok { "Created" } else { "Create failed" };
                            let _ = tx.send(AdbMsg::DeviceActionResult(
                                "emulator".into(),
                                format!("AVD {name}: {s} - {msg}"),
                            ));
                            let avds = adb::list_avds();
                            let _ = tx.send(AdbMsg::AvdList(avds));
                        });
                    }
                });
            });
    }

    // ─── Device panel ────────────────────────────────────────────────────────

    fn draw_device_panel(&mut self, ui: &mut egui::Ui, serial: &str) {
        let serial_owned = serial.to_string();
        let active_sub = self.devices.get(serial).map_or(0, |ds| ds.active_sub_tab);

        ui.horizontal(|ui| {
            if ui.selectable_label(active_sub == 0, "Logs").clicked() {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.active_sub_tab = 0;
                }
            }
            if ui.selectable_label(active_sub == 1, "File Logs").clicked() {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.active_sub_tab = 1;
                }
            }
            if ui.selectable_label(active_sub == 2, "Shell").clicked() {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.active_sub_tab = 2;
                }
            }
            if ui.selectable_label(active_sub == 3, "Screen").clicked() {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.active_sub_tab = 3;
                }
            }
            if ui.selectable_label(active_sub == 4, "Explorer").clicked() {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.active_sub_tab = 4;
                }
            }
            if ui.selectable_label(active_sub == 5, "Device").clicked() {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.active_sub_tab = 5;
                }
            }
            if ui.selectable_label(active_sub == 6, "Debug").clicked() {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.active_sub_tab = 6;
                }
            }
            if ui.selectable_label(active_sub == 7, "Monitor").clicked() {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.active_sub_tab = 7;
                }
            }
            if ui.selectable_label(active_sub == 8, "Deploy").clicked() {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.active_sub_tab = 8;
                }
            }
            if ui.selectable_label(active_sub == 9, "Mirror").clicked() {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.active_sub_tab = 9;
                }
            }
        });
        ui.separator();

        match active_sub {
            0 => self.draw_logs_tab(ui, &serial_owned),
            1 => self.draw_file_logs_tab(ui, &serial_owned),
            2 => self.draw_shell_tab(ui, &serial_owned),
            3 => self.draw_screen_tab(ui, &serial_owned),
            4 => self.draw_explorer_tab(ui, &serial_owned),
            5 => self.draw_device_tab(ui, &serial_owned),
            6 => self.draw_debug_tab(ui, &serial_owned),
            7 => self.draw_monitor_tab(ui, &serial_owned),
            8 => self.draw_deploy_tab(ui, &serial_owned),
            9 => self.draw_mirror_tab(ui, &serial_owned),
            _ => {}
        }
    }
}
