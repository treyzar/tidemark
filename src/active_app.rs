//! The window the user is working in: its app name, title and icon, plus
//! quick actions on it (keep on top, see-through, minimize).

use windows::core::{w, HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, COLORREF, HANDLE, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
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
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
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
        if !VerQueryValueW(
            data.as_ptr() as *const _,
            w!("\\VarFileInfo\\Translation"),
            &mut ptr,
            &mut len,
        )
        .as_bool()
            || len < 4
        {
            return stem;
        }
        let lang = *(ptr as *const u16);
        let codepage = *(ptr as *const u16).add(1);
        let key = HSTRING::from(format!(
            "\\StringFileInfo\\{lang:04x}{codepage:04x}\\FileDescription"
        ));
        if VerQueryValueW(data.as_ptr() as *const _, &key, &mut ptr, &mut len).as_bool() && len > 1
        {
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(
                ptr as *const u16,
                len as usize - 1,
            ));
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
        "Progman"
            | "WorkerW"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "WinIslandWindow"
            | "WinIslandSettings"
            | "NotifyIconOverflowWindow"
            | "TopLevelWindowForOverflowXamlIsland"
            | "XamlExplorerHostIslandWindow"
            | "ForegroundStaging"
            | "MultitaskingViewFrame"
            | "TaskListThumbnailWnd"
            | "#32768"
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
        let host = exe
            .to_ascii_lowercase()
            .ends_with("\\applicationframehost.exe");
        let name = if host && !title.is_empty() {
            title.clone()
        } else {
            friendly_name(&exe)
        };
        let ex = GetWindowLongPtrW(fg, GWL_EXSTYLE) as u32;
        let mut alpha = 255u8;
        if ex & WS_EX_LAYERED.0 != 0 {
            let mut a = 0u8;
            let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS(0);
            if GetLayeredWindowAttributes(fg, None, Some(&mut a), Some(&mut flags)).is_ok()
                && flags.0 & LWA_ALPHA.0 != 0
            {
                alpha = a;
            }
        }
        Some(ActiveApp {
            hwnd: fg.0 as isize,
            exe,
            name,
            title,
            topmost: ex & WS_EX_TOPMOST.0 != 0,
            alpha,
        })
    }
}

fn hwnd(app: &ActiveApp) -> HWND {
    HWND(app.hwnd as *mut _)
}

/// Keep the window above all others (or stop doing so). Returns the new state.
pub fn toggle_topmost(app: &ActiveApp) -> std::result::Result<bool, String> {
    ensure(app)?;
    unsafe {
        let on = GetWindowLongPtrW(hwnd(app), GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 == 0;
        SetWindowPos(
            hwnd(app),
            Some(if on { HWND_TOPMOST } else { HWND_NOTOPMOST }),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        )
        .map_err(|_| "Не удалось изменить закрепление окна".to_string())?;
        let actual = GetWindowLongPtrW(hwnd(app), GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0;
        if actual == on {
            Ok(actual)
        } else {
            Err("Windows не подтвердила изменение закрепления".into())
        }
    }
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
pub fn cycle_opacity(app: &ActiveApp) -> std::result::Result<u8, String> {
    ensure(app)?;
    let alpha = next_alpha(app.alpha);
    unsafe {
        let h = hwnd(app);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        let added = ex & WS_EX_LAYERED.0 as isize == 0;
        if added {
            SetWindowLongPtrW(h, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as isize);
            let _ = SetPropW(
                h,
                w!("Tidemark.LayeredAdded"),
                Some(HANDLE(1usize as *mut _)),
            );
        }
        let mut key = COLORREF(0);
        let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS(0);
        let _ = GetLayeredWindowAttributes(h, Some(&mut key), None, Some(&mut flags));
        if SetLayeredWindowAttributes(
            h,
            key,
            alpha,
            LWA_ALPHA | LAYERED_WINDOW_ATTRIBUTES_FLAGS(flags.0 & LWA_COLORKEY.0),
        )
        .is_err()
        {
            if added {
                SetWindowLongPtrW(h, GWL_EXSTYLE, ex);
                let _ = RemovePropW(h, w!("Tidemark.LayeredAdded"));
            }
            return Err("Не удалось изменить прозрачность этого окна".into());
        }
        if alpha == 255 && !GetPropW(h, w!("Tidemark.LayeredAdded")).0.is_null() {
            SetWindowLongPtrW(
                h,
                GWL_EXSTYLE,
                GetWindowLongPtrW(h, GWL_EXSTYLE) & !(WS_EX_LAYERED.0 as isize),
            );
            let _ = RemovePropW(h, w!("Tidemark.LayeredAdded"));
        }
    }
    Ok(alpha)
}

pub fn minimize(app: &ActiveApp) -> std::result::Result<(), String> {
    ensure(app)?;
    unsafe {
        PostMessageW(
            Some(hwnd(app)),
            WM_SYSCOMMAND,
            WPARAM(SC_MINIMIZE as usize),
            LPARAM(0),
        )
        .map_err(|_| "Не удалось свернуть окно".into())
    }
}

fn ensure(app: &ActiveApp) -> std::result::Result<(), String> {
    if alive(app) && exe_of(hwnd(app)).is_some_and(|e| e.eq_ignore_ascii_case(&app.exe)) {
        Ok(())
    } else {
        Err("Окно уже закрыто или недоступно".into())
    }
}
pub fn activate(app: &ActiveApp) -> std::result::Result<(), String> {
    ensure(app)?;
    unsafe {
        if IsIconic(hwnd(app)).as_bool() {
            let _ = ShowWindowAsync(hwnd(app), SW_RESTORE);
        }
        if SetForegroundWindow(hwnd(app)).as_bool() {
            Ok(())
        } else {
            Err("Windows не разрешила переключиться на это окно".into())
        }
    }
}
pub fn toggle_maximize(app: &ActiveApp) -> std::result::Result<(), String> {
    ensure(app)?;
    unsafe {
        let cmd = if IsZoomed(hwnd(app)).as_bool() {
            SC_RESTORE
        } else {
            SC_MAXIMIZE
        };
        PostMessageW(
            Some(hwnd(app)),
            WM_SYSCOMMAND,
            WPARAM(cmd as usize),
            LPARAM(0),
        )
        .map_err(|_| "Не удалось изменить размер окна".into())
    }
}
pub fn split_bounds(work: RECT, right: bool) -> RECT {
    let middle = work.left + (work.right - work.left) / 2;
    if right {
        RECT {
            left: middle,
            ..work
        }
    } else {
        RECT {
            right: middle,
            ..work
        }
    }
}
pub fn snap(app: &ActiveApp, right: bool) -> std::result::Result<(), String> {
    ensure(app)?;
    unsafe {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(
            MonitorFromWindow(hwnd(app), MONITOR_DEFAULTTONEAREST),
            &mut info,
        )
        .as_bool()
        {
            return Err("Не удалось определить рабочую область монитора".into());
        }
        if IsZoomed(hwnd(app)).as_bool() || IsIconic(hwnd(app)).as_bool() {
            let _ = ShowWindow(hwnd(app), SW_RESTORE);
        }
        let r = split_bounds(info.rcWork, right);
        SetWindowPos(
            hwnd(app),
            None,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .map_err(|_| "Не удалось разместить окно".into())
    }
}

pub fn reveal(app: &ActiveApp) -> std::result::Result<(), String> {
    ensure(app)?;
    unsafe {
        let result = windows::Win32::UI::Shell::ShellExecuteW(
            None,
            w!("open"),
            w!("explorer.exe"),
            &HSTRING::from(format!("/select,\"{}\"", app.exe)),
            None,
            SW_SHOWNORMAL,
        );
        if result.0 as isize > 32 {
            Ok(())
        } else {
            Err("Не удалось открыть папку приложения".into())
        }
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
    fn snap_halves_cover_negative_origin_work_area_without_gap() {
        let work = RECT {
            left: -1920,
            top: 24,
            right: -1,
            bottom: 1080,
        };
        let left = split_bounds(work, false);
        let right = split_bounds(work, true);
        assert_eq!(left.left, work.left);
        assert_eq!(left.right, right.left);
        assert_eq!(right.right, work.right);
        assert_eq!(left.top, 24);
        assert_eq!(right.bottom, 1080);
    }

    #[test]
    fn unavailable_windows_report_failure() {
        let app = ActiveApp {
            hwnd: 0,
            exe: "missing.exe".into(),
            name: String::new(),
            title: String::new(),
            topmost: false,
            alpha: 255,
        };
        assert!(toggle_topmost(&app).is_err());
        assert!(cycle_opacity(&app).is_err());
        assert!(minimize(&app).is_err());
        assert!(activate(&app).is_err());
    }

    #[test]
    fn native_window_pin_and_opacity_cycle_preserve_original_layer_style() {
        unsafe extern "system" fn proc(
            h: HWND,
            m: u32,
            w: WPARAM,
            l: LPARAM,
        ) -> windows::Win32::Foundation::LRESULT {
            DefWindowProcW(h, m, w, l)
        }
        unsafe {
            let module = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                .ok()
                .map(Into::into);
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: module.unwrap_or_default(),
                lpszClassName: w!("TidemarkActionTest"),
                ..Default::default()
            };
            RegisterClassW(&class);
            for originally_layered in [false, true] {
                let h = CreateWindowExW(
                    if originally_layered {
                        WS_EX_LAYERED
                    } else {
                        WINDOW_EX_STYLE(0)
                    },
                    w!("TidemarkActionTest"),
                    w!("Tidemark window test"),
                    WS_POPUP,
                    -30000,
                    -30000,
                    80,
                    80,
                    None,
                    None,
                    module,
                    None,
                )
                .unwrap();
                if originally_layered {
                    SetLayeredWindowAttributes(h, COLORREF(0), 255, LWA_ALPHA).unwrap();
                }
                let mut app = ActiveApp {
                    hwnd: h.0 as isize,
                    exe: std::env::current_exe()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    name: String::new(),
                    title: String::new(),
                    topmost: false,
                    alpha: 255,
                };
                assert!(toggle_topmost(&app).unwrap());
                assert!(!toggle_topmost(&app).unwrap());
                for expected in [217, 178, 255] {
                    app.alpha = cycle_opacity(&app).unwrap();
                    assert_eq!(app.alpha, expected);
                    if expected != 255 {
                        let mut alpha = 0;
                        GetLayeredWindowAttributes(h, None, Some(&mut alpha), None).unwrap();
                        assert_eq!(alpha, expected);
                    }
                }
                assert_eq!(
                    GetWindowLongPtrW(h, GWL_EXSTYLE) as u32 & WS_EX_LAYERED.0 != 0,
                    originally_layered
                );
                let _ = DestroyWindow(h);
            }
        }
    }

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
