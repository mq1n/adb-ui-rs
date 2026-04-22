use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;

use crate::adb;
use crate::adb::mirror::{DeviceRotation, DeviceRotationMode, MirrorMode};
use crate::device::DeviceState;

impl super::App {
    pub(super) fn draw_mirror_tab(&mut self, ui: &mut egui::Ui, serial: &str) {
        let serial_owned = serial.to_string();

        self.update_mirror_texture(ui.ctx(), &serial_owned);
        self.draw_mirror_toolbar(ui, serial, &serial_owned);
        ui.separator();

        let available = ui.available_rect_before_wrap();
        let nav_height = 36.0;
        let video_height = (available.height() - nav_height - 4.0).max(100.0);

        let video_rect =
            egui::Rect::from_min_size(available.min, egui::vec2(available.width(), video_height));
        let nav_y = available.min.y + video_height + 4.0;
        let nav_rect = egui::Rect::from_min_size(
            egui::pos2(available.min.x, nav_y),
            egui::vec2(available.width(), nav_height),
        );
        ui.allocate_rect(available, egui::Sense::hover());

        self.draw_mirror_video(ui, serial, &serial_owned, video_rect);
        self.draw_mirror_nav_bar(ui, serial, nav_rect);

        if self.devices.get(serial).is_some_and(|ds| ds.mirror.active) {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
    }

    // ─── Toolbar ────────────────────────────────────────────────────────

    fn draw_mirror_toolbar(&mut self, ui: &mut egui::Ui, serial: &str, serial_owned: &str) {
        ui.horizontal(|ui| {
            let is_active = self.devices.get(serial).is_some_and(|ds| ds.mirror.active);

            if is_active {
                if ui.button("Stop Mirror").clicked() {
                    self.stop_mirroring(serial_owned);
                }
                ui.colored_label(egui::Color32::from_rgb(100, 200, 100), "LIVE");
            } else {
                if ui.button("Start Mirror").clicked() {
                    self.start_mirroring(serial_owned);
                }

                ui.separator();
                self.draw_mirror_resolution_combo(ui, serial, serial_owned);
                ui.separator();
                self.draw_mirror_bitrate_combo(ui, serial, serial_owned);
            }

            ui.separator();
            self.draw_mirror_rotation_menu(ui, serial_owned);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(ds) = self.devices.get(serial) {
                    ui.label(&ds.mirror.status);
                }
                ui.separator();
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    self.draw_mirror_server_inline(ui, serial, serial_owned);
                });
            });
        });
    }

    fn draw_mirror_rotation_menu(&mut self, ui: &mut egui::Ui, serial_owned: &str) {
        ui.menu_button("Rotate", |ui| {
            if ui.button(DeviceRotationMode::Auto.label()).clicked() {
                self.spawn_mirror_rotation(serial_owned, DeviceRotationMode::Auto);
                ui.close();
            }

            ui.separator();

            for rotation in DeviceRotation::ALL {
                let mode = DeviceRotationMode::Locked(rotation);
                if ui.button(mode.label()).clicked() {
                    self.spawn_mirror_rotation(serial_owned, mode);
                    ui.close();
                }
            }

            ui.separator();
            ui.label(
                egui::RichText::new("Restarts live mirroring after the rotation change.")
                    .small()
                    .color(egui::Color32::from_rgb(160, 160, 160)),
            );
        });
    }

    fn draw_mirror_resolution_combo(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
    ) {
        let current = self
            .devices
            .get(serial)
            .map(|ds| mirror_resolution_label(ds.mirror_config.width, ds.mirror_config.height))
            .unwrap_or_default();

        egui::ComboBox::from_id_salt(format!("mirror_res_{serial}"))
            .selected_text(&current)
            .width(90.0)
            .show_ui(ui, |ui| {
                for (label, w, h) in [
                    ("480p", 480, 854),
                    ("720p", 720, 1280),
                    ("1080p", 1080, 1920),
                ] {
                    let selected = self.devices.get(serial).is_some_and(|ds| {
                        ds.mirror_config.width == w && ds.mirror_config.height == h
                    });
                    if ui.selectable_label(selected, label).clicked() {
                        if let Some(ds) = self.devices.get_mut(serial_owned) {
                            ds.mirror_config.width = w;
                            ds.mirror_config.height = h;
                        }
                        self.log_mirror_info(
                            serial,
                            format!("Resolution set to {label} ({w}x{h})"),
                        );
                    }
                }
            });
    }

    fn draw_mirror_bitrate_combo(&mut self, ui: &mut egui::Ui, serial: &str, serial_owned: &str) {
        let current = self
            .devices
            .get(serial)
            .map(|ds| format_bitrate(ds.mirror_config.bitrate))
            .unwrap_or_default();

        egui::ComboBox::from_id_salt(format!("mirror_br_{serial}"))
            .selected_text(&current)
            .width(80.0)
            .show_ui(ui, |ui| {
                for (label, br) in [
                    ("1 Mbps", 1_000_000),
                    ("2 Mbps", 2_000_000),
                    ("4 Mbps", 4_000_000),
                    ("8 Mbps", 8_000_000),
                ] {
                    let selected = self
                        .devices
                        .get(serial)
                        .is_some_and(|ds| ds.mirror_config.bitrate == br);
                    if ui.selectable_label(selected, label).clicked() {
                        if let Some(ds) = self.devices.get_mut(serial_owned) {
                            ds.mirror_config.bitrate = br;
                        }
                        self.log_mirror_info(serial, format!("Bitrate set to {label}"));
                    }
                }
            });
    }

    // ─── Video display ──────────────────────────────────────────────────

    fn draw_mirror_video(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        video_rect: egui::Rect,
    ) {
        let mut video_ui = ui.new_child(egui::UiBuilder::new().max_rect(video_rect));
        video_ui.set_clip_rect(video_rect);

        let display_info = self.devices.get(serial).and_then(|ds| {
            if !ds.mirror.active {
                return None;
            }
            ds.mirror_texture.as_ref().map(|tex| {
                (
                    tex.id(),
                    tex.size_vec2(),
                    ds.mirror.device_width,
                    ds.mirror.device_height,
                    ds.mirror.fps,
                )
            })
        });

        let Some((tex_id, tex_size, dev_w, dev_h, fps)) = display_info else {
            let active = self.devices.get(serial).is_some_and(|ds| ds.mirror.active);
            if active {
                let status = self
                    .devices
                    .get(serial)
                    .map(|ds| ds.mirror.status.clone())
                    .unwrap_or_default();
                video_ui.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.spinner();
                        ui.label(status);
                    });
                });
            } else {
                video_ui.centered_and_justified(|ui| {
                    ui.label("Press 'Start Mirror' to begin screen mirroring.");
                });
            }
            return;
        };

        let display_rect = fitted_image_rect(video_rect, tex_size);
        video_ui.painter().image(
            tex_id,
            display_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );

        video_ui.painter().text(
            egui::pos2(video_rect.min.x + 4.0, video_rect.min.y + 4.0),
            egui::Align2::LEFT_TOP,
            format!("{fps:.0} fps"),
            egui::FontId::proportional(12.0),
            egui::Color32::from_rgba_premultiplied(200, 200, 200, 180),
        );

        let response = video_ui.allocate_rect(display_rect, egui::Sense::click_and_drag());

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (fallback_w, fallback_h) = (tex_size.x.max(0.0) as u32, tex_size.y.max(0.0) as u32);
        let touch_w = if dev_w > 0 { dev_w } else { fallback_w };
        let touch_h = if dev_h > 0 { dev_h } else { fallback_h };
        if touch_w > 0 && touch_h > 0 {
            self.handle_mirror_input(
                &response,
                serial,
                serial_owned,
                display_rect,
                touch_w,
                touch_h,
            );
        }
    }

    fn handle_mirror_input(
        &mut self,
        response: &egui::Response,
        serial: &str,
        serial_owned: &str,
        display_rect: egui::Rect,
        dev_w: u32,
        dev_h: u32,
    ) {
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let (x, y) = map_to_device(pos, display_rect, dev_w, dev_h);
                self.send_mirror_tap(serial, x, y);
            }
        }
        if response.drag_started() {
            if let Some(pos) = response.interact_pointer_pos() {
                if let Some(ds) = self.devices.get_mut(serial_owned) {
                    ds.mirror.drag_start = Some(pos);
                }
            }
        }
        if response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                if let Some(ds) = self.devices.get_mut(serial_owned) {
                    ds.mirror.drag_current = Some(pos);
                }
            }
        }
        if response.drag_stopped() {
            let drag_info = self.devices.get_mut(serial_owned).and_then(|ds| {
                let start = ds.mirror.drag_start.take()?;
                let end = ds.mirror.drag_current.take()?;
                Some((start, end))
            });
            if let Some((start, end)) = drag_info {
                let (sx, sy) = map_to_device(start, display_rect, dev_w, dev_h);
                let (ex, ey) = map_to_device(end, display_rect, dev_w, dev_h);
                self.send_mirror_swipe(serial, sx, sy, ex, ey, 300);
            }
        }
    }

    // ─── Navigation bar ─────────────────────────────────────────────────

    fn send_mirror_tap(&self, serial: &str, x: u32, y: u32) {
        if let Some(control) = self
            .devices
            .get(serial)
            .and_then(|ds| ds.mirror_control.clone())
        {
            control.send_tap(x, y);
        } else {
            adb::mirror::send_tap(serial, x, y);
        }
    }

    fn send_mirror_swipe(
        &self,
        serial: &str,
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
        duration_ms: u32,
    ) {
        if let Some(control) = self
            .devices
            .get(serial)
            .and_then(|ds| ds.mirror_control.clone())
        {
            control.send_swipe(x1, y1, x2, y2, duration_ms);
        } else {
            adb::mirror::send_swipe(serial, x1, y1, x2, y2, duration_ms);
        }
    }

    fn send_mirror_key(&self, serial: &str, keycode: u32) {
        if let Some(control) = self
            .devices
            .get(serial)
            .and_then(|ds| ds.mirror_control.clone())
        {
            control.send_key_event(keycode);
        } else {
            adb::mirror::send_key_event(serial, keycode);
        }
    }

    fn draw_mirror_nav_bar(&self, ui: &mut egui::Ui, serial: &str, nav_rect: egui::Rect) {
        let mut nav_ui = ui.new_child(egui::UiBuilder::new().max_rect(nav_rect));
        nav_ui.set_clip_rect(nav_rect);

        if !self.devices.get(serial).is_some_and(|ds| ds.mirror.active) {
            return;
        }

        nav_ui.horizontal_centered(|ui| {
            if ui.button("Back").clicked() {
                self.send_mirror_key(serial, adb::mirror::keycode::BACK);
            }
            if ui.button("Home").clicked() {
                self.send_mirror_key(serial, adb::mirror::keycode::HOME);
            }
            if ui.button("Recent").clicked() {
                self.send_mirror_key(serial, adb::mirror::keycode::APP_SWITCH);
            }
            ui.separator();
            if ui.button("Vol-").clicked() {
                self.send_mirror_key(serial, adb::mirror::keycode::VOLUME_DOWN);
            }
            if ui.button("Vol+").clicked() {
                self.send_mirror_key(serial, adb::mirror::keycode::VOLUME_UP);
            }
            ui.separator();
            if ui.button("Power").clicked() {
                self.send_mirror_key(serial, adb::mirror::keycode::POWER);
            }
            if ui.button("Menu").clicked() {
                self.send_mirror_key(serial, adb::mirror::keycode::MENU);
            }
        });
    }

    // ─── Server management panel ────────────────────────────────────────

    fn draw_mirror_server_inline(&mut self, ui: &mut egui::Ui, serial: &str, serial_owned: &str) {
        let (installed, running, busy, status) =
            self.devices
                .get(serial)
                .map_or((None, None, false, String::new()), |ds| {
                    (
                        ds.mirror_server.installed,
                        ds.mirror_server.running,
                        ds.mirror_server.busy,
                        ds.mirror_server.status.clone(),
                    )
                });

        let mirror_active = self.devices.get(serial).is_some_and(|ds| ds.mirror.active);
        let disabled = busy || mirror_active;

        ui.label("Server");
        ui.colored_label(
            status_color(installed),
            format!("Inst: {}", status_label(installed)),
        );
        ui.colored_label(
            status_color(running),
            format!("Run: {}", status_label(running)),
        );
        if busy {
            ui.spinner();
        }
        if ui
            .add_enabled(!disabled, egui::Button::new("Check"))
            .clicked()
        {
            self.spawn_server_check(serial_owned);
        }
        if ui
            .add_enabled(!disabled, egui::Button::new("Install..."))
            .clicked()
        {
            self.spawn_server_install(serial_owned);
        }
        if ui
            .add_enabled(!disabled, egui::Button::new("Remove"))
            .clicked()
        {
            self.spawn_server_remove(serial_owned);
        }
        if ui
            .add_enabled(!disabled, egui::Button::new("Build"))
            .clicked()
        {
            self.spawn_server_build(serial_owned);
        }
        if ui
            .add_enabled(!disabled, egui::Button::new("Kill"))
            .clicked()
        {
            self.spawn_server_kill(serial_owned);
        }
        if !status.is_empty() {
            ui.separator();
            ui.label(
                egui::RichText::new(status)
                    .small()
                    .color(egui::Color32::from_rgb(160, 160, 160)),
            );
        }
    }

    // ─── Server action spawners ─────────────────────────────────────────

    fn spawn_server_check(&mut self, serial: &str) {
        if let Some(ds) = self.devices.get_mut(serial) {
            ds.mirror_server.busy = true;
        }
        self.log_mirror_info(serial, "Checking mirror server status");
        let tx = self.tx.clone();
        let serial = serial.to_string();
        std::thread::spawn(move || {
            let (installed, running, msg) = match adb::mirror::check_server_status(&serial) {
                Ok((installed, running)) => (
                    Some(installed),
                    Some(running),
                    format!(
                        "Installed: {}, Running: {}",
                        yes_no(installed),
                        yes_no(running)
                    ),
                ),
                Err(error) => (None, None, format!("Status check failed: {error}")),
            };
            let _ = tx.send(adb::AdbMsg::MirrorServerStatus(
                serial, installed, running, msg,
            ));
        });
    }

    fn spawn_server_install(&mut self, serial: &str) {
        let default_path = project_server_source_path();
        let mut dialog = rfd::FileDialog::new()
            .set_title("Select mirror-server.jar")
            .add_filter("JAR", &["jar"]);
        if let Some(parent) = default_path.parent() {
            dialog = dialog.set_directory(parent);
        }
        if let Some(file_name) = default_path.file_name().and_then(|name| name.to_str()) {
            dialog = dialog.set_file_name(file_name);
        }
        let Some(path) = dialog.pick_file() else {
            self.log_cancelled(serial, "mirror server install");
            return;
        };
        if let Some(ds) = self.devices.get_mut(serial) {
            ds.mirror_server.busy = true;
            ds.mirror_server.status = "Pushing...".into();
        }
        self.log_mirror_info(
            serial,
            format!("Installing mirror server from {}", path.display()),
        );
        let tx = self.tx.clone();
        let serial = serial.to_string();
        std::thread::spawn(move || {
            let action_result = adb::mirror::push_server(&serial, &path.display().to_string())
                .map(|()| "Server installed".to_string())
                .map_err(|error| format!("Install failed: {error}"));
            let (installed, running, msg) = refresh_server_status_message(&serial, action_result);
            let _ = tx.send(adb::AdbMsg::MirrorServerStatus(
                serial, installed, running, msg,
            ));
        });
    }

    fn spawn_server_remove(&mut self, serial: &str) {
        if let Some(ds) = self.devices.get_mut(serial) {
            ds.mirror_server.busy = true;
            ds.mirror_server.status = "Removing...".into();
        }
        self.log_mirror_info(serial, "Removing mirror server");
        let tx = self.tx.clone();
        let serial = serial.to_string();
        std::thread::spawn(move || {
            let _ = adb::mirror::kill_server(&serial);
            let action_result = adb::mirror::remove_server(&serial)
                .map(|()| "Server removed".to_string())
                .map_err(|error| format!("Remove failed: {error}"));
            let (installed, running, msg) = refresh_server_status_message(&serial, action_result);
            let _ = tx.send(adb::AdbMsg::MirrorServerStatus(
                serial, installed, running, msg,
            ));
        });
    }

    fn spawn_server_build(&mut self, serial: &str) {
        if let Some(ds) = self.devices.get_mut(serial) {
            ds.mirror_server.busy = true;
            ds.mirror_server.status = "Building...".into();
        }
        self.log_mirror_info(serial, "Building mirror server");
        let tx = self.tx.clone();
        let serial = serial.to_string();
        std::thread::spawn(move || {
            let action_result = match adb::mirror::build_server() {
                Ok(jar_path) => adb::mirror::push_server(&serial, &jar_path.display().to_string())
                    .map(|()| format!("Built & installed from {}", jar_path.display()))
                    .map_err(|error| format!("Build succeeded but install failed: {error}")),
                Err(error) => Err(format!("Build failed: {error}")),
            };
            let (installed, running, msg) = refresh_server_status_message(&serial, action_result);
            let _ = tx.send(adb::AdbMsg::MirrorServerStatus(
                serial, installed, running, msg,
            ));
        });
    }

    fn spawn_server_kill(&mut self, serial: &str) {
        if let Some(ds) = self.devices.get_mut(serial) {
            ds.mirror_server.busy = true;
            ds.mirror_server.status = "Killing...".into();
        }
        self.log_mirror_info(serial, "Killing mirror server");
        let tx = self.tx.clone();
        let serial = serial.to_string();
        std::thread::spawn(move || {
            let action_result = adb::mirror::kill_server(&serial)
                .map(|count| {
                    if count == 0 {
                        "No running mirror server found".to_string()
                    } else {
                        format!("Killed {count} mirror server process(es)")
                    }
                })
                .map_err(|error| format!("Kill failed: {error}"));
            let (installed, running, msg) = refresh_server_status_message(&serial, action_result);
            let _ = tx.send(adb::AdbMsg::MirrorServerStatus(
                serial, installed, running, msg,
            ));
        });
    }

    // ─── Lifecycle ──────────────────────────────────────────────────────

    pub(super) fn start_mirroring(&mut self, serial: &str) {
        let config = self
            .devices
            .get(serial)
            .map(|ds| ds.mirror_config.clone())
            .unwrap_or_default();
        let mode = Self::resolve_mirror_mode(serial);
        let Some(session) = self
            .devices
            .get_mut(serial)
            .map(DeviceState::start_next_mirror_session)
        else {
            return;
        };

        match adb::mirror::start_mirror(serial, session, &config, mode, self.tx.clone()) {
            Ok(handle) => {
                if let Some(ds) = self.devices.get_mut(serial) {
                    ds.attach_mirror_session(session, handle);
                }
                self.log_mirror_info(
                    serial,
                    format!(
                        "Starting ({}, {}x{}, {})",
                        if adb::is_wsa_serial(serial) {
                            "WSA screencap fallback"
                        } else {
                            mode.label()
                        },
                        config.width,
                        config.height,
                        format_bitrate(config.bitrate)
                    ),
                );
            }
            Err(e) => {
                if let Some(ds) = self.devices.get_mut(serial) {
                    ds.finish_mirror_session(format!("Failed: {e}"));
                }
                self.log_mirror_error(serial, format!("Start failed: {e}"));
            }
        }
    }

    pub(super) fn stop_mirroring(&mut self, serial: &str) {
        if let Some(ds) = self.devices.get_mut(serial) {
            ds.cancel_mirror_session("Stopped");
        }
        self.log_mirror_info(serial, "Stopped");
    }

    fn spawn_mirror_rotation(&mut self, serial: &str, mode: DeviceRotationMode) {
        self.log_mirror_info(serial, format!("Applying rotation: {}", mode.label()));

        let tx = self.tx.clone();
        let serial = serial.to_string();
        let label = mode.label().to_string();
        std::thread::spawn(move || {
            let result = adb::mirror::apply_device_rotation(&serial, mode);
            let _ = tx.send(adb::AdbMsg::MirrorRotationResult(serial, label, result));
        });
    }

    const fn resolve_mirror_mode(serial: &str) -> MirrorMode {
        let _ = serial;
        MirrorMode::Server
    }

    // ─── Frame update ───────────────────────────────────────────────────

    fn update_mirror_texture(&mut self, ctx: &egui::Context, serial: &str) {
        let frame = self
            .devices
            .get(serial)
            .and_then(|ds| ds.mirror_frame_buffer.as_ref()?.take());

        let Some(frame) = frame else {
            return;
        };

        let Some(ds) = self.devices.get_mut(serial) else {
            return;
        };

        ds.mirror.video_width = u32::try_from(frame.width).unwrap_or(0);
        ds.mirror.video_height = u32::try_from(frame.height).unwrap_or(0);

        ds.mirror.frame_count += 1;
        let now = ctx.input(|i| i.time);
        if ds.mirror.last_fps_time == 0.0 {
            ds.mirror.last_fps_time = now;
            ds.mirror.last_fps_count = ds.mirror.frame_count;
        }
        let elapsed = now - ds.mirror.last_fps_time;
        if elapsed >= 1.0 {
            let frames_since = ds.mirror.frame_count - ds.mirror.last_fps_count;
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
            let fps = frames_since as f32 / elapsed as f32;
            ds.mirror.fps = fps;
            ds.mirror.last_fps_time = now;
            ds.mirror.last_fps_count = ds.mirror.frame_count;
        }

        ds.mirror.status = format!(
            "{}x{} @ {:.0} fps",
            frame.width, frame.height, ds.mirror.fps
        );

        let size = [frame.width, frame.height];
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &frame.rgba);

        if let Some(tex) = ds.mirror_texture.as_mut() {
            tex.set(color_image, egui::TextureOptions::LINEAR);
        } else {
            let tex = ctx.load_texture(
                format!("mirror_{serial}"),
                color_image,
                egui::TextureOptions::LINEAR,
            );
            ds.mirror_texture = Some(tex);
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn map_to_device(pos: egui::Pos2, display_rect: egui::Rect, dev_w: u32, dev_h: u32) -> (u32, u32) {
    let rel_x = ((pos.x - display_rect.min.x) / display_rect.width()).clamp(0.0, 1.0);
    let rel_y = ((pos.y - display_rect.min.y) / display_rect.height()).clamp(0.0, 1.0);
    let x = (rel_x * dev_w as f32) as u32;
    let y = (rel_y * dev_h as f32) as u32;
    (
        x.min(dev_w.saturating_sub(1)),
        y.min(dev_h.saturating_sub(1)),
    )
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

fn format_bitrate(bps: u32) -> String {
    if bps >= 1_000_000 {
        format!("{} Mbps", bps / 1_000_000)
    } else {
        format!("{} kbps", bps / 1000)
    }
}

fn mirror_resolution_label(width: u32, height: u32) -> String {
    match (width, height) {
        (480, 854) => "480p".to_string(),
        (720, 1280) => "720p".to_string(),
        (1080, 1920) => "1080p".to_string(),
        _ => format!("{width}x{height}"),
    }
}

fn refresh_server_status_message(
    serial: &str,
    action_result: Result<String, String>,
) -> (Option<bool>, Option<bool>, String) {
    match (action_result, adb::mirror::check_server_status(serial)) {
        (Ok(message), Ok((installed, running))) => (Some(installed), Some(running), message),
        (Err(error), Ok((installed, running))) => (Some(installed), Some(running), error),
        (Ok(message), Err(error)) => (
            None,
            None,
            format!("{message} (status refresh failed: {error})"),
        ),
        (Err(action_error), Err(status_error)) => (
            None,
            None,
            format!("{action_error} (status refresh failed: {status_error})"),
        ),
    }
}

fn project_server_source_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("server")
        .join("Server.java")
}

const fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

const fn status_color(val: Option<bool>) -> egui::Color32 {
    match val {
        Some(true) => egui::Color32::from_rgb(100, 220, 100),
        Some(false) => egui::Color32::from_rgb(180, 180, 180),
        None => egui::Color32::from_rgb(120, 120, 120),
    }
}

const fn status_label(val: Option<bool>) -> &'static str {
    match val {
        Some(true) => "Yes",
        Some(false) => "No",
        None => "?",
    }
}
