//! Notification-area icon with a small context menu.

use crate::autostart;
use crate::config::Config;
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_TRAY: u32 = WM_APP + 1;
const ID_AUTOSTART: usize = 1;
const ID_EXIT: usize = 2;
const ID_OPEN_CONFIG: usize = 3;
const ID_RELOAD: usize = 4;
const ID_SETTINGS: usize = 5;

pub enum TrayAction {
    None,
    Reload,
    Settings,
    Exit,
}

fn data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: crate::brand::icon(),
        ..Default::default()
    };
    for (dst, src) in nid.szTip.iter_mut().zip("Tidemark".encode_utf16()) {
        *dst = src;
    }
    nid
}

pub fn add(hwnd: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &data(hwnd));
    }
}

pub fn remove(hwnd: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data(hwnd));
    }
}

fn open_config() {
    let path = Config::path();
    if !path.exists() {
        Config::default().save();
    }
    unsafe {
        ShellExecuteW(None, w!("open"), w!("notepad.exe"), &HSTRING::from(path.as_os_str()), None, SW_SHOWNORMAL);
    }
}

/// Handles the tray callback.
pub fn on_message(hwnd: HWND, event: u32) -> TrayAction {
    if event != WM_RBUTTONUP && event != WM_LBUTTONUP {
        return TrayAction::None;
    }
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return TrayAction::None };
        let auto_flags = if autostart::is_enabled() { MF_STRING | MF_CHECKED } else { MF_STRING };
        let _ = AppendMenuW(menu, MF_STRING, ID_SETTINGS, w!("Настройки…"));
        let _ = AppendMenuW(menu, MF_STRING, ID_OPEN_CONFIG, w!("Открыть config.toml"));
        let _ = AppendMenuW(menu, MF_STRING, ID_RELOAD, w!("Перечитать config.toml"));
        let _ = AppendMenuW(menu, auto_flags, ID_AUTOSTART, w!("Запускать вместе с Windows"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, w!("Выход"));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY, pt.x, pt.y, None, hwnd, None);
        let _ = DestroyMenu(menu);

        match cmd.0 as usize {
            ID_AUTOSTART => {
                autostart::set_enabled(!autostart::is_enabled());
                TrayAction::None
            }
            ID_OPEN_CONFIG => {
                open_config();
                TrayAction::None
            }
            ID_SETTINGS => TrayAction::Settings,
            ID_RELOAD => TrayAction::Reload,
            ID_EXIT => TrayAction::Exit,
            _ => TrayAction::None,
        }
    }
}
