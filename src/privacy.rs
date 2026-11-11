//! Which apps are using the camera or microphone right now. Windows keeps
//! this in the registry ("CapabilityAccessManager\ConsentStore"): an app is
//! using the device while LastUsedTimeStart is set and LastUsedTimeStop is
//! 0. A thread sleeps on RegNotifyChangeKeyValue and wakes only on changes.

use std::sync::{Arc, Mutex};
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_PRIVACY: u32 = WM_APP + 10;
const STORE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore");

#[derive(Debug, Clone, Default, PartialEq)]
pub struct InUse {
    pub camera: Vec<String>,
    pub microphone: Vec<String>,
}

/// "Microsoft.WindowsCamera_8wekyb3d8bbwe" -> "WindowsCamera";
/// "C:#Program Files#Discord#Discord.exe" -> "Discord".
pub fn app_label(key: &str) -> String {
    if key.contains('#') {
        let file = key.rsplit('#').next().unwrap_or(key);
        return file.rsplit_once('.').map_or(file, |(s, _)| s).to_string();
    }
    let package = key.split('_').next().unwrap_or(key);
    package.rsplit('.').next().unwrap_or(package).to_string()
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn open(parent: HKEY, path: PCWSTR) -> Option<Key> {
    let mut key = HKEY::default();
    unsafe { RegOpenKeyExW(parent, path, None, KEY_READ | KEY_NOTIFY, &mut key) }.is_ok().then_some(Key(key))
}

fn qword(key: HKEY, name: PCWSTR) -> u64 {
    let mut value = 0u64;
    let mut size = 8u32;
    let ok = unsafe { RegQueryValueExW(key, name, None, None, Some(&mut value as *mut u64 as *mut u8), Some(&mut size)) };
    if ok.is_ok() { value } else { 0 }
}

fn subkeys(key: HKEY) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0.. {
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let r = unsafe { RegEnumKeyExW(key, i, Some(PWSTR(buf.as_mut_ptr())), &mut len, None, None, None, None) };
        if r.is_err() {
            break;
        }
        out.push(String::from_utf16_lossy(&buf[..len as usize]));
    }
    out
}

fn in_use(store: HKEY, capability: &str) -> Vec<String> {
    let mut apps = Vec::new();
    let Some(cap) = open(store, PCWSTR(windows::core::HSTRING::from(capability).as_ptr())) else { return apps };
    let mut scan = |parent: HKEY, names: Vec<String>| {
        for name in names {
            if name == "NonPackaged" {
                continue;
            }
            let wide = windows::core::HSTRING::from(name.as_str());
            if let Some(k) = open(parent, PCWSTR(wide.as_ptr())) {
                if qword(k.0, w!("LastUsedTimeStart")) != 0 && qword(k.0, w!("LastUsedTimeStop")) == 0 {
                    let label = app_label(&name);
                    if !apps.contains(&label) {
                        apps.push(label);
                    }
                }
            }
        }
    };
    scan(cap.0, subkeys(cap.0));
    if let Some(np) = open(cap.0, w!("NonPackaged")) {
        scan(np.0, subkeys(np.0));
    }
    apps
}

pub fn scan() -> InUse {
    let Some(store) = open(HKEY_CURRENT_USER, STORE) else { return InUse::default() };
    InUse { camera: in_use(store.0, "webcam"), microphone: in_use(store.0, "microphone") }
}

pub struct Watcher {
    pub latest: Arc<Mutex<InUse>>,
}

impl Watcher {
    pub fn start(hwnd: HWND) -> Option<Self> {
        let latest = Arc::new(Mutex::new(scan()));
        let shared = latest.clone();
        let hwnd_raw = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("privacy".into())
            .stack_size(128 * 1024)
            .spawn(move || unsafe {
                let Some(store) = open(HKEY_CURRENT_USER, STORE) else { return };
                let Ok(event) = CreateEventW(None, false, false, None) else { return };
                loop {
                    let filter = REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_CHANGE_NAME;
                    if RegNotifyChangeKeyValue(store.0, true, filter, Some(event), true).is_err() {
                        break;
                    }
                    if WaitForSingleObject(event, INFINITE) != WAIT_OBJECT_0 {
                        break;
                    }
                    // Apps write several values in a row; let them settle.
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    let now = scan();
                    let changed = {
                        let mut cur = shared.lock().unwrap();
                        let changed = *cur != now;
                        *cur = now;
                        changed
                    };
                    if changed {
                        let _ = PostMessageW(Some(HWND(hwnd_raw as *mut _)), WM_PRIVACY, WPARAM(0), LPARAM(0));
                    }
                }
                let _ = CloseHandle(event);
            })
            .ok()?;
        Some(Self { latest })
    }

    pub fn get(&self) -> InUse {
        self.latest.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::app_label;

    #[test]
    fn labels_from_registry_keys() {
        assert_eq!(app_label("Microsoft.WindowsCamera_8wekyb3d8bbwe"), "WindowsCamera");
        assert_eq!(app_label("C:#Users#me#AppData#Local#Discord#app-1.0#Discord.exe"), "Discord");
        assert_eq!(app_label("C:#Program Files#Zoom#bin#Zoom.exe"), "Zoom");
    }
}

/// Live: `cargo test privacy::live -- --ignored --nocapture`.
#[cfg(test)]
mod live {
    #[test]
    #[ignore]
    fn current_state() {
        println!("in use: {:?}", super::scan());
    }
}
