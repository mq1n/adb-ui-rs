use eframe::egui;

use super::AppLogLevel;
use crate::adb;
use crate::config::AppConfig;
use crate::device::CapabilityStatus;

use super::FILE_WATCH_INTERVAL;

impl super::App {
    // ─── Settings ────────────────────────────────────────────────────────────

    pub(super) fn draw_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.separator();
        self.draw_settings_fields(ui);
        self.draw_platform_tools(ui);
        self.draw_settings_actions(ui);
        self.draw_settings_summary(ui);
    }

    fn draw_settings_fields(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Bundle ID / Package Name").strong());
        ui.add_space(2.0);
        if self.streamer_mode_active() {
            let mut masked = self.display_text(&self.bundle_id_input);
            ui.add_enabled(false, egui::TextEdit::singleline(&mut masked));
        } else {
            ui.text_edit_singleline(&mut self.bundle_id_input);
        }
        ui.add_space(12.0);

        ui.label(egui::RichText::new("Activity / Component (for am start -n)").strong());
        ui.add_space(2.0);
        if self.streamer_mode_active() {
            let mut masked = self.display_text(&self.activity_class_input);
            ui.add_enabled(false, egui::TextEdit::singleline(&mut masked));
        } else {
            ui.add(
                egui::TextEdit::singleline(&mut self.activity_class_input)
                    .hint_text("e.g. .MainActivity or com.app.name/.MainActivity"),
            );
        }
        ui.colored_label(
            egui::Color32::from_rgb(120, 120, 120),
            "Leave empty to auto-resolve the launchable activity",
        );
        ui.add_space(12.0);

        ui.label(egui::RichText::new("Logcat Tags (one per line)").strong());
        ui.add_space(2.0);
        egui::ScrollArea::vertical()
            .id_salt("settings_tags_scroll")
            .max_height(300.0)
            .show(ui, |ui| {
                if self.streamer_mode_active() {
                    let mut masked = self.display_text(&self.log_tags_input);
                    ui.add_enabled(
                        false,
                        egui::TextEdit::multiline(&mut masked)
                            .desired_width(f32::INFINITY)
                            .desired_rows(12)
                            .font(egui::FontId::monospace(13.0)),
                    );
                } else {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.log_tags_input)
                            .desired_width(f32::INFINITY)
                            .desired_rows(12)
                            .font(egui::FontId::monospace(13.0)),
                    );
                }
            });

        ui.add_space(12.0);
    }

    fn draw_settings_actions(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Save & Apply").clicked() {
                self.apply_settings();
            }

            if ui.button("Reset to Defaults").clicked() {
                self.reset_settings_to_defaults();
            }
        });
    }

    fn draw_platform_tools(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Platform Tools").strong());
        ui.add_space(2.0);

        ui.horizontal(|ui| {
            if ui.button("adb devices -l").clicked() {
                self.spawn_platform_tools_task("adb devices -l", adb::adb_devices_long);
            }
            if ui.button("Restart ADB Server").clicked() {
                self.fatal_error = None;
                self.hidden_devices.clear();
                self.last_device_poll = 0.0;
                self.spawn_platform_tools_task("Restart ADB server", adb::restart_adb_server);
            }
        });

        ui.horizontal(|ui| {
            ui.label("Pair:");
            ui.add_sized(
                [170.0, 20.0],
                egui::TextEdit::singleline(&mut self.pair_address_input).hint_text("host:port"),
            );
            ui.add_sized(
                [110.0, 20.0],
                egui::TextEdit::singleline(&mut self.pair_code_input).hint_text("pairing code"),
            );
            if ui.button("adb pair").clicked() {
                let addr = self.pair_address_input.trim().to_string();
                let code = self.pair_code_input.trim().to_string();
                if addr.is_empty() || code.is_empty() {
                    self.log(
                        AppLogLevel::Warn,
                        "[platform-tools] adb pair skipped: address or pairing code is empty",
                    );
                } else {
                    self.spawn_platform_tools_task("adb pair", move || adb::adb_pair(&addr, &code));
                }
            }
        });

        ui.horizontal(|ui| {
            ui.label("Fastboot:");
            ui.add_sized(
                [150.0, 20.0],
                egui::TextEdit::singleline(&mut self.fastboot_serial_input)
                    .hint_text("optional serial"),
            );
            ui.add_sized(
                [110.0, 20.0],
                egui::TextEdit::singleline(&mut self.fastboot_partition_input)
                    .hint_text("partition"),
            );
            if ui.button("fastboot devices").clicked() {
                self.spawn_platform_tools_task("fastboot devices", adb::list_fastboot_devices);
            }
            if ui.button("Flash Image...").clicked() {
                let partition = self.fastboot_partition_input.trim().to_string();
                if partition.is_empty() {
                    self.log(
                        AppLogLevel::Warn,
                        "[platform-tools] fastboot flash skipped: partition is empty",
                    );
                } else if let Some(path) = rfd::FileDialog::new()
                    .set_title("Select image to flash with fastboot")
                    .pick_file()
                {
                    let serial = self.fastboot_serial_input.trim().to_string();
                    let image = path.display().to_string();
                    self.spawn_platform_tools_task("fastboot flash", move || {
                        let serial = (!serial.is_empty()).then_some(serial.as_str());
                        adb::flash_partition(serial, &partition, &image)
                    });
                } else {
                    self.log(
                        AppLogLevel::Info,
                        "[platform-tools] fastboot flash cancelled",
                    );
                }
            }
        });

        ui.add_space(12.0);
    }

    fn draw_settings_summary(&self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                self.display_text(&format!("Config: {}", self.config_path.display())),
            )
            .small()
            .color(egui::Color32::from_rgb(150, 150, 150)),
        );
        ui.label(
            egui::RichText::new(self.display_text(&format!("Current: {}", self.config.bundle_id)))
                .small()
                .color(egui::Color32::from_rgb(150, 150, 150)),
        );
        ui.label(
            egui::RichText::new(
                self.display_text(&format!("Tags: {}", self.config.logcat_tags.join(", "))),
            )
            .small()
            .color(egui::Color32::from_rgb(150, 150, 150)),
        );
    }

    fn apply_settings(&mut self) {
        let mut next_config = self.config.clone();
        next_config.bundle_id = self.bundle_id_input.trim().to_string();
        next_config.activity_class = self.activity_class_input.trim().to_string();
        next_config.logcat_tags = self
            .log_tags_input
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        if let Err(error) = next_config.save() {
            self.log(AppLogLevel::Error, format!("Config save failed: {error}"));
            return;
        }

        let bundle_id_changed = next_config.bundle_id != self.config.bundle_id;
        self.config = next_config;
        if bundle_id_changed {
            self.invalidate_run_as_statuses();
        }

        self.log(
            AppLogLevel::Info,
            format!(
                "Config saved: bundle_id={}, tags={}",
                self.config.bundle_id,
                self.config.logcat_tags.len(),
            ),
        );

        self.restart_logcat_streams();
        self.restart_file_watchers();
    }

    fn reset_settings_to_defaults(&mut self) {
        let defaults = AppConfig::default();
        self.bundle_id_input.clone_from(&defaults.bundle_id);
        self.activity_class_input
            .clone_from(&defaults.activity_class);
        self.log_tags_input = defaults.logcat_tags.join("\n");
        self.log(AppLogLevel::Info, "Settings reset to defaults");
    }

    fn restart_logcat_streams(&mut self) {
        let serials: Vec<String> = self.logcat_procs.keys().cloned().collect();
        for serial in serials {
            if let Some(mut child) = self.logcat_procs.remove(&serial) {
                if let Err(error) = child.kill() {
                    self.log(
                        AppLogLevel::Warn,
                        format!("[{serial}] Failed to stop logcat before restart: {error}"),
                    );
                }
            }
            let Some(session) = self
                .devices
                .get_mut(&serial)
                .map(crate::device::DeviceState::start_next_logcat_session)
            else {
                self.log_missing_device_state(&serial, "restart logcat");
                continue;
            };
            if let Some(child) = adb::spawn_logcat(&serial, session, self.tx.clone(), &self.config)
            {
                self.logcat_procs.insert(serial.clone(), child);
                if let Some(ds) = self.devices.get_mut(&serial) {
                    ds.logcat_ui.running = true;
                    ds.logcat_status = "Restarted with new config".into();
                }
            } else {
                self.log(
                    AppLogLevel::Error,
                    format!("[{serial}] Failed to restart logcat with updated settings"),
                );
                if let Some(ds) = self.devices.get_mut(&serial) {
                    ds.logcat_ui.running = false;
                    ds.logcat_status = "Failed to restart with new config".into();
                }
            }
        }
    }

    fn restart_file_watchers(&mut self) {
        let watcher_serials: Vec<String> = self
            .devices
            .iter()
            .filter(|(_, ds)| ds.file_activity.watching)
            .map(|(serial, _)| serial.clone())
            .collect();

        for serial in watcher_serials {
            if let Some(ds) = self.devices.get_mut(&serial) {
                ds.stop_watcher();
                let session = ds.start_next_file_watch_session();
                ds.file_logs.clear();
                ds.sorted_keys.clear();
                ds.file_activity.watching = true;
                ds.file_status = "Restarting watcher...".into();
                let stop = adb::spawn_file_watcher(
                    &serial,
                    session,
                    self.tx.clone(),
                    FILE_WATCH_INTERVAL,
                    self.config.bundle_id.clone(),
                );
                ds.watcher_stop = Some(stop);
            } else {
                self.log_missing_device_state(&serial, "restart file watcher");
            }
        }
    }

    fn invalidate_run_as_statuses(&mut self) {
        for device in self.devices.values_mut() {
            device.deploy.run_as = CapabilityStatus::Unknown;
        }
    }

    pub(super) fn persist_resolved_activity(&mut self, serial: &str, component: &str) {
        let component = component.trim();
        if component.is_empty() || self.config.activity_class == component {
            return;
        }

        self.config.activity_class = component.to_string();
        self.activity_class_input
            .clone_from(&self.config.activity_class);

        match self.config.save() {
            Ok(()) => {
                self.log(
                    AppLogLevel::Info,
                    format!(
                        "[{serial}] Saved resolved launch activity: {}",
                        self.config.activity_class
                    ),
                );
            }
            Err(error) => {
                self.log(
                    AppLogLevel::Error,
                    format!(
                        "[{serial}] Failed to save resolved activity {}: {error}",
                        self.config.activity_class
                    ),
                );
            }
        }
    }

    // ─── ADB not found ──────────────────────────────────────────────────────

    pub(super) fn draw_adb_not_found(&mut self, ui: &mut egui::Ui, err: &str) {
        ui.vertical_centered(|ui| {
            ui.add_space(80.0);
            ui.label(
                egui::RichText::new("ADB Not Found")
                    .size(24.0)
                    .color(egui::Color32::from_rgb(255, 80, 80))
                    .strong(),
            );
            ui.add_space(8.0);
            ui.label(err);
            ui.add_space(24.0);
            ui.label(format!("Provide the path to {}:", adb_binary_label()));
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                let max_w = 500.0_f32.min(ui.available_width() - 120.0);
                ui.add_sized(
                    [max_w, 24.0],
                    egui::TextEdit::singleline(&mut self.adb_path_candidate)
                        .hint_text(adb_path_hint()),
                );
                if ui.button("Browse...").clicked() {
                    if let Some(path) = adb_file_dialog().pick_file() {
                        self.adb_path_candidate = path.display().to_string();
                    }
                }
                if ui.button("Set & Retry").clicked() && !self.adb_path_candidate.is_empty() {
                    match adb::set_adb_path(&self.adb_path_candidate) {
                        Ok(()) => {
                            self.fatal_error = None;
                            self.adb_override_message.clear();
                            self.last_device_poll = 0.0;
                            self.log(
                                AppLogLevel::Info,
                                format!("ADB override set to {}", self.adb_path_candidate),
                            );
                        }
                        Err(e) => {
                            self.log(AppLogLevel::Error, format!("ADB override failed: {e}"));
                            self.adb_override_message = e;
                        }
                    }
                }
            });

            if !self.adb_override_message.is_empty() {
                ui.add_space(8.0);
                ui.colored_label(
                    egui::Color32::from_rgb(255, 150, 50),
                    &self.adb_override_message,
                );
            }
        });
    }
}

const fn adb_binary_label() -> &'static str {
    #[cfg(windows)]
    {
        "adb.exe"
    }

    #[cfg(not(windows))]
    {
        "adb"
    }
}

const fn adb_path_hint() -> &'static str {
    #[cfg(windows)]
    {
        "C:/Android/Sdk/platform-tools/adb.exe"
    }

    #[cfg(target_os = "macos")]
    {
        "/Users/yourname/Library/Android/sdk/platform-tools/adb"
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "/home/yourname/Android/Sdk/platform-tools/adb"
    }

    #[cfg(not(any(windows, unix)))]
    {
        "path/to/adb"
    }
}

fn adb_file_dialog() -> rfd::FileDialog {
    let dialog = rfd::FileDialog::new().set_title(format!("Select {}", adb_binary_label()));

    #[cfg(windows)]
    {
        dialog.add_filter("adb executable", &["exe"])
    }

    #[cfg(not(windows))]
    {
        dialog
    }
}
