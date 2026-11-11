//! Knowing when a full-screen app (game, video, slideshow) covers the
//! island's monitor. Windows tells appbars about full-screen apps
//! (ABN_FULLSCREENAPP), so the island registers as an appbar that reserves
//! no space; a foreground-change hook catches Alt+Tab between windows.
//! Both only trigger a re-check: the foreground window must really cover
//! the whole monitor.

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Shell::{SHAppBarMessage, ABM_NEW, ABM_REMOVE, APPBARDATA};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_APPBAR: u32 = WM_APP + 7;
/// Posted by the foreground hook so the check runs from the window proc.
pub const WM_FOREGROUND: u32 = WM_APP + 8;
pub const ABN_FULLSCREENAPP: usize = 2;

fn appbar(hwnd: HWND) -> APPBARDATA {
    APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: hwnd, uCallbackMessage: WM_APPBAR, ..Default::default() }
}

thread_local! {
    static NOTIFY: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

unsafe extern "system" fn on_foreground(_: HWINEVENTHOOK, _: u32, _: HWND, _: i32, _: i32, _: u32, _: u32) {
    let target = NOTIFY.with(|n| n.get());
    if target != 0 {
        let _ = PostMessageW(Some(HWND(target as *mut _)), WM_FOREGROUND, WPARAM(0), LPARAM(0));
    }
}

pub struct Watcher {
    hwnd: HWND,
    hook: HWINEVENTHOOK,
}

impl Watcher {
    pub fn start(hwnd: HWND) -> Self {
        unsafe {
            let mut data = appbar(hwnd);
            SHAppBarMessage(ABM_NEW, &mut data);
            NOTIFY.with(|n| n.set(hwnd.0 as isize));
            let hook = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_foreground), 0, 0, WINEVENT_OUTOFCONTEXT);
            Self { hwnd, hook }
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        unsafe {
            let mut data = appbar(self.hwnd);
            SHAppBarMessage(ABM_REMOVE, &mut data);
            if !self.hook.is_invalid() {
                let _ = UnhookWinEvent(self.hook);
            }
            NOTIFY.with(|n| n.set(0));
        }
    }
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Desktop and taskbar windows report full-monitor rectangles too.
pub fn is_shell_class(class: &str) -> bool {
    matches!(class, "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
}

/// Does `window` cover all of `monitor`? Allows a pixel of slack for
/// windows that hang slightly off-screen.
pub fn covers(window: &RECT, monitor: &RECT) -> bool {
    window.left <= monitor.left + 1
        && window.top <= monitor.top + 1
        && window.right >= monitor.right - 1
        && window.bottom >= monitor.bottom - 1
}

/// The island's monitor rectangle (the primary one).
pub fn island_monitor() -> RECT {
    unsafe {
        let m = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(m, &mut info);
        info.rcMonitor
    }
}

/// True when the foreground window is a full-screen app on the island's
/// monitor (not the island itself, the desktop or the taskbar).
pub fn active(island: HWND) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg == island || !IsWindowVisible(fg).as_bool() || IsIconic(fg).as_bool() {
            return false;
        }
        if is_shell_class(&class_name(fg)) {
            return false;
        }
        let mut rect = RECT::default();
        if GetWindowRect(fg, &mut rect).is_err() {
            return false;
        }
        let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut info);
        info.rcMonitor == island_monitor() && covers(&rect, &info.rcMonitor)
    }
}

/// Does `window` reach the top of the monitor across the island's center?
/// True for maximized windows (whose frame overhangs the edge) and windows
/// snapped to the top half; their tabs and title bars live under the island.
pub fn reaches_island(window: &RECT, monitor: &RECT, island_center_x: i32) -> bool {
    window.top <= monitor.top + 2
        && window.left <= island_center_x
        && window.right >= island_center_x
        && window.bottom - window.top > 100
}

/// True when the foreground window sits under the island (see
/// `reaches_island`) without being full-screen.
pub fn window_under(island: HWND, island_center_x: i32) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg == island || !IsWindowVisible(fg).as_bool() || IsIconic(fg).as_bool() {
            return false;
        }
        if is_shell_class(&class_name(fg)) {
            return false;
        }
        let mut rect = RECT::default();
        if GetWindowRect(fg, &mut rect).is_err() {
            return false;
        }
        reaches_island(&rect, &island_monitor(), island_center_x)
    }
}

/// Class and rectangle of the foreground window, for the stats log.
pub fn foreground_debug() -> String {
    unsafe {
        let fg = GetForegroundWindow();
        let mut r = RECT::default();
        let _ = GetWindowRect(fg, &mut r);
        format!("{} ({},{},{},{})", class_name(fg), r.left, r.top, r.right, r.bottom)
    }
}

/// Whether the cursor sits on the top edge of the monitor, within the
/// horizontal span [left, right).
pub fn cursor_at_top(left: i32, right: i32) -> bool {
    let mut pt = POINT::default();
    if unsafe { GetCursorPos(&mut pt) }.is_err() {
        return false;
    }
    let mon = island_monitor();
    pt.y <= mon.top + 1 && pt.x >= left && pt.x < right
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(l: i32, t: i32, rr: i32, b: i32) -> RECT {
        RECT { left: l, top: t, right: rr, bottom: b }
    }

    #[test]
    fn full_monitor_coverage() {
        let mon = r(0, 0, 2560, 1440);
        assert!(covers(&r(0, 0, 2560, 1440), &mon));
        assert!(covers(&r(-8, -8, 2568, 1448), &mon)); // maximized-style overhang
        assert!(!covers(&r(0, 0, 2560, 1392), &mon)); // maximized above the taskbar
        assert!(!covers(&r(100, 0, 2560, 1440), &mon));
        // A window on another monitor does not cover this one.
        assert!(!covers(&r(2560, 0, 5120, 1440), &mon));
    }

    #[test]
    fn windows_under_the_island() {
        let mon = r(0, 0, 2560, 1440);
        let c = 1280;
        assert!(reaches_island(&r(-8, -8, 2568, 1400), &mon, c)); // maximized
        assert!(reaches_island(&r(0, 0, 2560, 720), &mon, c)); // snapped to top half
        assert!(!reaches_island(&r(0, 0, 1280 - 1, 1440), &mon, c)); // left half only
        assert!(!reaches_island(&r(200, 150, 1800, 900), &mon, c)); // floating window
        assert!(!reaches_island(&r(0, 0, 2560, 40), &mon, c)); // thin bar, not a window
    }

    #[test]
    fn shell_windows_are_ignored() {
        assert!(is_shell_class("Progman"));
        assert!(is_shell_class("WorkerW"));
        assert!(!is_shell_class("Chrome_WidgetWin_1"));
    }
}
