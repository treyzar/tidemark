//! The window the user is working in: its app name, title and icon, plus
//! quick actions on it (keep on top, see-through, minimize).

use windows::core::{w, HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, COLORREF, HWND, RECT};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Debug, Clone, PartialEq)]
pub struct ActiveApp {
    pub hwnd: isize,
    pub exe: String,
    pub name: String,
    pub title: String,
    pub topmost: bool,
    /// 255 = opaque.
    pub alpha: u8,
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn window_text(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn exe_of(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(process);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// "Google Chrome" from the exe's version resource, else "chrome".
pub fn friendly_name(exe: &str) -> String {
    let stem = exe.rsplit('\\').next().unwrap_or(exe);
    let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s).to_string();
    unsafe {
        let path = HSTRING::from(exe);
        let size = GetFileVersionInfoSizeW(&path, None);
        if size == 0 {
            return stem;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(&path, None, size, data.as_mut_ptr() as *mut _).is_err() {
            return stem;
        }
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        if !VerQueryValueW(data.as_ptr() as *const _, w!("\\VarFileInfo\\Translation"), &mut ptr, &mut len).as_bool() || len < 4 {
            return stem;
        }
        let lang = *(ptr as *const u16);
        let codepage = *(ptr as *const u16).add(1);
        let key = HSTRING::from(format!("\\StringFileInfo\\{lang:04x}{codepage:04x}\\FileDescription"));
        if VerQueryValueW(data.as_ptr() as *const _, &key, &mut ptr, &mut len).as_bool() && len > 1 {
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr as *const u16, len as usize - 1));
            let s = s.trim();
            if !s.is_empty() {
                return s.to_string();
            }
        }
        stem
    }
}

/// Windows that are not "apps": the shell, the taskbar, the island itself.
pub fn is_ignored_class(class: &str) -> bool {
    matches!(
        class,
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "WinIslandWindow" | "NotifyIconOverflowWindow"
            | "TopLevelWindowForOverflowXamlIsland" | "XamlExplorerHostIslandWindow" | "ForegroundStaging"
            | "MultitaskingViewFrame" | "TaskListThumbnailWnd" | "#32768"
    )
}

pub fn current() -> Option<ActiveApp> {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || !IsWindowVisible(fg).as_bool() || is_ignored_class(&class_name(fg)) {
            return None;
        }
        let exe = exe_of(fg)?;
        let title = window_text(fg);
        // UWP apps run inside ApplicationFrameHost: the title names the app.
        let host = exe.to_ascii_lowercase().ends_with("\\applicationframehost.exe");
        let name = if host && !title.is_empty() { title.clone() } else { friendly_name(&exe) };
        let ex = GetWindowLongPtrW(fg, GWL_EXSTYLE) as u32;
        let mut alpha = 255u8;
        if ex & WS_EX_LAYERED.0 != 0 {
            let mut a = 0u8;
            let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS(0);
            if GetLayeredWindowAttributes(fg, None, Some(&mut a), Some(&mut flags)).is_ok() && flags.0 & LWA_ALPHA.0 != 0 {
                alpha = a;
            }
        }
        Some(ActiveApp { hwnd: fg.0 as isize, exe, name, title, topmost: ex & WS_EX_TOPMOST.0 != 0, alpha })
    }
}

fn hwnd(app: &ActiveApp) -> HWND {
    HWND(app.hwnd as *mut _)
}

/// Keep the window above all others (or stop doing so). Returns the new state.
pub fn toggle_topmost(app: &ActiveApp) -> bool {
    let on = !app.topmost;
    unsafe {
        let _ = SetWindowPos(
            hwnd(app),
            Some(if on { HWND_TOPMOST } else { HWND_NOTOPMOST }),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    on
}

/// 100 % → 85 % → 70 % → 100 %.
pub fn next_alpha(alpha: u8) -> u8 {
    match alpha {
        a if a > 230 => 217,
        a if a > 190 => 178,
        _ => 255,
    }
}

/// Makes the window see-through by one step. Returns the new alpha.
pub fn cycle_opacity(app: &ActiveApp) -> u8 {
    let alpha = next_alpha(app.alpha);
    unsafe {
        let h = hwnd(app);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        if alpha == 255 {
            let _ = SetLayeredWindowAttributes(h, COLORREF(0), 255, LWA_ALPHA);
            SetWindowLongPtrW(h, GWL_EXSTYLE, ex & !(WS_EX_LAYERED.0 as isize));
        } else {
            SetWindowLongPtrW(h, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as isize);
            let _ = SetLayeredWindowAttributes(h, COLORREF(0), alpha, LWA_ALPHA);
        }
    }
    alpha
}

pub fn minimize(app: &ActiveApp) {
    unsafe {
        let _ = ShowWindow(hwnd(app), SW_MINIMIZE);
    }
}

/// The window still exists and is where we left it.
pub fn alive(app: &ActiveApp) -> bool {
    unsafe {
        let mut r = RECT::default();
        IsWindow(Some(hwnd(app))).as_bool() && GetWindowRect(hwnd(app), &mut r).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opacity_steps_cycle() {
        assert_eq!(next_alpha(255), 217);
        assert_eq!(next_alpha(217), 178);
        assert_eq!(next_alpha(178), 255);
        assert_eq!(next_alpha(100), 255);
    }

    #[test]
    fn shell_windows_are_not_apps() {
        assert!(is_ignored_class("Shell_TrayWnd"));
        assert!(is_ignored_class("WinIslandWindow"));
        assert!(!is_ignored_class("Chrome_WidgetWin_1"));
    }

    #[test]
    fn friendly_name_of_a_system_exe() {
        let windir = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        let name = friendly_name(&format!("{windir}\\System32\\notepad.exe"));
        assert!(!name.is_empty());
        assert_eq!(friendly_name("C:\\nope\\missing-app.exe"), "missing-app");
    }
}
