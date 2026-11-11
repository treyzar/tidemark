//! Throne (sing-box proxy client) without any setup on its side: the island
//! reads whether it runs, whether its TUN adapter is up and whether the
//! Windows system proxy is on, can switch the system proxy, and opens
//! Throne's own window for everything else (servers, TUN).

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_BUFFER_OVERFLOW};
use windows::Win32::NetworkManagement::IpHelper::*;
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinInet::{InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED};
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    /// Throne.exe exists in its usual folder.
    pub installed: bool,
    pub running: bool,
    pub tun_up: bool,
    pub proxy_on: bool,
    /// "127.0.0.1:2080"
    pub proxy_server: String,
}

pub fn exe_path() -> Option<String> {
    let p = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from)?.join("Throne").join("Throne.exe");
    p.exists().then(|| p.to_string_lossy().into_owned())
}

fn process_running(name: &str) -> bool {
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return false };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = false;
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
                if String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case(name) {
                    found = true;
                    break;
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        found
    }
}

/// Is a network adapter named like Throne's TUN ("throne-tun", or a
/// "sing-tun" device) up?
pub fn is_tun_name(friendly: &str, description: &str) -> bool {
    let f = friendly.to_ascii_lowercase();
    let d = description.to_ascii_lowercase();
    f.contains("throne") || d.contains("sing-tun")
}

fn tun_up() -> bool {
    unsafe {
        let mut size = 16 * 1024u32;
        for _ in 0..3 {
            let mut buf = vec![0u8; size as usize];
            let first = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
            let r = GetAdaptersAddresses(0, GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER, None, Some(first), &mut size);
            if r == ERROR_BUFFER_OVERFLOW.0 {
                continue;
            }
            if r != 0 {
                return false;
            }
            let mut a = first;
            while !a.is_null() {
                let friendly = (*a).FriendlyName.to_string().unwrap_or_default();
                let description = (*a).Description.to_string().unwrap_or_default();
                if is_tun_name(&friendly, &description) && (*a).OperStatus == IfOperStatusUp {
                    return true;
                }
                a = (*a).Next;
            }
            return false;
        }
        false
    }
}

const INTERNET_SETTINGS: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings");

fn proxy() -> (bool, String) {
    unsafe {
        let mut enable = 0u32;
        let mut size = 4u32;
        let on = RegGetValueW(HKEY_CURRENT_USER, INTERNET_SETTINGS, w!("ProxyEnable"), RRF_RT_REG_DWORD, None, Some(&mut enable as *mut u32 as *mut _), Some(&mut size)).is_ok() && enable != 0;
        let mut buf = [0u16; 256];
        let mut bytes = (buf.len() * 2) as u32;
        let server = if RegGetValueW(HKEY_CURRENT_USER, INTERNET_SETTINGS, w!("ProxyServer"), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut bytes)).is_ok() {
            let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
            String::from_utf16_lossy(&buf[..len])
        } else {
            String::new()
        };
        (on, server)
    }
}

pub fn status() -> Status {
    let (proxy_on, proxy_server) = proxy();
    Status { installed: exe_path().is_some(), running: process_running("Throne.exe"), tun_up: tun_up(), proxy_on, proxy_server }
}

/// Turns the Windows system proxy on or off (keeping its address) and tells
/// running browsers to re-read the settings.
pub fn set_proxy(on: bool) -> bool {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, INTERNET_SETTINGS, None, KEY_SET_VALUE, &mut key).is_err() {
            return false;
        }
        let value = (on as u32).to_le_bytes();
        let ok = RegSetValueExW(key, w!("ProxyEnable"), None, REG_DWORD, Some(&value)).is_ok();
        let _ = RegCloseKey(key);
        let _ = InternetSetOptionW(None, INTERNET_OPTION_SETTINGS_CHANGED, None, 0);
        let _ = InternetSetOptionW(None, INTERNET_OPTION_REFRESH, None, 0);
        ok
    }
}

/// Brings Throne's window up: a visible one is focused; otherwise Throne
/// is started, and an already running copy shows its own window.
pub fn open() {
    unsafe {
        if let Ok(h) = FindWindowW(None, w!("Throne")) {
            if IsWindowVisible(h).as_bool() {
                let _ = SetForegroundWindow(h);
                return;
            }
        }
    }
    if let Some(exe) = exe_path() {
        crate::dock::launch(&exe);
    }
}

/// "TUN включён · прокси 127.0.0.1:2080" style summary.
pub fn summary(s: &Status) -> String {
    if !s.running {
        return "Не запущен".into();
    }
    match (s.tun_up, s.proxy_on) {
        (true, true) => "Работает · TUN и системный прокси".into(),
        (true, false) => "Работает · TUN".into(),
        (false, true) => "Работает · только системный прокси".into(),
        (false, false) => "Запущен, но трафик не идёт через него".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tun_adapter_names() {
        assert!(is_tun_name("throne-tun", "sing-tun Tunnel"));
        assert!(is_tun_name("Ethernet 3", "sing-tun Tunnel"));
        assert!(!is_tun_name("Wi-Fi", "Intel(R) Wi-Fi 6E AX211"));
    }

    #[test]
    fn summaries() {
        let mut s = Status { running: true, tun_up: true, proxy_on: true, ..Default::default() };
        assert_eq!(summary(&s), "Работает · TUN и системный прокси");
        s.tun_up = false;
        assert_eq!(summary(&s), "Работает · только системный прокси");
        s.running = false;
        assert_eq!(summary(&s), "Не запущен");
    }

    /// Live (read-only): `cargo test throne::tests::live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live() {
        println!("{:?}", status());
    }
}
