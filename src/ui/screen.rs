use eframe::egui;

use super::helpers::{copy_png_as_file, copy_png_to_clipboard, get_screenshot_temp_path};
use super::{now_str, AppLogLevel};
use crate::adb::{self, AdbMsg};

impl super::App {
    pub(super) fn draw_screen_tab(&mut self, ui: &mut egui::Ui, serial: &str) {
        let serial_owned = serial.to_string();
        let now = ui.input(|i| i.time);
        let mut screen_errors = Vec::new();

        self.maybe_auto_capture_screen(&serial_owned, now);
        self.draw_screen_toolbar(ui, serial, &serial_owned, &mut screen_errors);
        ui.separator();

        let available_rect = ui.available_rect_before_wrap();
        let history_height = 80.0;
        let image_height = available_rect.height() - history_height - 4.0;
        let image_rect = egui::Rect::from_min_size(
            available_rect.min,
            egui::vec2(available_rect.width(), image_height.max(100.0)),
        );
        let history_rect = egui::Rect::from_min_size(
            egui::pos2(
                available_rect.min.x,
                available_rect.min.y + image_height + 4.0,
            ),
            egui::vec2(available_rect.width(), history_height),
        );
        ui.allocate_rect(available_rect, egui::Sense::hover());

        self.draw_screen_image_panel(ui, serial, &serial_owned, image_rect, &mut screen_errors);
        self.draw_screen_history_panel(ui, serial, &serial_owned, history_rect, &mut screen_errors);
        self.flush_screen_errors(serial, screen_errors);
    }

    fn maybe_auto_capture_screen(&mut self, serial_owned: &str, now: f64) {
        if let Some(ds) = self.devices.get_mut(serial_owned) {
            if let Some(interval) = ds.screen_auto_interval {
                if !ds.screen.capturing && now - ds.screen_last_auto > interval {
                    ds.screen_last_auto = now;
                    ds.screen.capturing = true;
                    self.spawn_screenshot_capture(serial_owned.to_string());
                }
            }
        }
    }

    fn draw_screen_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        screen_errors: &mut Vec<String>,
    ) {
        ui.horizontal(|ui| {
            let is_capturing = self
                .devices
                .get(serial)
                .is_some_and(|ds| ds.screen.capturing);
            if is_capturing {
                ui.spinner();
            } else if ui.button("Capture").clicked() {
                if let Some(ds) = self.devices.get_mut(serial_owned) {
                    ds.screen.capturing = true;
                    ds.screen_status = "Capturing...".into();
                }
                self.spawn_screenshot_capture(serial_owned.to_string());
            }

            ui.separator();
            self.draw_screen_auto_controls(ui, serial_owned);

            ui.separator();
            self.draw_screen_recording_controls(ui, serial, serial_owned);

            ui.separator();
            if ui.button("Save As...").clicked() {
                match self.save_current_capture(serial) {
                    Ok(ScreenSaveResult::Saved(path)) => {
                        self.log(
                            AppLogLevel::Info,
                            format!("[{serial}] Screenshot saved to {}", path.display()),
                        );
                    }
                    Ok(ScreenSaveResult::Cancelled) => {
                        self.log_cancelled(serial, "save screenshot");
                    }
                    Ok(ScreenSaveResult::NoCapture) => {
                        self.log_skipped(serial, "save screenshot", "no capture is selected");
                    }
                    Err(error) => screen_errors.push(error),
                }
            }
            if ui.button("Clear History").clicked() {
                if let Some(ds) = self.devices.get_mut(serial_owned) {
                    ds.screen_captures.clear();
                    ds.screen_view_idx = None;
                }
            }

            if let Some(ds) = self.devices.get(serial) {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(self.display_text(&ds.screen_status));
                });
            }
        });
    }

    fn draw_screen_auto_controls(&mut self, ui: &mut egui::Ui, serial_owned: &str) {
        let auto_on = self
            .devices
            .get(serial_owned)
            .and_then(|ds| ds.screen_auto_interval)
            .is_some();
        if auto_on {
            if ui.button("Stop Auto").clicked() {
                if let Some(ds) = self.devices.get_mut(serial_owned) {
                    ds.screen_auto_interval = None;
                }
            }
            ui.colored_label(egui::Color32::from_rgb(100, 200, 100), "Auto: ON");
        } else {
            for seconds in [2.0, 5.0] {
                if ui.button(format!("Auto {seconds:.0}s")).clicked() {
                    if let Some(ds) = self.devices.get_mut(serial_owned) {
                        ds.screen_auto_interval = Some(seconds);
                        ds.screen_last_auto = 0.0;
                    }
                }
            }
        }
    }

    fn draw_screen_recording_controls(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
    ) {
        let is_recording = self
            .devices
            .get(serial)
            .is_some_and(|ds| ds.screen.recording);
        if is_recording {
            if ui.button("Stop Recording").clicked() {
                self.stop_screen_recording(serial, serial_owned);
            }
            ui.colored_label(egui::Color32::from_rgb(255, 80, 80), "REC");
        } else if ui.button("Record (180s)").clicked() {
            self.start_screen_recording(serial, serial_owned);
        }
    }

    fn stop_screen_recording(&mut self, serial: &str, serial_owned: &str) {
        if let Some(mut process) = self.recording_procs.remove(serial) {
            let _ = process.kill();
        }
        if let Some(ds) = self.devices.get_mut(serial_owned) {
            ds.screen.recording = false;
            ds.screen_status = "Recording stopped".into();
        }

        let serial = serial_owned.to_string();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if let Some(save) = rfd::FileDialog::new()
                .set_file_name("recording.mp4")
                .add_filter("Video", &["mp4"])
                .set_title("Save recording")
                .save_file()
            {
                let (ok, msg) = adb::pull_remote_file(
                    &serial,
                    "/sdcard/adb_ui_recording.mp4",
                    &save.display().to_string(),
                );
                let status = if ok {
                    "Recording saved"
                } else {
                    "Recording save failed"
                };
                let _ = tx.send(AdbMsg::DeviceActionResult(
                    serial.clone(),
                    format!("{status}: {msg}"),
                ));
            } else {
                let _ = tx.send(AdbMsg::DeviceActionResult(
                    serial.clone(),
                    "Recording save cancelled".into(),
                ));
            }
            let (deleted, delete_msg) =
                adb::delete_remote(&serial, "/sdcard/adb_ui_recording.mp4", false);
            if !deleted {
                let _ = tx.send(AdbMsg::DeviceActionResult(
                    serial,
                    format!("Recording cleanup failed: {delete_msg}"),
                ));
            }
        });
    }

    fn start_screen_recording(&mut self, serial: &str, serial_owned: &str) {
        match adb::start_screen_record(serial, "/sdcard/adb_ui_recording.mp4", 180) {
            Ok(child) => {
                self.recording_procs.insert(serial_owned.to_string(), child);
                if let Some(ds) = self.devices.get_mut(serial_owned) {
                    ds.screen.recording = true;
                    ds.screen_status = "Recording...".into();
                }
                self.log(
                    AppLogLevel::Info,
                    format!("[{serial}] Screen recording started"),
                );
            }
            Err(error) => {
                if let Some(ds) = self.devices.get_mut(serial_owned) {
                    ds.screen.recording = false;
                    ds.screen_status = format!("Recording failed: {error}");
                }
                self.log(
                    AppLogLevel::Error,
                    format!("[{serial}] Screen recording failed: {error}"),
                );
            }
        }
    }

    fn save_current_capture(&self, serial: &str) -> Result<ScreenSaveResult, String> {
        let Some(ds) = self.devices.get(serial) else {
            return Ok(ScreenSaveResult::NoCapture);
        };
        let Some(idx) = ds.screen_view_idx else {
            return Ok(ScreenSaveResult::NoCapture);
        };
        let Some(capture) = ds.screen_captures.get(idx) else {
            return Ok(ScreenSaveResult::NoCapture);
        };
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!(
                "screenshot_{}.png",
                capture.timestamp.replace(':', "-")
            ))
            .add_filter("PNG", &["png"])
            .save_file()
        else {
            return Ok(ScreenSaveResult::Cancelled);
        };

        save_png_bytes(&path, &capture.png_bytes)
            .map_err(|error| format!("Save screenshot: {}: {error}", path.display()))?;
        Ok(ScreenSaveResult::Saved(path))
    }

    fn draw_screen_image_panel(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        image_rect: egui::Rect,
        screen_errors: &mut Vec<String>,
    ) {
        let mut image_ui = ui.new_child(egui::UiBuilder::new().max_rect(image_rect));
        image_ui.set_clip_rect(image_rect);

        let Some(ds) = self.devices.get_mut(serial_owned) else {
            return;
        };
        let Some(idx) = ds.screen_view_idx else {
            image_ui.centered_and_justified(|ui| {
                ui.label("Press 'Capture' to take a screenshot.");
            });
            return;
        };
        let Some(capture) = ds.screen_captures.get_mut(idx) else {
            image_ui.centered_and_justified(|ui| {
                ui.label("Press 'Capture' to take a screenshot.");
            });
            return;
        };

        if let Err(error) = ensure_screen_texture(image_ui.ctx(), serial, idx, capture) {
            screen_errors.push(error);
        }

        let Some(texture) = capture.texture.as_ref() else {
            return;
        };
        let image_display = fitted_image_rect(image_rect, texture.size_vec2());
        image_ui.painter().image(
            texture.id(),
            image_display,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        image_ui.painter().text(
            egui::pos2(image_rect.min.x + 4.0, image_rect.min.y + 4.0),
            egui::Align2::LEFT_TOP,
            format!(
                "{} ({}x{})",
                capture.timestamp, capture.width, capture.height
            ),
            egui::FontId::proportional(12.0),
            egui::Color32::from_rgba_premultiplied(200, 200, 200, 180),
        );

        let response = image_ui.allocate_rect(image_rect, egui::Sense::click());
        let png_bytes = capture.png_bytes.clone();
        let timestamp = capture.timestamp.clone();
        response.context_menu(|ui| {
            draw_capture_context_menu(ui, &png_bytes, &timestamp, screen_errors);
        });
    }

    fn draw_screen_history_panel(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        history_rect: egui::Rect,
        screen_errors: &mut Vec<String>,
    ) {
        let mut history_ui = ui.new_child(egui::UiBuilder::new().max_rect(history_rect));
        history_ui.set_clip_rect(history_rect);
        history_ui.painter().line_segment(
            [
                egui::pos2(history_rect.min.x, history_rect.min.y),
                egui::pos2(history_rect.max.x, history_rect.min.y),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 60, 60)),
        );

        if let Some(ds) = self.devices.get_mut(serial_owned) {
            for (index, capture) in ds.screen_captures.iter_mut().enumerate() {
                if let Err(error) = ensure_screen_texture(history_ui.ctx(), serial, index, capture)
                {
                    screen_errors.push(error);
                }
            }
        }

        let capture_count = self
            .devices
            .get(serial)
            .map_or(0, |ds| ds.screen_captures.len());
        let current_idx = self.devices.get(serial).and_then(|ds| ds.screen_view_idx);
        if capture_count == 0 {
            history_ui.centered_and_justified(|ui| {
                ui.colored_label(egui::Color32::from_rgb(120, 120, 120), "No captures yet.");
            });
            return;
        }

        egui::ScrollArea::horizontal()
            .id_salt(format!("screen_history_{serial}"))
            .show(&mut history_ui, |ui| {
                ui.horizontal(|ui| {
                    for index in (0..capture_count).rev() {
                        self.draw_screen_thumbnail(
                            ui,
                            serial,
                            serial_owned,
                            index,
                            current_idx == Some(index),
                            screen_errors,
                        );
                    }
                });
            });
    }

    fn draw_screen_thumbnail(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        index: usize,
        is_selected: bool,
        screen_errors: &mut Vec<String>,
    ) {
        let Some((timestamp, texture_id, texture_size)) = self.devices.get(serial).and_then(|ds| {
            ds.screen_captures.get(index).and_then(|capture| {
                capture
                    .texture
                    .as_ref()
                    .map(|texture| (capture.timestamp.clone(), texture.id(), texture.size_vec2()))
            })
        }) else {
            return;
        };

        let thumb_height = 55.0_f32;
        let aspect = texture_size.x / texture_size.y.max(1.0);
        let thumb_width = thumb_height * aspect;
        let total_size = egui::vec2(thumb_width + 4.0, thumb_height + 16.0);
        let (rect, response) = ui.allocate_exact_size(total_size, egui::Sense::click());

        let stroke = if is_selected {
            egui::Stroke::new(2.0, egui::Color32::from_rgb(80, 160, 255))
        } else if response.hovered() {
            egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 100, 100))
        } else {
            egui::Stroke::new(1.0, egui::Color32::from_rgb(50, 50, 50))
        };
        ui.painter()
            .rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Outside);

        let image_rect = egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 2.0, rect.min.y + 2.0),
            egui::vec2(thumb_width, thumb_height),
        );
        ui.painter().image(
            texture_id,
            image_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        ui.painter().text(
            egui::pos2(rect.center().x, rect.max.y - 2.0),
            egui::Align2::CENTER_BOTTOM,
            &timestamp,
            egui::FontId::proportional(9.0),
            egui::Color32::from_rgb(160, 160, 160),
        );

        if response.clicked() {
            if let Some(ds) = self.devices.get_mut(serial_owned) {
                ds.screen_view_idx = Some(index);
            }
        }

        response.context_menu(|ui| {
            self.draw_screen_thumbnail_menu(ui, serial, serial_owned, index, screen_errors);
        });
    }

    fn draw_screen_thumbnail_menu(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        index: usize,
        screen_errors: &mut Vec<String>,
    ) {
        if ui.button("View").clicked() {
            if let Some(ds) = self.devices.get_mut(serial_owned) {
                ds.screen_view_idx = Some(index);
            }
            ui.close();
        }

        let capture_snapshot = self.devices.get(serial).and_then(|ds| {
            ds.screen_captures
                .get(index)
                .map(|capture| (capture.png_bytes.clone(), capture.timestamp.clone()))
        });
        let Some((png_bytes, timestamp)) = capture_snapshot else {
            return;
        };

        if ui.button("Save As...").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .set_file_name(format!("screenshot_{}.png", timestamp.replace(':', "-")))
                .add_filter("PNG", &["png"])
                .save_file()
            {
                if let Err(error) = save_png_bytes(&path, &png_bytes) {
                    screen_errors.push(format!(
                        "Save screenshot failed: {}: {error}",
                        path.display()
                    ));
                }
            }
            ui.close();
        }
        if ui.button("Copy Image").clicked() {
            if let Err(error) = copy_png_to_clipboard(&png_bytes) {
                screen_errors.push(format!("Copy image: {error}"));
            }
            ui.close();
        }
        if ui.button("Copy as File").clicked() {
            if let Err(error) = copy_png_as_file(&png_bytes, &timestamp) {
                screen_errors.push(format!("Copy as file: {error}"));
            }
            ui.close();
        }
        if ui.button("Copy Path").clicked() {
            let path = get_screenshot_temp_path(&timestamp);
            if let Err(error) = save_png_bytes(&path, &png_bytes) {
                screen_errors.push(format!(
                    "Write temp screenshot: {}: {error}",
                    path.display()
                ));
            } else {
                ui.ctx().copy_text(path.display().to_string());
            }
            ui.close();
        }
        ui.separator();
        if ui.button("Delete").clicked() {
            if let Some(ds) = self.devices.get_mut(serial_owned) {
                if index < ds.screen_captures.len() {
                    ds.screen_captures.remove(index);
                    if let Some(view_idx) = &mut ds.screen_view_idx {
                        if *view_idx >= ds.screen_captures.len() {
                            *view_idx = ds.screen_captures.len().saturating_sub(1);
                        }
                        if ds.screen_captures.is_empty() {
                            ds.screen_view_idx = None;
                        }
                    }
                }
            }
            ui.close();
        }
    }

    fn spawn_screenshot_capture(&self, serial: String) {
        let tx = self.tx.clone();
        std::thread::spawn(move || match adb::capture_screenshot_bytes(&serial) {
            Ok(bytes) => {
                let _ = tx.send(AdbMsg::ScreenshotReady(serial, bytes, now_str()));
            }
            Err(error) => {
                let _ = tx.send(AdbMsg::ScreenshotError(serial, error));
            }
        });
    }

    fn flush_screen_errors(&mut self, serial: &str, screen_errors: Vec<String>) {
        for error in screen_errors {
            self.log(AppLogLevel::Error, format!("[{serial}] {error}"));
        }
    }
}

fn ensure_screen_texture(
    ctx: &egui::Context,
    serial: &str,
    index: usize,
    capture: &mut crate::device::ScreenCapture,
) -> Result<(), String> {
    if capture.texture.is_some() {
        return Ok(());
    }

    let image = image::load_from_memory_with_format(&capture.png_bytes, image::ImageFormat::Png)
        .map_err(|error| format!("Decode screenshot failed: {error}"))?;
    let rgba = image.to_rgba8();
    let size = [
        usize::try_from(rgba.width()).map_err(|_| "Screenshot width is too large".to_string())?,
        usize::try_from(rgba.height()).map_err(|_| "Screenshot height is too large".to_string())?,
    ];
    let pixels = rgba.as_flat_samples();
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());
    let texture = ctx.load_texture(
        format!("screen_{serial}_{index}"),
        color_image,
        egui::TextureOptions::LINEAR,
    );
    capture.texture = Some(texture);
    Ok(())
}

fn fitted_image_rect(area: egui::Rect, texture_size: egui::Vec2) -> egui::Rect {
    let scale = (area.width() / texture_size.x)
        .min(area.height() / texture_size.y)
        .min(1.0);
    let display_size = texture_size * scale;
    let offset = egui::vec2(
        (area.width() - display_size.x) / 2.0,
        (area.height() - display_size.y) / 2.0,
    );
    egui::Rect::from_min_size(area.min + offset, display_size)
}

fn draw_capture_context_menu(
    ui: &mut egui::Ui,
    png_bytes: &std::sync::Arc<Vec<u8>>,
    timestamp: &str,
    screen_errors: &mut Vec<String>,
) {
    if ui.button("Save As...").clicked() {
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("screenshot_{}.png", timestamp.replace(':', "-")))
            .add_filter("PNG", &["png"])
            .save_file()
        {
            if let Err(error) = save_png_bytes(&path, png_bytes) {
                screen_errors.push(format!(
                    "Save screenshot failed: {}: {error}",
                    path.display()
                ));
            }
        }
        ui.close();
    }
    if ui.button("Copy Image").clicked() {
        if let Err(error) = copy_png_to_clipboard(png_bytes) {
            screen_errors.push(format!("Copy image: {error}"));
        }
        ui.close();
    }
    if ui.button("Copy as File").clicked() {
        if let Err(error) = copy_png_as_file(png_bytes, timestamp) {
            screen_errors.push(format!("Copy as file: {error}"));
        }
        ui.close();
    }
    if ui.button("Copy Path").clicked() {
        let path = get_screenshot_temp_path(timestamp);
        if let Err(error) = save_png_bytes(&path, png_bytes) {
            screen_errors.push(format!(
                "Write temp screenshot: {}: {error}",
                path.display()
            ));
        } else {
            ui.ctx().copy_text(path.display().to_string());
        }
        ui.close();
    }
}

fn save_png_bytes(path: &std::path::Path, png_bytes: &[u8]) -> Result<(), std::io::Error> {
    std::fs::write(path, png_bytes)
}

enum ScreenSaveResult {
    Saved(std::path::PathBuf),
    Cancelled,
    NoCapture,
}
