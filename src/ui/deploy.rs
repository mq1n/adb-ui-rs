use eframe::egui;

use crate::adb::{self, AdbMsg};
use crate::device::{CapabilityStatus, DeployMethod};

impl super::App {
    pub(super) fn draw_deploy_tab(&mut self, ui: &mut egui::Ui, serial: &str) {
        let serial_owned = serial.to_string();
        let bundle_id = self.config.bundle_id.clone();

        // ── Deploy header ──────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Deploy Game Data").strong().size(14.0));
            ui.separator();

            // run-as status indicator
            let run_as_status = self
                .devices
                .get(serial)
                .map_or(CapabilityStatus::Unknown, |ds| ds.deploy.run_as);
            match run_as_status {
                CapabilityStatus::Available => {
                    ui.colored_label(egui::Color32::from_rgb(100, 220, 100), "run-as: available");
                }
                CapabilityStatus::Unavailable => {
                    ui.colored_label(
                        egui::Color32::from_rgb(180, 180, 180),
                        "run-as: unavailable",
                    );
                }
                CapabilityStatus::Unknown => {
                    ui.colored_label(egui::Color32::from_rgb(160, 160, 160), "run-as: unchecked");
                }
            }
            if ui.small_button("Check").clicked() {
                if bundle_id.trim().is_empty() {
                    self.log_skipped(serial, "check run-as", "bundle ID is not configured");
                    return;
                }
                let serial = serial_owned.clone();
                let bid = bundle_id.clone();
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let ok = adb::check_run_as(&serial, &bid);
                    let _ = tx.send(AdbMsg::DeviceActionResult(
                        serial.clone(),
                        format!(
                            "run-as ({bid}): {}",
                            if ok { "available" } else { "not available" }
                        ),
                    ));
                    let _ = tx.send(AdbMsg::RunAsAvailability(serial, bid, ok));
                });
            }

            let deploying = self.devices.get(serial).is_some_and(|ds| ds.deploy.running);
            if deploying {
                ui.separator();
                ui.spinner();
                ui.label("Deploying...");
            }
        });

        ui.separator();

        // ── Deploy method selection ─────────────────────────────────
        ui.horizontal(|ui| {
            ui.label("Method:");
            if let Some(ds) = self.devices.get_mut(&serial_owned) {
                ui.radio_value(
                    &mut ds.deploy.method,
                    DeployMethod::External,
                    "External (adb push)",
                )
                .on_hover_text("Push directly to /sdcard/Android/data/<pkg>/files/");
                ui.radio_value(
                    &mut ds.deploy.method,
                    DeployMethod::Internal,
                    "Internal (run-as)",
                )
                .on_hover_text("Stage via /data/local/tmp, then copy into app via run-as");
            }
        });

        ui.add_space(4.0);

        // ── Quick push directory ────────────────────────────────────
        ui.label(egui::RichText::new("Push Directory").strong());
        ui.horizontal(|ui| {
            if ui
                .button("Pick Folder & Push...")
                .on_hover_text("Select a local folder to push to device")
                .clicked()
            {
                if bundle_id.trim().is_empty() {
                    self.log_skipped(serial, "deploy folder", "bundle ID is not configured");
                    return;
                }
                if let Some(dir) = rfd::FileDialog::new()
                    .set_title("Select folder to push")
                    .pick_folder()
                {
                    let local = dir.display().to_string();
                    let folder_name = dir
                        .file_name()
                        .map_or_else(|| "data".into(), |n| n.to_string_lossy().into_owned());
                    let use_internal = self
                        .devices
                        .get(serial)
                        .is_some_and(|ds| matches!(ds.deploy.method, DeployMethod::Internal));

                    if let Some(ds) = self.devices.get_mut(&serial_owned) {
                        ds.deploy.running = true;
                        ds.deploy.status = format!("Pushing {folder_name}...");
                    }
                    let serial = serial_owned.clone();
                    let bid = bundle_id.clone();
                    let tx = self.tx.clone();
                    let label = folder_name.clone();
                    std::thread::spawn(move || {
                        let result = if use_internal {
                            let (ok, msg) =
                                adb::deploy_via_run_as(&serial, &local, &folder_name, &bid);
                            if ok {
                                Ok(msg)
                            } else {
                                Err(msg)
                            }
                        } else {
                            let remote = format!("/sdcard/Android/data/{bid}/files/{folder_name}");
                            let (ok, msg) = adb::push_directory(&serial, &local, &remote);
                            if ok {
                                let (perm_ok, perm_msg) = adb::fix_permissions(&serial, &remote);
                                if perm_ok {
                                    Ok(format!("Pushed: {msg}"))
                                } else {
                                    Err(format!("Push succeeded but chmod failed: {perm_msg}"))
                                }
                            } else {
                                Err(format!("Push failed: {msg}"))
                            }
                        };
                        let _ = tx.send(AdbMsg::DeployResult(serial, label, result));
                    });
                } else {
                    self.log_cancelled(serial, "deploy folder");
                }
            }
        });

        ui.add_space(4.0);

        // ── Configured deploy directories ───────────────────────────
        if self.config.deploy_dirs.is_empty() {
            ui.add_space(4.0);
            ui.colored_label(
                egui::Color32::from_rgb(150, 150, 150),
                "No deploy directories configured. Add them to adb-ui-rs.json:",
            );
            ui.label(
                egui::RichText::new(
                    r#"  "deploy_dirs": [
    { "label": "Game Paks", "local_path": "C:/game/pack", "remote_suffix": "pack" }
  ]"#,
                )
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(120, 180, 120)),
            );
        } else {
            ui.label(egui::RichText::new("Configured Directories").strong());
            ui.add_space(2.0);

            let dirs = self.config.deploy_dirs.clone();
            let deploying = self.devices.get(serial).is_some_and(|ds| ds.deploy.running);

            for dir in &dirs {
                ui.horizontal(|ui| {
                    let exists = std::path::Path::new(&dir.local_path).exists();
                    let label_color = if exists {
                        egui::Color32::from_rgb(200, 200, 200)
                    } else {
                        egui::Color32::from_rgb(255, 100, 100)
                    };
                    ui.colored_label(label_color, &dir.label);
                    ui.colored_label(
                        egui::Color32::from_rgb(120, 120, 120),
                        format!("({} -> {})", dir.local_path, dir.remote_suffix),
                    );

                    if !deploying
                        && exists
                        && ui
                            .small_button("Push")
                            .on_hover_text("Deploy this directory")
                            .clicked()
                    {
                        if let Some(ds) = self.devices.get_mut(&serial_owned) {
                            ds.deploy.running = true;
                            ds.deploy.status = format!("Pushing {}...", dir.label);
                        }
                        let use_internal = self
                            .devices
                            .get(serial)
                            .is_some_and(|ds| matches!(ds.deploy.method, DeployMethod::Internal));
                        let serial = serial_owned.clone();
                        let bid = bundle_id.clone();
                        let tx = self.tx.clone();
                        let local = dir.local_path.clone();
                        let suffix = dir.remote_suffix.clone();
                        let label = dir.label.clone();
                        std::thread::spawn(move || {
                            let result = if use_internal {
                                let (ok, msg) =
                                    adb::deploy_via_run_as(&serial, &local, &suffix, &bid);
                                if ok {
                                    Ok(msg)
                                } else {
                                    Err(msg)
                                }
                            } else {
                                let remote = format!("/sdcard/Android/data/{bid}/files/{suffix}");
                                let (ok, msg) = adb::push_directory(&serial, &local, &remote);
                                if ok {
                                    let _ = adb::fix_permissions(&serial, &remote);
                                    Ok(format!("Pushed: {msg}"))
                                } else {
                                    Err(format!("Push failed: {msg}"))
                                }
                            };
                            let _ = tx.send(AdbMsg::DeployResult(serial, label, result));
                        });
                    }

                    if !exists {
                        ui.colored_label(egui::Color32::from_rgb(255, 80, 80), "not found");
                    }
                });
            }

            ui.add_space(2.0);

            // Deploy All button
            if !deploying
                && ui
                    .button("Deploy All")
                    .on_hover_text("Push all configured directories")
                    .clicked()
            {
                if bundle_id.trim().is_empty() {
                    self.log_skipped(serial, "deploy all", "bundle ID is not configured");
                    return;
                }
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.deploy.running = true;
                    ds.deploy.status = "Deploying all...".into();
                }
                let use_internal = self
                    .devices
                    .get(serial)
                    .is_some_and(|ds| matches!(ds.deploy.method, DeployMethod::Internal));
                let serial = serial_owned.clone();
                let bid = bundle_id.clone();
                let tx = self.tx.clone();
                let dirs = dirs.clone();
                std::thread::spawn(move || {
                    let mut ok_count = 0;
                    let mut failures = Vec::new();
                    for dir in &dirs {
                        if !std::path::Path::new(&dir.local_path).exists() {
                            failures.push(format!("{}: local path not found", dir.label));
                            continue;
                        }
                        let result = if use_internal {
                            let (ok, msg) = adb::deploy_via_run_as(
                                &serial,
                                &dir.local_path,
                                &dir.remote_suffix,
                                &bid,
                            );
                            if ok {
                                Ok(())
                            } else {
                                Err(msg)
                            }
                        } else {
                            let remote =
                                format!("/sdcard/Android/data/{bid}/files/{}", dir.remote_suffix);
                            let (ok, msg) = adb::push_directory(&serial, &dir.local_path, &remote);
                            if ok {
                                let (perm_ok, perm_msg) = adb::fix_permissions(&serial, &remote);
                                if perm_ok {
                                    Ok(())
                                } else {
                                    Err(format!("chmod failed: {perm_msg}"))
                                }
                            } else {
                                Err(msg)
                            }
                        };
                        match result {
                            Ok(()) => ok_count += 1,
                            Err(error) => {
                                failures.push(format!("{}: {error}", dir.label));
                            }
                        }
                    }
                    let fail_count = failures.len();
                    let msg = format!("Deploy complete: {ok_count} OK, {fail_count} failed");
                    let result = if fail_count == 0 {
                        Ok(msg)
                    } else {
                        Err(format!("{msg} ({})", failures.join("; ")))
                    };
                    let _ = tx.send(AdbMsg::DeployResult(serial, "Deploy All".into(), result));
                });
            }

            ui.add_space(2.0);
            ui.colored_label(
                egui::Color32::from_rgb(120, 120, 120),
                "Configure deploy directories in Settings (JSON config)",
            );
        }

        ui.add_space(8.0);
        ui.separator();

        // ── Deploy status / log ─────────────────────────────────────
        ui.label(egui::RichText::new("Deploy Log").strong());
        if let Some(ds) = self.devices.get(serial) {
            if !ds.deploy.status.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(180, 180, 180), &ds.deploy.status);
            }
        }

        // Show crash logcat section
        ui.add_space(8.0);
        ui.separator();
        ui.label(egui::RichText::new("Crash Logcat").strong());
        ui.horizontal(|ui| {
            if ui
                .button("Fetch Crashes")
                .on_hover_text("AndroidRuntime:E libc:F DEBUG:V *:F")
                .clicked()
            {
                let serial = serial_owned.clone();
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let result = adb::crash_logcat(&serial);
                    let _ = tx.send(AdbMsg::CrashLogcat(serial, result));
                });
            }
            if ui.button("Clear").clicked() {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.deploy.crash_log.clear();
                }
            }
            if self
                .devices
                .get(serial)
                .is_some_and(|ds| !ds.deploy.crash_log.is_empty())
                && ui.button("Copy").clicked()
            {
                if let Some(ds) = self.devices.get(serial) {
                    ui.ctx().copy_text(ds.deploy.crash_log.clone());
                }
            }
        });

        if let Some(ds) = self.devices.get(serial) {
            if !ds.deploy.crash_log.is_empty() {
                egui::ScrollArea::both()
                    .id_salt(format!("crash_log_{serial}"))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                        for line in ds.deploy.crash_log.lines() {
                            let color = if line.contains("FATAL")
                                || line.contains("Error")
                                || line.contains("Exception")
                            {
                                egui::Color32::from_rgb(255, 80, 80)
                            } else if line.contains("Warning") {
                                egui::Color32::from_rgb(255, 200, 50)
                            } else {
                                egui::Color32::from_rgb(200, 200, 200)
                            };
                            ui.label(egui::RichText::new(line).color(color));
                        }
                    });
            }
        }
    }
}
