use std::process::Stdio;

use super::{adb_command, CommandExt, CREATE_NO_WINDOW};

/// Key-value device properties. `bundle_id` is used to check app install status.
pub fn get_device_props(serial: &str, bundle_id: &str) -> Vec<(String, String)> {
    let props = [
        ("Model", "ro.product.model"),
        ("Manufacturer", "ro.product.manufacturer"),
        ("Android Version", "ro.build.version.release"),
        ("SDK / API Level", "ro.build.version.sdk"),
        ("Build ID", "ro.build.display.id"),
        ("Hardware", "ro.hardware"),
        ("CPU ABI", "ro.product.cpu.abi"),
        ("Kernel", ""),
    ];

    let mut result = Vec::new();
    result.push(("Serial".into(), serial.to_string()));

    for (label, prop) in &props {
        if prop.is_empty() {
            // Special: kernel version
            if *label == "Kernel" {
                let val = adb_shell(serial, "uname -r");
                result.push(((*label).to_string(), val));
            }
        } else {
            let val = adb_getprop(serial, prop);
            result.push(((*label).to_string(), val));
        }
    }

    // Battery
    if let Some(battery) = parse_battery(serial) {
        result.push(("Battery".into(), battery));
    }

    // Screen size
    let wm = adb_shell(serial, "wm size");
    if let Some(size) = wm.lines().find(|l| l.contains("Physical")) {
        result.push(("Screen".into(), size.trim().to_string()));
    } else if !wm.trim().is_empty() {
        result.push((
            "Screen".into(),
            wm.lines().next().unwrap_or("").trim().to_string(),
        ));
    }

    // IP address
    let ip = adb_shell(serial, "ip route");
    if let Some(line) = ip.lines().find(|l| l.contains("src")) {
        if let Some(addr) = line.split_whitespace().skip_while(|w| *w != "src").nth(1) {
            result.push(("IP Address".into(), addr.to_string()));
        }
    }

    // Disk usage
    let df = adb_shell(serial, "df /data | tail -1");
    if !df.trim().is_empty() && !df.contains("No such") {
        let parts: Vec<&str> = df.split_whitespace().collect();
        if parts.len() >= 5 {
            result.push((
                "Storage (/data)".into(),
                format!("Used {} / {} ({})", parts[2], parts[1], parts[4].trim_end()),
            ));
        }
    }

    // App info
    if !bundle_id.is_empty() {
        let installed = is_package_installed(serial, bundle_id);
        if installed {
            let ver = get_package_version(serial, bundle_id).unwrap_or_else(|| "?".into());
            result.push(("App Installed".into(), format!("{bundle_id} v{ver}")));
        } else {
            result.push((
                "App Installed".into(),
                format!("{bundle_id}: NOT INSTALLED"),
            ));
        }
    }

    result
}

fn adb_getprop(serial: &str, prop: &str) -> String {
    adb_shell(serial, &format!("getprop {prop}"))
}

pub(super) fn adb_shell(serial: &str, cmd: &str) -> String {
    let output = adb_command().and_then(|mut c| {
        c.args(["-s", serial, "shell", cmd])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
    });

    match output {
        Some(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => String::new(),
    }
}

fn parse_battery(serial: &str) -> Option<String> {
    let raw = adb_shell(serial, "dumpsys battery");
    let mut level = None;
    let mut status = None;
    let mut temp = None;

    for line in raw.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("level:") {
            level = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("status:") {
            let s = match v.trim() {
                "1" => "Unknown",
                "2" => "Charging",
                "3" => "Discharging",
                "4" => "Not charging",
                "5" => "Full",
                other => other,
            };
            status = Some(s.to_string());
        } else if let Some(v) = line.strip_prefix("temperature:") {
            if let Ok(t) = v.trim().parse::<f64>() {
                temp = Some(format!("{:.1}C", t / 10.0));
            }
        }
    }

    level.map(|l| {
        use std::fmt::Write;
        let mut s = format!("{l}%");
        if let Some(st) = status {
            let _ = write!(s, " ({st})");
        }
        if let Some(t) = temp {
            let _ = write!(s, " {t}");
        }
        s
    })
}

/// Check if a package is installed.
pub fn is_package_installed(serial: &str, bundle_id: &str) -> bool {
    let bid = super::sanitize_shell_arg(bundle_id);
    let out = adb_shell(serial, &format!("pm path {bid}"));
    !out.is_empty() && out.contains("package:")
}

/// Get package version info.
pub fn get_package_version(serial: &str, bundle_id: &str) -> Option<String> {
    let bid = super::sanitize_shell_arg(bundle_id);
    let out = adb_shell(serial, &format!("dumpsys package {bid} | grep versionName"));
    let line = out.lines().next()?.trim();
    line.strip_prefix("versionName=")
        .map(std::string::ToString::to_string)
}

/// Run a simple adb command, return (success, output).
pub fn run_device_action(serial: &str, args: &[&str]) -> (bool, String) {
    let mut cmd_args = vec!["-s", serial];
    cmd_args.extend_from_slice(args);

    let output = adb_command().and_then(|mut c| {
        c.args(&cmd_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
    });

    match output {
        Some(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            let text = match (stdout.is_empty(), stderr.is_empty()) {
                (false, true) => stdout,
                (true, false) => stderr,
                (false, false) => format!("{stdout}\n{stderr}"),
                (true, true) => String::new(),
            };
            (o.status.success(), text)
        }
        None => (false, "Failed to run adb".into()),
    }
}
