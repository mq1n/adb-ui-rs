//! Screen mirroring via a custom on-device server with ADB reverse/forward transport.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use openh264::formats::YUVSource;

use super::{
    adb_command, adb_path, is_wsa_serial, sdk_root_candidates, AdbLogLevel, AdbMsg, CommandExt,
    CREATE_NO_WINDOW,
};

const EMBEDDED_SERVER_SOURCE: &str = include_str!("../../server/Server.java");

/// Maximum size of the H.264 accumulation buffer.
const MAX_STREAM_BUF: usize = 4 * 1024 * 1024;
const STREAM_BUF_RETAIN_TAIL: usize = 256 * 1024;
const MAX_CONSECUTIVE_DECODE_ERRORS: u32 = 200;
const MIRROR_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MIRROR_CONNECT_RETRY_DELAY: Duration = Duration::from_millis(150);
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(3);
const SCREENCAP_POLL_INTERVAL: Duration = Duration::from_millis(200);
const MIRROR_DISPLAY_STATE_POLL_INTERVAL: Duration = Duration::from_millis(750);
const MIN_VIDEO_EDGE: u32 = 2;
const MAX_SERVER_LOG_LINES: usize = 20;

/// Remote path where the server JAR is pushed.
pub const SERVER_REMOTE_PATH: &str = "/data/local/tmp/mirror-server.jar";

/// Local-abstract socket name prefix the server listens on.
const SERVER_SOCKET_NAME: &str = "adb-mirror";

/// Main class in the server JAR.
const SERVER_MAIN_CLASS: &str = "com.adbui.server.Server";

fn send_mirror_log(tx: &Sender<AdbMsg>, serial: &str, level: AdbLogLevel, msg: impl Into<String>) {
    let _ = tx.send(AdbMsg::MirrorLog(serial.to_string(), level, msg.into()));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MirrorMode {
    #[default]
    Screenrecord,
    Server,
}

impl MirrorMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Screenrecord => "Screenrecord",
            Self::Server => "Server",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceRotation {
    Portrait,
    LandscapeLeft,
    ReversePortrait,
    LandscapeRight,
}

impl DeviceRotation {
    pub const ALL: [Self; 4] = [
        Self::Portrait,
        Self::LandscapeLeft,
        Self::ReversePortrait,
        Self::LandscapeRight,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Portrait => "Portrait",
            Self::LandscapeLeft => "Landscape Left",
            Self::ReversePortrait => "Reverse Portrait",
            Self::LandscapeRight => "Landscape Right",
        }
    }

    const fn wm_value(self) -> &'static str {
        match self {
            Self::Portrait => "0",
            Self::LandscapeLeft => "1",
            Self::ReversePortrait => "2",
            Self::LandscapeRight => "3",
        }
    }

    const fn from_wm_value(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Portrait),
            1 => Some(Self::LandscapeLeft),
            2 => Some(Self::ReversePortrait),
            3 => Some(Self::LandscapeRight),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeviceRotationMode {
    #[default]
    Auto,
    Locked(DeviceRotation),
}

impl DeviceRotationMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto Rotate",
            Self::Locked(rotation) => rotation.label(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DisplayState {
    width: u32,
    height: u32,
    rotation: DeviceRotation,
    mode: DeviceRotationMode,
}

#[derive(Debug, Clone)]
pub struct MirrorConfig {
    pub width: u32,
    pub height: u32,
    pub bitrate: u32,
}

impl Default for MirrorConfig {
    fn default() -> Self {
        Self {
            width: 720,
            height: 1280,
            bitrate: 4_000_000,
        }
    }
}

impl MirrorConfig {
    fn normalized(&self) -> Self {
        Self {
            width: self.width.max(320),
            height: self.height.max(320),
            bitrate: self.bitrate.clamp(500_000, 64_000_000),
        }
    }
}

pub struct MirrorFrame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

pub struct MirrorFrameBuffer {
    frame: Mutex<Option<MirrorFrame>>,
}

impl MirrorFrameBuffer {
    pub const fn new() -> Self {
        Self {
            frame: Mutex::new(None),
        }
    }

    pub fn put(&self, frame: MirrorFrame) {
        *self
            .frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(frame);
    }

    pub fn take(&self) -> Option<MirrorFrame> {
        self.frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

#[derive(Clone)]
pub struct MirrorControl {
    inner: Arc<Mutex<MirrorControlState>>,
}

struct MirrorControlState {
    serial: String,
    transport: MirrorControlTransport,
}

enum MirrorControlTransport {
    AdbShell,
    Socket(TcpStream),
}

#[derive(Clone, Copy)]
enum MirrorControlEvent {
    Tap {
        x: u32,
        y: u32,
    },
    Swipe {
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
        duration_ms: u32,
    },
    Key {
        keycode: u32,
    },
}

impl MirrorControl {
    fn new(serial: &str) -> Self {
        Self {
            inner: Arc::new(Mutex::new(MirrorControlState {
                serial: serial.to_string(),
                transport: MirrorControlTransport::AdbShell,
            })),
        }
    }

    fn attach_socket(&self, stream: TcpStream) {
        let _ = stream.set_nodelay(true);
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.transport = MirrorControlTransport::Socket(stream);
    }

    fn detach_socket(&self) {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.transport = MirrorControlTransport::AdbShell;
    }

    pub fn send_tap(&self, x: u32, y: u32) {
        self.send_event(MirrorControlEvent::Tap { x, y });
    }

    pub fn send_swipe(&self, x1: u32, y1: u32, x2: u32, y2: u32, duration_ms: u32) {
        self.send_event(MirrorControlEvent::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration_ms,
        });
    }

    pub fn send_key_event(&self, keycode: u32) {
        self.send_event(MirrorControlEvent::Key { keycode });
    }

    fn send_event(&self, event: MirrorControlEvent) {
        let serial = {
            let mut state = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            if let MirrorControlTransport::Socket(stream) = &mut state.transport {
                if write_control_event(stream, event).is_ok() {
                    return;
                }
                state.transport = MirrorControlTransport::AdbShell;
            }

            state.serial.clone()
        };

        dispatch_adb_input(serial, event);
    }
}

fn write_control_event(stream: &mut TcpStream, event: MirrorControlEvent) -> std::io::Result<()> {
    match event {
        MirrorControlEvent::Tap { x, y } => {
            stream.write_all(&[1])?;
            stream.write_all(&x.to_be_bytes())?;
            stream.write_all(&y.to_be_bytes())?;
        }
        MirrorControlEvent::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration_ms,
        } => {
            stream.write_all(&[2])?;
            stream.write_all(&x1.to_be_bytes())?;
            stream.write_all(&y1.to_be_bytes())?;
            stream.write_all(&x2.to_be_bytes())?;
            stream.write_all(&y2.to_be_bytes())?;
            stream.write_all(&duration_ms.to_be_bytes())?;
        }
        MirrorControlEvent::Key { keycode } => {
            stream.write_all(&[3])?;
            stream.write_all(&keycode.to_be_bytes())?;
        }
    }

    stream.flush()
}

pub struct MirrorHandle {
    pub stop: Arc<AtomicBool>,
    pub frame_buffer: Arc<MirrorFrameBuffer>,
    pub control: MirrorControl,
}

struct ChildGuard(Option<Child>);

impl ChildGuard {
    const fn new(child: Child) -> Self {
        Self(Some(child))
    }

    fn wait(&mut self) {
        if let Some(ref mut child) = self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.0 = None;
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.wait();
    }
}

type StopAction = Box<dyn FnOnce(String) + Send>;

struct SendOnDrop {
    reason: Mutex<String>,
    action: Mutex<Option<StopAction>>,
}

impl SendOnDrop {
    fn new(action: impl FnOnce(String) + Send + 'static) -> Self {
        Self {
            reason: Mutex::new("Unexpected exit".into()),
            action: Mutex::new(Some(Box::new(action))),
        }
    }

    fn set_reason(&self, reason: String) {
        *self
            .reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = reason;
    }
}

impl Drop for SendOnDrop {
    fn drop(&mut self) {
        let reason = self
            .reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let action = self
            .action
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();

        if let Some(action) = action {
            action(reason);
        }
    }
}

struct ForwardCleanup {
    serial: String,
    port: u16,
}

impl Drop for ForwardCleanup {
    fn drop(&mut self) {
        let _ = remove_adb_forward(&self.serial, self.port);
    }
}

struct ReverseCleanup {
    serial: String,
    port: u16,
}

impl Drop for ReverseCleanup {
    fn drop(&mut self) {
        let _ = remove_adb_reverse(&self.serial, self.port);
    }
}

#[derive(Clone, Copy)]
struct StreamConfig {
    video_width: u32,
    video_height: u32,
    bitrate: u32,
    display_width: u32,
    display_height: u32,
}

impl StreamConfig {
    fn fallback(request: &MirrorConfig) -> Self {
        let request = request.normalized();
        Self {
            video_width: make_even_dimension(request.width),
            video_height: make_even_dimension(request.height),
            bitrate: request.bitrate,
            display_width: 0,
            display_height: 0,
        }
    }
}

enum ServerTransport {
    Reverse {
        video_listener: TcpListener,
        control_listener: TcpListener,
        video_port: u16,
        control_port: u16,
        _video_cleanup: ReverseCleanup,
        _control_cleanup: ReverseCleanup,
    },
    Forward {
        video_port: u16,
        control_port: u16,
        video_socket_name: String,
        control_socket_name: String,
        _video_cleanup: ForwardCleanup,
        _control_cleanup: ForwardCleanup,
    },
}

impl ServerTransport {
    fn server_args(&self) -> Vec<String> {
        match self {
            Self::Reverse {
                video_port,
                control_port,
                ..
            } => vec![
                "reverse".to_string(),
                video_port.to_string(),
                control_port.to_string(),
            ],
            Self::Forward {
                video_socket_name,
                control_socket_name,
                ..
            } => vec![
                "forward".to_string(),
                video_socket_name.clone(),
                control_socket_name.clone(),
            ],
        }
    }
}

pub fn start_mirror(
    serial: &str,
    session: u64,
    config: &MirrorConfig,
    mode: MirrorMode,
    tx: Sender<AdbMsg>,
) -> Result<MirrorHandle, String> {
    let _ = adb_path()?;

    let stop = Arc::new(AtomicBool::new(false));
    let frame_buffer = Arc::new(MirrorFrameBuffer::new());
    let control = MirrorControl::new(serial);

    let stop_c = stop.clone();
    let buf_c = frame_buffer.clone();
    let control_c = control.clone();
    let serial_c = serial.to_string();
    let config_c = config.clone();
    let tx_worker = tx.clone();

    std::thread::Builder::new()
        .name(format!("mirror-{serial}"))
        .spawn(move || match mode {
            MirrorMode::Screenrecord => {
                worker_screenrecord(&serial_c, session, &config_c, &stop_c, &buf_c, &tx_worker);
            }
            MirrorMode::Server => {
                worker_server(
                    &serial_c, session, &config_c, &stop_c, &buf_c, &control_c, &tx_worker,
                );
            }
        })
        .map_err(|error| format!("Failed to spawn mirror thread: {error}"))?;

    spawn_display_state_watcher(serial, session, stop.clone(), tx);

    Ok(MirrorHandle {
        stop,
        frame_buffer,
        control,
    })
}

fn spawn_display_state_watcher(
    serial: &str,
    session: u64,
    stop: Arc<AtomicBool>,
    tx: Sender<AdbMsg>,
) {
    let serial = serial.to_string();
    std::thread::spawn(move || {
        let mut last_state: Option<DisplayState> = None;
        while !stop.load(Ordering::Acquire) {
            if let Ok(state) = get_display_state(&serial) {
                let changed = last_state != Some(state);
                if changed {
                    let _ = tx.send(AdbMsg::MirrorDisplayState(
                        serial.clone(),
                        session,
                        state.width,
                        state.height,
                        state.rotation,
                        state.mode,
                    ));
                    last_state = Some(state);
                }
            }
            std::thread::sleep(MIRROR_DISPLAY_STATE_POLL_INTERVAL);
        }
    });
}

fn worker_screenrecord(
    serial: &str,
    session: u64,
    config: &MirrorConfig,
    stop: &AtomicBool,
    frame_buffer: &MirrorFrameBuffer,
    tx: &Sender<AdbMsg>,
) {
    let stream_config = resolve_stream_config_or_fallback(serial, session, config, tx);
    send_mirror_log(
        tx,
        serial,
        AdbLogLevel::Warn,
        format!(
            "Using legacy screenrecord transport at {}x{} / {}",
            stream_config.video_width, stream_config.video_height, stream_config.bitrate
        ),
    );

    if is_wsa_serial(serial) {
        send_mirror_log(
            tx,
            serial,
            AdbLogLevel::Warn,
            "WSA detected; skipping server streaming and using screencap polling compatibility mode",
        );

        let serial_owned = serial.to_string();
        let tx_g = tx.clone();
        let stopped_guard = SendOnDrop::new(move |reason| {
            let _ = tx_g.send(AdbMsg::MirrorStopped(serial_owned, session, reason));
        });

        match run_screencap_poll_loop(serial, stop, frame_buffer) {
            Ok(()) => stopped_guard.set_reason("Stopped".into()),
            Err(error) => stopped_guard.set_reason(error),
        }
        return;
    }

    let serial_owned = serial.to_string();
    let tx_g = tx.clone();
    let stopped_guard = SendOnDrop::new(move |reason| {
        let _ = tx_g.send(AdbMsg::MirrorStopped(serial_owned, session, reason));
    });

    loop {
        if stop.load(Ordering::Acquire) {
            stopped_guard.set_reason("Stopped".into());
            return;
        }

        match run_screenrecord_session(serial, stream_config, stop, frame_buffer) {
            Ok(true) => {
                if stop.load(Ordering::Acquire) {
                    stopped_guard.set_reason("Stopped".into());
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Ok(false) => {
                stopped_guard.set_reason("Stopped".into());
                return;
            }
            Err(error) => {
                stopped_guard.set_reason(error);
                return;
            }
        }
    }
}

fn run_screenrecord_loop(
    serial: &str,
    stream_config: StreamConfig,
    stop: &AtomicBool,
    frame_buffer: &MirrorFrameBuffer,
) -> Result<(), String> {
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }

        match run_screenrecord_session(serial, stream_config, stop, frame_buffer) {
            Ok(true) => {
                if stop.load(Ordering::Acquire) {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Ok(false) => return Ok(()),
            Err(error) => return Err(error),
        }
    }
}

fn run_screencap_poll_loop(
    serial: &str,
    stop: &AtomicBool,
    frame_buffer: &MirrorFrameBuffer,
) -> Result<(), String> {
    while !stop.load(Ordering::Acquire) {
        let png = super::capture_screenshot_bytes(serial)?;
        let image = image::load_from_memory(&png)
            .map_err(|error| format!("Failed to decode screencap PNG: {error}"))?;
        let rgba = image.to_rgba8();
        let (width, height) = rgba.dimensions();
        frame_buffer.put(MirrorFrame {
            width: usize::try_from(width).unwrap_or(0),
            height: usize::try_from(height).unwrap_or(0),
            rgba: rgba.into_raw(),
        });

        std::thread::sleep(SCREENCAP_POLL_INTERVAL);
    }

    Ok(())
}

fn run_screenrecord_session(
    serial: &str,
    stream_config: StreamConfig,
    stop: &AtomicBool,
    frame_buffer: &MirrorFrameBuffer,
) -> Result<bool, String> {
    let mut cmd = adb_command().ok_or_else(|| "ADB not available".to_string())?;

    cmd.args([
        "-s",
        serial,
        "exec-out",
        "screenrecord",
        "--output-format=h264",
        "--size",
        &format!(
            "{}x{}",
            stream_config.video_width, stream_config.video_height
        ),
        "--bit-rate",
        &stream_config.bitrate.to_string(),
        "--time-limit",
        "180",
        "-",
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .creation_flags(CREATE_NO_WINDOW);

    let child = cmd
        .spawn()
        .map_err(|error| format!("screenrecord spawn failed: {error}"))?;
    let mut guard = ChildGuard::new(child);
    let stdout = guard
        .0
        .as_mut()
        .and_then(|child| child.stdout.take())
        .ok_or_else(|| "No stdout from screenrecord".to_string())?;

    decode_stream(stdout, stop, frame_buffer, true)
}

fn worker_server(
    serial: &str,
    session: u64,
    config: &MirrorConfig,
    stop: &AtomicBool,
    frame_buffer: &MirrorFrameBuffer,
    control: &MirrorControl,
    tx: &Sender<AdbMsg>,
) {
    let stream_config = resolve_stream_config_or_fallback(serial, session, config, tx);

    if is_wsa_serial(serial) {
        send_mirror_log(
            tx,
            serial,
            AdbLogLevel::Warn,
            format!(
                "WSA detected; skipping server streaming and using screencap polling compatibility mode at {}x{}",
                stream_config.display_width.max(stream_config.video_width),
                stream_config.display_height.max(stream_config.video_height)
            ),
        );

        let serial_owned = serial.to_string();
        let tx_g = tx.clone();
        let stopped_guard = SendOnDrop::new(move |reason| {
            let _ = tx_g.send(AdbMsg::MirrorStopped(serial_owned, session, reason));
        });

        match run_screencap_poll_loop(serial, stop, frame_buffer) {
            Ok(()) => stopped_guard.set_reason("Stopped".into()),
            Err(error) => stopped_guard.set_reason(error),
        }
        return;
    }

    send_mirror_log(
        tx,
        serial,
        AdbLogLevel::Info,
        format!(
            "Preparing server transport at {}x{} / {}",
            stream_config.video_width, stream_config.video_height, stream_config.bitrate
        ),
    );

    let serial_owned = serial.to_string();
    let tx_g = tx.clone();
    let stopped_guard = SendOnDrop::new(move |reason| {
        let _ = tx_g.send(AdbMsg::MirrorStopped(serial_owned, session, reason));
    });

    if let Err(error) = ensure_server_installed(serial, tx) {
        send_mirror_log(
            tx,
            serial,
            AdbLogLevel::Error,
            format!("Mirror server preparation failed: {error}"),
        );
        stopped_guard.set_reason(format!("Mirror server preparation failed: {error}"));
        return;
    }

    let _ = kill_server(serial);

    let reverse_error = match setup_reverse_transport(serial) {
        Ok(transport) => {
            if let ServerTransport::Reverse {
                video_port,
                control_port,
                ..
            } = &transport
            {
                send_mirror_log(
                    tx,
                    serial,
                    AdbLogLevel::Info,
                    format!(
                        "Using adb reverse transport (video tcp:{video_port}, control tcp:{control_port})"
                    ),
                );
            }

            match run_server_transport(
                serial,
                stream_config,
                transport,
                stop,
                frame_buffer,
                control,
                tx,
            ) {
                Ok(()) => {
                    stopped_guard.set_reason("Stopped".into());
                    return;
                }
                Err(error) if !stop.load(Ordering::Acquire) => {
                    send_mirror_log(
                        tx,
                        serial,
                        AdbLogLevel::Warn,
                        format!("Reverse transport failed, retrying with adb forward: {error}"),
                    );
                    Some(error)
                }
                Err(error) => {
                    stopped_guard.set_reason(error);
                    return;
                }
            }
        }
        Err(error) => {
            send_mirror_log(
                tx,
                serial,
                AdbLogLevel::Warn,
                format!("ADB reverse unavailable, using adb forward: {error}"),
            );
            Some(error)
        }
    };

    let transport = match setup_forward_transport(serial, session) {
        Ok(transport) => transport,
        Err(error) => {
            send_mirror_log(
                tx,
                serial,
                AdbLogLevel::Error,
                format!("Mirror transport setup failed: {error}"),
            );
            stopped_guard.set_reason(error);
            return;
        }
    };
    if let ServerTransport::Forward {
        video_port,
        control_port,
        ..
    } = &transport
    {
        send_mirror_log(
            tx,
            serial,
            AdbLogLevel::Info,
            format!(
                "Using adb forward transport (video tcp:{video_port}, control tcp:{control_port})"
            ),
        );
    }

    let server_error = match run_server_transport(
        serial,
        stream_config,
        transport,
        stop,
        frame_buffer,
        control,
        tx,
    ) {
        Ok(()) => {
            stopped_guard.set_reason("Stopped".into());
            return;
        }
        Err(error) => {
            send_mirror_log(
                tx,
                serial,
                AdbLogLevel::Error,
                format!("Mirror transport failed: {error}"),
            );
            error
        }
    };

    if stop.load(Ordering::Acquire) {
        stopped_guard.set_reason("Stopped".into());
        return;
    }

    let server_error = reverse_error
        .map(|error| format!("{error}; then forward failed: {server_error}"))
        .unwrap_or(server_error);
    send_mirror_log(
        tx,
        serial,
        AdbLogLevel::Warn,
        format!("Falling back to screenrecord transport: {server_error}"),
    );
    match run_screenrecord_loop(serial, stream_config, stop, frame_buffer) {
        Ok(()) => stopped_guard.set_reason("Stopped".into()),
        Err(error) => {
            if stop.load(Ordering::Acquire) {
                stopped_guard.set_reason("Stopped".into());
                return;
            }
            send_mirror_log(
                tx,
                serial,
                AdbLogLevel::Warn,
                format!("Screenrecord fallback failed, switching to screencap polling: {error}"),
            );
            match run_screencap_poll_loop(serial, stop, frame_buffer) {
                Ok(()) => stopped_guard.set_reason("Stopped".into()),
                Err(screencap_error) => {
                    stopped_guard.set_reason(format!(
                        "Server path failed: {server_error}; screenrecord fallback failed: {error}; screencap fallback failed: {screencap_error}"
                    ));
                }
            }
        }
    }
}

fn ensure_server_installed(serial: &str, tx: &Sender<AdbMsg>) -> Result<(), String> {
    if is_server_installed(serial)? {
        send_mirror_log(
            tx,
            serial,
            AdbLogLevel::Info,
            "Mirror server already installed",
        );
    } else {
        send_mirror_log(
            tx,
            serial,
            AdbLogLevel::Warn,
            "Mirror server missing; building and installing embedded server",
        );
        let jar_path = build_server()?;
        push_server(serial, &jar_path.display().to_string())?;
        send_mirror_log(
            tx,
            serial,
            AdbLogLevel::Info,
            "Embedded mirror server installed",
        );
    }
    Ok(())
}

fn run_server_transport(
    serial: &str,
    stream_config: StreamConfig,
    transport: ServerTransport,
    stop: &AtomicBool,
    frame_buffer: &MirrorFrameBuffer,
    control: &MirrorControl,
    tx: &Sender<AdbMsg>,
) -> Result<(), String> {
    let transport_args = transport.server_args();
    let width = stream_config.video_width.to_string();
    let height = stream_config.video_height.to_string();
    let bitrate = stream_config.bitrate.to_string();

    let Some(mut cmd) = adb_command() else {
        send_mirror_log(tx, serial, AdbLogLevel::Error, "ADB not available");
        return Err("ADB not available".into());
    };

    cmd.args([
        "-s",
        serial,
        "shell",
        &format!("CLASSPATH={SERVER_REMOTE_PATH}"),
        "app_process",
        "/",
        SERVER_MAIN_CLASS,
        &width,
        &height,
        &bitrate,
    ]);
    cmd.args(&transport_args);
    let Ok(mut child) = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    else {
        send_mirror_log(
            tx,
            serial,
            AdbLogLevel::Error,
            "Failed to start mirror server process",
        );
        return Err("Failed to start server process".into());
    };
    send_mirror_log(
        tx,
        serial,
        AdbLogLevel::Info,
        "Mirror server process launched",
    );

    let server_logs = spawn_server_log_collector(&mut child, tx.clone(), serial.to_string());
    let mut server_guard = ChildGuard::new(child);

    std::thread::sleep(Duration::from_millis(1200));
    if stop.load(Ordering::Acquire) {
        return Err("Stopped".into());
    }

    if let Some(child) = server_guard.0.as_mut() {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(append_server_logs(
                format!(
                    "Mirror server exited during startup{}",
                    format_exit_code(status.code())
                ),
                &server_logs,
            ));
        }
    }

    let (video_stream, control_stream) = match establish_server_channels(transport, stop) {
        Ok(channels) => channels,
        Err(error) => {
            send_mirror_log(
                tx,
                serial,
                AdbLogLevel::Error,
                format!("Mirror channel setup failed: {error}"),
            );
            let _ = kill_server(serial);
            return Err(append_server_logs(error, &server_logs));
        }
    };
    send_mirror_log(
        tx,
        serial,
        AdbLogLevel::Info,
        "Mirror video and control channels connected",
    );

    let _ = video_stream.set_read_timeout(Some(Duration::from_millis(300)));
    control.attach_socket(control_stream);
    send_mirror_log(
        tx,
        serial,
        AdbLogLevel::Info,
        "Mirror control channel attached",
    );

    let result = decode_stream(video_stream, stop, frame_buffer, false)
        .map(|_| ())
        .map_err(|error| append_server_logs(error, &server_logs));

    control.detach_socket();
    let _ = kill_server(serial);
    result
}

fn setup_reverse_transport(serial: &str) -> Result<ServerTransport, String> {
    let video_listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("Failed to bind reverse video listener: {error}"))?;
    let control_listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("Failed to bind reverse control listener: {error}"))?;

    let video_port = video_listener
        .local_addr()
        .map_err(|error| format!("Failed to inspect reverse video listener: {error}"))?
        .port();
    let control_port = control_listener
        .local_addr()
        .map_err(|error| format!("Failed to inspect reverse control listener: {error}"))?
        .port();

    setup_adb_reverse(serial, video_port, video_port)?;
    if let Err(error) = setup_adb_reverse(serial, control_port, control_port) {
        let _ = remove_adb_reverse(serial, video_port);
        return Err(error);
    }

    Ok(ServerTransport::Reverse {
        video_listener,
        control_listener,
        video_port,
        control_port,
        _video_cleanup: ReverseCleanup {
            serial: serial.to_string(),
            port: video_port,
        },
        _control_cleanup: ReverseCleanup {
            serial: serial.to_string(),
            port: control_port,
        },
    })
}

fn setup_forward_transport(serial: &str, session: u64) -> Result<ServerTransport, String> {
    let video_socket_name = format!("{SERVER_SOCKET_NAME}-video-{session}");
    let control_socket_name = format!("{SERVER_SOCKET_NAME}-control-{session}");
    let video_port = setup_adb_forward(serial, &format!("localabstract:{video_socket_name}"))?;
    let control_port =
        match setup_adb_forward(serial, &format!("localabstract:{control_socket_name}")) {
            Ok(port) => port,
            Err(error) => {
                let _ = remove_adb_forward(serial, video_port);
                return Err(error);
            }
        };

    Ok(ServerTransport::Forward {
        video_port,
        control_port,
        video_socket_name,
        control_socket_name,
        _video_cleanup: ForwardCleanup {
            serial: serial.to_string(),
            port: video_port,
        },
        _control_cleanup: ForwardCleanup {
            serial: serial.to_string(),
            port: control_port,
        },
    })
}

fn establish_server_channels(
    transport: ServerTransport,
    stop: &AtomicBool,
) -> Result<(TcpStream, TcpStream), String> {
    match transport {
        ServerTransport::Reverse {
            video_listener,
            control_listener,
            ..
        } => {
            let video_stream = accept_listener(&video_listener, stop, "video reverse connection")?;
            let control_stream =
                accept_listener(&control_listener, stop, "control reverse connection")?;
            Ok((video_stream, control_stream))
        }
        ServerTransport::Forward {
            video_port,
            control_port,
            ..
        } => {
            let video_stream = connect_forwarded_socket(video_port, stop)?;
            let control_stream = connect_forwarded_socket(control_port, stop)?;
            Ok((video_stream, control_stream))
        }
    }
}

fn accept_listener(
    listener: &TcpListener,
    stop: &AtomicBool,
    description: &str,
) -> Result<TcpStream, String> {
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("Failed to set nonblocking accept for {description}: {error}"))?;

    let deadline = Instant::now() + MIRROR_CONNECT_TIMEOUT;
    let mut last_error = None;
    loop {
        if stop.load(Ordering::Acquire) {
            return Err("Stopped".into());
        }

        match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_nodelay(true);
                return Ok(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => last_error = Some(error.to_string()),
        }

        if Instant::now() >= deadline {
            return Err(format!(
                "Timed out waiting for {description}: {}",
                last_error.unwrap_or_else(|| "no incoming connection".to_string())
            ));
        }

        std::thread::sleep(MIRROR_CONNECT_RETRY_DELAY);
    }
}

fn connect_forwarded_socket(port: u16, stop: &AtomicBool) -> Result<TcpStream, String> {
    let address = format!("127.0.0.1:{port}");
    let deadline = Instant::now() + MIRROR_CONNECT_TIMEOUT;
    let mut last_error = None;

    while Instant::now() < deadline {
        if stop.load(Ordering::Acquire) {
            return Err("Stopped".into());
        }

        match TcpStream::connect(&address) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                return Ok(stream);
            }
            Err(error) => last_error = Some(error.to_string()),
        }

        std::thread::sleep(MIRROR_CONNECT_RETRY_DELAY);
    }

    Err(format!(
        "TCP connect failed: {}",
        last_error.unwrap_or_else(|| "timed out waiting for mirror server".to_string())
    ))
}

fn decode_stream(
    source: impl Read,
    stop: &AtomicBool,
    frame_buffer: &MirrorFrameBuffer,
    allow_restart: bool,
) -> Result<bool, String> {
    let mut decoder = openh264::decoder::Decoder::new()
        .map_err(|error| format!("H.264 decoder init failed: {error}"))?;

    let mut reader = BufReader::with_capacity(256 * 1024, source);
    let mut stream_buf = Vec::with_capacity(512 * 1024);
    let mut read_buf = vec![0u8; 65_536];
    let started_at = Instant::now();
    let mut got_data = false;
    let mut got_frame = false;
    let mut consecutive_decode_errors = 0;

    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(false);
        }

        match reader.read(&mut read_buf) {
            Ok(0) => {
                if !got_data && allow_restart {
                    return Err(
                        "screenrecord produced no H.264 output. This usually means the device or requested size is not supported."
                            .into(),
                    );
                }

                let (consumed, decoded_frame) = process_nal_units(
                    &mut decoder,
                    &stream_buf,
                    frame_buffer,
                    &mut consecutive_decode_errors,
                    true,
                )?;
                let _ = decoded_frame;
                if consumed > 0 {
                    stream_buf.drain(..consumed);
                }

                return Ok(allow_restart && got_data);
            }
            Ok(n) => {
                got_data = true;
                stream_buf.extend_from_slice(&read_buf[..n]);
                compact_stream_buffer(&mut stream_buf);

                let (consumed, decoded_frame) = process_nal_units(
                    &mut decoder,
                    &stream_buf,
                    frame_buffer,
                    &mut consecutive_decode_errors,
                    false,
                )?;
                got_frame |= decoded_frame;
                if consumed > 0 {
                    stream_buf.drain(..consumed);
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(format!("Stream read error: {error}")),
        }

        if !got_frame && started_at.elapsed() >= FIRST_FRAME_TIMEOUT {
            return Err("Timed out waiting for the first decoded video frame".into());
        }
    }
}

fn compact_stream_buffer(stream_buf: &mut Vec<u8>) {
    if stream_buf.len() <= MAX_STREAM_BUF {
        return;
    }

    let retain_from = stream_buf.len().saturating_sub(STREAM_BUF_RETAIN_TAIL);
    if let Some(relative_offset) = find_first_start_code(&stream_buf[retain_from..]) {
        stream_buf.drain(..retain_from + relative_offset);
    } else {
        stream_buf.clear();
    }
}

fn process_nal_units(
    decoder: &mut openh264::decoder::Decoder,
    data: &[u8],
    frame_buffer: &MirrorFrameBuffer,
    consecutive_errors: &mut u32,
    flush_tail: bool,
) -> Result<(usize, bool), String> {
    let positions = find_start_code_positions(data);
    if positions.is_empty() {
        return Ok((0, false));
    }

    let mut consumed = 0;
    let mut decoded_frame = false;
    for pair in positions.windows(2) {
        decoded_frame |= decode_nal(
            decoder,
            &data[pair[0]..pair[1]],
            frame_buffer,
            consecutive_errors,
        )?;
        consumed = pair[1];
    }

    if flush_tail {
        let start = *positions.last().unwrap_or(&0);
        if start < data.len() {
            decoded_frame |= decode_nal(decoder, &data[start..], frame_buffer, consecutive_errors)?;
            consumed = data.len();
        }
    }

    Ok((consumed, decoded_frame))
}

fn decode_nal(
    decoder: &mut openh264::decoder::Decoder,
    nal: &[u8],
    frame_buffer: &MirrorFrameBuffer,
    consecutive_errors: &mut u32,
) -> Result<bool, String> {
    match decoder.decode(nal) {
        Ok(Some(yuv)) => {
            *consecutive_errors = 0;
            let (width, height) = yuv.dimensions();
            if width > 0 && height > 0 {
                let mut rgba = vec![0u8; width * height * 4];
                yuv.write_rgba8(&mut rgba);
                frame_buffer.put(MirrorFrame {
                    width,
                    height,
                    rgba,
                });
                return Ok(true);
            }
        }
        Ok(None) => {}
        Err(_) => {
            *consecutive_errors += 1;
            if *consecutive_errors > MAX_CONSECUTIVE_DECODE_ERRORS {
                return Err(
                    "H.264 decoder failed repeatedly; the stream is likely corrupted".into(),
                );
            }
        }
    }

    Ok(false)
}

fn find_start_code_positions(data: &[u8]) -> Vec<usize> {
    let mut positions = Vec::new();
    if data.len() < 3 {
        return positions;
    }

    let mut i = 0;
    while i + 2 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 {
            if data[i + 2] == 1 {
                positions.push(i);
                i += 3;
                continue;
            }
            if i + 3 < data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                positions.push(i);
                i += 4;
                continue;
            }
        }
        i += 1;
    }

    positions
}

fn find_first_start_code(data: &[u8]) -> Option<usize> {
    find_start_code_positions(data).into_iter().next()
}

pub fn check_server_status(serial: &str) -> Result<(bool, bool), String> {
    Ok((
        is_server_installed(serial)?,
        !server_pids(serial)?.is_empty(),
    ))
}

pub fn push_server(serial: &str, local_path: &str) -> Result<(), String> {
    let mut cmd = adb_command().ok_or_else(|| "ADB not available".to_string())?;
    let output = cmd
        .args(["-s", serial, "push", local_path, SERVER_REMOTE_PATH])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("adb push failed: {error}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(describe_process_output(&output))
    }
}

pub fn remove_server(serial: &str) -> Result<(), String> {
    let output = adb_shell_output(serial, &["rm", "-f", SERVER_REMOTE_PATH])?;
    if output.status.success() {
        Ok(())
    } else {
        Err(describe_process_output(&output))
    }
}

pub fn kill_server(serial: &str) -> Result<usize, String> {
    let pids = server_pids(serial)?;
    for pid in &pids {
        let output = adb_shell_output(serial, &["kill", &pid.to_string()])?;
        if !output.status.success() {
            return Err(format!(
                "Failed to kill mirror server pid {pid}: {}",
                describe_process_output(&output)
            ));
        }
    }

    Ok(pids.len())
}

pub fn build_server() -> Result<PathBuf, String> {
    let android_jar = find_android_jar()?;
    let javac =
        find_tool_on_path("javac").ok_or_else(|| "javac not found; install a JDK".to_string())?;
    let d8 = find_d8()?;

    let build_dir = mirror_build_dir();
    let classes_dir = build_dir.join("classes");
    let source_path = build_dir.join("Server.java");
    let jar_path = build_dir.join("mirror-server.jar");

    std::fs::create_dir_all(&build_dir)
        .map_err(|error| format!("Failed to create build directory: {error}"))?;
    if classes_dir.exists() {
        std::fs::remove_dir_all(&classes_dir)
            .map_err(|error| format!("Failed to clear class output directory: {error}"))?;
    }
    if jar_path.exists() {
        std::fs::remove_file(&jar_path)
            .map_err(|error| format!("Failed to clear previous server jar: {error}"))?;
    }

    std::fs::create_dir_all(&classes_dir)
        .map_err(|error| format!("Failed to create class output directory: {error}"))?;
    std::fs::write(&source_path, EMBEDDED_SERVER_SOURCE)
        .map_err(|error| format!("Failed to write embedded server source: {error}"))?;

    let javac_output = std::process::Command::new(&javac)
        .args([
            "-source",
            "1.8",
            "-target",
            "1.8",
            "-cp",
            &android_jar.display().to_string(),
            "-d",
            &classes_dir.display().to_string(),
            &source_path.display().to_string(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("javac failed to start: {error}"))?;
    if !javac_output.status.success() {
        return Err(format!(
            "javac compilation failed: {}",
            describe_process_output(&javac_output)
        ));
    }

    let mut class_files = Vec::new();
    collect_class_files(&classes_dir, &mut class_files);
    if class_files.is_empty() {
        return Err("javac did not produce any .class files".into());
    }

    let mut d8_cmd = std::process::Command::new(&d8);
    d8_cmd
        .arg("--output")
        .arg(&jar_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    for class_file in &class_files {
        d8_cmd.arg(class_file);
    }

    let d8_output = d8_cmd
        .output()
        .map_err(|error| format!("d8 failed to start: {error}"))?;
    if !d8_output.status.success() {
        return Err(format!(
            "d8 DEX conversion failed: {}",
            describe_process_output(&d8_output)
        ));
    }
    if !jar_path.exists() {
        return Err("d8 completed without producing mirror-server.jar".into());
    }

    Ok(jar_path)
}

fn mirror_build_dir() -> PathBuf {
    let mut hasher = DefaultHasher::new();
    EMBEDDED_SERVER_SOURCE.hash(&mut hasher);

    std::env::temp_dir()
        .join("adb-ui-rs")
        .join("mirror-server")
        .join(format!("{:016x}", hasher.finish()))
}

fn setup_adb_reverse(serial: &str, device_port: u16, host_port: u16) -> Result<(), String> {
    let mut cmd = adb_command().ok_or_else(|| "ADB not available".to_string())?;
    let output = cmd
        .args([
            "-s",
            serial,
            "reverse",
            &format!("tcp:{device_port}"),
            &format!("tcp:{host_port}"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("adb reverse failed: {error}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(describe_process_output(&output))
    }
}

fn remove_adb_reverse(serial: &str, port: u16) -> Result<(), String> {
    let mut cmd = adb_command().ok_or_else(|| "ADB not available".to_string())?;
    let _ = cmd
        .args(["-s", serial, "reverse", "--remove", &format!("tcp:{port}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
    Ok(())
}

fn setup_adb_forward(serial: &str, remote_spec: &str) -> Result<u16, String> {
    let mut cmd = adb_command().ok_or_else(|| "ADB not available".to_string())?;
    let output = cmd
        .args(["-s", serial, "forward", "tcp:0", remote_spec])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("adb forward failed: {error}"))?;

    if !output.status.success() {
        return Err(describe_process_output(&output));
    }

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u16>()
        .map_err(|_| "adb forward did not return a valid port".into())
}

fn remove_adb_forward(serial: &str, port: u16) -> Result<(), String> {
    let mut cmd = adb_command().ok_or_else(|| "ADB not available".to_string())?;
    let _ = cmd
        .args(["-s", serial, "forward", "--remove", &format!("tcp:{port}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
    Ok(())
}

pub fn get_display_size(serial: &str) -> Result<(u32, u32), String> {
    if let Ok(state) = get_display_state(serial) {
        return Ok((state.width, state.height));
    }

    let output = adb_shell_output(serial, &["wm", "size"])?;
    if !output.status.success() {
        return Err(describe_process_output(&output));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut physical = None;
    let mut override_size = None;
    for line in text.lines() {
        if let Some(size) = line.strip_prefix("Physical size: ") {
            physical = parse_wxh(size);
        }
        if let Some(size) = line.strip_prefix("Override size: ") {
            override_size = parse_wxh(size);
        }
    }

    override_size
        .or(physical)
        .ok_or_else(|| "Could not parse display size".into())
}

fn get_display_state(serial: &str) -> Result<DisplayState, String> {
    let output = adb_shell_output(serial, &["dumpsys", "window", "displays"])?;
    if !output.status.success() {
        return Err(describe_process_output(&output));
    }

    parse_display_state(&String::from_utf8_lossy(&output.stdout))
}

fn parse_display_state(text: &str) -> Result<DisplayState, String> {
    let mut width = None;
    let mut height = None;
    let mut current_rotation = None;
    let mut user_rotation = None;
    let mut user_locked = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(cur) = trimmed
            .split_whitespace()
            .find_map(|part| part.strip_prefix("cur="))
        {
            if let Some((w, h)) = parse_wxh(cur) {
                width = Some(w);
                height = Some(h);
            }
        }
        if let Some(rotation) = trimmed.strip_prefix("mCurrentRotation=ROTATION_") {
            current_rotation =
                parse_rotation_number(rotation).and_then(DeviceRotation::from_wm_value);
        }
        if let Some(rest) = trimmed.strip_prefix("mUserRotationMode=") {
            user_locked = Some(rest.starts_with("USER_ROTATION_LOCKED"));
            if let Some(value) = rest
                .split_whitespace()
                .find_map(|part| part.strip_prefix("mUserRotation=ROTATION_"))
            {
                user_rotation =
                    parse_rotation_number(value).and_then(DeviceRotation::from_wm_value);
            }
        }
    }

    let width = width.ok_or_else(|| "Could not parse current display width".to_string())?;
    let height = height.ok_or_else(|| "Could not parse current display height".to_string())?;
    let rotation =
        current_rotation.ok_or_else(|| "Could not parse current display rotation".to_string())?;
    let mode = match (user_locked, user_rotation) {
        (Some(true), Some(rotation)) => DeviceRotationMode::Locked(rotation),
        _ => DeviceRotationMode::Auto,
    };

    Ok(DisplayState {
        width,
        height,
        rotation,
        mode,
    })
}

pub fn apply_device_rotation(serial: &str, mode: DeviceRotationMode) -> Result<(), String> {
    let command_sequences = rotation_command_sequences(mode);
    let mut errors = Vec::new();

    for sequence in command_sequences {
        match run_shell_sequence(serial, &sequence) {
            Ok(()) => return Ok(()),
            Err(error) => errors.push(format!("{}: {error}", format_shell_sequence(&sequence))),
        }
    }

    Err(errors.join("; "))
}

fn rotation_command_sequences(mode: DeviceRotationMode) -> Vec<Vec<Vec<String>>> {
    match mode {
        DeviceRotationMode::Auto => vec![
            vec![
                vec![
                    "wm".into(),
                    "fixed-to-user-rotation".into(),
                    "default".into(),
                ],
                vec!["wm".into(), "user-rotation".into(), "free".into()],
            ],
            vec![
                vec![
                    "wm".into(),
                    "fixed-to-user-rotation".into(),
                    "disabled".into(),
                ],
                vec!["wm".into(), "user-rotation".into(), "free".into()],
            ],
            vec![
                vec![
                    "cmd".into(),
                    "window".into(),
                    "fixed-to-user-rotation".into(),
                    "default".into(),
                ],
                vec![
                    "cmd".into(),
                    "window".into(),
                    "user-rotation".into(),
                    "free".into(),
                ],
            ],
            vec![vec![
                "settings".into(),
                "put".into(),
                "system".into(),
                "accelerometer_rotation".into(),
                "1".into(),
            ]],
        ],
        DeviceRotationMode::Locked(rotation) => {
            let value = rotation.wm_value().to_string();
            vec![
                vec![
                    vec![
                        "wm".into(),
                        "fixed-to-user-rotation".into(),
                        "enabled".into(),
                    ],
                    vec![
                        "wm".into(),
                        "user-rotation".into(),
                        "lock".into(),
                        value.clone(),
                    ],
                ],
                vec![
                    vec![
                        "cmd".into(),
                        "window".into(),
                        "fixed-to-user-rotation".into(),
                        "enabled".into(),
                    ],
                    vec![
                        "cmd".into(),
                        "window".into(),
                        "user-rotation".into(),
                        "lock".into(),
                        value.clone(),
                    ],
                ],
                vec![
                    vec![
                        "wm".into(),
                        "set-ignore-orientation-request".into(),
                        "false".into(),
                    ],
                    vec![
                        "wm".into(),
                        "fixed-to-user-rotation".into(),
                        "enabled".into(),
                    ],
                    vec![
                        "wm".into(),
                        "user-rotation".into(),
                        "lock".into(),
                        value.clone(),
                    ],
                ],
                vec![
                    vec![
                        "wm".into(),
                        "user-rotation".into(),
                        "lock".into(),
                        value.clone(),
                    ],
                    vec![
                        "settings".into(),
                        "put".into(),
                        "system".into(),
                        "accelerometer_rotation".into(),
                        "0".into(),
                    ],
                ],
                vec![
                    vec![
                        "settings".into(),
                        "put".into(),
                        "system".into(),
                        "user_rotation".into(),
                        value,
                    ],
                    vec![
                        "settings".into(),
                        "put".into(),
                        "system".into(),
                        "accelerometer_rotation".into(),
                        "0".into(),
                    ],
                ],
            ]
        }
    }
}

fn run_shell_sequence(serial: &str, sequence: &[Vec<String>]) -> Result<(), String> {
    for command in sequence {
        let args: Vec<&str> = command.iter().map(String::as_str).collect();
        let output = adb_shell_output(serial, &args)?;
        if !output.status.success() {
            return Err(describe_process_output(&output));
        }
    }

    Ok(())
}

fn format_shell_sequence(sequence: &[Vec<String>]) -> String {
    sequence
        .iter()
        .map(|command| command.join(" "))
        .collect::<Vec<_>>()
        .join(" && ")
}

fn parse_wxh(text: &str) -> Option<(u32, u32)> {
    let (width, height) = text.trim().split_once('x')?;
    Some((width.trim().parse().ok()?, height.trim().parse().ok()?))
}

fn parse_rotation_number(text: &str) -> Option<u8> {
    match text.trim().parse::<u16>().ok()? {
        0 => Some(0),
        90 => Some(1),
        180 => Some(2),
        270 => Some(3),
        value @ 0..=3 => u8::try_from(value).ok(),
        _ => None,
    }
}

fn resolve_stream_config_or_fallback(
    serial: &str,
    session: u64,
    config: &MirrorConfig,
    tx: &Sender<AdbMsg>,
) -> StreamConfig {
    match resolve_stream_config(serial, config) {
        Ok(stream_config) => {
            let _ = tx.send(AdbMsg::MirrorDisplaySize(
                serial.to_string(),
                session,
                stream_config.display_width,
                stream_config.display_height,
            ));
            send_mirror_log(
                tx,
                serial,
                AdbLogLevel::Info,
                format!(
                    "Stream sizing resolved from device display {}x{} to {}x{}",
                    stream_config.display_width,
                    stream_config.display_height,
                    stream_config.video_width,
                    stream_config.video_height
                ),
            );
            stream_config
        }
        Err(error) => {
            let stream_config = StreamConfig::fallback(config);
            send_mirror_log(
                tx,
                serial,
                AdbLogLevel::Warn,
                format!(
                    "Display size lookup failed; using requested fallback size {}x{}: {error}",
                    stream_config.video_width, stream_config.video_height
                ),
            );
            stream_config
        }
    }
}

fn resolve_stream_config(serial: &str, config: &MirrorConfig) -> Result<StreamConfig, String> {
    let config = config.normalized();
    let (display_width, display_height) = get_display_size(serial)?;
    let (video_width, video_height) =
        resolve_video_size(display_width, display_height, config.width, config.height);

    Ok(StreamConfig {
        video_width,
        video_height,
        bitrate: config.bitrate,
        display_width,
        display_height,
    })
}

fn resolve_video_size(
    display_width: u32,
    display_height: u32,
    max_width: u32,
    max_height: u32,
) -> (u32, u32) {
    if display_width == 0 || display_height == 0 {
        return (
            make_even_dimension(max_width.max(MIN_VIDEO_EDGE)),
            make_even_dimension(max_height.max(MIN_VIDEO_EDGE)),
        );
    }

    let width_ratio = f64::from(max_width.max(MIN_VIDEO_EDGE)) / f64::from(display_width);
    let height_ratio = f64::from(max_height.max(MIN_VIDEO_EDGE)) / f64::from(display_height);
    let scale = width_ratio.min(height_ratio).min(1.0);

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "scale is clamped to 0..=1, so result is always non-negative and fits u32"
    )]
    let scaled_width = (f64::from(display_width) * scale).floor() as u32;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "scale is clamped to 0..=1, so result is always non-negative and fits u32"
    )]
    let scaled_height = (f64::from(display_height) * scale).floor() as u32;

    (
        make_even_dimension(scaled_width.max(MIN_VIDEO_EDGE)),
        make_even_dimension(scaled_height.max(MIN_VIDEO_EDGE)),
    )
}

fn make_even_dimension(value: u32) -> u32 {
    let even = value.saturating_sub(value % 2);
    even.max(MIN_VIDEO_EDGE)
}

fn is_server_installed(serial: &str) -> Result<bool, String> {
    Ok(
        adb_shell_output(serial, &["test", "-f", SERVER_REMOTE_PATH])?
            .status
            .success(),
    )
}

fn server_pids(serial: &str) -> Result<Vec<u32>, String> {
    let output = adb_shell_output(serial, &["ps", "-A", "-o", "PID,ARGS"])?;
    if !output.status.success() {
        return Err(describe_process_output(&output));
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if !trimmed.contains(SERVER_MAIN_CLASS) {
                return None;
            }
            trimmed
                .split_whitespace()
                .next()
                .and_then(|pid| pid.parse::<u32>().ok())
        })
        .collect())
}

fn adb_shell_output(serial: &str, args: &[&str]) -> Result<Output, String> {
    let mut cmd = adb_command().ok_or_else(|| "ADB not available".to_string())?;
    cmd.arg("-s")
        .arg(serial)
        .arg("shell")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("adb shell failed: {error}"))
}

fn spawn_server_log_collector(
    child: &mut Child,
    tx: Sender<AdbMsg>,
    serial: String,
) -> Arc<Mutex<Vec<String>>> {
    let logs = Arc::new(Mutex::new(Vec::new()));
    if let Some(stdout) = child.stdout.take() {
        spawn_server_log_reader(stdout, logs.clone(), tx.clone(), serial.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_server_log_reader(stderr, logs.clone(), tx, serial);
    }

    logs
}

fn spawn_server_log_reader<R>(
    stream: R,
    logs: Arc<Mutex<Vec<String>>>,
    tx: Sender<AdbMsg>,
    serial: String,
) where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let mut guard = logs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.push(trimmed.to_string());
            if guard.len() > MAX_SERVER_LOG_LINES {
                let overflow = guard.len() - MAX_SERVER_LOG_LINES;
                guard.drain(..overflow);
            }
            drop(guard);

            let message = format!("[server] {trimmed}");
            let level = classify_server_log_level(trimmed);
            let _ = tx.send(AdbMsg::MirrorLog(serial.clone(), level, message));
        }
    });
}

fn classify_server_log_level(line: &str) -> AdbLogLevel {
    let lower = line.to_lowercase();
    if lower.contains("fatal")
        || lower.contains("exception")
        || lower.contains(" error")
        || lower.contains("failed")
    {
        AdbLogLevel::Error
    } else if lower.contains("stopped")
        || lower.contains("disconnected")
        || lower.contains("timed out")
        || lower.contains("timeout")
    {
        AdbLogLevel::Warn
    } else {
        AdbLogLevel::Info
    }
}

fn append_server_logs(message: String, logs: &Arc<Mutex<Vec<String>>>) -> String {
    let guard = logs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if guard.is_empty() {
        drop(guard);
        message
    } else {
        let suffix = guard.join(" | ");
        drop(guard);
        format!("{message}. Server log: {suffix}")
    }
}

fn format_exit_code(code: Option<i32>) -> String {
    code.map_or_else(String::new, |code| format!(" (exit code {code})"))
}

fn describe_process_output(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!(
            "process exited unsuccessfully{}",
            format_exit_code(output.status.code())
        )
    }
}

fn parse_android_platform_api(path: &Path) -> Option<u32> {
    path.file_name()?
        .to_str()?
        .strip_prefix("android-")?
        .parse()
        .ok()
}

fn find_android_jar() -> Result<PathBuf, String> {
    let mut best: Option<(u32, PathBuf)> = None;

    for root in sdk_root_candidates() {
        let platforms = root.join("platforms");
        let Ok(entries) = std::fs::read_dir(&platforms) else {
            continue;
        };

        for entry in entries.filter_map(Result::ok) {
            let platform_path = entry.path();
            let Some(api) = parse_android_platform_api(&platform_path) else {
                continue;
            };
            let jar = platform_path.join("android.jar");
            if !jar.exists() {
                continue;
            }

            let replace = best.as_ref().is_none_or(|(best_api, _)| api > *best_api);
            if replace {
                best = Some((api, jar));
            }
        }
    }

    best.map(|(_, jar)| jar)
        .ok_or_else(|| "android.jar not found in Android SDK".into())
}

fn parse_build_tools_version(path: &Path) -> Option<Vec<u32>> {
    path.file_name()?
        .to_str()?
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()
}

fn find_d8() -> Result<PathBuf, String> {
    let d8_name = if cfg!(windows) { "d8.bat" } else { "d8" };

    if let Some(path) = find_tool_on_path(d8_name) {
        return Ok(path);
    }

    let mut best: Option<(Vec<u32>, PathBuf)> = None;
    for root in sdk_root_candidates() {
        let build_tools_dir = root.join("build-tools");
        let Ok(entries) = std::fs::read_dir(&build_tools_dir) else {
            continue;
        };

        for entry in entries.filter_map(Result::ok) {
            let version_path = entry.path();
            let Some(version) = parse_build_tools_version(&version_path) else {
                continue;
            };
            let candidate = version_path.join(d8_name);
            if !candidate.exists() {
                continue;
            }

            let replace = best
                .as_ref()
                .is_none_or(|(best_version, _)| version > *best_version);
            if replace {
                best = Some((version, candidate));
            }
        }
    }

    best.map(|(_, path)| path)
        .ok_or_else(|| format!("{d8_name} not found in PATH or Android SDK build-tools"))
}

fn find_tool_on_path(name: &str) -> Option<PathBuf> {
    std::process::Command::new(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .ok()
        .filter(std::process::ExitStatus::success)
        .map(|_| PathBuf::from(name))
}

fn collect_class_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                collect_class_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "class") {
                out.push(path);
            }
        }
    }
}

fn dispatch_adb_input(serial: String, event: MirrorControlEvent) {
    std::thread::spawn(move || {
        let Some(mut cmd) = adb_command() else {
            return;
        };

        cmd.arg("-s").arg(&serial).arg("shell").arg("input");

        match event {
            MirrorControlEvent::Tap { x, y } => {
                cmd.arg("tap").arg(x.to_string()).arg(y.to_string());
            }
            MirrorControlEvent::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            } => {
                cmd.arg("swipe")
                    .arg(x1.to_string())
                    .arg(y1.to_string())
                    .arg(x2.to_string())
                    .arg(y2.to_string())
                    .arg(duration_ms.to_string());
            }
            MirrorControlEvent::Key { keycode } => {
                cmd.arg("keyevent").arg(keycode.to_string());
            }
        }

        let _ = cmd
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    });
}

pub fn send_tap(serial: &str, x: u32, y: u32) {
    dispatch_adb_input(serial.to_string(), MirrorControlEvent::Tap { x, y });
}

pub fn send_swipe(serial: &str, x1: u32, y1: u32, x2: u32, y2: u32, duration_ms: u32) {
    dispatch_adb_input(
        serial.to_string(),
        MirrorControlEvent::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration_ms,
        },
    );
}

pub fn send_key_event(serial: &str, keycode: u32) {
    dispatch_adb_input(serial.to_string(), MirrorControlEvent::Key { keycode });
}

#[allow(dead_code)]
pub fn send_text(serial: &str, text: &str) {
    let serial = serial.to_string();
    let encoded = text.replace(' ', "%s");

    std::thread::spawn(move || {
        let Some(mut cmd) = adb_command() else {
            return;
        };

        let _ = cmd
            .args(["-s", &serial, "shell", "input", "text", &encoded])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    });
}

pub mod keycode {
    pub const HOME: u32 = 3;
    pub const BACK: u32 = 4;
    pub const VOLUME_UP: u32 = 24;
    pub const VOLUME_DOWN: u32 = 25;
    pub const POWER: u32 = 26;
    pub const MENU: u32 = 82;
    pub const APP_SWITCH: u32 = 187;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_start_codes_basic() {
        let data = [0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68];
        assert_eq!(find_start_code_positions(&data), vec![0, 6]);
    }

    #[test]
    fn find_start_codes_three_byte() {
        let data = [0, 0, 1, 0x67, 0x42, 0, 0, 1, 0x68];
        assert_eq!(find_start_code_positions(&data), vec![0, 5]);
    }

    #[test]
    fn find_start_codes_mixed() {
        let data = [0, 0, 0, 1, 0x67, 0, 0, 1, 0x68, 0, 0, 0, 1, 0x65];
        assert_eq!(find_start_code_positions(&data), vec![0, 5, 9]);
    }

    #[test]
    fn find_start_codes_empty() {
        assert!(find_start_code_positions(&[]).is_empty());
        assert!(find_start_code_positions(&[0, 0]).is_empty());
    }

    #[test]
    fn parse_wxh_valid() {
        assert_eq!(parse_wxh("1080x2400"), Some((1080, 2400)));
        assert_eq!(parse_wxh(" 720x1280 "), Some((720, 1280)));
    }

    #[test]
    fn parse_wxh_invalid() {
        assert_eq!(parse_wxh("1080"), None);
        assert_eq!(parse_wxh("axb"), None);
        assert_eq!(parse_wxh(""), None);
    }

    #[test]
    fn parse_rotation_number_handles_android_rotation_formats() {
        assert_eq!(parse_rotation_number("0"), Some(0));
        assert_eq!(parse_rotation_number("90"), Some(1));
        assert_eq!(parse_rotation_number("180"), Some(2));
        assert_eq!(parse_rotation_number("270"), Some(3));
    }

    #[test]
    fn parse_display_state_reads_current_size_rotation_and_mode() {
        let text = r#"
            init=1080x2340 450dpi cur=2340x1080 app=2340x1080
            mCurrentRotation=ROTATION_270
            mUserRotationMode=USER_ROTATION_LOCKED mUserRotation=ROTATION_270
        "#;
        let state = parse_display_state(text).expect("display state");
        assert_eq!((state.width, state.height), (2340, 1080));
        assert_eq!(state.rotation, DeviceRotation::LandscapeRight);
        assert_eq!(
            state.mode,
            DeviceRotationMode::Locked(DeviceRotation::LandscapeRight)
        );
    }

    #[test]
    fn parse_display_state_defaults_to_auto_when_not_locked() {
        let text = r#"
            init=1080x2340 450dpi cur=1080x2340 app=1080x2340
            mCurrentRotation=ROTATION_0
            mUserRotationMode=USER_ROTATION_FREE mUserRotation=ROTATION_0
        "#;
        let state = parse_display_state(text).expect("display state");
        assert_eq!(state.mode, DeviceRotationMode::Auto);
    }

    #[test]
    fn frame_buffer_put_take() {
        let buf = MirrorFrameBuffer::new();
        assert!(buf.take().is_none());
        buf.put(MirrorFrame {
            width: 10,
            height: 10,
            rgba: vec![0; 400],
        });
        assert_eq!(buf.take().unwrap().width, 10);
        assert!(buf.take().is_none());
    }

    #[test]
    fn frame_buffer_overwrites() {
        let buf = MirrorFrameBuffer::new();
        buf.put(MirrorFrame {
            width: 10,
            height: 10,
            rgba: vec![0; 400],
        });
        buf.put(MirrorFrame {
            width: 20,
            height: 20,
            rgba: vec![0; 1600],
        });
        assert_eq!(buf.take().unwrap().width, 20);
    }

    #[test]
    fn resolve_video_size_preserves_aspect_ratio() {
        assert_eq!(resolve_video_size(1080, 2400, 720, 1280), (576, 1280));
        assert_eq!(resolve_video_size(2400, 1080, 1280, 720), (1280, 576));
    }

    #[test]
    fn resolve_video_size_never_upscales() {
        assert_eq!(resolve_video_size(720, 1280, 1080, 1920), (720, 1280));
    }

    #[test]
    fn parse_android_platform_api_ignores_invalid_entries() {
        assert_eq!(
            parse_android_platform_api(Path::new("android-34")),
            Some(34)
        );
        assert_eq!(parse_android_platform_api(Path::new("preview")), None);
    }

    #[test]
    fn parse_build_tools_version_handles_numeric_versions() {
        assert_eq!(
            parse_build_tools_version(Path::new("34.0.0")),
            Some(vec![34, 0, 0])
        );
        assert_eq!(parse_build_tools_version(Path::new("preview")), None);
    }

    #[test]
    fn make_even_dimension_enforces_minimum_even_size() {
        assert_eq!(make_even_dimension(1), 2);
        assert_eq!(make_even_dimension(5), 4);
        assert_eq!(make_even_dimension(8), 8);
    }

    #[test]
    fn device_rotation_values_match_android_wm_protocol() {
        assert_eq!(DeviceRotation::Portrait.wm_value(), "0");
        assert_eq!(DeviceRotation::LandscapeLeft.wm_value(), "1");
        assert_eq!(DeviceRotation::ReversePortrait.wm_value(), "2");
        assert_eq!(DeviceRotation::LandscapeRight.wm_value(), "3");
    }

    #[test]
    fn auto_rotation_sequences_restore_fixed_to_user_rotation() {
        let commands = rotation_command_sequences(DeviceRotationMode::Auto);
        assert_eq!(
            commands[0],
            vec![
                vec![
                    "wm".to_string(),
                    "fixed-to-user-rotation".to_string(),
                    "default".to_string(),
                ],
                vec![
                    "wm".to_string(),
                    "user-rotation".to_string(),
                    "free".to_string(),
                ],
            ]
        );
    }

    #[test]
    fn locked_rotation_sequences_enable_fixed_to_user_rotation() {
        let commands =
            rotation_command_sequences(DeviceRotationMode::Locked(DeviceRotation::LandscapeRight));
        assert_eq!(
            commands[0],
            vec![
                vec![
                    "wm".to_string(),
                    "fixed-to-user-rotation".to_string(),
                    "enabled".to_string(),
                ],
                vec![
                    "wm".to_string(),
                    "user-rotation".to_string(),
                    "lock".to_string(),
                    "3".to_string(),
                ],
            ]
        );
    }
}
