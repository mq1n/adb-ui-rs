use eframe::egui;

use super::AppLogLevel;
use crate::adb::{self, AdbMsg};

impl super::App {
    pub(super) fn draw_device_tab(&mut self, ui: &mut egui::Ui, serial: &str) {
        let serial_owned = serial.to_string();
        let bundle_id = self.config.bundle_id.clone();

        let needs_fetch = self
            .devices
            .get(serial)
            .is_some_and(|ds| ds.device_props.is_empty() && !ds.loading.props);
        if needs_fetch {
            self.fetch_device_props(serial);
        }

        let available_rect = ui.available_rect_before_wrap();
        let left_w = (available_rect.width() * 0.58).max(400.0);
        let sep = 4.0;
        let left_rect = egui::Rect::from_min_size(
            available_rect.min,
            egui::vec2(left_w, available_rect.height()),
        );
        let right_rect = egui::Rect::from_min_size(
            egui::pos2(available_rect.min.x + left_w + sep, available_rect.min.y),
            egui::vec2(
                (available_rect.width() - left_w - sep).max(100.0),
                available_rect.height(),
            ),
        );
        ui.allocate_rect(available_rect, egui::Sense::hover());

        let sep_x = available_rect.min.x + left_w + sep * 0.5;
        ui.painter().line_segment(
            [
                egui::pos2(sep_x, available_rect.min.y),
                egui::pos2(sep_x, available_rect.max.y),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 60, 60)),
        );

        let mut left_ui = ui.new_child(egui::UiBuilder::new().max_rect(left_rect));
        left_ui.set_clip_rect(left_rect);
        egui::ScrollArea::vertical()
            .id_salt(format!("devmgmt_{serial}"))
            .auto_shrink([false, false])
            .show(&mut left_ui, |ui| {
                self.draw_device_management_sections(ui, serial, &serial_owned, &bundle_id);
                self.draw_device_testing_and_diagnostics(ui, serial, &serial_owned, &bundle_id);
                self.draw_device_system_section(ui, serial, &serial_owned);
            });

        self.draw_device_action_log_panel(ui, serial, &serial_owned, right_rect);
    }

    fn draw_device_management_sections(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        bundle_id: &str,
    ) {
        let bundle_id_shell = adb::shell_quote(bundle_id);

        // ── Connect Devices ──────────────────────────────────
        Self::draw_section_header(ui, "Connect Devices");

        // WiFi / TCP connect.
        ui.horizontal(|ui| {
            ui.label("WiFi/TCP:");
            ui.add_sized(
                [180.0, 18.0],
                egui::TextEdit::singleline(&mut self.wifi_connect_addr)
                    .hint_text("192.168.1.x:5555"),
            );
            if ui.button("Connect").clicked() && !self.wifi_connect_addr.is_empty() {
                let addr = self.wifi_connect_addr.clone();
                let tx = self.tx.clone();
                self.log(AppLogLevel::Info, format!("Connecting to {addr}..."));
                std::thread::spawn(move || {
                    let (ok, msg) = adb::adb_connect(&addr);
                    let _ = tx.send(AdbMsg::DeviceActionResult(addr, format!("Connect: {msg}")));
                    let _ = ok;
                });
            }
        });

        // WSA connect.
        ui.horizontal(|ui| {
            ui.label("WSA Port:");
            ui.add_sized(
                [60.0, 18.0],
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
                    let _ = tx.send(AdbMsg::DeviceActionResult(addr, format!("{status}: {msg}")));
                });
            }
            if ui.button("WSA Settings").clicked() && !adb::open_wsa_settings() {
                self.log(
                    AppLogLevel::Error,
                    "Failed to open WSA Settings — is WSA installed?",
                );
            }
            if ui.button("Launch WSA").clicked() && !adb::launch_wsa() {
                self.log(AppLogLevel::Error, "Failed to launch WSA");
            }
        });

        // Enable wireless debugging on connected USB device.
        ui.horizontal(|ui| {
            if ui.button("Enable WiFi Debug (this device)").clicked() {
                self.run_action(serial, &["tcpip", "5555"], "Enable tcpip:5555");
            }
            if adb::is_tcp_device(serial) && ui.button("Disconnect this device").clicked() {
                let addr = serial_owned.to_string();
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let (_, msg) = adb::adb_disconnect(&addr);
                    let _ = tx.send(AdbMsg::DeviceActionResult(
                        addr,
                        format!("Disconnect: {msg}"),
                    ));
                });
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

        ui.add_space(8.0);

        // ── Emulator Management ──────────────────────────────
        Self::draw_section_header(ui, "Emulator Management");

        ui.horizontal(|ui| {
            if ui.button("Refresh AVDs").clicked() {
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

            let running = adb::is_emulator_serial(serial);
            if running {
                ui.separator();
                ui.colored_label(egui::Color32::from_rgb(180, 130, 255), "[Emulator]");
                if ui.button("Kill this emulator").clicked() {
                    let serial = serial_owned.to_string();
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        let (ok, msg) = adb::kill_emulator(&serial);
                        let s = if ok { "Emulator killed" } else { "Kill failed" };
                        let _ = tx.send(AdbMsg::DeviceActionResult(serial, format!("{s}: {msg}")));
                    });
                }
            }
        });

        if !self.available_avds.is_empty() {
            ui.add_space(4.0);

            for avd in self.available_avds.clone() {
                ui.horizontal(|ui| {
                    // Check if this AVD is running (from cached map, not blocking).
                    let is_running = self.running_emu_map.values().any(|name| name == &avd);

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
                            self.log(AppLogLevel::Info, format!("Starting emulator: {avd}"));
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
                            self.log(AppLogLevel::Info, format!("Cold-booting emulator: {avd}"));
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
                                let _ =
                                    tx.send(AdbMsg::DeviceActionResult(es, format!("{s}: {msg}")));
                            });
                        }
                    }

                    // Delete button (only if not running).
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
                            // Refresh AVD list.
                            let avds = adb::list_avds();
                            let _ = tx.send(AdbMsg::AvdList(avds));
                        });
                    }
                });
            }
        } else if !self.avds_loading {
            ui.colored_label(
                egui::Color32::from_rgb(140, 140, 140),
                "No AVDs found. Click 'Refresh AVDs' to scan.",
            );
        }

        ui.add_space(6.0);

        // Create AVD form.
        ui.label(egui::RichText::new("Create AVD").small().strong());
        ui.horizontal(|ui| {
            ui.label("Name:");
            ui.add_sized(
                [120.0, 18.0],
                egui::TextEdit::singleline(&mut self.new_avd_name).hint_text("my_avd"),
            );
            ui.label("Device:");
            ui.add_sized(
                [90.0, 18.0],
                egui::TextEdit::singleline(&mut self.new_avd_device).hint_text("pixel_6"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Image:");
            if self.available_system_images.is_empty() {
                ui.add_sized(
                    [250.0, 18.0],
                    egui::TextEdit::singleline(&mut self.new_avd_image)
                        .hint_text("system-images;android-34;google_apis;x86_64"),
                );
                if ui.small_button("Scan Images").clicked() {
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        let images = adb::list_system_images();
                        let _ = tx.send(AdbMsg::SystemImageList(images));
                    });
                }
            } else {
                egui::ComboBox::from_id_salt("avd_image_combo")
                    .selected_text(if self.new_avd_image.is_empty() {
                        "Select image..."
                    } else {
                        &self.new_avd_image
                    })
                    .width(350.0)
                    .show_ui(ui, |ui| {
                        for img in self.available_system_images.clone() {
                            ui.selectable_value(&mut self.new_avd_image, img.clone(), &img);
                        }
                    });
            }

            let can_create =
                !self.new_avd_name.trim().is_empty() && !self.new_avd_image.trim().is_empty();
            if ui
                .add_enabled(can_create, egui::Button::new("Create"))
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
                    // Refresh AVD list.
                    let avds = adb::list_avds();
                    let _ = tx.send(AdbMsg::AvdList(avds));
                });
            }
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        // ── Device Information ───────────────────────────────
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Device Information")
                    .strong()
                    .size(13.0),
            );
            if ui.button("Refresh").clicked() {
                self.fetch_device_props(serial);
            }
            let is_loading = self.devices.get(serial).is_some_and(|ds| ds.loading.props);
            if is_loading {
                ui.spinner();
            }

            // Show device type badge.
            if adb::is_wsa_serial(serial) {
                ui.colored_label(egui::Color32::from_rgb(180, 130, 255), "[WSA]");
            } else if adb::is_tcp_device(serial) {
                ui.colored_label(egui::Color32::from_rgb(100, 200, 255), "[WiFi]");
            } else {
                ui.colored_label(egui::Color32::from_rgb(200, 200, 200), "[USB]");
            }
        });
        ui.add_space(2.0);

        if let Some(ds) = self.devices.get(serial) {
            egui::Grid::new(format!("props_grid_{serial}"))
                .num_columns(2)
                .spacing([12.0, 3.0])
                .show(ui, |ui| {
                    for (key, val) in &ds.device_props {
                        ui.label(
                            egui::RichText::new(key).color(egui::Color32::from_rgb(140, 180, 220)),
                        );
                        ui.label(
                            egui::RichText::new(val)
                                .monospace()
                                .color(egui::Color32::from_rgb(220, 220, 220)),
                        );
                        ui.end_row();
                    }
                });
        }

        ui.add_space(8.0);

        // ── App Management ────────────────────────────────────
        Self::draw_section_header(ui, &format!("App: {bundle_id}"));

        // Launch & lifecycle.
        let activity = self.config.activity_class.clone();
        ui.horizontal(|ui| {
            if ui
                .button("Launch")
                .on_hover_text(if activity.is_empty() {
                    "Launch via monkey (set activity in Settings for am start)"
                } else {
                    "am start -n bundle/activity"
                })
                .clicked()
            {
                let Some(bid) = self.require_bundle_id(serial, "launch app") else {
                    return;
                };
                let serial = serial_owned.to_string();
                let act = activity.clone();
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let (ok, msg) = if act.is_empty() {
                        adb::launch_via_monkey(&serial, &bid)
                    } else {
                        adb::launch_activity(&serial, &bid, &act)
                    };
                    let status = if ok { "Launched" } else { "Launch failed" };
                    let _ = tx.send(AdbMsg::DeviceActionResult(
                        serial,
                        format!("{status}: {msg}"),
                    ));
                });
            }
            if ui.button("Force Stop").clicked() {
                self.run_action(
                    serial,
                    &["shell", "am", "force-stop", bundle_id],
                    "Force stop",
                );
            }
            if ui.button("Clear Data").clicked() {
                self.run_action(serial, &["shell", "pm", "clear", bundle_id], "Clear data");
            }
            if ui.button("Uninstall").clicked() {
                self.run_action(serial, &["uninstall", bundle_id], "Uninstall");
            }
            if ui
                .button("Purge")
                .on_hover_text("Force stop + uninstall + remove all app data")
                .clicked()
            {
                let serial = serial_owned.to_string();
                let bid = bundle_id.to_string();
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let (_, msg) = adb::purge_app(&serial, &bid);
                    let _ = tx.send(AdbMsg::DeviceActionResult(serial, msg));
                });
            }
        });
        ui.horizontal(|ui| {
            if ui
                .button("App Settings")
                .on_hover_text("Open Android settings page for this app")
                .clicked()
            {
                let serial = serial_owned.to_string();
                let bid = bundle_id.to_string();
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let (_, msg) = adb::open_app_settings(&serial, &bid);
                    let _ = tx.send(AdbMsg::DeviceActionResult(
                        serial,
                        format!("App settings: {msg}"),
                    ));
                });
            }
            if ui.button("Get PID").clicked() {
                let Some(bid) = self.require_bundle_id(serial, "get app pid") else {
                    return;
                };
                let serial = serial_owned.to_string();
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let msg = adb::get_app_pid(&serial, &bid).map_or_else(
                        || format!("{bid}: not running"),
                        |pid| format!("{bid}: PID {pid}"),
                    );
                    let _ = tx.send(AdbMsg::DeviceActionResult(serial, msg));
                });
            }
            if ui
                .button("Pull Logs")
                .on_hover_text("Pull app logs to local folder and open")
                .clicked()
            {
                let Some(bid) = self.require_bundle_id(serial, "pull app logs") else {
                    return;
                };
                let serial = serial_owned.to_string();
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let dest = std::env::temp_dir().join(format!(
                        "adb_logs_{}_{}",
                        serial.replace(':', "_"),
                        ts
                    ));
                    match adb::pull_logs_to_dir(&serial, &bid, &dest) {
                        Ok(summary) => {
                            if !summary.warnings.is_empty() {
                                let _ = tx.send(AdbMsg::DeviceActionResult(
                                    serial.clone(),
                                    format!("Pull logs warnings: {}", summary.warnings.join("; ")),
                                ));
                            }
                            let _ = tx.send(AdbMsg::PullLogsResult(serial, Ok(summary.count)));
                            // Open in file manager
                            #[cfg(windows)]
                            {
                                let _ = std::process::Command::new("explorer.exe")
                                    .arg(dest.to_string_lossy().replace('/', "\\"))
                                    .spawn();
                            }
                            #[cfg(target_os = "macos")]
                            {
                                let _ = std::process::Command::new("open").arg(&dest).spawn();
                            }
                            #[cfg(target_os = "linux")]
                            {
                                let _ = std::process::Command::new("xdg-open").arg(&dest).spawn();
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(AdbMsg::PullLogsResult(serial, Err(e)));
                        }
                    }
                });
            }
        });

        // Install variants.
        ui.horizontal(|ui| {
            ui.label("Install:");
            // Fresh install / update (replace)
            if ui
                .button("APK...")
                .on_hover_text("Install or update (-r)")
                .clicked()
            {
                self.pick_and_install_apk(serial, &["-r"]);
            }
            if ui
                .button("Downgrade...")
                .on_hover_text("Install with downgrade (-r -d)")
                .clicked()
            {
                self.pick_and_install_apk(serial, &["-r", "-d"]);
            }
            if ui
                .button("Debug APK...")
                .on_hover_text("Install as debuggable (-r -t)")
                .clicked()
            {
                self.pick_and_install_apk(serial, &["-r", "-t"]);
            }
        });

        // Debug controls.
        ui.horizontal(|ui| {
            ui.label("Debug:");
            if ui.button("Launch (wait debugger)").on_hover_text("am set-debug-app -w").clicked() {
                let cmd = format!(
                    "am set-debug-app -w --persistent {bundle_id_shell} && monkey -p {bundle_id_shell} -c android.intent.category.LAUNCHER 1"
                );
                self.run_action(serial, &["shell", &cmd], "Launch (wait-for-debugger)");
            }
            if ui.button("Clear debug flag").on_hover_text("am clear-debug-app").clicked() {
                self.run_action(serial, &["shell", "am", "clear-debug-app"], "Clear debug-app");
            }
        });

        // Permissions & data.
        ui.horizontal(|ui| {
            if ui.button("Grant Permissions").clicked() {
                let cmd = format!(
                    "pm grant {bundle_id_shell} android.permission.READ_EXTERNAL_STORAGE 2>/dev/null; \
                     pm grant {bundle_id_shell} android.permission.WRITE_EXTERNAL_STORAGE 2>/dev/null; \
                     pm grant {bundle_id_shell} android.permission.POST_NOTIFICATIONS 2>/dev/null; \
                     echo done"
                );
                self.run_action(serial, &["shell", &cmd], "Grant permissions");
            }
            if ui.button("Revoke Permissions").clicked() {
                let cmd = format!("pm reset-permissions -p {bundle_id_shell}");
                self.run_action(serial, &["shell", &cmd], "Revoke permissions");
            }
            if ui.button("App Info").clicked() {
                let cmd = format!("dumpsys package {bundle_id_shell} | head -60");
                self.run_action(serial, &["shell", &cmd], "App info");
            }
        });

        ui.add_space(8.0);
    }

    fn draw_device_testing_and_diagnostics(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        bundle_id: &str,
    ) {
        // ── Testing ──────────────────────────────────────────
        Self::draw_section_header(ui, "Testing");
        let bundle_id_shell = adb::shell_quote(bundle_id);

        // Monkey stress test.
        ui.horizontal(|ui| {
            ui.label("Monkey:");
            if let Some(ds) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [60.0, 18.0],
                    egui::TextEdit::singleline(&mut ds.monkey_event_count).hint_text("1000"),
                );
                ui.label("events");
            }

            let is_monkey_running = self
                .devices
                .get(serial)
                .is_some_and(|ds| ds.test_runs.monkey);

            if is_monkey_running {
                ui.spinner();
                ui.label("Running...");
            } else if ui
                .button("Run Monkey")
                .on_hover_text("Random UI stress test")
                .clicked()
            {
                let Some(bid) = self.require_bundle_id(serial, "run monkey") else {
                    return;
                };
                let raw_count = self
                    .devices
                    .get(serial)
                    .map_or_else(String::new, |ds| ds.monkey_event_count.clone());
                let count =
                    self.parse_u32_input_or_log(serial, "monkey event count", &raw_count, 1000);
                if let Some(ds) = self.devices.get_mut(serial_owned) {
                    ds.test_runs.monkey = true;
                }
                let serial = serial_owned.to_string();
                let tx = self.tx.clone();
                self.log(
                    AppLogLevel::Info,
                    format!("[{serial}] Monkey starting ({count} events)"),
                );
                std::thread::spawn(move || {
                    let (ok, output) = adb::run_monkey(&serial, &bid, count, None);
                    let _ = tx.send(AdbMsg::MonkeyDone(serial, ok, output));
                });
            }

            if ui
                .button("Monkey (seeded)")
                .on_hover_text("Reproducible run with seed=42")
                .clicked()
                && !is_monkey_running
            {
                let Some(bid) = self.require_bundle_id(serial, "run seeded monkey") else {
                    return;
                };
                let raw_count = self
                    .devices
                    .get(serial)
                    .map_or_else(String::new, |ds| ds.monkey_event_count.clone());
                let count =
                    self.parse_u32_input_or_log(serial, "monkey event count", &raw_count, 1000);
                if let Some(ds) = self.devices.get_mut(serial_owned) {
                    ds.test_runs.monkey = true;
                }
                let serial = serial_owned.to_string();
                let tx = self.tx.clone();
                self.log(
                    AppLogLevel::Info,
                    format!("[{serial}] Monkey starting ({count} events, seed=42)"),
                );
                std::thread::spawn(move || {
                    let (ok, output) = adb::run_monkey(&serial, &bid, count, Some(42));
                    let _ = tx.send(AdbMsg::MonkeyDone(serial, ok, output));
                });
            }
        });

        // UIAutomator.
        ui.horizontal(|ui| {
            ui.label("UI Automator:");
            let is_dumping = self
                .devices
                .get(serial)
                .is_some_and(|ds| ds.loading.uiautomator);
            if is_dumping {
                ui.spinner();
            } else if ui
                .button("Dump UI Hierarchy")
                .on_hover_text("uiautomator dump → XML view")
                .clicked()
            {
                if let Some(ds) = self.devices.get_mut(serial_owned) {
                    ds.loading.uiautomator = true;
                    ds.uiautomator_dump = None;
                }
                let serial = serial_owned.to_string();
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let result = adb::uiautomator_dump(&serial);
                    let _ = tx.send(AdbMsg::UiDump(serial, result));
                });
            }

            if self
                .devices
                .get(serial)
                .and_then(|ds| ds.uiautomator_dump.as_ref())
                .is_some()
            {
                if ui.button("Save XML...").clicked() {
                    let xml = self
                        .devices
                        .get(serial)
                        .and_then(|ds| ds.uiautomator_dump.clone());
                    if let Some(xml) = xml {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_file_name("ui_hierarchy.xml")
                            .add_filter("XML", &["xml"])
                            .set_title("Save UI dump")
                            .save_file()
                        {
                            match std::fs::write(&path, xml) {
                                Ok(()) => {
                                    self.log(
                                        AppLogLevel::Info,
                                        format!(
                                            "[{serial}] UI hierarchy saved to {}",
                                            path.display()
                                        ),
                                    );
                                }
                                Err(e) => {
                                    self.log(
                                        AppLogLevel::Error,
                                        format!("Save XML failed: {}: {e}", path.display()),
                                    );
                                }
                            }
                        } else {
                            self.log_cancelled(serial, "save UI hierarchy");
                        }
                    } else {
                        self.log_skipped(serial, "save UI hierarchy", "no UI dump is loaded");
                    }
                }
                if ui.button("Copy XML").clicked() {
                    if let Some(ds) = self.devices.get(serial) {
                        if let Some(ref xml) = ds.uiautomator_dump {
                            ui.ctx().copy_text(xml.clone());
                        }
                    }
                }
                if ui.button("Clear").clicked() {
                    if let Some(ds) = self.devices.get_mut(serial_owned) {
                        ds.uiautomator_dump = None;
                    }
                }
            }
        });

        // Show UI dump inline if present (compact tree view).
        if let Some(ds) = self.devices.get(serial) {
            if let Some(ref xml) = ds.uiautomator_dump {
                egui::ScrollArea::vertical()
                    .id_salt(format!("uidump_{serial}"))
                    .max_height(200.0)
                    .show(ui, |ui| {
                        ui.style_mut().override_font_id = Some(egui::FontId::monospace(11.0));
                        for line in xml.lines() {
                            let trimmed = line.trim();
                            if trimmed.is_empty() || trimmed.starts_with("<?xml") {
                                continue;
                            }
                            // Indent based on XML nesting depth.
                            let depth = line.len() - line.trim_start().len();
                            let indent = " ".repeat(depth);

                            // Color resource-id and text attributes.
                            let color = if trimmed.contains("resource-id=\"\"")
                                || trimmed.starts_with("</")
                            {
                                egui::Color32::from_rgb(120, 120, 120)
                            } else if trimmed.contains("clickable=\"true\"") {
                                egui::Color32::from_rgb(100, 220, 100)
                            } else if trimmed.contains("text=\"") {
                                egui::Color32::from_rgb(100, 180, 255)
                            } else {
                                egui::Color32::from_rgb(200, 200, 200)
                            };
                            ui.label(
                                egui::RichText::new(format!("{indent}{trimmed}")).color(color),
                            );
                        }
                    });
            }
        }

        ui.add_space(8.0);

        // ── Diagnostics ──────────────────────────────────────
        Self::draw_section_header(ui, "Diagnostics");

        // Bugreport.
        ui.horizontal(|ui| {
            let is_bugreport = self
                .devices
                .get(serial)
                .is_some_and(|ds| ds.test_runs.bugreport);
            if is_bugreport {
                ui.spinner();
                ui.label("Generating bugreport...");
            } else if ui
                .button("Bugreport")
                .on_hover_text("Full device bugreport (ZIP, may take minutes)")
                .clicked()
            {
                if let Some(save_path) = rfd::FileDialog::new()
                    .set_file_name("bugreport.zip")
                    .add_filter("ZIP", &["zip"])
                    .set_title("Save bugreport")
                    .save_file()
                {
                    if let Some(ds) = self.devices.get_mut(serial_owned) {
                        ds.test_runs.bugreport = true;
                    }
                    let serial = serial_owned.to_string();
                    let local = save_path.display().to_string();
                    let tx = self.tx.clone();
                    self.log(
                        AppLogLevel::Info,
                        format!("[{serial}] Bugreport starting..."),
                    );
                    std::thread::spawn(move || {
                        let (ok, msg) = adb::bugreport(&serial, &local);
                        let _ = tx.send(AdbMsg::BugreportDone(serial, ok, msg));
                    });
                } else {
                    self.log_cancelled(serial, "save bugreport");
                }
            }
        });

        // Crash analysis.
        ui.label(egui::RichText::new("Crash Analysis").small().strong());
        ui.horizontal(|ui| {
            if ui
                .button("ANR Traces")
                .on_hover_text("Recent ANR traces")
                .clicked()
            {
                self.run_action(
                    serial,
                    &["shell", "ls -lt /data/anr/ 2>/dev/null && cat /data/anr/traces.txt 2>/dev/null | head -200 || echo 'No ANR traces (may need root)'"],
                    "ANR traces",
                );
            }
            if ui
                .button("Tombstones")
                .on_hover_text("Native crash dumps")
                .clicked()
            {
                self.run_action(
                    serial,
                    &["shell", "ls -lt /data/tombstones/ 2>/dev/null || echo 'No tombstones (may need root)'"],
                    "Tombstones",
                );
            }
            if ui
                .button("Last Tombstone")
                .on_hover_text("Most recent native crash dump")
                .clicked()
            {
                self.run_action(
                    serial,
                    &["shell", "ls -t /data/tombstones/ 2>/dev/null | head -1 | xargs -I{} cat /data/tombstones/{} 2>/dev/null | head -150 || echo 'No tombstones (may need root)'"],
                    "Last tombstone",
                );
            }
            if ui
                .button("Dropbox Crashes")
                .on_hover_text("System crash entries from dropbox")
                .clicked()
            {
                self.run_action(
                    serial,
                    &["shell", "dumpsys dropbox --print 2>/dev/null | grep -A 5 'SYSTEM_CRASH\\|data_app_crash\\|data_app_anr' | head -100 || echo 'No dropbox crash entries'"],
                    "Dropbox crashes",
                );
            }
        });

        // App diagnostics.
        ui.label(egui::RichText::new("App Diagnostics").small().strong());
        ui.horizontal(|ui| {
            if ui
                .button("Meminfo")
                .on_hover_text("dumpsys meminfo for the app")
                .clicked()
            {
                let cmd = format!("dumpsys meminfo {bundle_id_shell}");
                self.run_action(serial, &["shell", &cmd], "Meminfo");
            }
            if ui
                .button("Activity Stack")
                .on_hover_text("Current activity stack")
                .clicked()
            {
                let cmd =
                    format!("dumpsys activity activities | grep -F -A 10 -- {bundle_id_shell}");
                self.run_action(serial, &["shell", &cmd], "Activity stack");
            }
            if ui
                .button("GPU Frames")
                .on_hover_text("dumpsys gfxinfo — frame rendering stats")
                .clicked()
            {
                let cmd = format!("dumpsys gfxinfo {bundle_id_shell}");
                self.run_action(serial, &["shell", &cmd], "GPU frame info");
            }
            if ui
                .button("App Processes")
                .on_hover_text("Processes for this app")
                .clicked()
            {
                let cmd = format!("ps -A | grep -F -- {bundle_id_shell}");
                self.run_action(serial, &["shell", &cmd], "App processes");
            }
        });

        // System stats.
        ui.label(egui::RichText::new("System Stats").small().strong());
        ui.horizontal(|ui| {
            if ui
                .button("Interrupts")
                .on_hover_text("/proc/interrupts")
                .clicked()
            {
                self.run_action(
                    serial,
                    &["shell", "cat /proc/interrupts"],
                    "Interrupts",
                );
            }
            if ui
                .button("Memory")
                .on_hover_text("/proc/meminfo")
                .clicked()
            {
                self.run_action(serial, &["shell", "cat /proc/meminfo"], "Meminfo");
            }
            if ui
                .button("CPU Load")
                .on_hover_text("/proc/loadavg + CPU summary")
                .clicked()
            {
                self.run_action(
                    serial,
                    &["shell", "echo '=== Load ===' && cat /proc/loadavg && echo '\\n=== CPU ===' && cat /proc/cpuinfo | grep -E 'processor|model name|cpu MHz|Hardware' && echo '\\n=== Freq ===' && cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq 2>/dev/null || true"],
                    "CPU load",
                );
            }
            if ui
                .button("Top")
                .on_hover_text("Process snapshot (-n 1)")
                .clicked()
            {
                self.run_action(
                    serial,
                    &["shell", "top -b -n 1 | head -40"],
                    "Top",
                );
            }
            if ui
                .button("Disk Usage")
                .on_hover_text("df -h")
                .clicked()
            {
                self.run_action(serial, &["shell", "df -h"], "Disk usage");
            }
        });
        ui.horizontal(|ui| {
            if ui
                .button("Thermal")
                .on_hover_text("Thermal zone temperatures")
                .clicked()
            {
                self.run_action(
                    serial,
                    &["shell", "for z in /sys/class/thermal/thermal_zone*; do echo \"$(cat $z/type 2>/dev/null): $(cat $z/temp 2>/dev/null)\"; done"],
                    "Thermal zones",
                );
            }
            if ui
                .button("Wakelocks")
                .on_hover_text("Active wakelocks")
                .clicked()
            {
                self.run_action(
                    serial,
                    &["shell", "dumpsys power | grep -A 2 'Wake Locks' | head -30; echo '---'; cat /sys/power/wake_lock 2>/dev/null || echo 'N/A'"],
                    "Wakelocks",
                );
            }
            if ui.button("Battery Stats").clicked() {
                self.run_action(serial, &["shell", "dumpsys batterystats --charged | head -80"], "Battery stats");
            }
            if ui
                .button("I/O Stats")
                .on_hover_text("/proc/diskstats summary")
                .clicked()
            {
                self.run_action(
                    serial,
                    &["shell", "cat /proc/diskstats | head -20"],
                    "I/O stats",
                );
            }
        });

        // Network & security.
        ui.label(egui::RichText::new("Network & Security").small().strong());
        ui.horizontal(|ui| {
            if ui.button("Network Info").clicked() {
                self.run_action(
                    serial,
                    &["shell", "echo '=== IP ===' && ip addr show && echo '\\n=== Route ===' && ip route && echo '\\n=== DNS ===' && getprop net.dns1 && getprop net.dns2"],
                    "Network info",
                );
            }
            if ui.button("WiFi").clicked() {
                self.run_action(
                    serial,
                    &["shell", "dumpsys wifi | grep -E 'Wi-Fi is|mWifiInfo|SSID|Link speed|RSSI|freq' | head -20"],
                    "WiFi info",
                );
            }
            if ui.button("Netstat").clicked() {
                self.run_action(
                    serial,
                    &["shell", "netstat -tlnp 2>/dev/null || ss -tlnp 2>/dev/null || echo 'netstat/ss not available'"],
                    "Netstat",
                );
            }
            if ui
                .button("SELinux")
                .on_hover_text("SELinux status + recent denials")
                .clicked()
            {
                self.run_action(
                    serial,
                    &["shell", "echo 'Mode:' && getenforce && echo '\\n=== Recent denials ===' && dmesg | grep 'avc: denied' | tail -20"],
                    "SELinux",
                );
            }
        });

        // Dumpsys extras.
        ui.label(egui::RichText::new("Dumpsys").small().strong());
        ui.horizontal(|ui| {
            if ui.button("SurfaceFlinger").clicked() {
                self.run_action(
                    serial,
                    &["shell", "dumpsys SurfaceFlinger --latency | head -60"],
                    "SurfaceFlinger",
                );
            }
            if ui.button("Window Manager").clicked() {
                self.run_action(
                    serial,
                    &[
                        "shell",
                        "dumpsys window windows | grep -E 'mCurrentFocus|mFocusedApp' | head -10",
                    ],
                    "Window manager",
                );
            }
            if ui.button("Input").clicked() {
                self.run_action(
                    serial,
                    &[
                        "shell",
                        "dumpsys input | grep -A 3 'FocusedApplication' | head -20",
                    ],
                    "Input",
                );
            }
            if ui.button("Alarms").clicked() {
                self.run_action(
                    serial,
                    &[
                        "shell",
                        &format!("dumpsys alarm | grep -F -A 3 -- {bundle_id_shell} | head -30"),
                    ],
                    "Alarms",
                );
            }
            if ui.button("Services").clicked() {
                self.run_action(
                    serial,
                    &["shell", "dumpsys activity services | head -60"],
                    "Services",
                );
            }
        });

        ui.add_space(8.0);

        // ── System ───────────────────────────────────────────
    }

    fn draw_device_system_section(&mut self, ui: &mut egui::Ui, serial: &str, serial_owned: &str) {
        // ── System ───────────────────────────────────────────
        Self::draw_section_header(ui, "System");

        ui.horizontal(|ui| {
            if ui.button("Reboot").clicked() {
                self.run_action(serial, &["reboot"], "Reboot");
            }
            if ui.button("Recovery").clicked() {
                self.run_action(serial, &["reboot", "recovery"], "Reboot recovery");
            }
            if ui.button("Bootloader").clicked() {
                self.run_action(serial, &["reboot", "bootloader"], "Reboot bootloader");
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Screenshot").clicked() {
                let serial = serial_owned.to_string();
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let remote = "/sdcard/screenshot_adbviewer.png";
                    let (ok1, _) =
                        adb::run_device_action(&serial, &["shell", "screencap", "-p", remote]);
                    if !ok1 {
                        let _ = tx.send(AdbMsg::DeviceActionResult(
                            serial,
                            "Screenshot failed".into(),
                        ));
                        return;
                    }
                    if let Some(save_path) = rfd::FileDialog::new()
                        .set_title("Save screenshot")
                        .set_file_name("screenshot.png")
                        .add_filter("PNG", &["png"])
                        .save_file()
                    {
                        let (ok2, msg) = adb::run_device_action(
                            &serial,
                            &["pull", remote, &save_path.display().to_string()],
                        );
                        let status = if ok2 {
                            "Screenshot saved"
                        } else {
                            "Screenshot pull failed"
                        };
                        let _ = tx.send(AdbMsg::DeviceActionResult(
                            serial.clone(),
                            format!("{status}: {msg}"),
                        ));
                    } else {
                        let _ = tx.send(AdbMsg::DeviceActionResult(
                            serial.clone(),
                            "Screenshot save cancelled".into(),
                        ));
                    }
                    let _ = adb::run_device_action(&serial, &["shell", "rm", remote]);
                });
            }
            if ui.button("Push File...").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Select file to push")
                    .pick_file()
                {
                    let path_str = path.display().to_string();
                    let fname = path
                        .file_name()
                        .map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
                    let remote = format!("/sdcard/{fname}");
                    self.run_action(
                        serial,
                        &["push", &path_str, &remote],
                        &format!("Push {fname}"),
                    );
                } else {
                    self.log_cancelled(serial, "push file");
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Screen On").clicked() {
                self.run_action(
                    serial,
                    &["shell", "input keyevent KEYCODE_WAKEUP"],
                    "Screen on",
                );
            }
            if ui.button("Screen Off").clicked() {
                self.run_action(
                    serial,
                    &["shell", "input keyevent KEYCODE_SLEEP"],
                    "Screen off",
                );
            }
            if ui.button("Home").clicked() {
                self.run_action(serial, &["shell", "input keyevent KEYCODE_HOME"], "Home");
            }
            if ui.button("Back").clicked() {
                self.run_action(serial, &["shell", "input keyevent KEYCODE_BACK"], "Back");
            }
        });
    }

    fn draw_device_action_log_panel(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        right_rect: egui::Rect,
    ) {
        // --- Right: Action log ---
        let mut right_ui = ui.new_child(egui::UiBuilder::new().max_rect(right_rect));
        right_ui.set_clip_rect(right_rect);

        right_ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Action Log").strong());
            if ui.button("Clear").clicked() {
                if let Some(ds) = self.devices.get_mut(serial_owned) {
                    ds.action_log.clear();
                }
            }
        });
        right_ui.separator();

        if let Some(ds) = self.devices.get(serial) {
            egui::ScrollArea::vertical()
                .id_salt(format!("action_log_{serial}"))
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(&mut right_ui, |ui| {
                    ui.style_mut().override_font_id = Some(egui::FontId::monospace(11.0));
                    for line in &ds.action_log {
                        let color = if line.contains("failed")
                            || line.contains("FAILED")
                            || line.contains("Error")
                        {
                            egui::Color32::from_rgb(255, 80, 80)
                        } else if line.contains("succeeded")
                            || line.contains("OK")
                            || line.contains("Success")
                            || line.contains("connected")
                        {
                            egui::Color32::from_rgb(100, 220, 100)
                        } else {
                            egui::Color32::from_rgb(200, 200, 200)
                        };
                        ui.label(egui::RichText::new(line).color(color));
                    }
                });
        }
    }
    pub(super) fn draw_section_header(ui: &mut egui::Ui, title: &str) {
        ui.separator();
        ui.label(egui::RichText::new(title).strong().size(13.0));
        ui.add_space(2.0);
    }

    pub(super) fn pick_and_install_apk(&mut self, serial: &str, flags: &[&str]) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("APK files", &["apk"])
            .set_title("Select APK to install")
            .pick_file()
        {
            let path_str = path.display().to_string();
            let serial = serial.to_string();
            let tx = self.tx.clone();
            let mut args = vec!["install".to_string()];
            for f in flags {
                args.push((*f).to_string());
            }
            args.push(path_str);
            std::thread::spawn(move || {
                let str_args: Vec<&str> = args.iter().map(std::string::String::as_str).collect();
                let (ok, msg) = adb::run_device_action(&serial, &str_args);
                let status = if ok {
                    "Install succeeded"
                } else {
                    "Install failed"
                };
                let _ = tx.send(AdbMsg::DeviceActionResult(
                    serial,
                    format!("{status}: {msg}"),
                ));
            });
        } else {
            self.log_cancelled(serial, "install APK");
        }
    }

    pub(super) fn fetch_device_props(&mut self, serial: &str) {
        if let Some(ds) = self.devices.get_mut(serial) {
            ds.loading.props = true;
        } else {
            self.log_missing_device_state(serial, "fetch device props");
            return;
        }
        let serial = serial.to_string();
        let bundle_id = self.config.bundle_id.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let props = adb::get_device_props(&serial, &bundle_id);
            let _ = tx.send(AdbMsg::DeviceProps(serial, props));
        });
    }

    pub(super) fn run_action(&self, serial: &str, args: &[&str], label: &str) {
        let serial = serial.to_string();
        let tx = self.tx.clone();
        let args: Vec<String> = args.iter().map(std::string::ToString::to_string).collect();
        let label = label.to_string();
        std::thread::spawn(move || {
            let str_args: Vec<&str> = args.iter().map(std::string::String::as_str).collect();
            let (ok, msg) = adb::run_device_action(&serial, &str_args);
            let status = if ok { "OK" } else { "FAILED" };
            let result = if msg.is_empty() {
                format!("{label}: {status}")
            } else {
                format!("{label}: {status} - {msg}")
            };
            let _ = tx.send(AdbMsg::DeviceActionResult(serial, result));
        });
    }
}
