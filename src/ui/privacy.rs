use std::process::{Command, Stdio};

use eframe::egui;

use super::{App, AppLogLevel, STREAMER_MODE_POLL_INTERVAL};

impl App {
    pub(super) fn maybe_poll_streamer_mode(&mut self, now: f64) {
        if now - self.last_streamer_mode_poll <= STREAMER_MODE_POLL_INTERVAL {
            return;
        }

        self.last_streamer_mode_poll = now;
        let next_state = obs_process_running();
        if next_state == self.streamer_mode {
            return;
        }

        self.streamer_mode = next_state;
        let message = if next_state {
            "Streamer Mode enabled automatically because OBS is running"
        } else {
            "Streamer Mode disabled because OBS is no longer running"
        };
        self.log(AppLogLevel::Info, message);
    }

    pub(super) const fn streamer_mode_active(&self) -> bool {
        self.streamer_mode
    }

    pub(super) fn ensure_streamer_device_alias(&mut self, serial: &str) {
        if self.streamer_device_aliases.contains_key(serial) {
            return;
        }

        let alias = self.next_streamer_device_alias;
        self.next_streamer_device_alias += 1;
        self.streamer_device_aliases
            .insert(serial.to_string(), alias);
    }

    pub(super) fn display_serial(&self, serial: &str) -> String {
        if !self.streamer_mode_active() {
            return serial.to_string();
        }

        self.streamer_device_aliases
            .get(serial)
            .map_or_else(|| "Device".to_string(), |alias| format!("Device {alias}"))
    }

    pub(super) fn display_model(&self, serial: &str, model: &str) -> String {
        if self.streamer_mode_active() {
            self.display_serial(serial)
        } else {
            model.to_string()
        }
    }

    pub(super) fn display_device_label(&self, serial: &str, model: &str) -> String {
        if self.streamer_mode_active() {
            return self.display_serial(serial);
        }

        if model == "unknown" {
            serial.to_string()
        } else {
            format!("{model} ({serial})")
        }
    }

    pub(super) fn display_text(&self, text: &str) -> String {
        if !self.streamer_mode_active() {
            return text.to_string();
        }

        let mut replacements = Vec::new();

        for serial in self.streamer_device_aliases.keys() {
            let alias = self.display_serial(serial);
            let model = self
                .streamer_device_models
                .get(serial)
                .map(String::as_str)
                .unwrap_or("unknown");
            replacements.push((raw_device_label(model, serial), alias.clone()));
            replacements.push((serial.clone(), alias.clone()));
            if model != "unknown" {
                replacements.push((model.to_string(), alias));
            }
        }

        for (value, replacement) in [
            (self.config.bundle_id.as_str(), "<package>"),
            (self.bundle_id_input.as_str(), "<package>"),
            (self.activity_class_input.as_str(), "<activity>"),
            (self.wifi_connect_addr.as_str(), "<address>"),
            (self.pair_address_input.as_str(), "<address>"),
            (self.pair_code_input.as_str(), "<pair-code>"),
            (self.fastboot_serial_input.as_str(), "<fastboot-serial>"),
            (self.adb_path_candidate.as_str(), "<adb-path>"),
        ] {
            if !value.trim().is_empty() {
                replacements.push((value.to_string(), replacement.to_string()));
            }
        }

        let config_path = self.config_path.display().to_string();
        if !config_path.is_empty() {
            replacements.push((config_path, "<config-path>".to_string()));
        }

        redact_text(text, &replacements)
    }

    pub(super) fn display_device_prop_value(&self, value: &str) -> String {
        if self.streamer_mode_active() {
            "<hidden by Streamer Mode>".to_string()
        } else {
            value.to_string()
        }
    }

    pub(super) fn streamer_mode_badge(&self) -> Option<egui::RichText> {
        self.streamer_mode_active().then(|| {
            egui::RichText::new("Streamer Mode")
                .small()
                .strong()
                .color(egui::Color32::from_rgb(255, 200, 50))
        })
    }
}

fn raw_device_label(model: &str, serial: &str) -> String {
    if model == "unknown" {
        serial.to_string()
    } else {
        format!("{model} ({serial})")
    }
}

fn obs_process_running() -> bool {
    list_process_names()
        .map(|names| names.iter().any(|name| matches_obs_process(name)))
        .unwrap_or(false)
}

fn list_process_names() -> Result<Vec<String>, String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;

        let output = Command::new("tasklist")
            .args(["/FO", "CSV", "/NH"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|error| format!("tasklist failed: {error}"))?;
        if !output.status.success() {
            return Err("tasklist exited unsuccessfully".to_string());
        }

        return Ok(parse_tasklist_csv(&String::from_utf8_lossy(&output.stdout)));
    }

    #[cfg(not(windows))]
    {
        let output = Command::new("ps")
            .args(["-A", "-o", "comm="])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("ps failed: {error}"))?;
        if !output.status.success() {
            return Err("ps exited unsuccessfully".to_string());
        }

        Ok(output
            .stdout
            .split(|byte| *byte == b'\n')
            .filter_map(|line| std::str::from_utf8(line).ok())
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect())
    }
}

fn parse_tasklist_csv(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.split(',').next())
        .map(|field| field.trim().trim_matches('"').to_string())
        .filter(|field| !field.is_empty())
        .collect()
}

fn matches_obs_process(name: &str) -> bool {
    let normalized = name
        .trim()
        .trim_matches('"')
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();

    matches!(
        normalized.as_str(),
        "obs"
            | "obs.exe"
            | "obs32"
            | "obs32.exe"
            | "obs64"
            | "obs64.exe"
            | "obs-studio"
            | "obs-studio.exe"
    )
}

fn redact_text(text: &str, replacements: &[(String, String)]) -> String {
    let mut sorted_replacements = replacements
        .iter()
        .filter(|(raw, _)| !raw.is_empty())
        .collect::<Vec<_>>();
    sorted_replacements.sort_by(|(lhs, _), (rhs, _)| rhs.len().cmp(&lhs.len()));

    let mut redacted = text.to_string();
    for (raw, replacement) in sorted_replacements {
        if redacted.contains(raw.as_str()) {
            redacted = redacted.replace(raw.as_str(), replacement);
        }
    }

    redact_path_like_tokens(&redacted)
}

fn redact_path_like_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut token = String::new();

    for ch in text.chars() {
        if ch.is_whitespace() {
            out.push_str(&mask_token(&token));
            token.clear();
            out.push(ch);
        } else {
            token.push(ch);
        }
    }

    out.push_str(&mask_token(&token));
    out
}

fn mask_token(token: &str) -> String {
    if token.is_empty() {
        return String::new();
    }

    let start = token
        .char_indices()
        .find(|(_, ch)| !is_leading_wrapper(*ch))
        .map_or(token.len(), |(idx, _)| idx);
    let end = token
        .char_indices()
        .rfind(|(_, ch)| !is_trailing_wrapper(*ch))
        .map_or(0, |(idx, ch)| idx + ch.len_utf8());

    if start >= end {
        return token.to_string();
    }

    let leading = &token[..start];
    let core = &token[start..end];
    let trailing = &token[end..];

    let replacement = if looks_like_path(core) {
        Some("<path>")
    } else if looks_like_address(core) {
        Some("<address>")
    } else {
        None
    };

    replacement.map_or_else(
        || token.to_string(),
        |replacement| format!("{leading}{replacement}{trailing}"),
    )
}

const fn is_leading_wrapper(ch: char) -> bool {
    matches!(ch, '"' | '\'' | '(' | '[' | '{' | '<')
}

const fn is_trailing_wrapper(ch: char) -> bool {
    matches!(ch, '"' | '\'' | ')' | ']' | '}' | '>' | ',' | ';')
}

fn looks_like_path(token: &str) -> bool {
    let normalized = token
        .trim_matches('"')
        .trim_matches('\'')
        .replace('\\', "/");
    if normalized.len() < 3 {
        return false;
    }

    if token.starts_with("\\\\") {
        return true;
    }

    if token.as_bytes().get(1) == Some(&b':') && (token.contains('\\') || token.contains('/')) {
        return true;
    }

    if normalized.starts_with("~/") {
        return true;
    }

    if normalized.starts_with('/') {
        let segment_count = normalized
            .split('/')
            .filter(|segment| !segment.is_empty())
            .count();
        return segment_count >= 2;
    }

    false
}

fn looks_like_address(token: &str) -> bool {
    let core = token.trim_matches('"').trim_matches('\'');
    looks_like_ipv4(core)
        || core
            .rsplit_once(':')
            .is_some_and(|(host, port)| looks_like_ipv4(host) && is_numeric_port(port))
        || core.strip_prefix("localhost:").is_some_and(is_numeric_port)
}

fn looks_like_ipv4(token: &str) -> bool {
    let parts = token.split('.').collect::<Vec<_>>();
    if parts.len() != 4 {
        return false;
    }

    parts.iter().all(|part| {
        !part.is_empty()
            && part.len() <= 3
            && part.chars().all(|ch| ch.is_ascii_digit())
            && part.parse::<u8>().is_ok()
    })
}

fn is_numeric_port(port: &str) -> bool {
    !port.is_empty() && port.chars().all(|ch| ch.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasklist_parser_reads_process_names() {
        let names = parse_tasklist_csv(
            "\"obs64.exe\",\"1234\",\"Console\",\"1\",\"120,000 K\"\n\"explorer.exe\",\"88\",\"Console\",\"1\",\"42,000 K\"",
        );
        assert_eq!(names, vec!["obs64.exe", "explorer.exe"]);
    }

    #[test]
    fn obs_process_detection_matches_common_binary_names() {
        assert!(matches_obs_process("obs64.exe"));
        assert!(matches_obs_process("/usr/bin/obs-studio"));
        assert!(!matches_obs_process("adb.exe"));
    }

    #[test]
    fn redaction_masks_sensitive_values_and_paths() {
        let text = "Pixel 8 (ABC123) saved C:\\Users\\Koray\\capture.png for com.example.app";
        let redacted = redact_text(
            text,
            &[
                ("Pixel 8 (ABC123)".to_string(), "Device 1".to_string()),
                ("ABC123".to_string(), "Device 1".to_string()),
                ("Pixel 8".to_string(), "Device 1".to_string()),
                ("com.example.app".to_string(), "<package>".to_string()),
            ],
        );
        assert_eq!(redacted, "Device 1 saved <path> for <package>");
    }

    #[test]
    fn redaction_masks_addresses() {
        assert_eq!(
            redact_text("Connecting to 192.168.1.42:5555", &[]),
            "Connecting to <address>"
        );
        assert_eq!(
            redact_text("Pairing with localhost:58526", &[]),
            "Pairing with <address>"
        );
    }
}
