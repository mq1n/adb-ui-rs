use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Stdio};

use crossbeam_channel::Sender;

use super::{adb_command, adb_path, AdbMsg, CommandExt, CREATE_NO_WINDOW};

/// Handle to an interactive adb shell. Holds the child process and its stdin.
pub struct ShellHandle {
    pub child: Child,
    pub stdin: std::process::ChildStdin,
}

impl ShellHandle {
    /// Send a command (appends newline).
    pub fn send(&mut self, cmd: &str) -> bool {
        writeln!(self.stdin, "{cmd}").is_ok()
    }

    /// Kill the shell process.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

/// Spawn an interactive `adb -s <serial> shell`.
/// Returns `ShellHandle` for sending commands; output lines are sent via `tx`.
pub fn spawn_shell(serial: &str, tx: Sender<AdbMsg>) -> Option<ShellHandle> {
    let serial_owned = serial.to_string();

    let Some(mut cmd) = adb_command() else {
        let error = adb_path().map_or_else(|inner| inner, |path| path.display().to_string());
        let _ = tx.send(AdbMsg::ShellExited(
            serial_owned,
            format!("ADB not available: {error}"),
        ));
        return None;
    };
    cmd.args(["-s", serial, "shell"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(AdbMsg::ShellExited(
                serial_owned,
                format!("Failed to spawn shell: {e}"),
            ));
            return None;
        }
    };

    let Some(stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = tx.send(AdbMsg::ShellExited(
            serial_owned,
            "Failed to capture stdin".into(),
        ));
        return None;
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = tx.send(AdbMsg::ShellExited(
            serial_owned,
            "Failed to capture stdout".into(),
        ));
        return None;
    };
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = tx.send(AdbMsg::ShellExited(
            serial_owned,
            "Failed to capture stderr".into(),
        ));
        return None;
    };

    // Stdout reader thread.
    let tx2 = tx.clone();
    let serial2 = serial_owned.clone();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::with_capacity(4096);
        let exit_reason = loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break "Process ended".to_string(),
                Err(error) => break format!("Shell stdout read failed: {error}"),
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf)
                        .trim_end_matches(['\n', '\r'])
                        .to_string();
                    if tx2
                        .send(AdbMsg::ShellOutput(serial2.clone(), line))
                        .is_err()
                    {
                        break "UI receiver disconnected".to_string();
                    }
                }
            }
        };
        let _ = tx2.send(AdbMsg::ShellExited(serial2, exit_reason));
    });

    // Stderr reader thread — merge into output.
    let tx3 = tx;
    let serial3 = serial_owned;
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut buf = Vec::with_capacity(1024);
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break,
                Err(error) => {
                    let _ = tx3.send(AdbMsg::ShellOutput(
                        serial3.clone(),
                        format!("--- stderr read failed: {error} ---"),
                    ));
                    break;
                }
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf)
                        .trim_end_matches(['\n', '\r'])
                        .to_string();
                    let _ = tx3.send(AdbMsg::ShellOutput(serial3.clone(), line));
                }
            }
        }
    });

    Some(ShellHandle { child, stdin })
}
