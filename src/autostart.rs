//! Start with Windows via HKCU\...\Run.

use windows::core::{w, PCWSTR};
use windows::Win32::System::Registry::*;

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE: PCWSTR = w!("WinIsland");

fn open(access: REG_SAM_FLAGS) -> Option<HKEY> {
    let mut key = HKEY::default();
    let err = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, access, &mut key) };
    err.is_ok().then_some(key)
}

pub fn is_enabled() -> bool {
    let Some(key) = open(KEY_READ) else { return false };
    let err = unsafe { RegQueryValueExW(key, VALUE, None, None, None, None) };
    unsafe {
        let _ = RegCloseKey(key);
    }
    err.is_ok()
}

pub fn set_enabled(enable: bool) {
    let Some(key) = open(KEY_SET_VALUE) else { return };
    unsafe {
        if enable {
            if let Ok(exe) = std::env::current_exe() {
                let cmd = format!("\"{}\"", exe.display());
                let wide: Vec<u16> = cmd.encode_utf16().chain(Some(0)).collect();
                let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
                let _ = RegSetValueExW(key, VALUE, None, REG_SZ, Some(bytes));
            }
        } else {
            let _ = RegDeleteValueW(key, VALUE);
        }
        let _ = RegCloseKey(key);
    }
}

/// Re-registers autostart with the current exe path, so moving or updating
/// the program does not leave a stale entry.
pub fn refresh() {
    if is_enabled() {
        set_enabled(true);
    }
}
