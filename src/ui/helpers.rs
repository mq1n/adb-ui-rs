use eframe::egui;

pub fn now_str() -> String {
    // Use Windows local time API for correct timezone.
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetLocalTime(lpSystemTime: *mut SystemTime);
        }
        #[repr(C)]
        struct SystemTime {
            year: u16,
            month: u16,
            _dow: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            millis: u16,
        }
        let mut st = SystemTime {
            year: 0,
            month: 0,
            _dow: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            millis: 0,
        };
        unsafe { GetLocalTime(&raw mut st) };
        format!("{:02}:{:02}:{:02}", st.hour, st.minute, st.second)
    }
    #[cfg(not(windows))]
    {
        // Use libc localtime for correct timezone on Unix
        #[cfg(unix)]
        {
            extern "C" {
                fn time(tloc: *mut i64) -> i64;
                fn localtime_r(timep: *const i64, result: *mut LibcTm) -> *mut LibcTm;
            }
            #[repr(C)]
            struct LibcTm {
                tm_sec: i32,
                tm_min: i32,
                tm_hour: i32,
                _rest: [i32; 6],
            }
            let mut t: i64 = 0;
            let mut tm = LibcTm {
                tm_sec: 0,
                tm_min: 0,
                tm_hour: 0,
                _rest: [0; 6],
            };
            unsafe {
                time(&raw mut t);
                localtime_r(&raw const t, &raw mut tm);
            }
            format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
        }
        #[cfg(not(unix))]
        {
            // Fallback: UTC for non-unix, non-windows
            let now = std::time::SystemTime::now();
            let secs = now
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let s = secs % 60;
            let m = (secs / 60) % 60;
            let h = (secs / 3600) % 24;
            format!("{h:02}:{m:02}:{s:02}")
        }
    }
}

// ─── Color helpers ───────────────────────────────────────────────────────────

/// Color logcat lines by the single-letter level column.
pub(super) fn logcat_line_color(line: &str) -> egui::Color32 {
    let bytes = line.as_bytes();
    if bytes.len() < 22 {
        return egui::Color32::from_rgb(200, 200, 200);
    }
    for i in 18..bytes.len().min(40) {
        if i + 2 < bytes.len()
            && bytes[i] == b' '
            && bytes[i + 1].is_ascii_uppercase()
            && bytes[i + 2] == b' '
        {
            return match bytes[i + 1] {
                b'V' => egui::Color32::from_rgb(150, 150, 150),
                b'D' => egui::Color32::from_rgb(100, 180, 255),
                b'I' => egui::Color32::from_rgb(100, 220, 100),
                b'W' => egui::Color32::from_rgb(255, 200, 50),
                b'E' => egui::Color32::from_rgb(255, 80, 80),
                b'F' => egui::Color32::from_rgb(255, 40, 40),
                _ => egui::Color32::from_rgb(200, 200, 200),
            };
        }
    }
    egui::Color32::from_rgb(200, 200, 200)
}

/// Color file log lines by keyword.
/// Looks for level keywords after the timestamp/pid prefix:
///   `"2026-04-03 13:16:03.869 [22043] error ..."`
///   `"2026-04-03 13:16:03.869 [22043] warning ..."`
///   `"[info] ..."`
///   `"ERROR: ..."`
pub(super) fn file_log_line_color(line: &str) -> egui::Color32 {
    // Try to find the level keyword. After "] " or at the start of the line.
    let search = line.find("] ").map_or(line, |pos| &line[pos + 2..]);

    // Get the first word (case-insensitive).
    let first_word = search
        .split(|c: char| c.is_whitespace() || c == ':')
        .next()
        .unwrap_or("")
        .to_lowercase();

    match first_word.as_str() {
        "fatal" | "critical" | "crit" => egui::Color32::from_rgb(255, 40, 40),
        "error" | "err" => egui::Color32::from_rgb(255, 80, 80),
        "warning" | "warn" => egui::Color32::from_rgb(255, 200, 50),
        "info" | "notice" => egui::Color32::from_rgb(100, 220, 100),
        "debug" | "dbg" => egui::Color32::from_rgb(100, 180, 255),
        "verbose" | "trace" => egui::Color32::from_rgb(150, 150, 150),
        _ => egui::Color32::from_rgb(200, 200, 200),
    }
}

/// Color debug output lines by common patterns.
pub(super) fn debug_line_color(line: &str) -> egui::Color32 {
    let lower = line.to_lowercase();
    if lower.contains("error") || lower.contains("failed") || lower.contains("denied") {
        egui::Color32::from_rgb(255, 80, 80)
    } else if lower.contains("warning") || lower.contains("slow") || lower.contains("jank") {
        egui::Color32::from_rgb(255, 200, 50)
    } else if lower.contains("===") || lower.contains("---") {
        egui::Color32::from_rgb(100, 180, 255)
    } else if lower.starts_with("total") || lower.starts_with("summary") {
        egui::Color32::from_rgb(100, 220, 100)
    } else {
        egui::Color32::from_rgb(200, 200, 200)
    }
}

pub(super) fn copy_png_to_clipboard(png_bytes: &[u8]) -> Result<(), String> {
    let img = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png)
        .map_err(|e| format!("Failed to decode PNG: {e}"))?;
    let rgba = img.to_rgba8();
    let img_data = arboard::ImageData {
        width: usize::try_from(rgba.width())
            .map_err(|_| "Image width does not fit in usize".to_string())?,
        height: usize::try_from(rgba.height())
            .map_err(|_| "Image height does not fit in usize".to_string())?,
        bytes: std::borrow::Cow::Borrowed(rgba.as_raw()),
    };
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("Clipboard init failed: {e}"))?;
    clipboard
        .set_image(img_data)
        .map_err(|e| format!("Clipboard set_image failed: {e}"))
}

pub(super) fn get_screenshot_temp_path(timestamp: &str) -> std::path::PathBuf {
    let fname = format!("screenshot_{}.png", timestamp.replace(':', "-"));
    std::env::temp_dir().join(fname)
}

pub(super) fn copy_png_as_file(png_bytes: &[u8], timestamp: &str) -> Result<(), String> {
    let path = get_screenshot_temp_path(timestamp);
    std::fs::write(&path, png_bytes).map_err(|e| format!("Failed to write temp file: {e}"))?;
    #[cfg(windows)]
    {
        copy_file_to_clipboard(&path);
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
    Ok(())
}

/// Copy a file to the Windows clipboard as `CF_HDROP` (file drop).
#[cfg(windows)]
pub(super) fn copy_file_to_clipboard(path: &std::path::Path) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn OpenClipboard(hWnd: *mut std::ffi::c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn SetClipboardData(uFormat: u32, hMem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalAlloc(uFlags: u32, dwBytes: usize) -> *mut std::ffi::c_void;
        fn GlobalLock(hMem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
        fn GlobalUnlock(hMem: *mut std::ffi::c_void) -> i32;
        fn GlobalFree(hMem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }

    const CF_HDROP: u32 = 15;
    const GMEM_MOVEABLE: u32 = 0x0002;
    const GMEM_ZEROINIT: u32 = 0x0040;
    const GHND: u32 = GMEM_MOVEABLE | GMEM_ZEROINIT;

    // DROPFILES struct: 20 bytes header, then wide string file path, double-null terminated.
    #[repr(C)]
    struct DropFiles {
        p_files: u32, // offset to file list
        pt_x: i32,
        pt_y: i32,
        f_nc: i32,
        f_wide: i32, // 1 = unicode
    }

    let path_wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0)) // null terminator for the path
        .chain(std::iter::once(0)) // double-null terminator for the list
        .collect();

    let header_size = std::mem::size_of::<DropFiles>();
    let header_size_u32 =
        u32::try_from(header_size).expect("DROPFILES header must fit in a 32-bit offset");
    let total_size = header_size + path_wide.len() * 2;

    unsafe {
        let hmem = GlobalAlloc(GHND, total_size);
        if hmem.is_null() {
            return;
        }
        let ptr = GlobalLock(hmem);
        if ptr.is_null() {
            GlobalFree(hmem);
            return;
        }

        // Write header without assuming alignment on the destination pointer.
        let header = DropFiles {
            p_files: header_size_u32,
            pt_x: 0,
            pt_y: 0,
            f_nc: 0,
            f_wide: 1,
        };
        std::ptr::copy_nonoverlapping(
            std::ptr::from_ref(&header).cast::<u8>(),
            ptr.cast::<u8>(),
            header_size,
        );

        // Write file path after header.
        std::ptr::copy_nonoverlapping(
            path_wide.as_ptr().cast::<u8>(),
            ptr.cast::<u8>().add(header_size),
            std::mem::size_of_val(path_wide.as_slice()),
        );

        GlobalUnlock(hmem);

        if OpenClipboard(std::ptr::null_mut()) != 0 {
            EmptyClipboard();
            SetClipboardData(CF_HDROP, hmem);
            CloseClipboard();
        } else {
            GlobalFree(hmem);
        }
    }
}

pub(super) fn export_single_file(name: &str, content: &str) -> Result<(), String> {
    let path = rfd::FileDialog::new()
        .set_title("Export log file")
        .set_file_name(name)
        .add_filter("Log files", &["log", "txt"])
        .add_filter("All files", &["*"])
        .save_file()
        .ok_or_else(|| "Export cancelled".to_string())?;
    std::fs::write(&path, content).map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

/// Fast line count (avoids iterating `.lines()` on large buffers every frame).
pub(super) fn bytecount_lines(s: &str) -> usize {
    if s.is_empty() {
        return 0;
    }
    s.bytes().filter(|&b| b == b'\n').count() + 1
}

pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format_scaled_size(bytes, 1024, "KB")
    } else {
        format_scaled_size(bytes, 1024 * 1024, "MB")
    }
}

fn format_scaled_size(bytes: usize, unit_size: usize, unit: &str) -> String {
    let bytes = u128::try_from(bytes).expect("usize always fits in u128");
    let unit_size = u128::try_from(unit_size).expect("usize always fits in u128");
    let scaled = (bytes * 10 + unit_size / 2) / unit_size;
    format!("{}.{} {unit}", scaled / 10, scaled % 10)
}
