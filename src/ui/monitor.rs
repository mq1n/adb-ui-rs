use eframe::egui;

use super::helpers::{bytecount_lines, debug_line_color, export_single_file};
use crate::adb::{self, AdbMsg};
use crate::device::MonitorCategory;

struct MonitorCommand<'a> {
    label: &'a str,
    hover: Option<&'a str>,
    shell_cmd: String,
}

impl<'a> MonitorCommand<'a> {
    fn new(label: &'a str, hover: Option<&'a str>, shell_cmd: impl Into<String>) -> Self {
        Self {
            label,
            hover,
            shell_cmd: shell_cmd.into(),
        }
    }
}

impl super::App {
    pub(super) fn draw_monitor_tab(&mut self, ui: &mut egui::Ui, serial: &str) {
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
            .map_or(MonitorCategory::Processes, |ds| ds.active_monitor_category);

        for &cat in MonitorCategory::ALL {
            let is_selected = active_cat == cat;
            let has_data = self
                .devices
                .get(serial)
                .is_some_and(|ds| ds.monitor_outputs.contains_key(&cat));
            let loading = self
                .devices
                .get(serial)
                .is_some_and(|ds| ds.monitor_loading.contains(&cat));

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
                    ds.active_monitor_category = cat;
                }
            }
        }

        // --- Content area ---
        let mut content_ui = ui.new_child(egui::UiBuilder::new().max_rect(content_rect));
        content_ui.set_clip_rect(content_rect);

        match active_cat {
            MonitorCategory::Processes => self.draw_monitor_processes(&mut content_ui, serial),
            MonitorCategory::Top => self.draw_monitor_top(&mut content_ui, serial),
            MonitorCategory::SystemInfo => self.draw_monitor_sysinfo(&mut content_ui, serial),
            MonitorCategory::Storage => self.draw_monitor_storage(&mut content_ui, serial),
            MonitorCategory::BatteryPower => self.draw_monitor_battery(&mut content_ui, serial),
            MonitorCategory::Thermal => self.draw_monitor_thermal(&mut content_ui, serial),
            MonitorCategory::IoStats => self.draw_monitor_iostats(&mut content_ui, serial),
            MonitorCategory::Services => self.draw_monitor_services(&mut content_ui, serial),
        }
    }

    /// Common toolbar + output view for a monitor category.
    pub(super) fn draw_monitor_output_area(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        cat: MonitorCategory,
    ) {
        let serial_owned = serial.to_string();
        let is_loading = self
            .devices
            .get(serial)
            .is_some_and(|ds| ds.monitor_loading.contains(&cat));

        ui.horizontal(|ui| {
            if is_loading {
                ui.spinner();
                ui.label("Running...");
            }

            if ui.button("Clear").clicked() {
                if let Some(ds) = self.devices.get_mut(&serial_owned) {
                    ds.monitor_outputs.remove(&cat);
                }
            }

            if ui.button("Copy").clicked() {
                if let Some(ds) = self.devices.get(serial) {
                    if let Some(text) = ds.monitor_outputs.get(&cat) {
                        ui.ctx().copy_text(text.clone());
                    }
                }
            }

            if ui.button("Export...").clicked() {
                if let Some(ds) = self.devices.get(serial) {
                    if let Some(text) = ds.monitor_outputs.get(&cat) {
                        let fname =
                            format!("{}_{}.txt", cat.label().replace(['/', ' '], "_"), serial);
                        let _ = export_single_file(&fname, text);
                    }
                }
            }

            ui.separator();
            ui.label("Filter:");
            if let Some(ds) = self.devices.get_mut(&serial_owned) {
                ui.text_edit_singleline(&mut ds.monitor_filter);
            }

            if let Some(ds) = self.devices.get(serial) {
                let count = ds
                    .monitor_outputs
                    .get(&cat)
                    .map_or(0, |t| bytecount_lines(t));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{count} lines"));
                });
            }
        });

        ui.separator();

        // Output area.
        if let Some(ds) = self.devices.get(serial) {
            if let Some(text) = ds.monitor_outputs.get(&cat) {
                let filter_lower = ds.monitor_filter.to_lowercase();
                egui::ScrollArea::both()
                    .id_salt(format!("monitor_{}_{serial}", cat.index()))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                        for line in text.lines() {
                            let visible_line = self.display_text(line);
                            if !filter_lower.is_empty()
                                && !visible_line.to_lowercase().contains(&filter_lower)
                            {
                                continue;
                            }
                            let color = debug_line_color(&visible_line);
                            ui.label(egui::RichText::new(visible_line).color(color));
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

    /// Send a monitor shell command asynchronously.
    pub(super) fn run_monitor_cmd(
        &mut self,
        serial: &str,
        cat: MonitorCategory,
        shell_cmd: String,
    ) {
        if let Some(ds) = self.devices.get_mut(serial) {
            ds.monitor_loading.insert(cat);
        }
        let serial = serial.to_string();
        let tx = self.tx.clone();
        let idx = cat.index();
        std::thread::spawn(move || {
            let result = adb::run_debug_shell(&serial, &shell_cmd);
            let _ = tx.send(AdbMsg::MonitorOutput(serial, idx, result));
        });
    }

    fn draw_monitor_command_row(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        cat: MonitorCategory,
        commands: Vec<MonitorCommand<'_>>,
    ) {
        ui.horizontal(|ui| {
            for command in commands {
                let mut button = ui.button(command.label);
                if let Some(hover) = command.hover {
                    button = button.on_hover_text(hover);
                }
                if button.clicked() {
                    self.run_monitor_cmd(serial, cat, command.shell_cmd);
                }
            }
        });
    }

    // ─── Monitor: Processes ────────────────────────────────────────────────

    pub(super) fn draw_monitor_processes(&mut self, ui: &mut egui::Ui, serial: &str) {
        let cat = MonitorCategory::Processes;
        let serial_owned = serial.to_string();
        let bundle_id = self.config.bundle_id.clone();
        let bundle_id_shell = adb::shell_quote(&bundle_id);

        ui.label(egui::RichText::new("Process List (ps)").strong());
        ui.add_space(2.0);

        self.draw_monitor_command_row(
            ui,
            serial,
            cat,
            vec![
                MonitorCommand::new(
                    "All Processes",
                    Some("ps -A with full details"),
                    "ps -A -o PID,PPID,USER,VSZ,RSS,WCHAN,ADDR,S,NAME 2>/dev/null || ps -A",
                ),
                MonitorCommand::new(
                    "Compact",
                    Some("PID, user, memory, name"),
                    "ps -A -o PID,USER,RSS,NAME 2>/dev/null || ps -A",
                ),
                MonitorCommand::new(
                    "Threads",
                    Some("All threads (-T)"),
                    "ps -AT -o PID,TID,USER,RSS,NAME 2>/dev/null || ps -AT",
                ),
                MonitorCommand::new(
                    "App Processes",
                    Some("Processes matching bundle ID"),
                    format!(
                        "ps -A -o PID,PPID,USER,VSZ,RSS,NAME | head -1; \
                         ps -A -o PID,PPID,USER,VSZ,RSS,NAME | grep -i -F -- {bundle_id_shell}"
                    ),
                ),
            ],
        );
        self.draw_monitor_command_row(
            ui,
            serial,
            cat,
            vec![
                MonitorCommand::new(
                    "By Memory",
                    Some("Sorted by RSS descending"),
                    "ps -A -o PID,USER,RSS,NAME --sort=-rss 2>/dev/null || \
                     ps -A -o PID,USER,RSS,NAME | sort -k3 -rn",
                ),
                MonitorCommand::new(
                    "Zombie",
                    Some("Zombie processes"),
                    "ps -A -o PID,PPID,USER,S,NAME | head -1; \
                     ps -A -o PID,PPID,USER,S,NAME | grep ' Z '",
                ),
                MonitorCommand::new(
                    "Count",
                    Some("Total process and thread count"),
                    "echo \"Processes: $(ps -A | wc -l)\"; echo \"Threads: $(ps -AT | wc -l)\"",
                ),
            ],
        );
        self.draw_process_search_and_signal(ui, serial, &serial_owned, cat);

        ui.separator();
        self.draw_monitor_output_area(ui, serial, cat);
    }

    fn draw_process_search_and_signal(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        serial_owned: &str,
        cat: MonitorCategory,
    ) {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label("Search:");
            if let Some(ds) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [120.0, 18.0],
                    egui::TextEdit::singleline(&mut ds.monitor_ps_search).hint_text("process name"),
                );
            }
            if ui.button("Find").clicked() {
                let search = self
                    .devices
                    .get(serial)
                    .map(|ds| ds.monitor_ps_search.clone())
                    .unwrap_or_default();
                let search = search.trim();
                if !search.is_empty() {
                    let search = adb::shell_quote(search);
                    self.run_monitor_cmd(
                        serial,
                        cat,
                        format!(
                            "ps -A -o PID,PPID,USER,VSZ,RSS,NAME | head -1; \
                             ps -A -o PID,PPID,USER,VSZ,RSS,NAME | grep -i -F -- {search}"
                        ),
                    );
                }
            }

            ui.separator();
            ui.label("PID:");
            if let Some(ds) = self.devices.get_mut(serial_owned) {
                ui.add_sized(
                    [50.0, 18.0],
                    egui::TextEdit::singleline(&mut ds.monitor_kill_pid).hint_text("PID"),
                );
            }
            if ui
                .button("Kill")
                .on_hover_text("kill <PID> (SIGTERM)")
                .clicked()
            {
                self.run_monitor_signal(serial, cat, false);
            }
            if ui
                .button("Kill -9")
                .on_hover_text("kill -9 <PID> (SIGKILL)")
                .clicked()
            {
                self.run_monitor_signal(serial, cat, true);
            }
        });
    }

    fn run_monitor_signal(&mut self, serial: &str, cat: MonitorCategory, force: bool) {
        let pid_input = self
            .devices
            .get(serial)
            .map(|ds| ds.monitor_kill_pid.clone())
            .unwrap_or_default();
        let pid = match adb::validate_pid(&pid_input) {
            Ok(pid) => pid,
            Err(error) => {
                self.publish_monitor_error(serial, cat, invalid_pid_message(&pid_input, error));
                return;
            }
        };

        let shell_cmd = if force {
            format!("kill -9 {pid} 2>&1 && echo 'Signal sent' || echo 'Failed'")
        } else {
            format!("kill {pid} 2>&1 && echo 'Signal sent' || echo 'Failed'")
        };
        self.run_monitor_cmd(serial, cat, shell_cmd);
    }

    fn publish_monitor_error(&self, serial: &str, cat: MonitorCategory, message: String) {
        let _ = self.tx.send(AdbMsg::MonitorOutput(
            serial.to_string(),
            cat.index(),
            Err(message),
        ));
    }

    // ─── Monitor: Top / Load ───────────────────────────────────────────────

    pub(super) fn draw_monitor_top(&mut self, ui: &mut egui::Ui, serial: &str) {
        let cat = MonitorCategory::Top;

        ui.label(egui::RichText::new("Top / CPU Load").strong());
        ui.add_space(2.0);

        ui.horizontal(|ui| {
            if ui
                .button("Top (snapshot)")
                .on_hover_text("One-shot top output")
                .clicked()
            {
                self.run_monitor_cmd(serial, cat, "top -b -n 1 2>/dev/null || top -n 1".into());
            }
            if ui
                .button("Top (by CPU)")
                .on_hover_text("Top 30 by CPU usage")
                .clicked()
            {
                self.run_monitor_cmd(
                    serial,
                    cat,
                    "top -b -n 1 -o %CPU 2>/dev/null | head -40 || top -n 1 | head -40".into(),
                );
            }
            if ui
                .button("Top (by MEM)")
                .on_hover_text("Top 30 by memory usage")
                .clicked()
            {
                self.run_monitor_cmd(
                    serial,
                    cat,
                    "top -b -n 1 -o %MEM 2>/dev/null | head -40 || top -n 1 -s rss 2>/dev/null | head -40"
                        .into(),
                );
            }
            if ui
                .button("Top (threads)")
                .on_hover_text("Thread-level view")
                .clicked()
            {
                self.run_monitor_cmd(
                    serial,
                    cat,
                    "top -b -n 1 -H 2>/dev/null | head -50".into(),
                );
            }
        });

        ui.horizontal(|ui| {
            if ui
                .button("Load Average")
                .on_hover_text("cat /proc/loadavg")
                .clicked()
            {
                self.run_monitor_cmd(
                    serial,
                    cat,
                    "echo '=== Load Average ==='; cat /proc/loadavg; \
                     echo '\\n=== Uptime ==='; uptime"
                        .into(),
                );
            }
            if ui
                .button("CPU Stats")
                .on_hover_text("/proc/stat CPU lines")
                .clicked()
            {
                self.run_monitor_cmd(
                    serial,
                    cat,
                    "echo '=== CPU Stats ==='; head -20 /proc/stat; \
                     echo '\\n=== CPU Frequencies ==='; \
                     for cpu in /sys/devices/system/cpu/cpu[0-9]*; do \
                       echo \"$(basename $cpu): $(cat $cpu/cpufreq/scaling_cur_freq 2>/dev/null || echo 'N/A') kHz\"; \
                     done"
                        .into(),
                );
            }
            if ui
                .button("Scheduler")
                .on_hover_text("Kernel scheduler stats")
                .clicked()
            {
                self.run_monitor_cmd(
                    serial,
                    cat,
                    "cat /proc/schedstat 2>/dev/null | head -20; \
                     echo '\\n=== Context Switches ==='; \
                     grep ctxt /proc/stat"
                        .into(),
                );
            }
        });

        ui.separator();
        self.draw_monitor_output_area(ui, serial, cat);
    }

    // ─── Monitor: System Info ──────────────────────────────────────────────

    pub(super) fn draw_monitor_sysinfo(&mut self, ui: &mut egui::Ui, serial: &str) {
        let cat = MonitorCategory::SystemInfo;

        ui.label(egui::RichText::new("System Information").strong());
        ui.add_space(2.0);

        ui.horizontal(|ui| {
            if ui
                .button("Overview")
                .on_hover_text("Combined system overview")
                .clicked()
            {
                self.run_monitor_cmd(
                    serial,
                    cat,
                    "echo '=== Uptime ==='; uptime; \
                     echo '\\n=== Kernel ==='; uname -a; \
                     echo '\\n=== Android Version ==='; getprop ro.build.version.release; \
                     echo '=== SDK Level ==='; getprop ro.build.version.sdk; \
                     echo '=== Security Patch ==='; getprop ro.build.version.security_patch; \
                     echo '\\n=== Model ==='; getprop ro.product.model; \
                     echo '=== Manufacturer ==='; getprop ro.product.manufacturer; \
                     echo '=== Device ==='; getprop ro.product.device; \
                     echo '=== Build ==='; getprop ro.build.display.id; \
                     echo '\\n=== Architecture ==='; uname -m; \
                     echo '=== ABI ==='; getprop ro.product.cpu.abi; \
                     echo '=== ABI List ==='; getprop ro.product.cpu.abilist"
                        .into(),
                );
            }
            if ui
                .button("Uptime")
                .on_hover_text("System uptime and idle time")
                .clicked()
            {
                self.run_monitor_cmd(
                    serial,
                    cat,
                    "echo '=== Uptime ==='; uptime; \
                     echo '\\n=== /proc/uptime (seconds up, idle) ==='; cat /proc/uptime; \
                     echo '\\n=== Boot Time ==='; \
                     BOOT=$(cat /proc/stat | grep btime | awk '{print $2}'); \
                     date -d @$BOOT 2>/dev/null || echo \"btime: $BOOT\""
                        .into(),
                );
            }
            if ui.button("Kernel").on_hover_text("uname -a").clicked() {
                self.run_monitor_cmd(serial, cat, "uname -a".into());
            }
        });

        ui.horizontal(|ui| {
            if ui
                .button("CPU Info")
                .on_hover_text("/proc/cpuinfo")
                .clicked()
            {
                self.run_monitor_cmd(serial, cat, "cat /proc/cpuinfo".into());
            }
            if ui
                .button("Build Props")
                .on_hover_text("Android build properties")
                .clicked()
            {
                self.run_monitor_cmd(
                    serial,
                    cat,
                    "getprop | grep -E 'ro\\.(build|product|hardware|bootimage)' | sort".into(),
                );
            }
            if ui
                .button("All Properties")
                .on_hover_text("Full getprop listing")
                .clicked()
            {
                self.run_monitor_cmd(serial, cat, "getprop | sort".into());
            }
            if ui
                .button("SELinux")
                .on_hover_text("SELinux status")
                .clicked()
            {
                self.run_monitor_cmd(
                    serial,
                    cat,
                    "echo '=== SELinux Mode ==='; getenforce 2>/dev/null || echo 'N/A'; \
                     echo '=== Policy ==='; getprop ro.build.selinux"
                        .into(),
                );
            }
        });

        ui.separator();
        self.draw_monitor_output_area(ui, serial, cat);
    }

    // ─── Monitor: Storage ──────────────────────────────────────────────────

    pub(super) fn draw_monitor_storage(&mut self, ui: &mut egui::Ui, serial: &str) {
        let cat = MonitorCategory::Storage;

        ui.label(egui::RichText::new("Storage & Disk").strong());
        ui.add_space(2.0);

        ui.horizontal(|ui| {
            if ui
                .button("df -h")
                .on_hover_text("Disk free space (human-readable)")
                .clicked()
            {
                self.run_monitor_cmd(serial, cat, "df -h".into());
            }
            if ui
                .button("df (all)")
                .on_hover_text("All filesystems including tmpfs")
                .clicked()
            {
                self.run_monitor_cmd(serial, cat, "df -ha".into());
            }
            if ui
                .button("Mounts")
                .on_hover_text("Active mount points")
                .clicked()
            {
                self.run_monitor_cmd(serial, cat, "cat /proc/mounts".into());
            }
            if ui
                .button("Disk Stats")
                .on_hover_text("/proc/diskstats")
                .clicked()
            {
                self.run_monitor_cmd(serial, cat, "cat /proc/diskstats".into());
            }
        });

        ui.horizontal(|ui| {
            if ui
                .button("Storage Info")
                .on_hover_text("dumpsys diskstats summary")
                .clicked()
            {
                self.run_monitor_cmd(serial, cat, "dumpsys diskstats".into());
            }
            if ui
                .button("Internal")
                .on_hover_text("/data partition")
                .clicked()
            {
                self.run_monitor_cmd(serial, cat, "df -h /data; echo ''; du -sh /data/data 2>/dev/null; du -sh /data/app 2>/dev/null".into());
            }
            if ui
                .button("SD Card")
                .on_hover_text("/sdcard partition")
                .clicked()
            {
                self.run_monitor_cmd(
                    serial,
                    cat,
                    "df -h /sdcard; echo ''; ls -la /sdcard/ | head -20".into(),
                );
            }
            if ui
                .button("Partitions")
                .on_hover_text("Block device partitions")
                .clicked()
            {
                self.run_monitor_cmd(serial, cat, "cat /proc/partitions".into());
            }
        });

        ui.separator();
        self.draw_monitor_output_area(ui, serial, cat);
    }

    // ─── Monitor: Battery & Power ──────────────────────────────────────────

    pub(super) fn draw_monitor_battery(&mut self, ui: &mut egui::Ui, serial: &str) {
        let cat = MonitorCategory::BatteryPower;

        ui.label(egui::RichText::new("Battery & Power").strong());
        ui.add_space(2.0);

        self.draw_monitor_command_row(
            ui,
            serial,
            cat,
            vec![
                MonitorCommand::new("Battery Status", Some("dumpsys battery"), "dumpsys battery"),
                MonitorCommand::new(
                    "Battery Stats",
                    Some("Battery statistics summary"),
                    "dumpsys batterystats | head -120",
                ),
                MonitorCommand::new(
                    "Battery Health",
                    Some("Health, temperature, voltage"),
                    "echo '=== Battery ==='; dumpsys battery; \
                     echo '\\n=== Health HAL ==='; \
                     dumpsys android.hardware.health.IHealth/default 2>/dev/null | head -30 || echo 'N/A'",
                ),
            ],
        );
        self.draw_monitor_command_row(
            ui,
            serial,
            cat,
            vec![
                MonitorCommand::new(
                    "Wake Locks",
                    Some("Active wake locks"),
                    "dumpsys power | grep -A 30 'Wake Locks'",
                ),
                MonitorCommand::new(
                    "Power Profile",
                    Some("Power manager state"),
                    "dumpsys power | head -80",
                ),
                MonitorCommand::new(
                    "Doze State",
                    Some("Device idle / Doze mode"),
                    "dumpsys deviceidle 2>/dev/null | head -40 || echo 'deviceidle not available'",
                ),
                MonitorCommand::new(
                    "Charging",
                    Some("USB/AC/wireless charging state"),
                    "echo '=== Charging ==='; \
                     cat /sys/class/power_supply/battery/status 2>/dev/null; \
                     cat /sys/class/power_supply/battery/charge_type 2>/dev/null; \
                     echo '\\n=== Current (uA) ==='; \
                     cat /sys/class/power_supply/battery/current_now 2>/dev/null || echo 'N/A'; \
                     echo '\\n=== Voltage (uV) ==='; \
                     cat /sys/class/power_supply/battery/voltage_now 2>/dev/null || echo 'N/A'",
                ),
            ],
        );
        self.draw_monitor_command_row(
            ui,
            serial,
            cat,
            vec![
                MonitorCommand::new("Alarms", Some("Pending alarms"), "dumpsys alarm | head -80"),
                MonitorCommand::new(
                    "CPU Wake",
                    Some("CPU wake-up sources"),
                    "cat /sys/kernel/debug/wakeup_sources 2>/dev/null | head -40 || \
                     cat /d/wakeup_sources 2>/dev/null | head -40 || \
                     echo 'Wake source info not available'",
                ),
            ],
        );

        ui.separator();
        self.draw_monitor_output_area(ui, serial, cat);
    }

    // ─── Monitor: Thermal ──────────────────────────────────────────────────

    pub(super) fn draw_monitor_thermal(&mut self, ui: &mut egui::Ui, serial: &str) {
        let cat = MonitorCategory::Thermal;

        ui.label(egui::RichText::new("Thermal Monitoring").strong());
        ui.add_space(2.0);

        self.draw_monitor_command_row(
            ui,
            serial,
            cat,
            vec![
                MonitorCommand::new(
                    "All Thermal Zones",
                    Some("Read all thermal zone temperatures"),
                    "for tz in /sys/class/thermal/thermal_zone*; do \
                       echo \"$(basename $tz) [$(cat $tz/type 2>/dev/null)]: $(cat $tz/temp 2>/dev/null) mdegC\"; \
                     done 2>/dev/null | sort || echo 'Thermal info not available'",
                ),
                MonitorCommand::new(
                    "Thermal Service",
                    Some("dumpsys thermalservice"),
                    "dumpsys thermalservice 2>/dev/null || echo 'thermalservice not available'",
                ),
                MonitorCommand::new(
                    "Quick Temps",
                    Some("Key temperatures"),
                    "echo '=== Battery Temp ==='; \
                     dumpsys battery | grep temperature; \
                     echo '\\n=== CPU Thermal Zones ==='; \
                     for tz in /sys/class/thermal/thermal_zone*; do \
                       TYPE=$(cat $tz/type 2>/dev/null); \
                       case $TYPE in *cpu*|*CPU*|*tsens*|*little*|*big*) \
                         echo \"$TYPE: $(cat $tz/temp 2>/dev/null) mdegC\";; \
                       esac; \
                     done 2>/dev/null",
                ),
            ],
        );
        self.draw_monitor_command_row(
            ui,
            serial,
            cat,
            vec![
                MonitorCommand::new(
                    "Cooling Devices",
                    Some("Active cooling devices and states"),
                    "for cd in /sys/class/thermal/cooling_device*; do \
                       echo \"$(basename $cd) [$(cat $cd/type 2>/dev/null)]: $(cat $cd/cur_state 2>/dev/null)/$(cat $cd/max_state 2>/dev/null)\"; \
                     done 2>/dev/null || echo 'Cooling device info not available'",
                ),
                MonitorCommand::new(
                    "CPU Frequencies",
                    Some("Current vs max CPU frequency (throttle check)"),
                    "echo 'CPU   Current(kHz)  Max(kHz)  Governor'; \
                     for cpu in /sys/devices/system/cpu/cpu[0-9]*; do \
                       CUR=$(cat $cpu/cpufreq/scaling_cur_freq 2>/dev/null || echo 'N/A'); \
                       MAX=$(cat $cpu/cpufreq/scaling_max_freq 2>/dev/null || echo 'N/A'); \
                       GOV=$(cat $cpu/cpufreq/scaling_governor 2>/dev/null || echo 'N/A'); \
                       echo \"$(basename $cpu)  $CUR  $MAX  $GOV\"; \
                     done",
                ),
                MonitorCommand::new(
                    "Throttle State",
                    Some("Check if CPU is being throttled"),
                    "echo '=== CPU Online Status ==='; \
                     for cpu in /sys/devices/system/cpu/cpu[0-9]*; do \
                       echo \"$(basename $cpu): online=$(cat $cpu/online 2>/dev/null || echo 'N/A')\"; \
                     done; \
                     echo '\\n=== Thermal Mitigation ==='; \
                     dumpsys thermalservice 2>/dev/null | grep -i -A 3 'throttl\\|mitigat\\|severity' || echo 'N/A'",
                ),
            ],
        );

        ui.separator();
        self.draw_monitor_output_area(ui, serial, cat);
    }

    // ─── Monitor: I/O Stats ────────────────────────────────────────────────

    pub(super) fn draw_monitor_iostats(&mut self, ui: &mut egui::Ui, serial: &str) {
        let cat = MonitorCategory::IoStats;

        ui.label(egui::RichText::new("I/O Statistics").strong());
        ui.add_space(2.0);

        ui.horizontal(|ui| {
            if ui
                .button("Disk I/O")
                .on_hover_text("/proc/diskstats")
                .clicked()
            {
                self.run_monitor_cmd(serial, cat, "cat /proc/diskstats".into());
            }
            if ui
                .button("iostat")
                .on_hover_text("I/O statistics (if available)")
                .clicked()
            {
                self.run_monitor_cmd(
                    serial,
                    cat,
                    "iostat 2>/dev/null || echo '--- iostat not available, showing /proc/diskstats ---'; cat /proc/diskstats"
                        .into(),
                );
            }
            if ui
                .button("I/O Pressure")
                .on_hover_text("PSI I/O pressure")
                .clicked()
            {
                self.run_monitor_cmd(
                    serial,
                    cat,
                    "echo '=== I/O Pressure ==='; cat /proc/pressure/io 2>/dev/null || echo 'N/A'; \
                     echo '\\n=== CPU Pressure ==='; cat /proc/pressure/cpu 2>/dev/null || echo 'N/A'; \
                     echo '\\n=== Memory Pressure ==='; cat /proc/pressure/memory 2>/dev/null || echo 'N/A'"
                        .into(),
                );
            }
        });

        ui.horizontal(|ui| {
            if ui
                .button("vmstat")
                .on_hover_text("Virtual memory statistics")
                .clicked()
            {
                self.run_monitor_cmd(
                    serial,
                    cat,
                    "vmstat 1 5 2>/dev/null || cat /proc/vmstat | head -40".into(),
                );
            }
            if ui
                .button("File Systems")
                .on_hover_text("Mounted filesystem types and options")
                .clicked()
            {
                self.run_monitor_cmd(
                    serial,
                    cat,
                    "mount | grep -E '^/dev' | column -t 2>/dev/null || mount | grep -E '^/dev'"
                        .into(),
                );
            }
            if ui
                .button("Block Devices")
                .on_hover_text("Block device info")
                .clicked()
            {
                self.run_monitor_cmd(
                    serial,
                    cat,
                    "ls -la /dev/block/by-name/ 2>/dev/null || echo 'N/A'; \
                     echo '\\n=== Block Stats ==='; \
                     for d in /sys/block/*/stat; do \
                       DEV=$(echo $d | cut -d/ -f4); \
                       echo \"$DEV: $(cat $d)\"; \
                     done 2>/dev/null"
                        .into(),
                );
            }
        });

        ui.separator();
        self.draw_monitor_output_area(ui, serial, cat);
    }

    // ─── Monitor: Services ─────────────────────────────────────────────────

    pub(super) fn draw_monitor_services(&mut self, ui: &mut egui::Ui, serial: &str) {
        let cat = MonitorCategory::Services;

        ui.label(egui::RichText::new("System Services").strong());
        ui.add_space(2.0);

        self.draw_monitor_command_row(
            ui,
            serial,
            cat,
            vec![
                MonitorCommand::new(
                    "Init Services",
                    Some("Init service states (init.svc.*)"),
                    "getprop | grep init.svc | sort",
                ),
                MonitorCommand::new("Running Services", Some("service list"), "service list"),
                MonitorCommand::new(
                    "Service Count",
                    Some("Number of registered services"),
                    "echo \"Running: $(getprop | grep 'init.svc.*running' | wc -l)\"; \
                     echo \"Stopped: $(getprop | grep 'init.svc.*stopped' | wc -l)\"; \
                     echo \"Total registered: $(service list | wc -l)\"",
                ),
            ],
        );
        self.draw_monitor_command_row(
            ui,
            serial,
            cat,
            vec![
                MonitorCommand::new(
                    "Scheduled Jobs",
                    Some("JobScheduler pending jobs"),
                    "dumpsys jobscheduler | head -100",
                ),
                MonitorCommand::new(
                    "Content Providers",
                    Some("Registered content providers"),
                    "dumpsys content | head -80",
                ),
                MonitorCommand::new(
                    "Package Stats",
                    Some("Installed package count"),
                    "echo \"Total packages: $(pm list packages | wc -l)\"; \
                     echo \"System: $(pm list packages -s | wc -l)\"; \
                     echo \"Third-party: $(pm list packages -3 | wc -l)\"; \
                     echo \"Disabled: $(pm list packages -d | wc -l)\"",
                ),
            ],
        );
        self.draw_monitor_command_row(
            ui,
            serial,
            cat,
            vec![
                MonitorCommand::new(
                    "Boot Status",
                    Some("Boot completion and encryption state"),
                    "echo \"Boot completed: $(getprop sys.boot_completed)\"; \
                     echo \"Encryption state: $(getprop ro.crypto.state)\"; \
                     echo \"Boot reason: $(getprop ro.boot.bootreason 2>/dev/null || getprop sys.boot.reason)\"; \
                     echo \"Bootloader: $(getprop ro.bootloader)\"",
                ),
                MonitorCommand::new("Users", Some("User accounts on device"), "pm list users"),
                MonitorCommand::new(
                    "Features",
                    Some("Hardware/software features"),
                    "pm list features | sort",
                ),
            ],
        );

        ui.separator();
        self.draw_monitor_output_area(ui, serial, cat);
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
