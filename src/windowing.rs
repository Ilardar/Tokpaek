//! Window placement and z-order verbs over raw HWNDs, shared by the widget,
//! the settings window and startup. One concept — "where and how a window
//! sits" — in one module: app.rs, settings_ui.rs and main.rs are callers.
//!
//! Pure decisions (snap geometry, which band) live in `frame_policy`; this
//! module is only the Win32 execution around them.

/// Put the window back at the top of the topmost band, without stealing focus.
#[cfg(windows)]
pub fn set_topmost(hwnd: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE,
    };
    unsafe {
        let _ = SetWindowPos(
            HWND(hwnd as *mut _),
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        );
    }
}

#[cfg(not(windows))]
pub fn set_topmost(_hwnd: isize) {}

/// Park the window at the top of the normal (non-topmost) z-band: above every
/// ordinary window, below topmost ones, without activating anything.
#[cfg(windows)]
pub fn drop_topmost(hwnd: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_NOTOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE,
    };
    unsafe {
        let _ = SetWindowPos(
            HWND(hwnd as *mut _),
            HWND_NOTOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        );
    }
}

#[cfg(not(windows))]
pub fn drop_topmost(_hwnd: isize) {}

/// "Home": the default spot — the top-left corner of the primary screen, the
/// same coordinates a fresh window with no saved position gets.
#[cfg(windows)]
pub fn home(hwnd: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    };
    unsafe {
        let _ = SetWindowPos(
            HWND(hwnd as *mut _),
            None,
            0,
            0,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
}

#[cfg(not(windows))]
pub fn home(_hwnd: isize) {}

/// Force the window back to a square of `side_px` physical pixels without
/// moving it or touching its z-order. The OS resize loop drags one axis at a
/// time; this keeps the invisible window's shape locked to the visible
/// circle's aspect ratio at every moment of the drag.
#[cfg(windows)]
pub fn place_square(hwnd: isize, side_px: f32) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOZORDER,
    };
    unsafe {
        let s = side_px.round() as i32;
        let _ = SetWindowPos(
            HWND(hwnd as *mut _),
            None,
            0,
            0,
            s,
            s,
            SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
        );
    }
}

#[cfg(not(windows))]
pub fn place_square(_hwnd: isize, _side_px: f32) {}

/// After a hand drag: snap the window to the work area of the monitor it now
/// sits on and make sure at least half of it stays visible, then return its
/// final (x, y) in *physical* pixels. Pure decision lives in
/// `frame_policy::snap_to_work_area`; this is only the Win32 read/move around
/// it, in one coordinate space end to end.
#[cfg(windows)]
pub fn snap(hwnd: isize) -> Option<(f32, f32)> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    };
    unsafe {
        let mut win = RECT::default();
        if GetWindowRect(HWND(hwnd as *mut _), &mut win).is_err() {
            return None;
        }
        let w = (win.right - win.left) as f32;
        let h = (win.bottom - win.top) as f32;
        let (x, y) = (win.left as f32, win.top as f32);

        let (wa_l, wa_t, wa_r, wa_b) =
            window_work_area(hwnd).or_else(cursor_work_area)?;
        let (nx, ny) = crate::frame_policy::snap_to_work_area(
            x,
            y,
            w,
            h,
            (wa_l as f32, wa_t as f32, wa_r as f32, wa_b as f32),
        );
        if (nx - x).abs() > 0.5 || (ny - y).abs() > 0.5 {
            let _ = SetWindowPos(
                HWND(hwnd as *mut _),
                None,
                nx.round() as i32,
                ny.round() as i32,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
        Some((nx, ny))
    }
}

/// Work area (left, top, right, bottom, in physical pixels) of the monitor the
/// pointer is on — captured when the window is opened, so later mouse movement
/// can't send the window to another screen.
#[cfg(windows)]
pub fn cursor_work_area() -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    unsafe {
        let mut pt = POINT::default();
        GetCursorPos(&mut pt).ok()?;
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
            return None;
        }
        let w = info.rcWork;
        Some((w.left, w.top, w.right, w.bottom))
    }
}

#[cfg(not(windows))]
pub fn cursor_work_area() -> Option<(i32, i32, i32, i32)> {
    None
}

/// Work area of the monitor a window sits on.
#[cfg(windows)]
pub fn window_work_area(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    unsafe {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let monitor = MonitorFromWindow(HWND(hwnd as *mut _), MONITOR_DEFAULTTONEAREST);
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        let w = info.rcWork;
        Some((w.left, w.top, w.right, w.bottom))
    }
}

#[cfg(not(windows))]
pub fn window_work_area(_hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    None
}

/// Centre a window in `area`. Done through Win32 on the real window: egui's
/// viewport position is logical and relative to one screen, which lands in the
/// wrong place on a multi-monitor desktop.
#[cfg(windows)]
pub fn center(hwnd: isize, area: Option<(i32, i32, i32, i32)>) -> bool {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, SetWindowPos, HWND_TOPMOST, SWP_NOSIZE, SWP_SHOWWINDOW,
    };
    let Some((left, top, right, bottom)) = area else {
        return true; // nothing to aim at — leave the window where it is
    };
    unsafe {
        let hwnd = HWND(hwnd as *mut _);
        let mut win = RECT::default();
        if GetWindowRect(hwnd, &mut win).is_err() {
            return false;
        }
        let w = win.right - win.left;
        let h = win.bottom - win.top;
        if w <= 0 || h <= 0 {
            return false;
        }
        let x = left + ((right - left) - w) / 2;
        let y = top + ((bottom - top) - h) / 2;
        SetWindowPos(hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_SHOWWINDOW).is_ok()
    }
}

#[cfg(not(windows))]
pub fn center(_hwnd: isize, _area: Option<(i32, i32, i32, i32)>) -> bool {
    true
}

/// Is (x, y) — logical points — on some monitor? Startup uses it to drop a
/// saved position that belonged to a monitor no longer attached.
#[cfg(windows)]
pub fn visible_on_some_monitor(x: f32, y: f32) -> bool {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONULL};
    unsafe {
        let pt = POINT {
            x: x as i32,
            y: y as i32,
        };
        !MonitorFromPoint(pt, MONITOR_DEFAULTTONULL).0.is_null()
    }
}

#[cfg(not(windows))]
pub fn visible_on_some_monitor(_x: f32, _y: f32) -> bool {
    true
}

/// Windows 11 rounds and outlines every top-level window itself. On a
/// borderless window that already paints its own rounded panel it shows up as a
/// second arc in each corner, so turn both off and let the panel define the
/// shape — and then make the window's own alpha count, or the pixels outside
/// that shape are composited as opaque black (G28).
#[cfg(windows)]
pub fn strip_chrome(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::Graphics::Dwm::{
        DwmEnableBlurBehindWindow, DwmSetWindowAttribute, DWMWA_BORDER_COLOR,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWM_BB_BLURREGION, DWM_BB_ENABLE,
        DWM_BLURBEHIND,
    };
    use windows::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject};
    const COLOR_NONE: u32 = 0xFFFF_FFFE;
    unsafe {
        let pref = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &COLOR_NONE as *const _ as *const _,
            std::mem::size_of_val(&COLOR_NONE) as u32,
        );
        // An empty blur region means "blur nothing, just honour the alpha
        // channel" — the same call winit makes for a transparent window, which
        // this viewport does not get.
        let region = CreateRectRgn(0, 0, -1, -1);
        let blur = DWM_BLURBEHIND {
            dwFlags: DWM_BB_ENABLE | DWM_BB_BLURREGION,
            fEnable: true.into(),
            hRgnBlur: region,
            fTransitionOnMaximized: false.into(),
        };
        let _ = DwmEnableBlurBehindWindow(hwnd, &blur);
        let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(region.0));
    }
}

/// Find our settings window by enumerating top-level windows of our process
/// that are distinct from the main widget strip.
#[cfg(windows)]
pub fn find_settings_wnd() -> Option<isize> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId};

    let main_h = crate::app::NATIVE_HWND.load(std::sync::atomic::Ordering::Relaxed);
    let my_pid = unsafe { GetCurrentProcessId() };

    struct Search {
        my_pid: u32,
        main_hwnd: isize,
        found: Option<isize>,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let s = &mut *(lparam.0 as *mut Search);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == s.my_pid {
            let h = hwnd.0 as isize;
            if s.main_hwnd == 0 || h != s.main_hwnd {
                let mut buf = [0u16; 64];
                let len = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(hwnd, &mut buf);
                if len > 0 {
                    let title = String::from_utf16_lossy(&buf[..len as usize]);
                    if title.contains("Настройки") || title.contains("Settings") {
                        s.found = Some(h);
                        return BOOL(0);
                    }
                }
            }
        }
        BOOL(1)
    }

    let mut search = Search {
        my_pid,
        main_hwnd: main_h,
        found: None,
    };

    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut search as *mut _ as isize));
    }
    search.found
}

#[cfg(not(windows))]
pub fn find_settings_wnd() -> Option<isize> {
    None
}
