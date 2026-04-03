use eframe::egui;

use super::helpers::{bytecount_lines, debug_line_color, export_single_file};
use crate::adb::{self, AdbMsg};
use crate::device::{DebugCategory, DebugRunKind};

struct DebugCommand<'a> {
    label: &'a str,
    hover: Option<&'a str>,
    shell_cmd: String,
}

impl<'a> DebugCommand<'a> {
    fn new(label: &'a str, hover: Option<&'a str>, shell_cmd: impl Into<String>) -> Self {
        Self {
            label,
            hover,
            shell_cmd: shell_cmd.into(),
        }
    }
}

impl super::App {
    pub(super) fn draw_debug_tab(&mut self, ui: &mut egui::Ui, serial: &str) {
        let serial_owned = serial.to_string();

        // Split: sidebar (left) | content (right).
        let available_rect = ui.available_rect_before_wrap();
        let sidebar_w = 130.0;
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

        let active_cat = self
            .devices
            .get(serial)
            .map_or(DebugCategory::ActivityManager, |ds| {
                ds.active_debug_category
            });

        for &cat in DebugCategory::ALL {
            let is_selected = active_cat == cat;
            let has_data = self
                .devices
                .get(serial)
                .is_some_and(|ds| ds.debug_outputs.contains_key(&cat));
            let loading = self
                .devices
                .get(serial)
                .is_some_and(|ds| ds.debug_loading.contains(&cat));

            let (indicator, ind_color) = if loading {
                ("~", egui::Color32::from_rgb(255, 200, 50))
            } else if has_data {
                ("*", egui::Color32::from_rgb(100, 180, 255))
            } else {
                (" ", egui::Color32::from_rgb(120, 120, 120))
            };

            let text = format!("{indicator} {}", cat.label());
            let color = if is_selected {
                egui::Color32::WHITE
            } else {
                ind_color
            };

            if sidebar_ui
                .selectable_label(
                    is_selected,
                    egui::RichText::new(text).color(color).size(12.0),
                )
                .clicked()
            {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.active_debug_category = cat;
                }
            }
        }

        // --- Content area ---
        let mut content_ui = ui.new_child(egui::UiBuilder::new().max_rect(content_rect));
        content_ui.set_clip_rect(content_rect);

        match active_cat {
            DebugCategory::ActivityManager => {
                self.draw_debug_activity_manager(&mut content_ui, serial);
            }
            DebugCategory::DumpsysServices => self.draw_debug_dumpsys(&mut content_ui, serial),
            DebugCategory::SystemTrace => self.draw_debug_atrace(&mut content_ui, serial),
            DebugCategory::Simpleperf => self.draw_debug_simpleperf(&mut content_ui, serial),
            DebugCategory::Strace => self.draw_debug_strace(&mut content_ui, serial),
            DebugCategory::MemoryAnalysis => self.draw_debug_memory(&mut content_ui, serial),
            DebugCategory::GpuGraphics => self.draw_debug_gpu(&mut content_ui, serial),
            DebugCategory::NetworkDiag => self.draw_debug_network(&mut content_ui, serial),
        }
    }

    /// Common toolbar + output view for a debug category.
    pub(super) fn draw_debug_output_area(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        cat: DebugCategory,
    ) {
        let serial_owned = serial.to_string();
        let is_loading = self
            .devices
            .get(serial)
            .is_some_and(|ds| ds.debug_loading.contains(&cat));

        // Bottom toolbar: filter + status.
        ui.horizontal(|ui| {
            if is_loading {
                ui.spinner();
                ui.label("Running...");
            }

            if ui.button("Clear").clicked() {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.debug_outputs.remove(&cat);
                }
            }

            if ui.button("Copy").clicked() {
                if let Some(ds) = self.devices.get(serial) {
                    if let Some(text) = ds.debug_outputs.get(&cat) {
                        ui.ctx().copy_text(text.clone());
                    }
                }
            }

            if ui.button("Export...").clicked() {
                if let Some(ds) = self.devices.get(serial) {
                    if let Some(text) = ds.debug_outputs.get(&cat) {
                        let fname =
                            format!("{}_{}.txt", cat.label().replace(['/', ' '], "_"), serial);
                        let _ = export_single_file(&fname, text);
                    }
                }
            }

            ui.separator();
            ui.label("Filter:");
            if let Some(ds) = self.devices.get_mut(&serial_owned) {
                ui.text_edit_singleline(&mut ds.debug_filter);
            }

            if let Some(ds) = self.devices.get(serial) {
                let count = ds.debug_outputs.get(&cat).map_or(0, |t| bytecount_lines(t));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{count} lines"));
                });
            }
        });

        ui.separator();

        // Output area.
        if let Some(ds) = self.devices.get(serial) {
            if let Some(text) = ds.debug_outputs.get(&cat) {
                let filter_lower = ds.debug_filter.to_lowercase();
                egui::ScrollArea::both()
                    .id_salt(format!("debug_{}_{serial}", cat.index()))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                        for line in text.lines() {
                            if !filter_lower.is_empty()
                                && !line.to_lowercase().contains(&filter_lower)
                            {
                                continue;
                            }
                            let color = debug_line_color(line);
                            ui.label(egui::RichText::new(line).color(color));
                        }
                    });
            } else if !is_loading {
                ui.centered_and_justified(|ui| {
                    ui.label("Run a command above to see output.");
                });
            }
        } else if !is_loading {
            ui.centered_and_justified(|ui| {
                ui.label("Run a command above to see output.");
            });
        }
    }

    /// Send a debug shell command asynchronously.
    pub(super) fn run_debug_cmd(&mut self, serial: &str, cat: DebugCategory, shell_cmd: String) {
        if let Some(ds) = self.devices.get_mut(serial) {
            ds.debug_loading.insert(cat);
        }
        let serial = serial.to_string();
        let tx = self.tx.clone();
        let idx = cat.index();
        std::thread::spawn(move || {
            let result = adb::run_debug_shell(&serial, &shell_cmd);
            let _ = tx.send(AdbMsg::DebugOutput(serial, idx, result));
        });
    }

    fn draw_debug_command_row(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        cat: DebugCategory,
        commands: Vec<DebugCommand<'_>>,
    ) {
        ui.horizontal(|ui| {
            for command in commands {
                let mut button = ui.button(command.label);
                if let Some(hover) = command.hover {
                    button = button.on_hover_text(hover);
                }
                if button.clicked() {
                    self.run_debug_cmd(serial, cat, command.shell_cmd);
                }
            }
        });
    }

    // ─── Debug: Activity Manager ────────────────────────────────────────────

    pub(super) fn draw_debug_activity_manager(&mut self, ui: &mut egui::Ui, serial: &str) {
        let cat = DebugCategory::ActivityManager;
        let bundle_id = self.config.bundle_id.clone();
        let bundle_id_shell = adb::shell_quote(&bundle_id);

        ui.label(egui::RichText::new("Activity Manager (am / dumpsys activity)").strong());
        ui.add_space(2.0);

        ui.horizontal(|ui| {
            if ui
                .button("Activities")
                .on_hover_text("Running activities")
                .clicked()
            {
                self.run_debug_cmd(serial, cat, "dumpsys activity activities".into());
            }
            if ui
                .button("App Activities")
                .on_hover_text("Activities for configured app")
                .clicked()
            {
                self.run_debug_cmd(
                    serial,
                    cat,
                    format!("dumpsys activity activities | grep -F -A 20 -- {bundle_id_shell}"),
                );
            }
            if ui
                .button("Services")
                .on_hover_text("Running services")
                .clicked()
            {
                self.run_debug_cmd(serial, cat, "dumpsys activity services".into());
            }
            if ui.button("App Services").clicked() {
                self.run_debug_cmd(
                    serial,
                    cat,
                    format!("dumpsys activity services | grep -F -A 10 -- {bundle_id_shell}"),
                );
            }
        });
        ui.horizontal(|ui| {
            if ui
                .button("Broadcasts")
                .on_hover_text("Pending broadcasts")
                .clicked()
            {
                self.run_debug_cmd(
                    serial,
                    cat,
                    "dumpsys activity broadcasts | head -100".into(),
                );
            }
            if ui
                .button("Recent Tasks")
                .on_hover_text("Recent task list")
                .clicked()
            {
                self.run_debug_cmd(serial, cat, "dumpsys activity recents".into());
            }
            if ui
                .button("Processes")
                .on_hover_text("All activity processes")
                .clicked()
            {
                self.run_debug_cmd(serial, cat, "dumpsys activity processes".into());
            }
            if ui
                .button("OOM Adj")
                .on_hover_text("OOM adjustment levels")
                .clicked()
            {
                self.run_debug_cmd(serial, cat, "dumpsys activity oom".into());
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Providers").on_hover_text("Content providers").clicked() {
                self.run_debug_cmd(serial, cat, "dumpsys activity providers".into());
            }
            if ui.button("Intents").on_hover_text("Pending intents").clicked() {
                self.run_debug_cmd(
                    serial,
                    cat,
                    "dumpsys activity intents | head -200".into(),
                );
            }
            if ui.button("App Focus").on_hover_text("Currently focused activity").clicked() {
                self.run_debug_cmd(
                    serial,
                    cat,
                    "dumpsys activity activities | grep -E 'mResumedActivity|mFocusedActivity|mLastPausedActivity'".into(),
                );
            }
            if ui.button("Task Stack").on_hover_text("Full task/activity stack").clicked() {
                self.run_debug_cmd(serial, cat, "dumpsys activity activities | grep -B1 -A5 'TaskRecord\\|ActivityRecord'".into());
            }
        });

        ui.separator();
        self.draw_debug_output_area(ui, serial, cat);
    }

    // ─── Debug: Dumpsys Services ────────────────────────────────────────────

    pub(super) fn draw_debug_dumpsys(&mut self, ui: &mut egui::Ui, serial: &str) {
        let cat = DebugCategory::DumpsysServices;
        let serial_owned = serial.to_string();

        ui.label(egui::RichText::new("Dumpsys Service Inspector").strong());
        ui.add_space(2.0);

        for services in [
            &[
                "meminfo",
                "cpuinfo",
                "battery",
                "power",
                "alarm",
                "notification",
            ][..],
            &[
                "display",
                "audio",
                "wifi",
                "connectivity",
                "telephony.registry",
                "usagestats",
            ][..],
            &[
                "window",
                "input",
                "SurfaceFlinger",
                "package",
                "activity",
                "statusbar",
            ][..],
        ] {
            self.draw_dumpsys_quick_access_row(ui, serial, cat, services);
        }
        ui.add_space(4.0);
        self.draw_dumpsys_service_picker(ui, serial, &serial_owned, cat);

        ui.separator();
        self.draw_debug_output_area(ui, serial, cat);
    }

    fn draw_dumpsys_quick_access_row(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        cat: DebugCategory,
        services: &[&str],
    ) {
        ui.horizontal(|ui| {
            for service in services {
                if ui.small_button(*service).clicked() {
                    self.run_debug_cmd(serial, cat, format!("dumpsys {service}"));
                }
            }
        });
    }

    fn draw_dumpsys_service_picker(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        cat: DebugCategory,
    ) {
        ui.horizontal(|ui| {
            ui.label("Service:");
            let has_list = self
                .devices
                .get(serial)
                .is_some_and(|ds| !ds.dumpsys_services_list.is_empty());

            if has_list {
                let current = self
                    .devices
                    .get(serial)
                    .map(|ds| ds.dumpsys_service.clone())
                    .unwrap_or_default();
                let display = if current.is_empty() {
                    "Select..."
                } else {
                    &current
                };
                egui::ComboBox::from_id_salt(format!("dumpsys_svc_{serial}"))
                    .selected_text(display)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        if let Some(ds) = self.devices.get_mut(serial_owned) {
                            for service in ds.dumpsys_services_list.clone() {
                                ui.selectable_value(
                                    &mut ds.dumpsys_service,
                                    service.clone(),
                                    &service,
                                );
                            }
                        }
                    });
            } else if let Some(ds) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [200.0, 18.0],
                    egui::TextEdit::singleline(&mut ds.dumpsys_service).hint_text("service name"),
                );
            }

            if ui.button("Run").clicked() {
                let service = self
                    .devices
                    .get(serial)
                    .map(|ds| ds.dumpsys_service.clone())
                    .unwrap_or_default();
                let service = adb::sanitize_shell_arg(&service);
                if !service.is_empty() {
                    self.run_debug_cmd(serial, cat, format!("dumpsys {service}"));
                }
            }

            if ui.button("List Services").clicked() {
                let serial = serial_owned.to_string();
                let tx = self.tx.clone();
                std::thread::spawn(move || match adb::list_dumpsys_services(&serial) {
                    Ok(services) => {
                        let _ = tx.send(AdbMsg::DumpsysServiceList(serial, services));
                    }
                    Err(error) => {
                        let _ = tx.send(AdbMsg::DeviceActionResult(
                            serial,
                            format!("List services failed: {error}"),
                        ));
                    }
                });
            }
        });
    }

    // ─── Debug: System Trace (atrace) ───────────────────────────────────────

    pub(super) fn draw_debug_atrace(&mut self, ui: &mut egui::Ui, serial: &str) {
        let cat = DebugCategory::SystemTrace;
        let serial_owned = serial.to_string();

        ui.label(egui::RichText::new("System Trace (atrace / systrace)").strong());
        ui.add_space(2.0);

        let is_running = self
            .devices
            .get(serial)
            .is_some_and(|ds| ds.debug_runs.atrace);

        ui.horizontal(|ui| {
            ui.label("Duration (s):");
            if let Some(ds) = self.devices.get_mut(&serial_owned) {
                ui.add_sized(
                    [40.0, 18.0],
                    egui::TextEdit::singleline(&mut ds.atrace_duration),
                );
            }

            if is_running {
                ui.spinner();
                ui.label("Tracing...");
            } else if ui.button("Start Trace").clicked() {
                let raw_duration = self
                    .devices
                    .get(serial)
                    .map_or_else(String::new, |ds| ds.atrace_duration.clone());
                let duration =
                    self.parse_u32_input_or_log(serial, "atrace duration", &raw_duration, 5);
                let cats: Vec<String> = self
                    .devices
                    .get(serial)
                    .map(|ds| ds.atrace_categories.clone())
                    .unwrap_or_default();

                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.debug_runs.atrace = true;
                    ds.debug_loading.insert(cat);
                }
                let serial = serial_owned.clone();
                let tx = self.tx.clone();
                let idx = cat.index();
                std::thread::spawn(move || {
                    let result = adb::run_atrace(&serial, &cats, duration);
                    let _ = tx.send(AdbMsg::DebugOutput(serial, idx, result));
                });
            }

            if !is_running && ui.button("Quick (3s default)").clicked() {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.debug_runs.atrace = true;
                    ds.debug_loading.insert(cat);
                }
                let serial = serial_owned.clone();
                let tx = self.tx.clone();
                let idx = cat.index();
                std::thread::spawn(move || {
                    let result = adb::run_atrace(&serial, &[], 3);
                    let _ = tx.send(AdbMsg::DebugOutput(serial, idx, result));
                });
            }

            if ui.button("Load Categories").clicked() {
                let serial = serial_owned.clone();
                let tx = self.tx.clone();
                std::thread::spawn(move || match adb::list_atrace_categories(&serial) {
                    Ok(cats) => {
                        let _ = tx.send(AdbMsg::AtraceCategories(serial, cats));
                    }
                    Err(e) => {
                        let _ = tx.send(AdbMsg::DeviceActionResult(
                            serial,
                            format!("Load atrace categories failed: {e}"),
                        ));
                    }
                });
            }
        });

        // Category selection (if loaded).
        let has_cats = self
            .devices
            .get(serial)
            .is_some_and(|ds| !ds.atrace_available_cats.is_empty());
        if has_cats {
            ui.horizontal_wrapped(|ui| {
                ui.label("Categories:");
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    for cat_name in ds.atrace_available_cats.clone() {
                        let mut on = ds.atrace_categories.contains(&cat_name);
                        if ui.checkbox(&mut on, &cat_name).changed() {
                            if on {
                                ds.atrace_categories.push(cat_name);
                            } else {
                                ds.atrace_categories.retain(|c| c != &cat_name);
                            }
                        }
                    }
                }
            });
        }

        // Reset running state when output arrives.
        if let Some(ds) = self.devices.get_mut(&serial_owned) {
            if !ds.debug_loading.contains(&cat) {
                ds.debug_runs.atrace = false;
            }
        }

        ui.separator();
        self.draw_debug_output_area(ui, serial, cat);
    }

    // ─── Debug: Simpleperf ──────────────────────────────────────────────────

    pub(super) fn draw_debug_simpleperf(&mut self, ui: &mut egui::Ui, serial: &str) {
        let cat = DebugCategory::Simpleperf;
        let serial_owned = serial.to_string();
        let bundle_id = self.config.bundle_id.clone();

        ui.label(egui::RichText::new("Simpleperf CPU Profiler").strong());
        ui.add_space(2.0);

        let is_running = self
            .devices
            .get(serial)
            .is_some_and(|ds| ds.debug_runs.simpleperf);
        self.draw_simpleperf_inputs(ui, serial, &serial_owned);
        self.sync_debug_run_flag(&serial_owned, cat);
        self.show_debug_spinner(ui, serial, cat, "Profiling...");
        self.draw_simpleperf_actions(ui, serial, &serial_owned, cat, &bundle_id, is_running);
        if !is_running {
            self.draw_debug_command_row(
                ui,
                serial,
                cat,
                vec![
                    DebugCommand::new("List Events", None, "simpleperf list 2>&1 | head -80"),
                    DebugCommand::new("List HW PMU", None, "simpleperf list hw 2>&1 || echo 'N/A'"),
                ],
            );
        }

        ui.separator();
        self.draw_debug_output_area(ui, serial, cat);
    }

    // ─── Debug: Strace ──────────────────────────────────────────────────────

    pub(super) fn draw_debug_strace(&mut self, ui: &mut egui::Ui, serial: &str) {
        let cat = DebugCategory::Strace;
        let serial_owned = serial.to_string();
        let bundle_id = self.config.bundle_id.clone();

        ui.label(egui::RichText::new("Strace (Syscall Tracer)").strong());
        ui.colored_label(
            egui::Color32::from_rgb(180, 180, 100),
            "Requires root or debuggable app",
        );
        ui.add_space(2.0);

        let is_running = self
            .devices
            .get(serial)
            .is_some_and(|ds| ds.debug_runs.strace);
        self.draw_strace_inputs(ui, serial, &serial_owned, cat, &bundle_id, is_running);
        self.sync_debug_run_flag(&serial_owned, cat);
        self.show_debug_spinner(ui, serial, cat, "Tracing...");

        ui.separator();
        self.draw_debug_output_area(ui, serial, cat);
    }

    // ─── Debug: Memory Analysis ─────────────────────────────────────────────

    fn draw_simpleperf_inputs(&mut self, ui: &mut egui::Ui, serial: &str, serial_owned: &str) {
        ui.horizontal(|ui| {
            ui.label("Duration (s):");
            if let Some(ds) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [40.0, 18.0],
                    egui::TextEdit::singleline(&mut ds.simpleperf_duration),
                );
            }
            ui.label("Event:");
            if let Some(ds) = self.devices.get_mut(serial_owned) {
                egui::ComboBox::from_id_salt(format!("simpleperf_ev_{serial}"))
                    .selected_text(&ds.simpleperf_event)
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        for event in &[
                            "cpu-cycles",
                            "instructions",
                            "cache-misses",
                            "branch-misses",
                            "task-clock",
                            "context-switches",
                            "page-faults",
                        ] {
                            ui.selectable_value(
                                &mut ds.simpleperf_event,
                                (*event).to_string(),
                                *event,
                            );
                        }
                    });
            }
        });
    }

    fn draw_simpleperf_actions(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        cat: DebugCategory,
        bundle_id: &str,
        is_running: bool,
    ) {
        ui.horizontal(|ui| {
            if !is_running && ui.button("Stat (system-wide)").clicked() {
                let (duration, event) = self.simpleperf_config(serial);
                self.start_simpleperf_system(serial_owned, cat, duration, event);
            }
            if !is_running && ui.button("Stat (app)").clicked() {
                let (duration, event) = self.simpleperf_config(serial);
                self.start_simpleperf_app(serial_owned, cat, duration, event, bundle_id);
            }
            if !is_running
                && ui
                    .button("Record + Report")
                    .on_hover_text("Record then display call graph report")
                    .clicked()
            {
                let (duration, event) = self.simpleperf_config(serial);
                self.start_simpleperf_record(serial_owned, cat, duration, event);
            }
        });
    }

    fn simpleperf_config(&mut self, serial: &str) -> (u32, String) {
        let raw_duration = self
            .devices
            .get(serial)
            .map_or_else(String::new, |ds| ds.simpleperf_duration.clone());
        let duration = self.parse_u32_input_or_log(serial, "simpleperf duration", &raw_duration, 5);
        let event = self
            .devices
            .get(serial)
            .map_or_else(|| "cpu-cycles".into(), |ds| ds.simpleperf_event.clone());
        (duration, event)
    }

    fn start_simpleperf_system(
        &mut self,
        serial_owned: &str,
        cat: DebugCategory,
        duration: u32,
        event: String,
    ) {
        self.mark_debug_job_running(serial_owned, cat, DebugRunKind::Simpleperf);
        let serial = serial_owned.to_string();
        let tx = self.tx.clone();
        let idx = cat.index();
        std::thread::spawn(move || {
            let result = adb::run_simpleperf_stat(&serial, None, duration, &event);
            let _ = tx.send(AdbMsg::DebugOutput(serial, idx, result));
        });
    }

    fn start_simpleperf_app(
        &mut self,
        serial_owned: &str,
        cat: DebugCategory,
        duration: u32,
        event: String,
        bundle_id: &str,
    ) {
        self.mark_debug_job_running(serial_owned, cat, DebugRunKind::Simpleperf);
        let serial = serial_owned.to_string();
        let tx = self.tx.clone();
        let idx = cat.index();
        let bundle_id = bundle_id.to_string();
        std::thread::spawn(move || {
            let result = match adb::run_debug_shell(
                &serial,
                &format!("pidof -s {}", adb::shell_quote(&bundle_id)),
            ) {
                Ok(pid_output) => adb::validate_pid(pid_output.trim()).map_or_else(
                    |_| Err(format!("App '{bundle_id}' not running - cannot find PID")),
                    |pid| adb::run_simpleperf_stat(&serial, Some(pid), duration, &event),
                ),
                Err(error) => Err(format!("PID lookup failed: {error}")),
            };
            let _ = tx.send(AdbMsg::DebugOutput(serial, idx, result));
        });
    }

    fn start_simpleperf_record(
        &mut self,
        serial_owned: &str,
        cat: DebugCategory,
        duration: u32,
        event: String,
    ) {
        self.mark_debug_job_running(serial_owned, cat, DebugRunKind::Simpleperf);
        let serial = serial_owned.to_string();
        let tx = self.tx.clone();
        let idx = cat.index();
        std::thread::spawn(move || {
            let result = adb::run_simpleperf_record(&serial, None, duration, &event);
            let _ = tx.send(AdbMsg::DebugOutput(serial, idx, result));
        });
    }

    fn draw_strace_inputs(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        cat: DebugCategory,
        bundle_id: &str,
        is_running: bool,
    ) {
        ui.horizontal(|ui| {
            ui.label("PID:");
            if let Some(ds) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [60.0, 18.0],
                    egui::TextEdit::singleline(&mut ds.strace_pid).hint_text("PID"),
                );
            }
            ui.label("Duration (s):");
            if let Some(ds) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [40.0, 18.0],
                    egui::TextEdit::singleline(&mut ds.strace_duration),
                );
            }
            if !is_running && ui.button("Trace PID").clicked() {
                self.start_strace_for_pid(serial, serial_owned, cat);
            }
            if !is_running
                && ui
                    .button("Trace App")
                    .on_hover_text("Find app PID and trace")
                    .clicked()
            {
                self.start_strace_for_app(serial, serial_owned, cat, bundle_id);
            }
            if !is_running && ui.button("Get App PID").clicked() {
                self.run_debug_cmd(
                    serial,
                    cat,
                    format!(
                        "pidof -s {} && echo '' || echo 'App not running'",
                        adb::shell_quote(bundle_id)
                    ),
                );
            }
        });
    }

    fn start_strace_for_pid(&mut self, serial: &str, serial_owned: &str, cat: DebugCategory) {
        let pid_input = self
            .devices
            .get(serial)
            .map(|ds| ds.strace_pid.clone())
            .unwrap_or_default();
        let pid = match adb::validate_pid(&pid_input) {
            Ok(pid) => pid.to_string(),
            Err(error) => {
                self.publish_debug_error(serial, cat, invalid_pid_message(&pid_input, error));
                return;
            }
        };

        let duration = self.strace_duration(serial);
        self.mark_debug_job_running(serial_owned, cat, DebugRunKind::Strace);
        let serial = serial_owned.to_string();
        let tx = self.tx.clone();
        let idx = cat.index();
        std::thread::spawn(move || {
            let result = adb::run_strace(&serial, &pid, duration);
            let _ = tx.send(AdbMsg::DebugOutput(serial, idx, result));
        });
    }

    fn start_strace_for_app(
        &mut self,
        serial: &str,
        serial_owned: &str,
        cat: DebugCategory,
        bundle_id: &str,
    ) {
        let duration = self.strace_duration(serial);
        self.mark_debug_job_running(serial_owned, cat, DebugRunKind::Strace);
        let serial = serial_owned.to_string();
        let tx = self.tx.clone();
        let idx = cat.index();
        let bundle_id = bundle_id.to_string();
        std::thread::spawn(move || {
            let result = match adb::run_debug_shell(
                &serial,
                &format!("pidof -s {}", adb::shell_quote(&bundle_id)),
            ) {
                Ok(pid_output) => adb::validate_pid(pid_output.trim()).map_or_else(
                    |_| Err(format!("App '{bundle_id}' not running - cannot find PID")),
                    |pid| adb::run_strace(&serial, pid, duration),
                ),
                Err(error) => Err(format!("PID lookup failed: {error}")),
            };
            let _ = tx.send(AdbMsg::DebugOutput(serial, idx, result));
        });
    }

    fn strace_duration(&mut self, serial: &str) -> u32 {
        let raw_duration = self
            .devices
            .get(serial)
            .map_or_else(String::new, |ds| ds.strace_duration.clone());
        self.parse_u32_input_or_log(serial, "strace duration", &raw_duration, 5)
    }

    fn sync_debug_run_flag(&mut self, serial_owned: &str, cat: DebugCategory) {
        if let Some(ds) = self.devices.get_mut(serial_owned) {
            if !ds.debug_loading.contains(&cat) {
                match cat.run_kind() {
                    Some(DebugRunKind::Atrace) => ds.debug_runs.atrace = false,
                    Some(DebugRunKind::Simpleperf) => ds.debug_runs.simpleperf = false,
                    Some(DebugRunKind::Strace) => ds.debug_runs.strace = false,
                    None => {}
                }
            }
        }
    }

    fn show_debug_spinner(&self, ui: &mut egui::Ui, serial: &str, cat: DebugCategory, label: &str) {
        let is_running = self
            .devices
            .get(serial)
            .is_some_and(|ds| match cat.run_kind() {
                Some(DebugRunKind::Atrace) => ds.debug_runs.atrace,
                Some(DebugRunKind::Simpleperf) => ds.debug_runs.simpleperf,
                Some(DebugRunKind::Strace) => ds.debug_runs.strace,
                None => false,
            });
        if is_running {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(label);
            });
        }
    }

    fn mark_debug_job_running(
        &mut self,
        serial_owned: &str,
        cat: DebugCategory,
        run_kind: DebugRunKind,
    ) {
        if let Some(ds) = self.devices.get_mut(serial_owned) {
            match run_kind {
                DebugRunKind::Atrace => ds.debug_runs.atrace = true,
                DebugRunKind::Simpleperf => ds.debug_runs.simpleperf = true,
                DebugRunKind::Strace => ds.debug_runs.strace = true,
            }
            ds.debug_loading.insert(cat);
        }
    }

    fn publish_debug_error(&self, serial: &str, cat: DebugCategory, message: String) {
        let _ = self.tx.send(AdbMsg::DebugOutput(
            serial.to_string(),
            cat.index(),
            Err(message),
        ));
    }

    pub(super) fn draw_debug_memory(&mut self, ui: &mut egui::Ui, serial: &str) {
        let cat = DebugCategory::MemoryAnalysis;
        let bundle_id = self.config.bundle_id.clone();
        let bundle_id_shell = adb::shell_quote(&bundle_id);

        ui.label(egui::RichText::new("Memory Analysis").strong());
        ui.add_space(2.0);

        ui.horizontal(|ui| {
            if ui
                .button("System Meminfo")
                .on_hover_text("/proc/meminfo")
                .clicked()
            {
                self.run_debug_cmd(serial, cat, "cat /proc/meminfo".into());
            }
            if ui
                .button("dumpsys meminfo")
                .on_hover_text("System-wide memory summary")
                .clicked()
            {
                self.run_debug_cmd(serial, cat, "dumpsys meminfo".into());
            }
            if ui
                .button("App Meminfo")
                .on_hover_text("Memory for configured app")
                .clicked()
            {
                self.run_debug_cmd(serial, cat, format!("dumpsys meminfo {bundle_id_shell}"));
            }
            if ui
                .button("vmstat")
                .on_hover_text("Virtual memory stats snapshot")
                .clicked()
            {
                self.run_debug_cmd(
                    serial,
                    cat,
                    "vmstat 1 5 2>/dev/null || cat /proc/vmstat | head -30".into(),
                );
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Procrank").on_hover_text("Process memory ranking").clicked() {
                self.run_debug_cmd(
                    serial,
                    cat,
                    "procrank 2>/dev/null || echo 'procrank not available (try dumpsys meminfo instead)'"
                        .into(),
                );
            }
            if ui.button("Showmap (app)").on_hover_text("Detailed memory map for app").clicked() {
                self.run_debug_cmd(
                    serial,
                    cat,
                    format!(
                        "PID=$(pidof -s {bundle_id_shell}); if [ -n \"$PID\" ]; then showmap $PID 2>/dev/null || cat /proc/$PID/smaps_rollup 2>/dev/null || echo 'showmap not available'; else echo 'App not running'; fi"
                    ),
                );
            }
            if ui.button("ION/DMA heaps").on_hover_text("GPU/DMA buffer allocations").clicked() {
                self.run_debug_cmd(
                    serial,
                    cat,
                    "cat /sys/kernel/debug/ion/heaps/* 2>/dev/null; \
                     cat /sys/kernel/debug/dma_buf/bufinfo 2>/dev/null; \
                     dumpsys gpu 2>/dev/null | head -50; \
                     echo '--- done ---'"
                        .into(),
                );
            }
            if ui.button("LMK Stats").on_hover_text("Low memory killer stats").clicked() {
                self.run_debug_cmd(
                    serial,
                    cat,
                    "dumpsys activity lmk 2>/dev/null; \
                     echo '--- /proc/pressure/memory ---'; \
                     cat /proc/pressure/memory 2>/dev/null || echo 'N/A'"
                        .into(),
                );
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Zram Stats").clicked() {
                self.run_debug_cmd(
                    serial,
                    cat,
                    "cat /sys/block/zram0/mm_stat 2>/dev/null; \
                     echo ''; cat /sys/block/zram0/stat 2>/dev/null; \
                     echo '--- Swap info ---'; cat /proc/swaps 2>/dev/null"
                        .into(),
                );
            }
            if ui.button("OOM Score").on_hover_text("OOM scores for all processes").clicked() {
                self.run_debug_cmd(
                    serial,
                    cat,
                    "for p in /proc/[0-9]*/oom_score_adj; do echo \"$(cat $p 2>/dev/null) $(cat $(dirname $p)/cmdline 2>/dev/null | tr '\\0' ' ')\"; done 2>/dev/null | sort -rn | head -30"
                        .into(),
                );
            }
        });

        ui.separator();
        self.draw_debug_output_area(ui, serial, cat);
    }

    // ─── Debug: GPU / Graphics ──────────────────────────────────────────────

    pub(super) fn draw_debug_gpu(&mut self, ui: &mut egui::Ui, serial: &str) {
        let cat = DebugCategory::GpuGraphics;
        let bundle_id = self.config.bundle_id.clone();
        let bundle_id_shell = adb::shell_quote(&bundle_id);

        ui.label(egui::RichText::new("GPU & Graphics Debugging").strong());
        ui.add_space(2.0);

        self.draw_debug_command_row(
            ui,
            serial,
            cat,
            vec![
                DebugCommand::new(
                    "gfxinfo",
                    Some("Frame rendering stats for app"),
                    format!("dumpsys gfxinfo {bundle_id_shell}"),
                ),
                DebugCommand::new(
                    "gfxinfo (reset)",
                    Some("Reset and get fresh stats"),
                    format!("dumpsys gfxinfo {bundle_id_shell} reset"),
                ),
                DebugCommand::new(
                    "gfxinfo (framestats)",
                    Some("Per-frame timestamps"),
                    format!("dumpsys gfxinfo {bundle_id_shell} framestats"),
                ),
                DebugCommand::new(
                    "SurfaceFlinger",
                    Some("Compositor info"),
                    "dumpsys SurfaceFlinger",
                ),
            ],
        );
        self.draw_debug_command_row(
            ui,
            serial,
            cat,
            vec![
                DebugCommand::new(
                    "Layer Latency",
                    Some("Per-layer frame latency"),
                    "dumpsys SurfaceFlinger --latency",
                ),
                DebugCommand::new(
                    "GPU Info",
                    Some("GPU hardware/driver info"),
                    "dumpsys gpu 2>/dev/null; \
                     echo '--- GPU props ---'; \
                     getprop | grep -i gpu; \
                     echo '--- EGL ---'; \
                     dumpsys SurfaceFlinger --dump-static 2>/dev/null | grep -A5 'EGL\\|GLES'",
                ),
                DebugCommand::new(
                    "HWUI Debug",
                    Some("Hardware UI renderer debug"),
                    format!(
                        "dumpsys gfxinfo {bundle_id_shell} | grep -A 20 'Profile data\\|Jank\\|Draw\\|Process\\|Execute'"
                    ),
                ),
                DebugCommand::new(
                    "Display Info",
                    Some("Display/screen configuration"),
                    "dumpsys display | head -80",
                ),
            ],
        );
        self.draw_debug_command_row(
            ui,
            serial,
            cat,
            vec![
                DebugCommand::new(
                    "Choreographer",
                    Some("Frame scheduling stats"),
                    "dumpsys SurfaceFlinger --list 2>/dev/null; \
                     echo '---'; \
                     service call SurfaceFlinger 1013 2>/dev/null; \
                     echo '--- Vsync ---'; \
                     dumpsys SurfaceFlinger --vsync 2>/dev/null | head -20",
                ),
                DebugCommand::new(
                    "Render Stages",
                    Some("Enable GPU profiling bars"),
                    "setprop debug.hwui.profile true; \
                     setprop debug.hwui.overdraw show; \
                     echo 'GPU profiling and overdraw visualization enabled. \
                     Restart the app to see the effect. \
                     Disable with: setprop debug.hwui.profile false'",
                ),
            ],
        );

        ui.separator();
        self.draw_debug_output_area(ui, serial, cat);
    }

    // ─── Debug: Network ─────────────────────────────────────────────────────

    pub(super) fn draw_debug_network(&mut self, ui: &mut egui::Ui, serial: &str) {
        let cat = DebugCategory::NetworkDiag;

        ui.label(egui::RichText::new("Network Diagnostics").strong());
        ui.add_space(2.0);

        self.draw_debug_command_row(
            ui,
            serial,
            cat,
            vec![
                DebugCommand::new(
                    "Connectivity",
                    Some("dumpsys connectivity"),
                    "dumpsys connectivity | head -100",
                ),
                DebugCommand::new(
                    "Network Stats",
                    Some("Data usage stats"),
                    "dumpsys netstats | head -100",
                ),
                DebugCommand::new("WiFi Info", Some("Detailed WiFi state"), "dumpsys wifi"),
                DebugCommand::new(
                    "IP Config",
                    Some("ip addr + route"),
                    "echo '=== Interfaces ==='; ip addr show; \
                     echo '\\n=== Routes ==='; ip route show; \
                     echo '\\n=== DNS ==='; getprop net.dns1; getprop net.dns2; \
                     echo '\\n=== ARP ==='; ip neigh show",
                ),
            ],
        );
        self.draw_debug_command_row(
            ui,
            serial,
            cat,
            vec![
                DebugCommand::new(
                    "Netstat",
                    Some("Open sockets"),
                    "netstat -tlnp 2>/dev/null || ss -tlnp 2>/dev/null || echo 'Not available'",
                ),
                DebugCommand::new(
                    "TCP Stats",
                    Some("/proc/net/tcp stats"),
                    "cat /proc/net/tcp | head -30; echo ''; cat /proc/net/tcp6 | head -20",
                ),
                DebugCommand::new(
                    "iptables",
                    Some("Firewall rules (needs root)"),
                    "iptables -L -n -v 2>/dev/null || echo 'iptables not available (needs root)'",
                ),
                DebugCommand::new("Ping Test", Some("Ping 8.8.8.8"), "ping -c 4 8.8.8.8 2>&1"),
            ],
        );
        self.draw_debug_command_row(
            ui,
            serial,
            cat,
            vec![
                DebugCommand::new(
                    "Telephony",
                    Some("Cellular/telephony info"),
                    "dumpsys telephony.registry | head -60",
                ),
                DebugCommand::new(
                    "VPN",
                    Some("VPN status"),
                    "dumpsys connectivity | grep -A 5 VPN; \
                     echo '---'; \
                     ip tun show 2>/dev/null || echo 'no tunnels'",
                ),
                DebugCommand::new(
                    "Bandwidth",
                    Some("Network interface stats"),
                    "cat /proc/net/dev | column -t; \
                     echo '\\n=== Traffic stats ==='; \
                     dumpsys netstats --uid | head -40",
                ),
            ],
        );

        ui.separator();
        self.draw_debug_output_area(ui, serial, cat);
    }
}

fn invalid_pid_message(pid_input: &str, error: &str) -> String {
    let trimmed = pid_input.trim();
    if trimmed.is_empty() {
        error.to_string()
    } else {
        format!("{error}: '{trimmed}'")
    }
}
