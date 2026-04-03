use eframe::egui;

use super::AppLogLevel;
use crate::adb;

impl super::App {
    pub(super) fn draw_shell_tab(&mut self, ui: &mut egui::Ui, serial: &str) {
        let serial_owned = serial.to_string();
        let is_running = self.devices.get(serial).is_some_and(|ds| ds.shell.running);
        self.draw_shell_toolbar(ui, serial, &serial_owned, is_running);
        ui.separator();
        self.draw_shell_output(ui, serial);
        self.draw_shell_input(ui, serial, &serial_owned, is_running);
    }

    fn draw_shell_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        is_running: bool,
    ) {
        ui.horizontal(|ui| {
            if is_running {
                if ui.button("Disconnect").clicked() {
                    if let Some(mut handle) = self.shell_handles.remove(serial) {
                        handle.kill();
                    }
                    if let Some(ds) = self.devices.get_mut(serial_owned) {
                        ds.shell.running = false;
                        ds.push_shell_output("--- Disconnected ---".into());
                    }
                }
                ui.colored_label(egui::Color32::from_rgb(100, 200, 100), "Connected");
            } else {
                if ui.button("Connect Shell").clicked() {
                    self.connect_shell(serial, serial_owned);
                }
                ui.colored_label(egui::Color32::from_rgb(180, 180, 180), "Disconnected");
            }

            ui.separator();

            if ui.button("Clear").clicked() {
                if let Some(ds) = self.devices.get_mut(serial_owned) {
                    ds.shell_output.clear();
                }
            }

            if let Some(ds) = self.devices.get(serial) {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{} lines", ds.shell_output.len()));
                });
            }
        });
    }

    fn connect_shell(&mut self, serial: &str, serial_owned: &str) {
        if let Some(handle) = adb::spawn_shell(serial, self.tx.clone()) {
            self.shell_handles.insert(serial_owned.to_string(), handle);
            if let Some(ds) = self.devices.get_mut(serial_owned) {
                ds.shell.running = true;
                ds.shell_output.clear();
                ds.push_shell_output("--- Shell connected ---".into());
            }
            self.log(AppLogLevel::Info, format!("[{serial}] Shell started"));
        } else {
            self.log(AppLogLevel::Error, format!("[{serial}] Shell start failed"));
        }
    }

    fn draw_shell_output(&self, ui: &mut egui::Ui, serial: &str) {
        if let Some(ds) = self.devices.get(serial) {
            let lines: Vec<(String, egui::Color32)> = ds
                .shell_output
                .iter()
                .map(|line| (line.clone(), shell_line_color(line)))
                .collect();
            egui::ScrollArea::vertical()
                .id_salt(format!("shell_output_{serial}"))
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .max_height((ui.available_height() - 30.0).max(50.0))
                .show(ui, |ui| {
                    ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                    for (line, color) in &lines {
                        ui.label(egui::RichText::new(line).color(*color));
                    }
                });
        }
    }

    fn draw_shell_input(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        is_running: bool,
    ) {
        if !is_running || !self.devices.contains_key(serial_owned) {
            return;
        }

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("$")
                    .monospace()
                    .color(egui::Color32::from_rgb(100, 220, 100)),
            );

            let Some(ds) = self.devices.get_mut(serial_owned) else {
                return;
            };
            let input_response = ui.add_sized(
                [ui.available_width() - 60.0, 20.0],
                egui::TextEdit::singleline(&mut ds.shell_input)
                    .font(egui::FontId::monospace(12.0))
                    .hint_text("Enter command..."),
            );
            let enter_pressed =
                input_response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            if ui.button("Send").clicked() || enter_pressed {
                self.send_shell_command(serial, serial_owned);
                input_response.request_focus();
            }

            if input_response.has_focus() {
                self.handle_shell_history_keys(ui, serial_owned);
            }
        });
    }

    fn send_shell_command(&mut self, serial: &str, serial_owned: &str) {
        let mut send_failed = false;
        let mut handle_missing = false;
        if let Some(ds) = self.devices.get_mut(serial_owned) {
            let cmd = ds.shell_input.clone();
            if cmd.is_empty() {
                return;
            }
            ds.push_shell_output(format!("$ {cmd}"));
            ds.shell_history.push(cmd.clone());
            ds.shell_history_pos = ds.shell_history.len();
            ds.shell_input.clear();
            if let Some(handle) = self.shell_handles.get_mut(serial) {
                if !handle.send(&cmd) {
                    send_failed = true;
                    ds.push_shell_output("--- Send failed (pipe broken) ---".into());
                }
            } else {
                handle_missing = true;
            }
        }
        if send_failed {
            self.log(
                AppLogLevel::Error,
                format!("[{serial}] Shell command send failed"),
            );
        }
        if handle_missing {
            self.log_skipped(
                serial,
                "send shell command",
                "shell handle is not connected",
            );
        }
    }

    fn handle_shell_history_keys(&mut self, ui: &egui::Ui, serial_owned: &str) {
        if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
            if let Some(ds) = self.devices.get_mut(serial_owned) {
                if ds.shell_history_pos > 0 {
                    ds.shell_history_pos -= 1;
                    if let Some(cmd) = ds.shell_history.get(ds.shell_history_pos) {
                        ds.shell_input.clone_from(cmd);
                    }
                }
            }
        }
        if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
            if let Some(ds) = self.devices.get_mut(serial_owned) {
                if ds.shell_history_pos < ds.shell_history.len() {
                    ds.shell_history_pos += 1;
                    ds.shell_input = ds
                        .shell_history
                        .get(ds.shell_history_pos)
                        .cloned()
                        .unwrap_or_default();
                }
            }
        }
    }
}

fn shell_line_color(line: &str) -> egui::Color32 {
    if line.starts_with("$ ") {
        egui::Color32::from_rgb(100, 220, 100)
    } else if line.starts_with("--- ") {
        egui::Color32::from_rgb(100, 180, 255)
    } else {
        egui::Color32::from_rgb(200, 200, 200)
    }
}
