use eframe::egui;

use crate::adb::{self, AdbMsg};

impl super::App {
    pub(super) fn draw_device_extended_tools(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
    ) {
        Self::draw_section_header(ui, "Extended Tooling");

        egui::CollapsingHeader::new("Package Tools")
            .default_open(true)
            .show(ui, |ui| {
                self.draw_package_tools_section(ui, serial, serial_owned);
            });

        egui::CollapsingHeader::new("Networking & Tunnels")
            .default_open(false)
            .show(ui, |ui| {
                self.draw_network_tools_section(ui, serial, serial_owned);
            });

        egui::CollapsingHeader::new("Automation & Services")
            .default_open(false)
            .show(ui, |ui| {
                self.draw_command_tools_section(ui, serial, serial_owned);
            });

        egui::CollapsingHeader::new("Display & Settings")
            .default_open(false)
            .show(ui, |ui| {
                self.draw_display_and_settings_section(ui, serial, serial_owned);
            });

        egui::CollapsingHeader::new("Content & SQLite")
            .default_open(false)
            .show(ui, |ui| {
                self.draw_data_access_section(ui, serial, serial_owned);
            });

        egui::CollapsingHeader::new("OTA & Recovery")
            .default_open(false)
            .show(ui, |ui| {
                self.draw_ota_tools_section(ui, serial);
            });

        ui.add_space(8.0);
    }

    fn draw_package_tools_section(&mut self, ui: &mut egui::Ui, serial: &str, serial_owned: &str) {
        ui.horizontal(|ui| {
            ui.label("Package:");
            if let Some(device) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [260.0, 20.0],
                    egui::TextEdit::singleline(&mut device.package_tools.package_name)
                        .hint_text(&self.config.bundle_id),
                )
                .on_hover_text("Leave empty to use the configured package from Settings");
            }
            if ui.small_button("Use Config").clicked() {
                if let Some(device) = self.devices.get_mut(serial_owned) {
                    device
                        .package_tools
                        .package_name
                        .clone_from(&self.config.bundle_id);
                }
            }
        });

        ui.horizontal(|ui| {
            ui.label("List:");
            if let Some(device) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [220.0, 20.0],
                    egui::TextEdit::singleline(&mut device.package_tools.package_filter)
                        .hint_text("optional package filter"),
                );
            }
            if ui.button("All").clicked() {
                self.run_package_listing(serial, "pm list packages");
            }
            if ui.button("User").clicked() {
                self.run_package_listing(serial, "pm list packages -3");
            }
            if ui.button("Disabled").clicked() {
                self.run_package_listing(serial, "pm list packages -d");
            }
        });

        ui.horizontal(|ui| {
            if ui.button("Force Stop").clicked() {
                let Some(package) = self.selected_package_for_device(serial) else {
                    self.log_skipped(serial, "force-stop package", "package is not configured");
                    return;
                };
                self.run_action(
                    serial,
                    &["shell", "am", "force-stop", &package],
                    "Force stop package",
                );
            }
            if ui.button("Clear Data").clicked() {
                let Some(package) = self.selected_package_for_device(serial) else {
                    self.log_skipped(serial, "clear package data", "package is not configured");
                    return;
                };
                self.run_action(serial, &["shell", "pm", "clear", &package], "Clear package");
            }
            if ui.button("Enable").clicked() {
                let Some(package) = self.selected_package_for_device(serial) else {
                    self.log_skipped(serial, "enable package", "package is not configured");
                    return;
                };
                self.run_action(
                    serial,
                    &["shell", "pm", "enable", &package],
                    "Enable package",
                );
            }
            if ui.button("Disable").clicked() {
                let Some(package) = self.selected_package_for_device(serial) else {
                    self.log_skipped(serial, "disable package", "package is not configured");
                    return;
                };
                self.run_action(
                    serial,
                    &["shell", "pm", "disable-user", "--user", "0", &package],
                    "Disable package",
                );
            }
        });

        ui.horizontal(|ui| {
            ui.label("Permission:");
            if let Some(device) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [280.0, 20.0],
                    egui::TextEdit::singleline(&mut device.package_tools.permission_name)
                        .hint_text("android.permission.POST_NOTIFICATIONS"),
                );
            }
            if ui.button("Grant").clicked() {
                let Some(package) = self.selected_package_for_device(serial) else {
                    self.log_skipped(serial, "grant permission", "package is not configured");
                    return;
                };
                let permission = self
                    .devices
                    .get(serial)
                    .map(|device| device.package_tools.permission_name.trim().to_string())
                    .unwrap_or_default();
                if permission.is_empty() {
                    self.log_skipped(serial, "grant permission", "permission name is empty");
                    return;
                }
                self.run_action(
                    serial,
                    &["shell", "pm", "grant", &package, &permission],
                    "Grant permission",
                );
            }
            if ui.button("Revoke").clicked() {
                let Some(package) = self.selected_package_for_device(serial) else {
                    self.log_skipped(serial, "revoke permission", "package is not configured");
                    return;
                };
                let permission = self
                    .devices
                    .get(serial)
                    .map(|device| device.package_tools.permission_name.trim().to_string())
                    .unwrap_or_default();
                if permission.is_empty() {
                    self.log_skipped(serial, "revoke permission", "permission name is empty");
                    return;
                }
                self.run_action(
                    serial,
                    &["shell", "pm", "revoke", &package, &permission],
                    "Revoke permission",
                );
            }
            if ui.button("Split APKs...").clicked() {
                self.pick_and_install_split_apks(serial);
            }
        });
    }

    fn draw_network_tools_section(&mut self, ui: &mut egui::Ui, serial: &str, serial_owned: &str) {
        ui.horizontal(|ui| {
            ui.label("TCP/IP:");
            if let Some(device) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [70.0, 20.0],
                    egui::TextEdit::singleline(&mut device.connection_tools.tcpip_port)
                        .hint_text("5555"),
                );
            }
            if ui.button("Enable").clicked() {
                let port = self
                    .devices
                    .get(serial)
                    .map(|device| device.connection_tools.tcpip_port.trim().to_string())
                    .unwrap_or_default();
                if port.is_empty() {
                    self.log_skipped(serial, "enable tcpip", "TCP/IP port is empty");
                    return;
                }
                self.run_action(serial, &["tcpip", &port], "Enable tcpip");
            }
        });

        ui.horizontal(|ui| {
            if let Some(device) = self.devices.get_mut(serial_owned) {
                ui.label("Forward:");
                ui.add_sized(
                    [100.0, 20.0],
                    egui::TextEdit::singleline(&mut device.connection_tools.forward_local)
                        .hint_text("tcp:8080"),
                );
                ui.label("->");
                ui.add_sized(
                    [130.0, 20.0],
                    egui::TextEdit::singleline(&mut device.connection_tools.forward_remote)
                        .hint_text("tcp:8080"),
                );
            }
            if ui.button("Add").clicked() {
                let Some((local, remote)) = self.forward_specs(serial) else {
                    self.log_skipped(serial, "add port forward", "forward spec is incomplete");
                    return;
                };
                self.run_action(serial, &["forward", &local, &remote], "Forward add");
            }
            if ui.button("Remove").clicked() {
                let local = self
                    .devices
                    .get(serial)
                    .map(|device| device.connection_tools.forward_local.trim().to_string())
                    .unwrap_or_default();
                if local.is_empty() {
                    self.log_skipped(serial, "remove port forward", "local spec is empty");
                    return;
                }
                self.run_action(serial, &["forward", "--remove", &local], "Forward remove");
            }
            if ui.button("List").clicked() {
                self.run_action(serial, &["forward", "--list"], "Forward list");
            }
        });

        ui.horizontal(|ui| {
            if let Some(device) = self.devices.get_mut(serial_owned) {
                ui.label("Reverse:");
                ui.add_sized(
                    [100.0, 20.0],
                    egui::TextEdit::singleline(&mut device.connection_tools.reverse_remote)
                        .hint_text("tcp:8081"),
                );
                ui.label("->");
                ui.add_sized(
                    [130.0, 20.0],
                    egui::TextEdit::singleline(&mut device.connection_tools.reverse_local)
                        .hint_text("tcp:8081"),
                );
            }
            if ui.button("Add").clicked() {
                let Some((remote, local)) = self.reverse_specs(serial) else {
                    self.log_skipped(serial, "add reverse port", "reverse spec is incomplete");
                    return;
                };
                self.run_action(serial, &["reverse", &remote, &local], "Reverse add");
            }
            if ui.button("Remove").clicked() {
                let remote = self
                    .devices
                    .get(serial)
                    .map(|device| device.connection_tools.reverse_remote.trim().to_string())
                    .unwrap_or_default();
                if remote.is_empty() {
                    self.log_skipped(serial, "remove reverse port", "remote spec is empty");
                    return;
                }
                self.run_action(serial, &["reverse", "--remove", &remote], "Reverse remove");
            }
            if ui.button("List").clicked() {
                self.run_action(serial, &["reverse", "--list"], "Reverse list");
            }
        });
    }

    fn draw_command_tools_section(&mut self, ui: &mut egui::Ui, serial: &str, serial_owned: &str) {
        ui.horizontal(|ui| {
            ui.label("Instrumentation:");
            if let Some(device) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [360.0, 20.0],
                    egui::TextEdit::singleline(&mut device.command_tools.instrument_runner)
                        .hint_text("com.example.test/androidx.test.runner.AndroidJUnitRunner"),
                );
            }
            if ui.button("Run").clicked() {
                let runner = self
                    .devices
                    .get(serial)
                    .map(|device| device.command_tools.instrument_runner.trim().to_string())
                    .unwrap_or_default();
                if runner.is_empty() {
                    self.log_skipped(serial, "run instrumentation", "runner is empty");
                    return;
                }
                let cmd = format!("am instrument -w {}", adb::shell_quote(&runner));
                self.run_action(serial, &["shell", &cmd], "Instrumentation");
            }
        });

        ui.horizontal(|ui| {
            ui.label("am:");
            if let Some(device) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [420.0, 20.0],
                    egui::TextEdit::singleline(&mut device.command_tools.am_command)
                        .hint_text("start -n com.example.app/.MainActivity"),
                );
            }
            if ui.button("Run").clicked() {
                let am_command = self
                    .devices
                    .get(serial)
                    .map(|device| device.command_tools.am_command.trim().to_string())
                    .unwrap_or_default();
                if am_command.is_empty() {
                    self.log_skipped(serial, "run am command", "am command is empty");
                    return;
                }
                let cmd = format!("am {am_command}");
                self.run_action(serial, &["shell", &cmd], "am");
            }
        });

        ui.horizontal(|ui| {
            ui.label("pm:");
            if let Some(device) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [420.0, 20.0],
                    egui::TextEdit::singleline(&mut device.command_tools.pm_command)
                        .hint_text("list packages -f"),
                );
            }
            if ui.button("Run").clicked() {
                let pm_command = self
                    .devices
                    .get(serial)
                    .map(|device| device.command_tools.pm_command.trim().to_string())
                    .unwrap_or_default();
                if pm_command.is_empty() {
                    self.log_skipped(serial, "run pm command", "pm command is empty");
                    return;
                }
                let cmd = format!("pm {pm_command}");
                self.run_action(serial, &["shell", &cmd], "pm");
            }
        });

        ui.horizontal(|ui| {
            ui.label("cmd:");
            if let Some(device) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [120.0, 20.0],
                    egui::TextEdit::singleline(&mut device.command_tools.cmd_service)
                        .hint_text("package"),
                );
                ui.add_sized(
                    [280.0, 20.0],
                    egui::TextEdit::singleline(&mut device.command_tools.cmd_args)
                        .hint_text("list packages"),
                );
            }
            if ui.button("Run").clicked() {
                let (service, args) = self
                    .devices
                    .get(serial)
                    .map(|device| {
                        (
                            device.command_tools.cmd_service.trim().to_string(),
                            device.command_tools.cmd_args.trim().to_string(),
                        )
                    })
                    .unwrap_or_default();
                if service.is_empty() {
                    self.log_skipped(serial, "run cmd command", "service name is empty");
                    return;
                }
                let cmd = if args.is_empty() {
                    format!("cmd {service}")
                } else {
                    format!("cmd {service} {args}")
                };
                self.run_action(serial, &["shell", &cmd], "cmd");
            }
        });
    }

    fn draw_display_and_settings_section(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
    ) {
        ui.horizontal(|ui| {
            ui.label("wm size:");
            if let Some(device) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [120.0, 20.0],
                    egui::TextEdit::singleline(&mut device.command_tools.wm_size)
                        .hint_text("1080x1920"),
                );
            }
            if ui.button("Read").clicked() {
                self.run_action(serial, &["shell", "wm size"], "wm size");
            }
            if ui.button("Set").clicked() {
                let size = self
                    .devices
                    .get(serial)
                    .map(|device| device.command_tools.wm_size.trim().to_string())
                    .unwrap_or_default();
                if size.is_empty() {
                    self.log_skipped(serial, "set wm size", "wm size is empty");
                    return;
                }
                let cmd = format!("wm size {size}");
                self.run_action(serial, &["shell", &cmd], "wm size set");
            }
            if ui.button("Reset").clicked() {
                self.run_action(serial, &["shell", "wm size reset"], "wm size reset");
            }
        });

        ui.horizontal(|ui| {
            ui.label("wm density:");
            if let Some(device) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [120.0, 20.0],
                    egui::TextEdit::singleline(&mut device.command_tools.wm_density)
                        .hint_text("420"),
                );
            }
            if ui.button("Read").clicked() {
                self.run_action(serial, &["shell", "wm density"], "wm density");
            }
            if ui.button("Set").clicked() {
                let density = self
                    .devices
                    .get(serial)
                    .map(|device| device.command_tools.wm_density.trim().to_string())
                    .unwrap_or_default();
                if density.is_empty() {
                    self.log_skipped(serial, "set wm density", "wm density is empty");
                    return;
                }
                let cmd = format!("wm density {density}");
                self.run_action(serial, &["shell", &cmd], "wm density set");
            }
            if ui.button("Reset").clicked() {
                self.run_action(serial, &["shell", "wm density reset"], "wm density reset");
            }
        });

        ui.horizontal(|ui| {
            ui.label("settings:");
            if let Some(device) = self.devices.get_mut(serial_owned) {
                egui::ComboBox::from_id_salt(format!("settings_ns_{serial}"))
                    .selected_text(&device.command_tools.settings_namespace)
                    .width(80.0)
                    .show_ui(ui, |ui| {
                        for namespace in ["global", "secure", "system"] {
                            ui.selectable_value(
                                &mut device.command_tools.settings_namespace,
                                namespace.to_string(),
                                namespace,
                            );
                        }
                    });
                ui.add_sized(
                    [140.0, 20.0],
                    egui::TextEdit::singleline(&mut device.command_tools.settings_key)
                        .hint_text("development_settings_enabled"),
                );
                ui.add_sized(
                    [160.0, 20.0],
                    egui::TextEdit::singleline(&mut device.command_tools.settings_value)
                        .hint_text("1"),
                );
            }
        });

        let settings_parts = self.devices.get(serial).map(|device| {
            (
                device.command_tools.settings_namespace.clone(),
                device.command_tools.settings_key.trim().to_string(),
                device.command_tools.settings_value.trim().to_string(),
            )
        });
        let Some((namespace, key, value)) = settings_parts else {
            return;
        };

        ui.horizontal(|ui| {
            if ui.button("Get").clicked() {
                if key.is_empty() {
                    self.log_skipped(serial, "settings get", "settings key is empty");
                    return;
                }
                let cmd = format!("settings get {namespace} {}", adb::shell_quote(&key));
                self.run_action(serial, &["shell", &cmd], "settings get");
            }
            if ui.button("Put").clicked() {
                if key.is_empty() {
                    self.log_skipped(serial, "settings put", "settings key is empty");
                    return;
                }
                let cmd = format!(
                    "settings put {namespace} {} {}",
                    adb::shell_quote(&key),
                    adb::shell_quote(&value)
                );
                self.run_action(serial, &["shell", &cmd], "settings put");
            }
            if ui.button("Delete").clicked() {
                if key.is_empty() {
                    self.log_skipped(serial, "settings delete", "settings key is empty");
                    return;
                }
                let cmd = format!("settings delete {namespace} {}", adb::shell_quote(&key));
                self.run_action(serial, &["shell", &cmd], "settings delete");
            }
            if ui.button("Dev Options ON").clicked() {
                let cmd = "settings put global development_settings_enabled 1; \
                           settings put global adb_enabled 1";
                self.run_action(serial, &["shell", cmd], "Enable developer options");
            }
            if ui.button("Dev Options OFF").clicked() {
                let cmd = "settings put global development_settings_enabled 0";
                self.run_action(serial, &["shell", cmd], "Disable developer options");
            }
        });
    }

    fn draw_data_access_section(&mut self, ui: &mut egui::Ui, serial: &str, serial_owned: &str) {
        ui.horizontal(|ui| {
            ui.label("content:");
            if let Some(device) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [420.0, 20.0],
                    egui::TextEdit::singleline(&mut device.command_tools.content_command)
                        .hint_text("query --uri content://settings/global --projection name:value"),
                );
            }
            if ui.button("Run").clicked() {
                let content_command = self
                    .devices
                    .get(serial)
                    .map(|device| device.command_tools.content_command.trim().to_string())
                    .unwrap_or_default();
                if content_command.is_empty() {
                    self.log_skipped(serial, "run content command", "content command is empty");
                    return;
                }
                let cmd = format!("content {content_command}");
                self.run_action(serial, &["shell", &cmd], "content");
            }
        });

        ui.horizontal_wrapped(|ui| {
            if ui.small_button("Query Template").clicked() {
                if let Some(device) = self.devices.get_mut(serial_owned) {
                    device.command_tools.content_command =
                        "query --uri content://settings/global --projection name:value"
                            .to_string();
                }
            }
            if ui.small_button("Insert Template").clicked() {
                if let Some(device) = self.devices.get_mut(serial_owned) {
                    device.command_tools.content_command =
                        "insert --uri content://settings/global --bind name:s:demo --bind value:s:1"
                            .to_string();
                }
            }
            if ui.small_button("Update Template").clicked() {
                if let Some(device) = self.devices.get_mut(serial_owned) {
                    device.command_tools.content_command =
                        "update --uri content://settings/global --bind value:s:0 --where \"name='demo'\""
                            .to_string();
                }
            }
            if ui.small_button("Delete Template").clicked() {
                if let Some(device) = self.devices.get_mut(serial_owned) {
                    device.command_tools.content_command =
                        "delete --uri content://settings/global --where \"name='demo'\""
                            .to_string();
                }
            }
        });

        ui.separator();
        ui.label(
            egui::RichText::new("SQLite access (works best with debuggable apps via run-as)")
                .small()
                .strong(),
        );

        ui.horizontal(|ui| {
            ui.label("run-as:");
            if let Some(device) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [220.0, 20.0],
                    egui::TextEdit::singleline(&mut device.command_tools.sqlite_run_as_package)
                        .hint_text(&self.config.bundle_id),
                );
                ui.label("DB:");
                ui.add_sized(
                    [220.0, 20.0],
                    egui::TextEdit::singleline(&mut device.command_tools.sqlite_path)
                        .hint_text("databases/app.db"),
                );
            }
        });

        if let Some(device) = self.devices.get_mut(serial_owned) {
            ui.add(
                egui::TextEdit::multiline(&mut device.command_tools.sqlite_query)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .font(egui::FontId::monospace(12.0)),
            );
        }

        ui.horizontal(|ui| {
            if ui.button("Run Query").clicked() {
                let Some((db_path, query, run_as_package)) = self.sqlite_inputs(serial) else {
                    self.log_missing_device_state(serial, "sqlite query");
                    return;
                };
                if db_path.is_empty() {
                    self.log_skipped(serial, "sqlite query", "database path is empty");
                    return;
                }
                if query.is_empty() {
                    self.log_skipped(serial, "sqlite query", "SQL query is empty");
                    return;
                }

                let db_path = adb::shell_quote(&db_path);
                let query = adb::shell_quote(&query);
                let cmd = run_as_package.map_or_else(
                    || format!("sqlite3 {db_path} {query}"),
                    |package| {
                        format!(
                            "run-as {} sqlite3 {db_path} {query}",
                            adb::shell_quote(&package)
                        )
                    },
                );
                self.run_action(serial, &["shell", &cmd], "sqlite3");
            }

            if ui.button("List DBs").clicked() {
                let run_as_package = self
                    .sqlite_inputs(serial)
                    .and_then(|(_, _, package)| package);
                let Some(package) = run_as_package else {
                    self.log_skipped(serial, "list sqlite files", "run-as package is empty");
                    return;
                };
                let cmd = format!(
                    "run-as {} find . -maxdepth 4 -type f | grep -E '\\\\.(db|sqlite|sqlite3)$'",
                    adb::shell_quote(&package)
                );
                self.run_action(serial, &["shell", &cmd], "SQLite files");
            }
        });
    }

    fn draw_ota_tools_section(&mut self, ui: &mut egui::Ui, serial: &str) {
        ui.horizontal(|ui| {
            if ui.button("Reboot Sideload").clicked() {
                self.run_action(serial, &["reboot", "sideload"], "Reboot sideload");
            }
            if ui.button("Sideload OTA...").clicked() {
                self.pick_and_sideload_ota(serial);
            }
        });
        ui.colored_label(
            egui::Color32::from_rgb(140, 140, 140),
            "Use 'Reboot Sideload' only on devices or recoveries that support adb sideload mode.",
        );
    }

    fn run_package_listing(&self, serial: &str, base_command: &str) {
        let filter = self
            .devices
            .get(serial)
            .map(|device| device.package_tools.package_filter.trim().to_string())
            .unwrap_or_default();

        let command = if filter.is_empty() {
            base_command.to_string()
        } else {
            format!("{base_command} | grep -i -- {}", adb::shell_quote(&filter))
        };
        self.run_action(serial, &["shell", &command], "Package list");
    }

    fn selected_package_for_device(&self, serial: &str) -> Option<String> {
        let typed_package = self
            .devices
            .get(serial)
            .map(|device| device.package_tools.package_name.trim().to_string())
            .unwrap_or_default();

        let package = if typed_package.is_empty() {
            self.config.bundle_id.trim().to_string()
        } else {
            typed_package
        };

        if package.is_empty() {
            None
        } else {
            Some(package)
        }
    }

    fn forward_specs(&self, serial: &str) -> Option<(String, String)> {
        let device = self.devices.get(serial)?;
        let local = device.connection_tools.forward_local.trim();
        let remote = device.connection_tools.forward_remote.trim();
        if local.is_empty() || remote.is_empty() {
            None
        } else {
            Some((local.to_string(), remote.to_string()))
        }
    }

    fn reverse_specs(&self, serial: &str) -> Option<(String, String)> {
        let device = self.devices.get(serial)?;
        let remote = device.connection_tools.reverse_remote.trim();
        let local = device.connection_tools.reverse_local.trim();
        if local.is_empty() || remote.is_empty() {
            None
        } else {
            Some((remote.to_string(), local.to_string()))
        }
    }

    fn sqlite_inputs(&self, serial: &str) -> Option<(String, String, Option<String>)> {
        let device = self.devices.get(serial)?;
        let run_as_package = if device.command_tools.sqlite_run_as_package.trim().is_empty() {
            self.selected_package_for_device(serial)
        } else {
            Some(
                device
                    .command_tools
                    .sqlite_run_as_package
                    .trim()
                    .to_string(),
            )
        };
        Some((
            device.command_tools.sqlite_path.trim().to_string(),
            device.command_tools.sqlite_query.trim().to_string(),
            run_as_package,
        ))
    }

    fn pick_and_install_split_apks(&mut self, serial: &str) {
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter("APK files", &["apk"])
            .set_title("Select split APKs to install")
            .pick_files()
        {
            let serial = serial.to_string();
            let tx = self.tx.clone();
            let mut args = vec!["install-multiple".to_string(), "-r".to_string()];
            args.extend(paths.into_iter().map(|path| path.display().to_string()));
            std::thread::spawn(move || {
                let refs: Vec<&str> = args.iter().map(std::string::String::as_str).collect();
                let (ok, msg) = adb::run_device_action(&serial, &refs);
                let status = if ok {
                    "Split APK install succeeded"
                } else {
                    "Split APK install failed"
                };
                let _ = tx.send(AdbMsg::DeviceActionResult(
                    serial,
                    format!("{status}: {msg}"),
                ));
            });
        } else {
            self.log_cancelled(serial, "install split APKs");
        }
    }

    fn pick_and_sideload_ota(&mut self, serial: &str) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Select OTA package for adb sideload")
            .pick_file()
        {
            let serial = serial.to_string();
            let tx = self.tx.clone();
            let path = path.display().to_string();
            std::thread::spawn(move || {
                let (ok, msg) = adb::run_device_action(&serial, &["sideload", &path]);
                let status = if ok {
                    "OTA sideload succeeded"
                } else {
                    "OTA sideload failed"
                };
                let _ = tx.send(AdbMsg::DeviceActionResult(
                    serial,
                    format!("{status}: {msg}"),
                ));
            });
        } else {
            self.log_cancelled(serial, "sideload OTA package");
        }
    }

    pub(super) fn spawn_platform_tools_task<F>(&self, label: &str, task: F)
    where
        F: FnOnce() -> (bool, String) + Send + 'static,
    {
        let tx = self.tx.clone();
        let label = label.to_string();
        std::thread::spawn(move || {
            let (ok, msg) = task();
            let status = if ok { "OK" } else { "FAILED" };
            let result = if msg.trim().is_empty() {
                format!("{label}: {status}")
            } else {
                format!("{label}: {status} - {msg}")
            };
            let _ = tx.send(AdbMsg::DeviceActionResult("platform-tools".into(), result));
        });
    }
}
