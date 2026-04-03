mod app_log;
mod debug;
mod deploy;
mod device_tab;
mod explorer;
mod file_logs;
mod helpers;
mod logs;
mod monitor;
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

pub struct App {
    pub rx: Receiver<AdbMsg>,
    pub tx: Sender<AdbMsg>,
    pub devices: HashMap<String, DeviceState>,
    pub device_order: Vec<String>,
    pub active_device: Option<String>,
    pub logcat_procs: HashMap<String, Child>,
    pub last_device_poll: f64,
    pub status: String,
    pub fatal_error: Option<String>,
    pub adb_path_candidate: String,
    pub adb_override_message: String,
    pub config: AppConfig,
    pub config_path: PathBuf,
    pub show_settings: bool,
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
            status: "Scanning for devices...".into(),
            fatal_error: None,
            adb_path_candidate: String::new(),
            adb_override_message: String::new(),
            config,
            config_path,
            show_settings: false,
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
            hidden_devices: std::collections::HashSet::new(),
            available_avds: Vec::new(),
            avds_loading: false,
            new_avd_name: String::new(),
            new_avd_image: String::new(),
            new_avd_device: "pixel_6".into(),
            available_system_images: Vec::new(),
            running_emu_map: HashMap::new(),
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
                    if let Some(ds) = self.devices.get_mut(&serial) {
                        ds.push_action_log(format!("{} {msg}", now_str()));
                    }
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
        self.maybe_poll_devices(now);
        ctx.request_repaint_after(self.repaint_interval());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Top bar.
        egui::Panel::top("top_bar").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                ui.separator();

                // Device tabs inline.
                let order = self.device_order.clone();
                let mut close_serial: Option<String> = None;
                for serial in &order {
                    if let Some(ds) = self.devices.get(serial) {
                        let label = ds.label();
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

                        let (tab_rect, tab_response) =
                            ui.allocate_exact_size(egui::vec2(tab_w, tab_h), egui::Sense::click());

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

                ui.separator();
                if ui.button("Refresh Devices").clicked() {
                    self.fatal_error = None;
                    self.last_device_poll = 0.0;
                    self.hidden_devices.clear(); // un-hide all closed tabs
                }
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
            });
        });

        // Settings panel (right side).
        if self.show_settings {
            egui::Panel::right("settings_panel")
                .default_size(350.0)
                .show_inside(ui, |ui| {
                    self.draw_settings(ui);
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
            if ui.selectable_label(active_sub == 9, "App Log").clicked() {
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
            9 => self.draw_app_log_tab(ui),
            _ => {}
        }
    }
}
