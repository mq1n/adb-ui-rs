use eframe::egui;
use egui_extras::{Column, TableBuilder};

use super::helpers::{export_single_file, file_log_line_color, format_size};
use super::{AppLogLevel, FILE_WATCH_INTERVAL};
use crate::adb;
use crate::device::FileSortBy;

impl super::App {
    pub(super) fn draw_file_logs_tab(&mut self, ui: &mut egui::Ui, serial: &str) {
        let serial_owned = serial.to_string();
        let bundle_id = self.config.bundle_id.clone();

        // Toolbar.
        ui.horizontal(|ui| {
            let is_pulling = self
                .devices
                .get(serial)
                .is_some_and(|ds| ds.file_activity.pulling);
            let is_watching = self
                .devices
                .get(serial)
                .is_some_and(|ds| ds.file_activity.watching);

            if is_pulling {
                ui.spinner();
                ui.label("Pulling...");
            } else if !is_watching && ui.button("Pull Once").clicked() {
                if bundle_id.trim().is_empty() {
                    self.log_skipped(serial, "pull file logs", "bundle ID is not configured");
                } else {
                    if let Some(ds) = self.devices.get_mut(&serial_owned) {
                        ds.file_logs.clear();
                        ds.sorted_keys.clear();
                        ds.selected_file = None;
                        ds.file_activity.pulling = true;
                        ds.file_status = "Pulling...".into();
                    }
                    adb::pull_file_logs(serial, self.tx.clone(), bundle_id.clone());
                }
            }

            ui.separator();

            if is_watching {
                if ui.button("Stop Watching").clicked() {
                    if let Some(ds) = self.devices.get_mut(&serial_owned) {
                        ds.stop_watcher();
                        ds.file_status = "Watcher stopped".into();
                    }
                }
                ui.spinner();
            } else if ui.button("Watch (live)").clicked() {
                if bundle_id.trim().is_empty() {
                    self.log_skipped(serial, "watch file logs", "bundle ID is not configured");
                } else if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    let session = ds.start_next_file_watch_session();
                    ds.file_logs.clear();
                    ds.sorted_keys.clear();
                    ds.selected_file = None;
                    ds.file_activity.watching = true;
                    ds.file_status = "Starting watcher...".into();
                    let stop = adb::spawn_file_watcher(
                        serial,
                        session,
                        self.tx.clone(),
                        FILE_WATCH_INTERVAL,
                        bundle_id.clone(),
                    );
                    ds.watcher_stop = Some(stop);
                } else {
                    self.log_missing_device_state(serial, "watch file logs");
                }
            }

            ui.separator();

            if ui.button("Export All").clicked() {
                self.export_all_files(serial);
            }

            if let Some(ds) = self.devices.get(serial) {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let color = if ds.file_activity.watching {
                        egui::Color32::from_rgb(100, 200, 100)
                    } else {
                        egui::Color32::from_rgb(180, 180, 180)
                    };
                    ui.colored_label(color, &ds.file_status);
                    ui.label(format!("{} file(s)", ds.file_logs.len()));
                });
            }
        });

        ui.separator();

        // Split layout using rects for full-height panels.
        let available_rect = ui.available_rect_before_wrap();
        let panel_width = (available_rect.width() * 0.35).clamp(300.0, 550.0);
        let sep = 4.0;

        let left_rect = egui::Rect::from_min_size(
            available_rect.min,
            egui::vec2(panel_width, available_rect.height()),
        );
        let right_rect = egui::Rect::from_min_size(
            egui::pos2(
                available_rect.min.x + panel_width + sep,
                available_rect.min.y,
            ),
            egui::vec2(
                available_rect.width() - panel_width - sep,
                available_rect.height(),
            ),
        );

        ui.allocate_rect(available_rect, egui::Sense::hover());

        // Separator line.
        let sep_x = available_rect.min.x + panel_width + sep * 0.5;
        ui.painter().line_segment(
            [
                egui::pos2(sep_x, available_rect.min.y),
                egui::pos2(sep_x, available_rect.max.y),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 60, 60)),
        );

        let mut left_ui = ui.new_child(egui::UiBuilder::new().max_rect(left_rect));
        left_ui.set_clip_rect(left_rect);
        self.draw_file_table(&mut left_ui, serial);

        let mut right_ui = ui.new_child(egui::UiBuilder::new().max_rect(right_rect));
        right_ui.set_clip_rect(right_rect);
        self.draw_file_content(&mut right_ui, serial);
    }

    pub(super) fn draw_file_table(&mut self, ui: &mut egui::Ui, serial: &str) {
        let serial_owned = serial.to_string();
        let sorted_keys = self
            .devices
            .get(serial)
            .map(|ds| ds.sorted_keys.clone())
            .unwrap_or_default();
        let selected_key = self
            .devices
            .get(serial)
            .and_then(|ds| ds.selected_file.clone());

        if sorted_keys.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label("No files yet.");
            });
            return;
        }

        self.draw_file_table_contents(
            ui,
            serial,
            &serial_owned,
            &sorted_keys,
            selected_key.as_ref(),
        );
    }

    fn draw_file_table_contents(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        sorted_keys: &[String],
        selected_key: Option<&String>,
    ) {
        let sort = self
            .devices
            .get(serial)
            .map_or((FileSortBy::Modified, false), |ds| {
                (ds.file_sort.by, ds.file_sort.ascending)
            });
        let row_height = 18.0;
        let table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(30.0).resizable(true).clip(true))
            .column(Column::remainder().resizable(true).clip(true))
            .column(Column::initial(65.0).resizable(true).clip(true))
            .column(Column::initial(110.0).resizable(true).clip(true))
            .sense(egui::Sense::click());

        table
            .header(row_height, |mut header| {
                self.draw_file_table_header(&mut header, serial, sort.0, sort.1);
            })
            .body(|body| {
                body.rows(row_height, sorted_keys.len(), |mut row| {
                    self.draw_file_table_row(
                        &mut row,
                        serial,
                        serial_owned,
                        sorted_keys,
                        selected_key,
                    );
                });
            });
    }

    fn draw_file_table_header(
        &mut self,
        header: &mut egui_extras::TableRow<'_, '_>,
        serial: &str,
        sort_by: FileSortBy,
        sort_asc: bool,
    ) {
        for (label, column) in [
            ("Src", FileSortBy::Source),
            ("Name", FileSortBy::Name),
            ("Size", FileSortBy::Size),
            ("Modified", FileSortBy::Modified),
        ] {
            header.col(|ui| {
                if ui
                    .button(format!(
                        "{label}{}",
                        sort_indicator(sort_by, sort_asc, column)
                    ))
                    .clicked()
                {
                    self.toggle_sort(serial, column);
                }
            });
        }
    }

    fn draw_file_table_row(
        &mut self,
        row: &mut egui_extras::TableRow<'_, '_>,
        serial: &str,
        serial_owned: &str,
        sorted_keys: &[String],
        selected_key: Option<&String>,
    ) {
        let idx = row.index();
        let key = &sorted_keys[idx];
        row.set_selected(selected_key == Some(key));

        let entry = self
            .devices
            .get(serial)
            .and_then(|ds| ds.file_logs.get(key).cloned());
        let Some(entry) = entry else {
            return;
        };

        let (src_tag, src_color) = file_source_badge(&entry.source);
        row.col(|ui| {
            ui.style_mut().interaction.selectable_labels = false;
            ui.label(egui::RichText::new(src_tag).color(src_color).small());
        });
        row.col(|ui| {
            ui.style_mut().interaction.selectable_labels = false;
            ui.label(
                egui::RichText::new(&entry.name)
                    .monospace()
                    .color(egui::Color32::from_rgb(220, 220, 220)),
            );
        });
        row.col(|ui| {
            ui.style_mut().interaction.selectable_labels = false;
            ui.label(
                egui::RichText::new(format_size(entry.size))
                    .color(egui::Color32::from_rgb(170, 170, 170)),
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
        if response.clicked() {
            if let Some(ds) = self.devices.get_mut(serial_owned) {
                ds.selected_file = Some(key.clone());
            }
        }

        response.context_menu(|ui| {
            self.draw_file_row_context_menu(ui, serial, &entry);
        });
    }

    fn draw_file_row_context_menu(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        entry: &crate::adb::FileEntry,
    ) {
        if ui.button("Copy filename").clicked() {
            ui.ctx().copy_text(entry.name.clone());
            ui.close();
        }
        if ui.button("Copy content").clicked() {
            ui.ctx().copy_text(entry.content.clone());
            ui.close();
        }
        ui.separator();
        if ui.button("Export to file...").clicked() {
            match export_single_file(&entry.name, &entry.content) {
                Ok(()) => {
                    self.log(
                        AppLogLevel::Info,
                        format!("[{serial}] Exported {}", entry.name),
                    );
                }
                Err(error) if error == "Export cancelled" => {
                    self.log_cancelled(serial, &format!("export {}", entry.name));
                }
                Err(error) => {
                    self.log(
                        AppLogLevel::Error,
                        format!("[{serial}] Export failed: {error}"),
                    );
                }
            }
            ui.close();
        }
    }
    pub(super) fn toggle_sort(&mut self, serial: &str, col: FileSortBy) {
        if let Some(ds) = self.devices.get_mut(serial) {
            if ds.file_sort.by == col {
                ds.file_sort.ascending = !ds.file_sort.ascending;
            } else {
                ds.file_sort.by = col;
                ds.file_sort.ascending = col == FileSortBy::Name; // name asc, others desc by default
            }
            ds.rebuild_sorted_keys();
        }
    }

    pub(super) fn draw_file_content(&mut self, ui: &mut egui::Ui, serial: &str) {
        let serial_owned = serial.to_string();
        let selected_key = self
            .devices
            .get(serial)
            .and_then(|ds| ds.selected_file.clone());
        let mut file_export_result: Option<(String, Result<(), String>)> = None;

        let Some(key) = selected_key else {
            ui.centered_and_justified(|ui| {
                ui.label("Select a file from the list.");
            });
            return;
        };

        // Header.
        if let Some(ds) = self.devices.get_mut(&serial_owned) {
            if let Some(entry) = ds.file_logs.get(&key).cloned() {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&entry.name).strong().monospace());
                    ui.colored_label(
                        egui::Color32::from_rgb(140, 140, 140),
                        format!(
                            "[{}] {} | {}",
                            entry.source,
                            format_size(entry.size),
                            entry.modified,
                        ),
                    );
                    ui.separator();
                    if ui.button("Export").clicked() {
                        file_export_result = Some((
                            entry.name.clone(),
                            export_single_file(&entry.name, &entry.content),
                        ));
                    }
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(entry.content.clone());
                    }
                    ui.separator();
                    ui.label("Search:");
                    ui.text_edit_singleline(&mut ds.file_content_filter);
                });
            }
        }
        if let Some((name, result)) = file_export_result {
            match result {
                Ok(()) => {
                    self.log(AppLogLevel::Info, format!("[{serial}] Exported {name}"));
                }
                Err(error) if error == "Export cancelled" => {
                    self.log_cancelled(serial, &format!("export {name}"));
                }
                Err(error) => {
                    self.log(
                        AppLogLevel::Error,
                        format!("[{serial}] Export failed: {error}"),
                    );
                }
            }
        }
        ui.separator();

        // Content.
        if let Some(ds) = self.devices.get(serial) {
            if let Some(entry) = ds.file_logs.get(&key) {
                let filter_lower = ds.file_content_filter.to_lowercase();

                egui::ScrollArea::both()
                    .id_salt(format!("filecontent_{serial}"))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                        for line in entry.content.lines() {
                            if !filter_lower.is_empty()
                                && !line.to_lowercase().contains(&filter_lower)
                            {
                                continue;
                            }
                            let color = file_log_line_color(line);
                            ui.label(egui::RichText::new(line).color(color));
                        }
                    });
            }
        }
    }

    pub(super) fn export_all_files(&mut self, serial: &str) {
        let Some(entries) = self.devices.get(serial).map(|ds| {
            ds.file_logs
                .values()
                .cloned()
                .collect::<Vec<crate::adb::FileEntry>>()
        }) else {
            self.log_missing_device_state(serial, "export all file logs");
            return;
        };
        if entries.is_empty() {
            self.log_skipped(serial, "export all file logs", "no files are loaded");
            return;
        }

        let Some(dir) = rfd::FileDialog::new()
            .set_title("Select export directory")
            .pick_folder()
        else {
            self.log_cancelled(serial, "export all file logs");
            return;
        };

        let total = entries.len();
        let mut count = 0usize;
        let mut errors = Vec::new();
        for entry in &entries {
            let safe_name = entry.name.replace(['/', '\\', ':'], "_");
            let prefix = if entry.source == "internal" {
                "int_"
            } else {
                "ext_"
            };
            let path = dir.join(format!("{prefix}{safe_name}"));
            match std::fs::write(&path, &entry.content) {
                Ok(()) => count += 1,
                Err(e) => errors.push(format!("{}: {e}", path.display())),
            }
        }

        if errors.is_empty() {
            self.log(
                AppLogLevel::Info,
                format!("Exported {count}/{total} files to {}", dir.display()),
            );
        } else {
            self.log(
                AppLogLevel::Error,
                format!(
                    "Exported {count}/{total} files to {} ({} failed: {})",
                    dir.display(),
                    errors.len(),
                    errors.join(", ")
                ),
            );
        }

        // Open the directory.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("explorer.exe")
                .arg(dir.to_string_lossy().replace('/', "\\"))
                .creation_flags(0x0800_0000)
                .spawn();
        }
    }
}

fn sort_indicator(active: FileSortBy, ascending: bool, column: FileSortBy) -> &'static str {
    if active != column {
        ""
    } else if ascending {
        " \u{25B2}"
    } else {
        " \u{25BC}"
    }
}

fn file_source_badge(source: &str) -> (&'static str, egui::Color32) {
    if source == "internal" {
        ("INT", egui::Color32::from_rgb(100, 180, 255))
    } else {
        ("EXT", egui::Color32::from_rgb(180, 220, 100))
    }
}
