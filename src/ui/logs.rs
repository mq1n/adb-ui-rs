use eframe::egui;

use super::helpers::logcat_line_color;
use super::AppLogLevel;
use crate::adb::{self, AdbMsg};
use crate::device::{self, LogSource, LEVEL_NAMES, LOG_PAGE_SIZE};

impl super::App {
    pub(super) fn draw_logs_tab(&mut self, ui: &mut egui::Ui, serial: &str) {
        let serial_owned = serial.to_string();

        // Split: sidebar (left) | content (right).
        let available_rect = ui.available_rect_before_wrap();
        let sidebar_w = 120.0;
        let sep = 4.0;

        let sidebar_rect = egui::Rect::from_min_size(
            available_rect.min,
            egui::vec2(sidebar_w, available_rect.height()),
        );
        let content_rect = egui::Rect::from_min_size(
            egui::pos2(available_rect.min.x + sidebar_w + sep, available_rect.min.y),
            egui::vec2(
                available_rect.width() - sidebar_w - sep,
                available_rect.height(),
            ),
        );
        ui.allocate_rect(available_rect, egui::Sense::hover());

        // Separator line.
        let sep_x = available_rect.min.x + sidebar_w + sep * 0.5;
        ui.painter().line_segment(
            [
                egui::pos2(sep_x, available_rect.min.y),
                egui::pos2(sep_x, available_rect.max.y),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 60, 60)),
        );

        // --- Sidebar ---
        let mut sidebar_ui = ui.new_child(egui::UiBuilder::new().max_rect(sidebar_rect));
        sidebar_ui.set_clip_rect(sidebar_rect);

        let active_source = self
            .devices
            .get(serial)
            .map_or(LogSource::Logcat, |ds| ds.active_log_source);

        for &source in LogSource::ALL {
            let is_selected = active_source == source;
            let label = source.label();

            // Status indicator.
            let (indicator, ind_color) = if source == LogSource::Logcat {
                let running = self
                    .devices
                    .get(serial)
                    .is_some_and(|ds| ds.logcat_ui.running);
                if running {
                    ("*", egui::Color32::from_rgb(100, 220, 100))
                } else {
                    (" ", egui::Color32::from_rgb(120, 120, 120))
                }
            } else {
                let has_data = self
                    .devices
                    .get(serial)
                    .is_some_and(|ds| ds.log_buffers.contains_key(&source));
                let loading = self
                    .devices
                    .get(serial)
                    .is_some_and(|ds| ds.log_loading.contains(&source));
                let watching = self
                    .devices
                    .get(serial)
                    .is_some_and(|ds| ds.log_watchers.contains_key(&source));
                if watching {
                    ("*", egui::Color32::from_rgb(100, 220, 100))
                } else if loading {
                    ("~", egui::Color32::from_rgb(255, 200, 50))
                } else if has_data {
                    ("*", egui::Color32::from_rgb(100, 180, 255))
                } else {
                    (" ", egui::Color32::from_rgb(120, 120, 120))
                }
            };

            let text = format!("{indicator} {label}");
            let color = if is_selected {
                egui::Color32::from_rgb(255, 255, 255)
            } else {
                ind_color
            };

            let resp = sidebar_ui.selectable_label(
                is_selected,
                egui::RichText::new(text).color(color).size(12.0),
            );
            if resp.clicked() {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.active_log_source = source;
                    ds.log_page = 0; // reset to newest page
                }
                // Auto-fetch snapshot sources if no data yet.
                if !source.is_live() {
                    let has = self
                        .devices
                        .get(serial)
                        .is_some_and(|ds| ds.log_buffers.contains_key(&source));
                    if !has {
                        self.fetch_log_source(serial, source);
                    }
                }
            }
        }

        // --- Content area ---
        let mut content_ui = ui.new_child(egui::UiBuilder::new().max_rect(content_rect));
        content_ui.set_clip_rect(content_rect);

        if active_source.is_live() {
            self.draw_logcat_content(&mut content_ui, serial);
        } else {
            self.draw_snapshot_log_content(&mut content_ui, serial, active_source);
        }
    }

    pub(super) fn fetch_log_source(&mut self, serial: &str, source: LogSource) {
        let mut already_loading = false;
        if let Some(ds) = self.devices.get_mut(serial) {
            if ds.log_loading.contains(&source) {
                already_loading = true;
            } else {
                ds.log_loading.insert(source);
            }
        } else {
            self.log_missing_device_state(serial, "fetch log snapshot");
            return;
        }
        if already_loading {
            self.log(
                AppLogLevel::Info,
                format!("[{serial}] {} fetch already in progress", source.label()),
            );
            return;
        }
        let serial = serial.to_string();
        let tx = self.tx.clone();
        let idx = source.index();
        std::thread::spawn(move || {
            let lines = adb::fetch_log_snapshot(&serial, idx);
            let _ = tx.send(AdbMsg::LogBuffer(serial, idx, lines));
        });
    }

    pub(super) fn start_log_watcher(
        &mut self,
        serial: &str,
        source: LogSource,
        interval: std::time::Duration,
    ) {
        if !self.devices.contains_key(serial) {
            self.log_missing_device_state(serial, "start log watcher");
            return;
        }
        let idx = source.index();
        let stop = adb::spawn_log_watcher(serial, idx, self.tx.clone(), interval);
        if let Some(ds) = self.devices.get_mut(serial) {
            ds.log_watchers.insert(source, stop);
        }
        self.log(
            AppLogLevel::Info,
            format!(
                "[{serial}] {} watcher started ({}s interval)",
                source.label(),
                interval.as_secs()
            ),
        );
    }

    /// Draw the live logcat content area (toolbar + scrolling lines).
    pub(super) fn draw_logcat_content(&mut self, ui: &mut egui::Ui, serial: &str) {
        let serial_owned = serial.to_string();

        // Toolbar.
        ui.horizontal(|ui| {
            let is_running = self
                .devices
                .get(serial)
                .is_some_and(|ds| ds.logcat_ui.running);

            if is_running {
                if ui.button("Stop").clicked() {
                    if let Some(mut child) = self.logcat_procs.remove(serial) {
                        if let Err(error) = child.kill() {
                            self.log(
                                AppLogLevel::Warn,
                                format!("[{serial}] Failed to stop logcat: {error}"),
                            );
                        }
                    }
                    if let Some(ds) = self.devices.get_mut(&serial_owned) {
                        ds.logcat_ui.running = false;
                        ds.logcat_status = "Stopped by user".into();
                    }
                }
            } else if ui.button("Start").clicked() {
                if let Some(session) = self.devices.get_mut(&serial_owned).map(|ds| {
                    ds.logcat_status = "Starting...".into();
                    ds.start_next_logcat_session()
                }) {
                    if let Some(child) =
                        adb::spawn_logcat(serial, session, self.tx.clone(), &self.config)
                    {
                        self.logcat_procs.insert(serial_owned.clone(), child);
                        if let Some(ds) = self.devices.get_mut(&serial_owned) {
                            ds.logcat_ui.running = true;
                            ds.logcat_status = "Running".into();
                        }
                    } else {
                        if let Some(ds) = self.devices.get_mut(&serial_owned) {
                            ds.logcat_status = "Failed to start".into();
                        }
                        self.log(
                            AppLogLevel::Error,
                            format!("[{serial}] Logcat start failed"),
                        );
                    }
                } else {
                    self.log_missing_device_state(serial, "start logcat");
                }
            }

            if ui.button("Clear").clicked() {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.logcat_lines.clear();
                }
            }

            ui.separator();

            if let Some(ds) = self.devices.get_mut(&serial_owned) {
                ui.label("Level:");
                egui::ComboBox::from_id_salt(format!("level_{serial}"))
                    .selected_text(LEVEL_NAMES[ds.level_filter])
                    .show_ui(ui, |ui| {
                        for (i, name) in LEVEL_NAMES.iter().enumerate() {
                            ui.selectable_value(&mut ds.level_filter, i, *name);
                        }
                    });
            }

            ui.separator();

            if let Some(ds) = self.devices.get_mut(&serial_owned) {
                ui.label("Filter:");
                ui.text_edit_singleline(&mut ds.logcat_filter)
                    .on_hover_text("Case-insensitive substring filter");
            }

            ui.separator();

            if let Some(ds) = self.devices.get_mut(&serial_owned) {
                ui.label("Tag:");
                ui.add_sized(
                    [100.0, 20.0],
                    egui::TextEdit::singleline(&mut ds.logcat_tag_filter)
                        .hint_text("ActivityManager"),
                )
                .on_hover_text("Case-insensitive tag filter");
            }

            ui.separator();

            if let Some(ds) = self.devices.get_mut(&serial_owned) {
                ui.label("PID:");
                ui.add_sized(
                    [70.0, 20.0],
                    egui::TextEdit::singleline(&mut ds.logcat_pid_filter).hint_text("1234"),
                )
                .on_hover_text("Show only lines from this process ID");
            }

            ui.separator();

            if let Some(ds) = self.devices.get_mut(&serial_owned) {
                ui.checkbox(&mut ds.logcat_ui.auto_scroll, "Auto-scroll");
            }

            if let Some(ds) = self.devices.get(serial) {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let status_color = if ds.logcat_ui.running {
                        egui::Color32::from_rgb(100, 200, 100)
                    } else {
                        egui::Color32::from_rgb(180, 180, 180)
                    };
                    ui.colored_label(status_color, &ds.logcat_status);
                    ui.label(format!("{} lines", ds.logcat_lines.len()));
                });
            }
        });

        ui.separator();

        // Gather data needed for paging + rendering.
        let meta = self.devices.get(serial).map(|ds| {
            let total = ds.logcat_lines.len();
            (
                ds.logcat_filter.to_lowercase(),
                ds.logcat_tag_filter.to_lowercase(),
                ds.logcat_pid_filter.trim().to_string(),
                ds.level_filter,
                total,
                ds.logcat_ui.auto_scroll,
            )
        });
        let Some((filter_lower, tag_filter, pid_filter, level, total, auto_scroll)) = meta else {
            return;
        };

        if total >= LOG_PAGE_SIZE {
            self.draw_log_paging_bar(ui, serial, total);
        }

        let (page_start, page_end) = self.resolve_page_range(serial, total);

        if let Some(ds) = self.devices.get(serial) {
            let len = ds.logcat_lines.len();
            let page_lines = &ds.logcat_lines[page_start.min(len)..page_end.min(len)];
            egui::ScrollArea::vertical()
                .stick_to_bottom(auto_scroll)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                    for line in page_lines {
                        if !device::line_passes_level(line, level) {
                            continue;
                        }
                        if !device::line_passes_tag(line, &tag_filter) {
                            continue;
                        }
                        if !device::line_passes_pid(line, &pid_filter) {
                            continue;
                        }
                        if !filter_lower.is_empty() && !line.to_lowercase().contains(&filter_lower)
                        {
                            continue;
                        }
                        let color = logcat_line_color(line);
                        ui.label(egui::RichText::new(line).color(color));
                    }
                });
        }
    }

    /// Draw a snapshot (non-live) log content area.
    pub(super) fn draw_snapshot_log_content(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        source: LogSource,
    ) {
        let serial_owned = serial.to_string();

        // Toolbar.
        ui.horizontal(|ui| {
            let is_loading = self
                .devices
                .get(serial)
                .is_some_and(|ds| ds.log_loading.contains(&source));

            let is_watching = self
                .devices
                .get(serial)
                .is_some_and(|ds| ds.log_watchers.contains_key(&source));

            if is_loading {
                ui.spinner();
                ui.label("Fetching...");
            } else if !is_watching && ui.button("Refresh").clicked() {
                self.fetch_log_source(serial, source);
            }

            ui.separator();

            // Watch toggle.
            if is_watching {
                if ui.button("Stop Watch").clicked() {
                    if let Some(ds) = self.devices.get_mut(&serial_owned) {
                        ds.stop_log_watcher(source);
                    }
                    self.log(
                        AppLogLevel::Info,
                        format!("[{serial}] {} watcher stopped", source.label()),
                    );
                }
                ui.spinner();
            } else if ui
                .button("Watch (3s)")
                .on_hover_text("Auto-refresh every 3 seconds")
                .clicked()
            {
                self.start_log_watcher(serial, source, std::time::Duration::from_secs(3));
            }

            if ui.button("Clear").clicked() {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.log_buffers.remove(&source);
                }
            }

            ui.separator();

            if let Some(ds) = self.devices.get_mut(&serial_owned) {
                ui.label("Filter:");
                ui.text_edit_singleline(&mut ds.logcat_filter)
                    .on_hover_text("Case-insensitive substring filter");
            }

            ui.separator();

            if let Some(ds) = self.devices.get_mut(&serial_owned) {
                ui.label("Tag:");
                ui.add_sized(
                    [100.0, 20.0],
                    egui::TextEdit::singleline(&mut ds.logcat_tag_filter)
                        .hint_text("ActivityManager"),
                )
                .on_hover_text("Case-insensitive tag filter");
            }

            ui.separator();

            if let Some(ds) = self.devices.get_mut(&serial_owned) {
                ui.label("PID:");
                ui.add_sized(
                    [70.0, 20.0],
                    egui::TextEdit::singleline(&mut ds.logcat_pid_filter).hint_text("1234"),
                )
                .on_hover_text("Show only lines from this process ID");
            }

            ui.separator();

            if let Some(ds) = self.devices.get_mut(&serial_owned) {
                ui.checkbox(&mut ds.logcat_ui.auto_scroll, "Auto-scroll");
            }

            if let Some(ds) = self.devices.get(serial) {
                let count = ds.log_buffers.get(&source).map_or(0, std::vec::Vec::len);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if is_watching {
                        ui.colored_label(egui::Color32::from_rgb(100, 200, 100), "Watching");
                        ui.separator();
                    }
                    ui.label(format!("{count} lines"));
                    ui.colored_label(egui::Color32::from_rgb(140, 140, 140), source.label());
                });
            }
        });

        ui.separator();

        // Read total + filter + auto_scroll, then drop borrow so paging bar can borrow &mut self.
        let snap_meta = self.devices.get(serial).and_then(|ds| {
            ds.log_buffers.get(&source).map(|lines| {
                (
                    lines.len(),
                    ds.logcat_filter.to_lowercase(),
                    ds.logcat_tag_filter.to_lowercase(),
                    ds.logcat_pid_filter.trim().to_string(),
                )
            })
        });

        let Some((total, filter_lower, tag_filter, pid_filter)) = snap_meta else {
            ui.centered_and_justified(|ui| {
                ui.label("Click a log source to fetch, or press Refresh.");
            });
            return;
        };

        if total >= LOG_PAGE_SIZE {
            self.draw_log_paging_bar(ui, serial, total);
        }

        let (page_start, page_end) = self.resolve_page_range(serial, total);

        if let Some(ds) = self.devices.get(serial) {
            if let Some(lines) = ds.log_buffers.get(&source) {
                let len = lines.len();
                let page_lines = &lines[page_start.min(len)..page_end.min(len)];
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                        for line in page_lines.iter().rev() {
                            if !device::line_passes_tag(line, &tag_filter) {
                                continue;
                            }
                            if !device::line_passes_pid(line, &pid_filter) {
                                continue;
                            }
                            if !filter_lower.is_empty()
                                && !line.to_lowercase().contains(&filter_lower)
                            {
                                continue;
                            }
                            let color = logcat_line_color(line);
                            ui.label(egui::RichText::new(line).color(color));
                        }
                    });
            }
        }
    }

    /// Draw the paging navigation bar.
    pub(super) fn draw_log_paging_bar(&mut self, ui: &mut egui::Ui, serial: &str, total: usize) {
        let serial_owned = serial.to_string();
        let total_pages = total.div_ceil(LOG_PAGE_SIZE);

        let cur_page = self.devices.get(serial).map_or(0, |ds| {
            if ds.log_page >= total_pages {
                total_pages.saturating_sub(1)
            } else {
                ds.log_page
            }
        });

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;

            if ui
                .add_enabled(cur_page > 0, egui::Button::new("<<").small())
                .clicked()
            {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.log_page = 0;
                }
            }
            if ui
                .add_enabled(cur_page > 0, egui::Button::new("<").small())
                .clicked()
            {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.log_page = cur_page.saturating_sub(1);
                }
            }

            let window_start = cur_page.saturating_sub(3);
            let window_end = (window_start + 7).min(total_pages);
            for p in window_start..window_end {
                let label = format!("{}", p + 1);
                if ui.selectable_label(p == cur_page, &label).clicked() {
                    if let Some(ds) = self.devices.get_mut(&serial_owned) {
                        ds.log_page = p;
                    }
                }
            }

            if ui
                .add_enabled(cur_page + 1 < total_pages, egui::Button::new(">").small())
                .clicked()
            {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.log_page = cur_page + 1;
                }
            }
            if ui
                .add_enabled(cur_page + 1 < total_pages, egui::Button::new(">>").small())
                .clicked()
            {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.log_page = total_pages.saturating_sub(1);
                }
            }

            ui.separator();

            let dir = if cur_page == 0 {
                " (Newest)"
            } else if cur_page + 1 == total_pages {
                " (Oldest)"
            } else {
                ""
            };
            ui.colored_label(
                egui::Color32::from_rgb(160, 160, 160),
                format!("Page {}/{}{dir} ({total} lines)", cur_page + 1, total_pages),
            );
        });
    }

    /// Resolve the current page to a (start, end) range in the line buffer.
    /// Resolve page to a (start, end) range. Page 0 = newest lines (end of buffer).
    pub(super) fn resolve_page_range(&self, serial: &str, total: usize) -> (usize, usize) {
        if total == 0 {
            return (0, 0);
        }
        let total_pages = total.div_ceil(LOG_PAGE_SIZE);
        let page = self
            .devices
            .get(serial)
            .map_or(0, |ds| ds.log_page.min(total_pages.saturating_sub(1)));
        // Reverse mapping: page 0 = newest (end of buffer), last page = oldest (start).
        let reversed = total_pages.saturating_sub(1) - page;
        let start = reversed * LOG_PAGE_SIZE;
        let end = (start + LOG_PAGE_SIZE).min(total);
        (start, end)
    }
}
