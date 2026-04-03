use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use egui_extras::{Column, TableBuilder};

use super::helpers::{bytecount_lines, debug_line_color, format_size};
use super::{now_str, AppLogLevel};
use crate::adb::{self, AdbMsg};
use crate::device::{
    join_remote_path, normalize_remote_dir_path, parent_remote_dir, resolve_remote_path,
    ExplorerPreviewSkipReason,
};

const SHORTCUTS_WIDTH: f32 = 235.0;
const INSPECTOR_WIDTH: f32 = 390.0;
const PANEL_SEPARATOR: f32 = 6.0;
const FOLLOW_INTERVAL: Duration = Duration::from_secs(1);

struct ExplorerShortcut {
    label: &'static str,
    path_template: &'static str,
    description: &'static str,
}

struct ExplorerCommandTemplate {
    label: &'static str,
    template: &'static str,
    description: &'static str,
}

impl super::App {
    pub(super) fn draw_explorer_tab(&mut self, ui: &mut egui::Ui, serial: &str) {
        let serial_owned = serial.to_string();

        let needs_load = self.devices.get(serial).is_some_and(|device| {
            !device.explorer_loaded_once
                && !device.loading.explorer
                && device.explorer_error.is_empty()
        });
        if needs_load {
            self.explorer_navigate_no_history(serial);
        }

        let current_display_path = self.devices.get(serial).map_or_else(
            || "/".to_string(),
            |device| device.explorer_visible_path().to_string(),
        );
        let current_entries_path = self
            .devices
            .get(serial)
            .map_or_else(|| "/".to_string(), |device| device.explorer_path.clone());
        let is_loading = self
            .devices
            .get(serial)
            .is_some_and(|device| device.loading.explorer);

        self.draw_explorer_toolbar(ui, serial, &serial_owned, &current_display_path, is_loading);

        if let Some(device) = self.devices.get(serial) {
            if !device.explorer_error.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(255, 80, 80), &device.explorer_error);
            } else if is_loading && current_display_path != current_entries_path {
                ui.colored_label(
                    egui::Color32::from_rgb(180, 180, 120),
                    format!(
                        "Loading {current_display_path}... showing previous folder until ready."
                    ),
                );
            }
        }

        ui.separator();

        let available_rect = ui.available_rect_before_wrap();
        let outer_budget = PANEL_SEPARATOR
            .mul_add(-2.0, available_rect.width() - 320.0)
            .max(0.0);
        let desired_outer = SHORTCUTS_WIDTH + INSPECTOR_WIDTH;
        let scale = if desired_outer > outer_budget && outer_budget > 0.0 {
            outer_budget / desired_outer
        } else {
            1.0
        };
        let shortcuts_width = (SHORTCUTS_WIDTH * scale).max(170.0);
        let inspector_width = (INSPECTOR_WIDTH * scale).max(280.0);
        let table_width = PANEL_SEPARATOR
            .mul_add(
                -2.0,
                available_rect.width() - shortcuts_width - inspector_width,
            )
            .max(260.0);

        let shortcuts_rect = egui::Rect::from_min_size(
            available_rect.min,
            egui::vec2(shortcuts_width, available_rect.height()),
        );
        let table_rect = egui::Rect::from_min_size(
            egui::pos2(shortcuts_rect.max.x + PANEL_SEPARATOR, available_rect.min.y),
            egui::vec2(table_width, available_rect.height()),
        );
        let inspector_rect = egui::Rect::from_min_size(
            egui::pos2(table_rect.max.x + PANEL_SEPARATOR, available_rect.min.y),
            egui::vec2(
                (available_rect.max.x - table_rect.max.x - PANEL_SEPARATOR).max(200.0),
                available_rect.height(),
            ),
        );
        ui.allocate_rect(available_rect, egui::Sense::hover());

        for separator_x in [
            PANEL_SEPARATOR.mul_add(0.5, shortcuts_rect.max.x),
            PANEL_SEPARATOR.mul_add(0.5, table_rect.max.x),
        ] {
            ui.painter().line_segment(
                [
                    egui::pos2(separator_x, available_rect.min.y),
                    egui::pos2(separator_x, available_rect.max.y),
                ],
                egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 60, 60)),
            );
        }

        let bundle_id = self.config.bundle_id.clone();

        let mut shortcuts_ui = ui.new_child(egui::UiBuilder::new().max_rect(shortcuts_rect));
        shortcuts_ui.set_clip_rect(shortcuts_rect);
        self.draw_explorer_shortcuts(&mut shortcuts_ui, serial, &bundle_id, &current_display_path);

        let mut table_ui = ui.new_child(egui::UiBuilder::new().max_rect(table_rect));
        table_ui.set_clip_rect(table_rect);
        self.draw_explorer_table(
            &mut table_ui,
            serial,
            &serial_owned,
            &current_entries_path,
            is_loading,
        );

        let mut inspector_ui = ui.new_child(egui::UiBuilder::new().max_rect(inspector_rect));
        inspector_ui.set_clip_rect(inspector_rect);
        self.draw_explorer_inspector(
            &mut inspector_ui,
            serial,
            &serial_owned,
            &current_display_path,
        );
    }

    fn draw_explorer_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        current_display_path: &str,
        is_loading: bool,
    ) {
        let mut navigate_to: Option<String> = None;

        ui.horizontal(|ui| {
            let has_history = self
                .devices
                .get(serial)
                .is_some_and(|device| !device.explorer_history.is_empty());
            if ui
                .add_enabled(has_history, egui::Button::new("<"))
                .on_hover_text("Back")
                .clicked()
            {
                self.explorer_navigate_back(serial);
            }

            let parent = parent_remote_dir(current_display_path);
            if ui
                .add_enabled(parent.is_some(), egui::Button::new("^"))
                .on_hover_text("Parent directory")
                .clicked()
            {
                if let Some(parent) = parent {
                    self.explorer_navigate(serial, Some(&parent));
                }
            }

            if ui.button("Refresh").clicked() {
                self.explorer_navigate_no_history(serial);
            }
            if ui.button("Copy Path").clicked() {
                ui.ctx().copy_text(current_display_path.to_string());
            }

            ui.separator();
            ui.label("Path:");

            let enter_pressed = if let Some(device) = self.devices.get_mut(serial) {
                let response = ui.add_sized(
                    [ui.available_width() - 255.0, 22.0],
                    egui::TextEdit::singleline(&mut device.explorer_path_input)
                        .font(egui::FontId::monospace(12.0))
                        .hint_text("/sdcard/Download"),
                );
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter))
            } else {
                false
            };

            if ui.button("Go").clicked() || enter_pressed {
                navigate_to = self.devices.get(serial).and_then(|device| {
                    let path = device.explorer_path_input.trim();
                    if path.is_empty() {
                        None
                    } else {
                        Some(path.to_string())
                    }
                });
            }

            if is_loading {
                ui.spinner();
            }
        });

        ui.horizontal_wrapped(|ui| {
            if ui.small_button("/").clicked() {
                self.explorer_navigate(serial, Some("/"));
            }

            let parts: Vec<&str> = current_display_path
                .split('/')
                .filter(|part| !part.is_empty())
                .collect();
            let mut built = String::new();
            for part in &parts {
                built.push('/');
                built.push_str(part);
                ui.label("/");
                if ui.small_button(*part).clicked() {
                    let target = built.clone();
                    self.explorer_navigate(serial, Some(&target));
                }
            }

            ui.separator();

            if ui.small_button("/sdcard").clicked() {
                self.explorer_navigate(serial, Some("/sdcard"));
            }
            if ui.small_button("/data").clicked() {
                self.explorer_navigate(serial, Some("/data"));
            }
            let bundle_id = self.config.bundle_id.clone();
            if ui.small_button("App Data").clicked() {
                let path = format!("/sdcard/Android/data/{bundle_id}/files");
                self.explorer_navigate(serial, Some(&path));
            }
            if ui.small_button("Temp").clicked() {
                self.explorer_navigate(serial, Some("/data/local/tmp"));
            }

            ui.separator();

            if ui
                .add_enabled(!is_loading, egui::Button::new("Upload..."))
                .clicked()
            {
                self.upload_file_to_explorer(serial_owned, current_display_path.to_string());
            }
            if ui
                .add_enabled(!is_loading, egui::Button::new("New Folder"))
                .clicked()
            {
                let path = join_remote_path(current_display_path, "new_folder");
                let serial = serial_owned.to_string();
                let refresh_path = current_display_path.to_string();
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let (ok, msg) = adb::mkdir_remote(&serial, &path);
                    let status = if ok { "Created" } else { "Failed" };
                    let _ = tx.send(AdbMsg::DeviceActionResult(
                        serial.clone(),
                        format!("mkdir {status}: {msg}"),
                    ));
                    if ok {
                        let _ = tx.send(AdbMsg::ExplorerReloadIfCurrent(serial, refresh_path));
                    }
                });
            }
        });

        if let Some(path) = navigate_to {
            self.explorer_navigate(serial, Some(&path));
        }
    }

    fn draw_explorer_shortcuts(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        bundle_id: &str,
        current_display_path: &str,
    ) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Shortcuts").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(current_display_path)
                        .small()
                        .monospace()
                        .color(egui::Color32::from_rgb(140, 140, 140)),
                );
            });
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .id_salt(format!("explorer_shortcuts_{serial}"))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.draw_shortcut_group(ui, serial, "App", APP_SHORTCUTS, bundle_id);
                self.draw_shortcut_group(ui, serial, "Storage", STORAGE_SHORTCUTS, bundle_id);
                self.draw_shortcut_group(ui, serial, "System", SYSTEM_SHORTCUTS, bundle_id);
                self.draw_shortcut_group(ui, serial, "Low-level", LOW_LEVEL_SHORTCUTS, bundle_id);
            });
    }

    fn draw_shortcut_group(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        title: &str,
        shortcuts: &[ExplorerShortcut],
        bundle_id: &str,
    ) {
        egui::CollapsingHeader::new(title)
            .default_open(true)
            .show(ui, |ui| {
                for shortcut in shortcuts {
                    let target = materialize_shortcut_path(shortcut.path_template, bundle_id);
                    ui.horizontal(|ui| {
                        if ui
                            .button(shortcut.label)
                            .on_hover_text(format!("{target}\n{}", shortcut.description))
                            .clicked()
                        {
                            self.explorer_navigate(serial, Some(&target));
                        }
                        ui.label(
                            egui::RichText::new(shortcut.description)
                                .small()
                                .color(egui::Color32::from_rgb(140, 140, 140)),
                        );
                    });
                }
            });
    }

    fn draw_explorer_table(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        current_entries_path: &str,
        is_loading: bool,
    ) {
        let entries = self
            .devices
            .get(serial)
            .map(|device| device.explorer_entries.clone())
            .unwrap_or_default();
        let selected = self
            .devices
            .get(serial)
            .and_then(|device| device.explorer_selected.clone());

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Files").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!("{} item(s)", entries.len()));
            });
        });
        ui.separator();

        let row_height = 18.0;
        let table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(20.0))
            .column(Column::remainder().resizable(true).clip(true))
            .column(Column::initial(65.0).resizable(true).clip(true))
            .column(Column::initial(50.0).resizable(true).clip(true))
            .column(Column::initial(110.0).resizable(true).clip(true))
            .sense(egui::Sense::click());

        table
            .header(row_height, |mut header| {
                header.col(|_| {});
                header.col(|ui| {
                    ui.label("Name");
                });
                header.col(|ui| {
                    ui.label("Size");
                });
                header.col(|ui| {
                    ui.label("Perm");
                });
                header.col(|ui| {
                    ui.label("Modified");
                });
            })
            .body(|body| {
                body.rows(row_height, entries.len(), |mut row| {
                    let index = row.index();
                    let entry = &entries[index];
                    let is_selected = selected.as_ref() == Some(&entry.name);
                    row.set_selected(is_selected);

                    let (icon, icon_color) = if entry.is_dir {
                        ("D", egui::Color32::from_rgb(255, 200, 80))
                    } else {
                        ("F", egui::Color32::from_rgb(160, 160, 160))
                    };
                    let name_color = if entry.is_dir {
                        egui::Color32::from_rgb(100, 200, 255)
                    } else {
                        egui::Color32::from_rgb(220, 220, 220)
                    };

                    row.col(|ui| {
                        ui.style_mut().interaction.selectable_labels = false;
                        ui.label(egui::RichText::new(icon).color(icon_color).small());
                    });
                    row.col(|ui| {
                        ui.style_mut().interaction.selectable_labels = false;
                        ui.label(
                            egui::RichText::new(&entry.name)
                                .monospace()
                                .color(name_color),
                        );
                    });
                    row.col(|ui| {
                        ui.style_mut().interaction.selectable_labels = false;
                        let size = if entry.is_dir {
                            "-".to_string()
                        } else {
                            format_size(entry.size)
                        };
                        ui.label(
                            egui::RichText::new(size).color(egui::Color32::from_rgb(170, 170, 170)),
                        );
                    });
                    row.col(|ui| {
                        ui.style_mut().interaction.selectable_labels = false;
                        ui.label(
                            egui::RichText::new(&entry.permissions)
                                .small()
                                .color(egui::Color32::from_rgb(130, 130, 130)),
                        );
                    });
                    row.col(|ui| {
                        ui.style_mut().interaction.selectable_labels = false;
                        ui.label(
                            egui::RichText::new(&entry.modified)
                                .small()
                                .color(egui::Color32::from_rgb(140, 140, 140)),
                        );
                    });

                    let response = row.response();
                    let full_path = join_remote_path(current_entries_path, &entry.name);

                    if response.clicked() && !is_loading {
                        if entry.is_dir {
                            if let Some(device) = self.devices.get_mut(serial_owned) {
                                device.explorer_selected = Some(entry.name.clone());
                                device.explorer_preview = None;
                                device.explorer_preview_error.clear();
                                device.explorer_preview_loading = false;
                            }
                        } else {
                            self.start_explorer_preview(serial_owned, entry);
                        }
                    }

                    if response.double_clicked() && entry.is_dir && !is_loading {
                        self.explorer_navigate(serial_owned, Some(&full_path));
                    }

                    let entry_clone = entry.clone();
                    let serial = serial_owned.to_string();
                    let tx = self.tx.clone();
                    let refresh_path = current_entries_path.to_string();
                    response.context_menu(|ui| {
                        if is_loading {
                            ui.colored_label(
                                egui::Color32::from_rgb(180, 180, 120),
                                "Wait for the current folder load to finish.",
                            );
                            return;
                        }

                        if entry_clone.is_dir && ui.button("Open").clicked() {
                            self.explorer_navigate(&serial, Some(&full_path));
                            ui.close();
                        }

                        if ui.button("Copy name").clicked() {
                            ui.ctx().copy_text(entry_clone.name.clone());
                            ui.close();
                        }

                        if ui.button("Copy path").clicked() {
                            ui.ctx().copy_text(full_path.clone());
                            ui.close();
                        }

                        if !entry_clone.is_dir && ui.button("Preview").clicked() {
                            self.start_explorer_preview(&serial, &entry_clone);
                            ui.close();
                        }

                        if !entry_clone.is_dir && ui.button("Download...").clicked() {
                            if let Some(save) = rfd::FileDialog::new()
                                .set_file_name(&entry_clone.name)
                                .set_title("Save file")
                                .save_file()
                            {
                                let local = save.display().to_string();
                                let remote = full_path.clone();
                                let serial = serial.clone();
                                let tx = tx.clone();
                                std::thread::spawn(move || {
                                    let (ok, msg) = adb::pull_remote_file(&serial, &remote, &local);
                                    let status = if ok { "Downloaded" } else { "Download failed" };
                                    let _ = tx.send(AdbMsg::DeviceActionResult(
                                        serial,
                                        format!("{status}: {msg}"),
                                    ));
                                });
                            }
                            ui.close();
                        }

                        ui.separator();

                        if ui.button("Delete").clicked() {
                            let remote = full_path.clone();
                            let serial = serial.clone();
                            let tx = tx.clone();
                            let is_dir = entry_clone.is_dir;
                            let refresh_path = refresh_path.clone();
                            std::thread::spawn(move || {
                                let (ok, msg) = adb::delete_remote(&serial, &remote, is_dir);
                                let status = if ok { "Deleted" } else { "Delete failed" };
                                let _ = tx.send(AdbMsg::DeviceActionResult(
                                    serial.clone(),
                                    format!("{status}: {msg}"),
                                ));
                                if ok {
                                    let _ = tx.send(AdbMsg::ExplorerReloadIfCurrent(
                                        serial,
                                        refresh_path,
                                    ));
                                }
                            });
                            ui.close();
                        }
                    });
                });
            });
    }

    fn draw_explorer_inspector(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        current_display_path: &str,
    ) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Inspector").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(device) = self.devices.get(serial) {
                    ui.label(
                        egui::RichText::new(device.explorer_visible_path())
                            .small()
                            .monospace()
                            .color(egui::Color32::from_rgb(140, 140, 140)),
                    );
                }
            });
        });
        ui.separator();

        ui.horizontal(|ui| {
            if let Some(device) = self.devices.get_mut(serial_owned) {
                ui.selectable_value(&mut device.explorer_right_tab, 0, "Preview");
                ui.selectable_value(&mut device.explorer_right_tab, 1, "Commands");
            }
        });
        ui.separator();

        let active_tab = self
            .devices
            .get(serial)
            .map_or(0, |device| device.explorer_right_tab);
        match active_tab {
            1 => self.draw_explorer_commands(ui, serial, serial_owned, current_display_path),
            _ => self.draw_explorer_preview(ui, serial),
        }
    }

    fn draw_explorer_preview(&self, ui: &mut egui::Ui, serial: &str) {
        if let Some(device) = self.devices.get(serial) {
            if let Some(selected) = device.explorer_selected.as_ref() {
                ui.label(egui::RichText::new(selected).strong().monospace());
                ui.separator();

                if device.explorer_preview_loading {
                    ui.colored_label(egui::Color32::from_rgb(180, 180, 120), "Loading preview...");
                } else if let Some(content) = device.explorer_preview.as_ref() {
                    egui::ScrollArea::both()
                        .id_salt(format!("explorer_preview_{serial}"))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                            for line in content.lines() {
                                ui.label(line);
                            }
                        });
                } else if !device.explorer_preview_error.is_empty() {
                    ui.colored_label(
                        egui::Color32::from_rgb(255, 80, 80),
                        &device.explorer_preview_error,
                    );
                } else {
                    ui.colored_label(
                        egui::Color32::from_rgb(140, 140, 140),
                        "Select a file to preview.",
                    );
                }
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("Select a file to preview.");
                });
            }
        }
    }

    fn draw_explorer_commands(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        current_display_path: &str,
    ) {
        let selected = self
            .devices
            .get(serial)
            .and_then(|device| device.explorer_selected.clone());
        let selected_path = selected
            .as_ref()
            .map(|name| join_remote_path(current_display_path, name));

        ui.label(egui::RichText::new("Explorer Commands").strong());
        ui.colored_label(
            egui::Color32::from_rgb(140, 140, 140),
            format!(
                "Working directory: {current_display_path}\n`cd` updates the explorer path. `tail -f` runs as live polling."
            ),
        );
        ui.add_space(4.0);

        let run_requested = self.draw_explorer_command_input_row(
            ui,
            serial,
            serial_owned,
            selected_path.as_deref(),
        );

        if run_requested {
            self.run_explorer_command(serial);
        }

        ui.separator();
        self.draw_explorer_command_templates(
            ui,
            serial,
            current_display_path,
            selected_path.as_deref(),
        );
        ui.separator();
        let available_rect = ui.available_rect_before_wrap();
        let output_height = (available_rect.height() * 0.62).max(160.0);
        let log_height = (available_rect.height() - output_height - 8.0).max(110.0);

        let output_rect = egui::Rect::from_min_size(
            available_rect.min,
            egui::vec2(available_rect.width(), output_height),
        );
        let log_rect = egui::Rect::from_min_size(
            egui::pos2(available_rect.min.x, output_rect.max.y + 8.0),
            egui::vec2(available_rect.width(), log_height),
        );
        ui.allocate_rect(available_rect, egui::Sense::hover());

        let mut output_ui = ui.new_child(egui::UiBuilder::new().max_rect(output_rect));
        output_ui.set_clip_rect(output_rect);
        self.draw_explorer_command_output(&mut output_ui, serial);

        let mut log_ui = ui.new_child(egui::UiBuilder::new().max_rect(log_rect));
        log_ui.set_clip_rect(log_rect);
        self.draw_explorer_log(&mut log_ui, serial, serial_owned);
    }

    fn draw_explorer_command_input_row(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        selected_path: Option<&str>,
    ) -> bool {
        let mut run_requested = false;
        let mut copy_output = false;
        let mut use_selected = false;
        let mut stop_requested = false;
        let mut clear_requested = false;

        ui.horizontal(|ui| {
            let is_running = self
                .devices
                .get(serial)
                .is_some_and(|device| device.explorer_command_running);

            if let Some(device) = self.devices.get_mut(serial_owned) {
                let response = ui.add_sized(
                    [ui.available_width() - 215.0, 22.0],
                    egui::TextEdit::singleline(&mut device.explorer_command_input)
                        .font(egui::FontId::monospace(12.0))
                        .hint_text("ls -la, cat /system/build.prop, grep -r pattern /sdcard"),
                );
                let enter_pressed =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if enter_pressed {
                    run_requested = true;
                }
                if response.has_focus() {
                    handle_explorer_command_history_keys(ui, device);
                }
            }

            if ui
                .add_enabled(!is_running, egui::Button::new("Run"))
                .clicked()
            {
                run_requested = true;
            }
            let can_stop = self
                .devices
                .get(serial)
                .is_some_and(|device| device.explorer_follow_stop.is_some());
            if ui
                .add_enabled(can_stop, egui::Button::new("Stop"))
                .clicked()
            {
                stop_requested = true;
            }
            if ui.button("Clear").clicked() {
                clear_requested = true;
            }
            if ui
                .add_enabled(selected_path.is_some(), egui::Button::new("Use Selected"))
                .clicked()
            {
                use_selected = true;
            }
            if ui.button("Copy").clicked() {
                copy_output = true;
            }
        });

        if stop_requested {
            self.stop_explorer_follow(serial);
        }
        if clear_requested {
            if let Some(device) = self.devices.get_mut(serial_owned) {
                device.explorer_command_output.clear();
                device.explorer_command_status.clear();
            }
        }
        if use_selected {
            if let (Some(path), Some(device)) = (selected_path, self.devices.get_mut(serial_owned))
            {
                if !device.explorer_command_input.is_empty()
                    && !device.explorer_command_input.ends_with(' ')
                {
                    device.explorer_command_input.push(' ');
                }
                device.explorer_command_input.push_str(path);
            }
        }
        if copy_output {
            if let Some(device) = self.devices.get(serial) {
                ui.ctx().copy_text(device.explorer_command_output.clone());
            }
        }

        run_requested
    }

    fn draw_explorer_command_templates(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        current_display_path: &str,
        selected_path: Option<&str>,
    ) {
        egui::ScrollArea::vertical()
            .id_salt(format!("explorer_command_templates_{serial}"))
            .max_height(220.0)
            .show(ui, |ui| {
                for (title, templates) in command_template_groups() {
                    egui::CollapsingHeader::new(title)
                        .default_open(matches!(title, "Navigation" | "Read"))
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                for template in templates {
                                    if ui
                                        .small_button(template.label)
                                        .on_hover_text(template.description)
                                        .clicked()
                                    {
                                        let command = materialize_command_template(
                                            template.template,
                                            current_display_path,
                                            selected_path,
                                        );
                                        if let Some(device) = self.devices.get_mut(serial) {
                                            device.explorer_command_input = command;
                                        }
                                    }
                                }
                            });
                        });
                }
            });
    }

    fn draw_explorer_command_output(&self, ui: &mut egui::Ui, serial: &str) {
        if let Some(device) = self.devices.get(serial) {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Output").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let line_count = bytecount_lines(&device.explorer_command_output);
                    if !device.explorer_command_status.is_empty() {
                        ui.colored_label(
                            if device.explorer_command_running {
                                egui::Color32::from_rgb(180, 180, 120)
                            } else {
                                egui::Color32::from_rgb(140, 140, 140)
                            },
                            &device.explorer_command_status,
                        );
                        ui.separator();
                    }
                    ui.label(format!("{line_count} line(s)"));
                });
            });
            ui.separator();

            if device.explorer_command_output.is_empty() {
                ui.colored_label(
                    egui::Color32::from_rgb(140, 140, 140),
                    "Run a command or choose a template above.",
                );
                return;
            }

            egui::ScrollArea::both()
                .id_salt(format!("explorer_command_output_{serial}"))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                    for line in device.explorer_command_output.lines() {
                        ui.label(egui::RichText::new(line).color(debug_line_color(line)));
                    }
                });
        }
    }

    fn draw_explorer_log(&mut self, ui: &mut egui::Ui, serial: &str, serial_owned: &str) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Command Log").strong());
            if ui.button("Clear Log").clicked() {
                if let Some(device) = self.devices.get_mut(serial_owned) {
                    device.explorer_log_lines.clear();
                }
            }
            if ui.button("Copy Log").clicked() {
                if let Some(device) = self.devices.get(serial) {
                    ui.ctx().copy_text(device.explorer_log_lines.join("\n"));
                }
            }
        });
        ui.separator();

        if let Some(device) = self.devices.get(serial) {
            if device.explorer_log_lines.is_empty() {
                ui.colored_label(
                    egui::Color32::from_rgb(140, 140, 140),
                    "Explorer command history will appear here.",
                );
                return;
            }

            egui::ScrollArea::vertical()
                .id_salt(format!("explorer_command_log_{serial}"))
                .max_height(150.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.style_mut().override_font_id = Some(egui::FontId::monospace(11.0));
                    for line in &device.explorer_log_lines {
                        ui.label(egui::RichText::new(line).color(debug_line_color(line)));
                    }
                });
        }
    }

    pub(super) fn handle_explorer_command_report(
        &mut self,
        serial: &str,
        session: u64,
        report: &adb::ExplorerCommandReport,
    ) {
        let level = if report.success {
            AppLogLevel::Info
        } else if report.timed_out {
            AppLogLevel::Warn
        } else {
            AppLogLevel::Error
        };
        let summary = format_explorer_command_summary(report);
        let response_summary = format_explorer_response_summary(&report.output);

        let should_log_globally = true;
        let mut stale_session = false;
        if let Some(device) = self.devices.get_mut(serial) {
            if device.explorer_command_session == session {
                device.set_explorer_command_output(report.output.clone());
                device.push_explorer_log(format!("{} {}", now_str(), summary));
                if !response_summary.is_empty() {
                    device.push_explorer_log(format!("{} [RESP] {response_summary}", now_str()));
                }

                if report.follow_poll {
                    device.explorer_command_status = if report.success {
                        format!("Following ({} ms)", report.duration_ms)
                    } else if report.timed_out {
                        format!("Follow poll timed out after {} ms", report.duration_ms)
                    } else {
                        format!("Follow poll failed after {} ms", report.duration_ms)
                    };
                } else {
                    let status = if report.success {
                        format!("Command completed in {} ms", report.duration_ms)
                    } else if report.timed_out {
                        format!("Command timed out after {} ms", report.duration_ms)
                    } else {
                        format!("Command failed in {} ms", report.duration_ms)
                    };
                    let _ = device.finish_explorer_command_session(session, status);
                }
            } else {
                stale_session = true;
            }
        } else {
            self.log_missing_device_state(serial, "explorer command result");
            return;
        }

        if stale_session {
            self.log_stale_message(serial, "explorer command result");
        } else if should_log_globally {
            self.log(level, format!("[{serial}] {summary}"));
        }
    }

    fn log_explorer_command_start(
        &mut self,
        serial: &str,
        cwd: &str,
        command: &str,
        follow_poll: bool,
    ) {
        let label = if follow_poll { "[FOLLOW]" } else { "[CMD]" };
        let line = format!("{label} START cwd={cwd} cmd={command}");
        if let Some(device) = self.devices.get_mut(serial) {
            device.push_explorer_log(format!("{} {line}", now_str()));
        }

        self.log(
            AppLogLevel::Info,
            format!("[{serial}] Explorer command started: {command} @ {cwd}"),
        );
    }

    fn run_explorer_command(&mut self, serial: &str) {
        let serial_owned = serial.to_string();
        let current_cwd = self.devices.get(serial).map_or_else(
            || "/".to_string(),
            |device| device.explorer_visible_path().to_string(),
        );

        let Some(command) = self.devices.get_mut(serial).and_then(|device| {
            let command = device.explorer_command_input.trim().to_string();
            if command.is_empty() {
                None
            } else {
                device.stop_explorer_follow();
                device.explorer_command_history.push(command.clone());
                device.explorer_command_history_pos = device.explorer_command_history.len();
                device.set_explorer_command_output(String::new());
                Some(command)
            }
        }) else {
            if self.devices.contains_key(serial) {
                self.log_skipped(serial, "run explorer command", "command is empty");
            } else {
                self.log_missing_device_state(serial, "run explorer command");
            }
            return;
        };

        let session = if let Some(device) = self.devices.get_mut(serial) {
            device.start_next_explorer_command_session()
        } else {
            self.log_missing_device_state(serial, "start explorer command session");
            return;
        };

        if let Some(path) = parse_cd_command(&command) {
            self.log_explorer_command_start(serial, &current_cwd, &command, false);
            let resolved = resolve_remote_path(&current_cwd, &path);
            let report = adb::ExplorerCommandReport {
                cwd: current_cwd.clone(),
                command: command.clone(),
                duration_ms: 0,
                output: format!("cd -> {resolved}"),
                success: true,
                timed_out: false,
                follow_poll: false,
            };
            if let Some(device) = self.devices.get_mut(serial) {
                device.set_explorer_command_output(report.output.clone());
                device.push_explorer_log(format!(
                    "{} {}",
                    now_str(),
                    format_explorer_command_summary(&report)
                ));
                let response_summary = format_explorer_response_summary(&report.output);
                if !response_summary.is_empty() {
                    device.push_explorer_log(format!("{} [RESP] {response_summary}", now_str()));
                }
                let _ = device.finish_explorer_command_session(
                    session,
                    format!("Changed directory to {resolved}"),
                );
            }
            self.log(
                AppLogLevel::Info,
                format!("[{serial}] {}", format_explorer_command_summary(&report)),
            );
            self.explorer_navigate(serial, Some(&resolved));
            return;
        }

        if command == "pwd" {
            self.log_explorer_command_start(serial, &current_cwd, &command, false);
            let output = format!("{current_cwd}\n");
            let report = adb::ExplorerCommandReport {
                cwd: current_cwd.clone(),
                command: command.clone(),
                duration_ms: 0,
                output: output.clone(),
                success: true,
                timed_out: false,
                follow_poll: false,
            };
            if let Some(device) = self.devices.get_mut(serial) {
                device.set_explorer_command_output(output);
                device.push_explorer_log(format!(
                    "{} {}",
                    now_str(),
                    format_explorer_command_summary(&report)
                ));
                let response_summary = format_explorer_response_summary(&report.output);
                if !response_summary.is_empty() {
                    device.push_explorer_log(format!("{} [RESP] {response_summary}", now_str()));
                }
                let _ =
                    device.finish_explorer_command_session(session, "Printed working directory");
            }
            self.log(
                AppLogLevel::Info,
                format!("[{serial}] {}", format_explorer_command_summary(&report)),
            );
            return;
        }

        if let Some(file_arg) = parse_tail_follow_target(&command) {
            self.log_explorer_command_start(serial, &current_cwd, &command, true);
            let target_file = resolve_remote_path(&current_cwd, &strip_matching_quotes(&file_arg));
            self.start_explorer_follow(serial, session, current_cwd, command, target_file);
            return;
        }

        self.log_explorer_command_start(serial, &current_cwd, &command, false);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let result = adb::run_explorer_command(&serial_owned, &current_cwd, &command);
            let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let (success, output) = match result {
                Ok(output) => (true, output),
                Err(error) => (false, error),
            };
            let report = adb::ExplorerCommandReport {
                cwd: current_cwd,
                command,
                duration_ms,
                timed_out: is_timeout_text(&output),
                output,
                success,
                follow_poll: false,
            };
            let _ = tx.send(AdbMsg::ExplorerCommandResult(serial_owned, session, report));
        });
    }

    fn start_explorer_follow(
        &mut self,
        serial: &str,
        session: u64,
        display_cwd: String,
        display_command: String,
        target_file: String,
    ) {
        let Some(device) = self.devices.get_mut(serial) else {
            self.log_missing_device_state(serial, "start explorer follow");
            return;
        };
        let stop = Arc::new(AtomicBool::new(false));
        device.explorer_follow_stop = Some(stop.clone());
        device.explorer_command_status = format!("Following {target_file}");

        let serial_owned = serial.to_string();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let poll_command = format!("tail -n 50 -- {}", adb::shell_quote(&target_file));
                let started = std::time::Instant::now();
                let result = adb::run_explorer_command(&serial_owned, "/", &poll_command);
                let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                let (success, output) = match result {
                    Ok(output) => (true, output),
                    Err(error) => (false, error),
                };
                let report = adb::ExplorerCommandReport {
                    cwd: display_cwd.clone(),
                    command: display_command.clone(),
                    duration_ms,
                    timed_out: is_timeout_text(&output),
                    output,
                    success,
                    follow_poll: true,
                };
                let _ = tx.send(AdbMsg::ExplorerCommandResult(
                    serial_owned.clone(),
                    session,
                    report,
                ));

                let mut waited = Duration::ZERO;
                let tick = Duration::from_millis(200);
                while waited < FOLLOW_INTERVAL && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(tick);
                    waited += tick;
                }
            }

            let _ = tx.send(AdbMsg::ExplorerCommandStopped(
                serial_owned,
                session,
                "Follow stopped".to_string(),
            ));
        });
    }

    fn stop_explorer_follow(&mut self, serial: &str) {
        if let Some(device) = self.devices.get_mut(serial) {
            device.stop_explorer_follow();
            device.explorer_command_status = "Stopping follow...".to_string();
        }
    }

    fn log_explorer_listing_start(&mut self, serial: &str, path: &str, reason: &str) {
        let line = format!("[LIST] START reason={reason} path={path}");
        if let Some(device) = self.devices.get_mut(serial) {
            device.push_explorer_log(format!("{} {line}", now_str()));
        }
        self.log(
            AppLogLevel::Info,
            format!("[{serial}] Explorer listing started: {path} ({reason})"),
        );
    }

    fn start_explorer_preview(&mut self, serial: &str, entry: &crate::adb::RemoteFileEntry) {
        let preview_request = if let Some(device) = self.devices.get_mut(serial) {
            device.start_explorer_preview_request(entry)
        } else {
            self.log_missing_device_state(serial, "start explorer preview");
            return;
        };

        let (listing_gen, preview_gen, remote) = match preview_request {
            Ok(request) => request,
            Err(ExplorerPreviewSkipReason::ListingInProgress) => {
                self.log_skipped(serial, "preview explorer file", "listing is still loading");
                return;
            }
            Err(ExplorerPreviewSkipReason::DirectorySelected) => {
                self.log_skipped(
                    serial,
                    "preview explorer file",
                    "selected entry is a directory",
                );
                return;
            }
            Err(ExplorerPreviewSkipReason::FileTooLarge(size)) => {
                self.log_skipped(
                    serial,
                    "preview explorer file",
                    &format!("file is too large to preview ({size} bytes)"),
                );
                return;
            }
        };

        let serial = serial.to_string();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = adb::cat_remote_file(&serial, &remote);
            let _ = tx.send(AdbMsg::ExplorerPreview(
                serial,
                listing_gen,
                preview_gen,
                result,
            ));
        });
    }

    fn upload_file_to_explorer(&mut self, serial: &str, target_dir: String) {
        if let Some(file) = rfd::FileDialog::new()
            .set_title("Upload file to device")
            .pick_file()
        {
            let local = file.display().to_string();
            let file_name = file
                .file_name()
                .map_or_else(|| "file".into(), |name| name.to_string_lossy().into_owned());
            let remote = join_remote_path(&target_dir, &file_name);
            let serial = serial.to_string();
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let (ok, msg) = adb::push_remote_file(&serial, &local, &remote);
                let status = if ok { "Upload OK" } else { "Upload FAILED" };
                let _ = tx.send(AdbMsg::DeviceActionResult(
                    serial.clone(),
                    format!("{status}: {msg}"),
                ));
                if ok {
                    let _ = tx.send(AdbMsg::ExplorerReloadIfCurrent(serial, target_dir));
                }
            });
        } else {
            self.log_cancelled(serial, "upload explorer file");
        }
    }

    fn spawn_explorer_listing_request(&self, serial: String, request_gen: u64, path: String) {
        let tx = self.tx.clone();
        std::thread::spawn(move || match adb::list_remote_dir(&serial, &path) {
            Ok(entries) => {
                let _ = tx.send(AdbMsg::ExplorerListing(serial, request_gen, path, entries));
            }
            Err(error) => {
                let _ = tx.send(AdbMsg::ExplorerError(serial, request_gen, error));
            }
        });
    }

    pub(super) fn explorer_navigate(&mut self, serial: &str, target: Option<&str>) {
        let Some(target) = target else {
            self.explorer_navigate_no_history(serial);
            return;
        };

        let serial_owned = serial.to_string();
        let current_path = self
            .devices
            .get(&serial_owned)
            .map(|device| device.explorer_visible_path().to_string());
        let normalized_target = normalize_remote_dir_path(target);
        let Some((request_gen, path)) = self
            .devices
            .get_mut(&serial_owned)
            .and_then(|device| device.start_explorer_navigation(target, true))
        else {
            match current_path {
                Some(current) if current == normalized_target => {
                    self.log(
                        AppLogLevel::Info,
                        format!("[{serial}] Explorer navigate skipped: already at {current}"),
                    );
                }
                Some(_) => {
                    self.log_skipped(serial, "explorer navigate", "target path did not change");
                }
                None => self.log_missing_device_state(serial, "explorer navigate"),
            }
            return;
        };

        self.log_explorer_listing_start(&serial_owned, &path, "navigate");
        self.spawn_explorer_listing_request(serial_owned, request_gen, path);
    }

    fn explorer_navigate_back(&mut self, serial: &str) {
        let serial_owned = serial.to_string();
        let Some((request_gen, path)) = self
            .devices
            .get_mut(&serial_owned)
            .and_then(crate::device::DeviceState::start_explorer_back_navigation)
        else {
            if self.devices.contains_key(&serial_owned) {
                self.log_skipped(serial, "explorer back", "no previous path is available");
            } else {
                self.log_missing_device_state(serial, "explorer back");
            }
            return;
        };

        self.log_explorer_listing_start(&serial_owned, &path, "back");
        self.spawn_explorer_listing_request(serial_owned, request_gen, path);
    }

    pub(super) fn explorer_navigate_no_history(&mut self, serial: &str) {
        let serial_owned = serial.to_string();
        let Some((request_gen, path)) = self
            .devices
            .get_mut(&serial_owned)
            .map(crate::device::DeviceState::start_explorer_refresh)
        else {
            self.log_missing_device_state(serial, "explorer refresh");
            return;
        };

        self.log_explorer_listing_start(&serial_owned, &path, "refresh");
        self.spawn_explorer_listing_request(serial_owned, request_gen, path);
    }
}

fn materialize_shortcut_path(template: &str, bundle_id: &str) -> String {
    template.replace("<package>", bundle_id)
}

fn materialize_command_template(
    template: &str,
    current_display_path: &str,
    selected_path: Option<&str>,
) -> String {
    let file = selected_path.unwrap_or("<file>");
    let dir = selected_path.unwrap_or(current_display_path);

    template
        .replace("<path>", current_display_path)
        .replace("<dir>", dir)
        .replace("<file>", file)
        .replace("<src>", selected_path.unwrap_or("<src>"))
        .replace(
            "<dst>",
            join_remote_path(current_display_path, "copy_target").as_str(),
        )
        .replace("<file1>", selected_path.unwrap_or("<file1>"))
        .replace("<file2>", selected_path.unwrap_or("<file2>"))
        .replace("<target>", selected_path.unwrap_or("<target>"))
        .replace(
            "<link>",
            join_remote_path(current_display_path, "link_name").as_str(),
        )
        .replace("<pattern>", "*.txt")
        .replace("<mode>", "0755")
        .replace("<owner>:<group>", "root:root")
        .replace("<N>", "50")
}

fn strip_matching_quotes(s: &str) -> String {
    let trimmed = s.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        trimmed[1..trimmed.len().saturating_sub(1)].to_string()
    } else {
        trimmed.to_string()
    }
}

fn parse_cd_command(command: &str) -> Option<String> {
    let command = command.trim();
    if command == "cd" {
        Some("/".to_string())
    } else {
        command.strip_prefix("cd ").map(strip_matching_quotes)
    }
}

fn parse_tail_follow_target(command: &str) -> Option<String> {
    command
        .trim()
        .strip_prefix("tail -f ")
        .map(std::string::ToString::to_string)
}

fn handle_explorer_command_history_keys(ui: &egui::Ui, device: &mut crate::device::DeviceState) {
    if ui.input(|input| input.key_pressed(egui::Key::ArrowUp))
        && device.explorer_command_history_pos > 0
    {
        device.explorer_command_history_pos -= 1;
        if let Some(command) = device
            .explorer_command_history
            .get(device.explorer_command_history_pos)
        {
            device.explorer_command_input.clone_from(command);
        }
    }

    if ui.input(|input| input.key_pressed(egui::Key::ArrowDown))
        && device.explorer_command_history_pos < device.explorer_command_history.len()
    {
        device.explorer_command_history_pos += 1;
        device.explorer_command_input = device
            .explorer_command_history
            .get(device.explorer_command_history_pos)
            .cloned()
            .unwrap_or_default();
    }
}

fn format_explorer_command_summary(report: &adb::ExplorerCommandReport) -> String {
    let label = if report.follow_poll {
        "[FOLLOW]"
    } else {
        "[CMD]"
    };
    let status = if report.success {
        "OK"
    } else if report.timed_out {
        "TIMEOUT"
    } else {
        "FAIL"
    };
    let line_count = bytecount_lines(&report.output);
    let byte_count = report.output.len();
    format!(
        "{label} {status} {} ms cwd={} cmd={} response={} lines / {} bytes",
        report.duration_ms, report.cwd, report.command, line_count, byte_count
    )
}

fn format_explorer_response_summary(output: &str) -> String {
    let excerpt = output
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    if excerpt.is_empty() {
        String::new()
    } else {
        truncate_text(excerpt, 180)
    }
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let shortened: String = text.chars().take(max_chars).collect();
        format!("{shortened}...")
    }
}

fn is_timeout_text(text: &str) -> bool {
    text.to_lowercase().contains("timed out")
}

const APP_SHORTCUTS: &[ExplorerShortcut] = &[
    ExplorerShortcut {
        label: "App Private",
        path_template: "/data/data/<package>/",
        description: "App private data",
    },
    ExplorerShortcut {
        label: "Databases",
        path_template: "/data/data/<package>/databases/",
        description: "App SQLite databases",
    },
    ExplorerShortcut {
        label: "Prefs",
        path_template: "/data/data/<package>/shared_prefs/",
        description: "App shared preferences",
    },
    ExplorerShortcut {
        label: "Cache",
        path_template: "/data/data/<package>/cache/",
        description: "App cache",
    },
    ExplorerShortcut {
        label: "Files",
        path_template: "/data/data/<package>/files/",
        description: "App internal files",
    },
    ExplorerShortcut {
        label: "External Data",
        path_template: "/sdcard/Android/data/<package>/",
        description: "App external data",
    },
    ExplorerShortcut {
        label: "OBB",
        path_template: "/sdcard/Android/obb/<package>/",
        description: "App OBB files",
    },
];

const STORAGE_SHORTCUTS: &[ExplorerShortcut] = &[
    ExplorerShortcut {
        label: "Internal Storage",
        path_template: "/sdcard/",
        description: "Internal storage",
    },
    ExplorerShortcut {
        label: "Emulated 0",
        path_template: "/storage/emulated/0/",
        description: "Primary shared storage",
    },
    ExplorerShortcut {
        label: "DCIM",
        path_template: "/sdcard/DCIM/",
        description: "Camera photos",
    },
    ExplorerShortcut {
        label: "Downloads",
        path_template: "/sdcard/Download/",
        description: "Downloads",
    },
    ExplorerShortcut {
        label: "Pictures",
        path_template: "/sdcard/Pictures/",
        description: "Pictures",
    },
    ExplorerShortcut {
        label: "Music",
        path_template: "/sdcard/Music/",
        description: "Music",
    },
    ExplorerShortcut {
        label: "Temp",
        path_template: "/data/local/tmp/",
        description: "Temp directory (writable)",
    },
];

const SYSTEM_SHORTCUTS: &[ExplorerShortcut] = &[
    ExplorerShortcut {
        label: "Installed APKs",
        path_template: "/data/app/",
        description: "Installed APKs",
    },
    ExplorerShortcut {
        label: "Misc",
        path_template: "/data/misc/",
        description: "Miscellaneous data",
    },
    ExplorerShortcut {
        label: "System Data",
        path_template: "/data/system/",
        description: "System data",
    },
    ExplorerShortcut {
        label: "System",
        path_template: "/system/",
        description: "System partition",
    },
    ExplorerShortcut {
        label: "System Apps",
        path_template: "/system/app/",
        description: "Pre-installed system apps",
    },
    ExplorerShortcut {
        label: "Priv Apps",
        path_template: "/system/priv-app/",
        description: "Privileged system apps",
    },
    ExplorerShortcut {
        label: "Framework",
        path_template: "/system/framework/",
        description: "Framework JARs",
    },
    ExplorerShortcut {
        label: "Libraries",
        path_template: "/system/lib64/",
        description: "System libraries (`/system/lib` on 32-bit devices)",
    },
    ExplorerShortcut {
        label: "System Etc",
        path_template: "/system/etc/",
        description: "System config files",
    },
    ExplorerShortcut {
        label: "build.prop",
        path_template: "/system/build.prop",
        description: "Build properties",
    },
    ExplorerShortcut {
        label: "Fonts",
        path_template: "/system/fonts/",
        description: "System fonts",
    },
    ExplorerShortcut {
        label: "Vendor",
        path_template: "/vendor/",
        description: "Vendor partition",
    },
];

const LOW_LEVEL_SHORTCUTS: &[ExplorerShortcut] = &[
    ExplorerShortcut {
        label: "Proc",
        path_template: "/proc/",
        description: "Process information",
    },
    ExplorerShortcut {
        label: "Sys",
        path_template: "/sys/",
        description: "Sysfs",
    },
    ExplorerShortcut {
        label: "Dev",
        path_template: "/dev/",
        description: "Device nodes",
    },
];

const NAVIGATION_COMMANDS: &[ExplorerCommandTemplate] = &[
    ExplorerCommandTemplate {
        label: "ls",
        template: "ls",
        description: "List directory",
    },
    ExplorerCommandTemplate {
        label: "ls -la",
        template: "ls -la",
        description: "List with details",
    },
    ExplorerCommandTemplate {
        label: "ls -R",
        template: "ls -R",
        description: "List recursively",
    },
    ExplorerCommandTemplate {
        label: "cd",
        template: "cd ..",
        description: "Change directory",
    },
    ExplorerCommandTemplate {
        label: "pwd",
        template: "pwd",
        description: "Print working directory",
    },
];

const READ_COMMANDS: &[ExplorerCommandTemplate] = &[
    ExplorerCommandTemplate {
        label: "cat",
        template: "cat <file>",
        description: "Display file contents",
    },
    ExplorerCommandTemplate {
        label: "head",
        template: "head -n <N> <file>",
        description: "Show first N lines",
    },
    ExplorerCommandTemplate {
        label: "tail",
        template: "tail -n <N> <file>",
        description: "Show last N lines",
    },
    ExplorerCommandTemplate {
        label: "tail -f",
        template: "tail -f <file>",
        description: "Follow file changes",
    },
    ExplorerCommandTemplate {
        label: "wc -l",
        template: "wc -l <file>",
        description: "Count lines",
    },
];

const FILE_COMMANDS: &[ExplorerCommandTemplate] = &[
    ExplorerCommandTemplate {
        label: "cp",
        template: "cp <src> <dst>",
        description: "Copy file",
    },
    ExplorerCommandTemplate {
        label: "cp -r",
        template: "cp -r <src> <dst>",
        description: "Copy directory recursively",
    },
    ExplorerCommandTemplate {
        label: "mv",
        template: "mv <src> <dst>",
        description: "Move or rename file",
    },
    ExplorerCommandTemplate {
        label: "rm",
        template: "rm <file>",
        description: "Remove file",
    },
    ExplorerCommandTemplate {
        label: "rm -rf",
        template: "rm -rf <dir>",
        description: "Remove directory recursively",
    },
    ExplorerCommandTemplate {
        label: "mkdir",
        template: "mkdir <dir>",
        description: "Create directory",
    },
    ExplorerCommandTemplate {
        label: "mkdir -p",
        template: "mkdir -p <path>",
        description: "Create nested directories",
    },
    ExplorerCommandTemplate {
        label: "rmdir",
        template: "rmdir <dir>",
        description: "Remove empty directory",
    },
    ExplorerCommandTemplate {
        label: "touch",
        template: "touch <file>",
        description: "Create or update timestamp",
    },
    ExplorerCommandTemplate {
        label: "ln -s",
        template: "ln -s <target> <link>",
        description: "Create symbolic link",
    },
];

const SEARCH_COMMANDS: &[ExplorerCommandTemplate] = &[
    ExplorerCommandTemplate {
        label: "find",
        template: "find <path> -name <pattern>",
        description: "Find files",
    },
    ExplorerCommandTemplate {
        label: "grep",
        template: "grep <pattern> <file>",
        description: "Search in files",
    },
    ExplorerCommandTemplate {
        label: "grep -r",
        template: "grep -r <pattern> <path>",
        description: "Search recursively",
    },
    ExplorerCommandTemplate {
        label: "diff",
        template: "diff <file1> <file2>",
        description: "Compare files",
    },
];

const META_COMMANDS: &[ExplorerCommandTemplate] = &[
    ExplorerCommandTemplate {
        label: "stat",
        template: "stat <file>",
        description: "File status",
    },
    ExplorerCommandTemplate {
        label: "file",
        template: "file <file>",
        description: "Determine file type",
    },
    ExplorerCommandTemplate {
        label: "md5sum",
        template: "md5sum <file>",
        description: "MD5 checksum",
    },
    ExplorerCommandTemplate {
        label: "sha1sum",
        template: "sha1sum <file>",
        description: "SHA1 checksum",
    },
    ExplorerCommandTemplate {
        label: "sha256sum",
        template: "sha256sum <file>",
        description: "SHA256 checksum",
    },
    ExplorerCommandTemplate {
        label: "chmod",
        template: "chmod <mode> <file>",
        description: "Change permissions",
    },
    ExplorerCommandTemplate {
        label: "chown",
        template: "chown <owner>:<group> <file>",
        description: "Change ownership",
    },
];

const SYSTEM_COMMANDS: &[ExplorerCommandTemplate] = &[
    ExplorerCommandTemplate {
        label: "mount",
        template: "mount",
        description: "Show mounts",
    },
    ExplorerCommandTemplate {
        label: "remount rw",
        template: "mount -o remount,rw /system",
        description: "Remount system as read-write",
    },
    ExplorerCommandTemplate {
        label: "umount",
        template: "umount <path>",
        description: "Unmount path",
    },
];

const fn command_template_groups() -> [(&'static str, &'static [ExplorerCommandTemplate]); 6] {
    [
        ("Navigation", NAVIGATION_COMMANDS),
        ("Read", READ_COMMANDS),
        ("File Ops", FILE_COMMANDS),
        ("Search", SEARCH_COMMANDS),
        ("Metadata", META_COMMANDS),
        ("System", SYSTEM_COMMANDS),
    ]
}
