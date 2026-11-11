//! System changes the island announces: master volume, screen brightness,
//! keyboard layout and Caps Lock. All are pushed by Windows; nothing polls.
//!
//! - volume: IAudioEndpointVolume change callback on the output device;
//! - brightness: power-setting notification GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS;
//! - Caps Lock and layout switches: raw keyboard input (not a hook, so it
//!   adds no input lag). Layout switches are key chords (Alt+Shift,
//!   Win+Space...), so releasing a modifier triggers one layout check.

use windows::core::{implement, Result, GUID};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::Globalization::{GetLocaleInfoW, LOCALE_SISO639LANGNAME, LOCALE_SNATIVELANGUAGENAME};
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl};
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator, AUDIO_VOLUME_NOTIFICATION_DATA};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::Power::{RegisterPowerSettingNotification, UnregisterPowerSettingNotification, HPOWERNOTIFY, POWERBROADCAST_SETTING};
use windows::Win32::System::SystemServices::GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL};
use windows::Win32::UI::Input::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_VOLUME: u32 = WM_APP + 9;

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCallback {
    hwnd: isize,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        if let Some(d) = unsafe { data.as_ref() } {
            let level = (d.fMasterVolume.clamp(0.0, 1.0) * 1000.0).round() as usize;
            unsafe {
                let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), WM_VOLUME, WPARAM(level), LPARAM(d.bMuted.0 as isize));
            }
        }
        Ok(())
    }
}

/// Keeps the volume callback and brightness registration alive.
pub struct Signals {
    endpoint: Option<IAudioEndpointVolume>,
    callback: Option<IAudioEndpointVolumeCallback>,
    brightness: Option<HPOWERNOTIFY>,
    /// Windows reports the current brightness right after registering.
    pub brightness_primed: bool,
    caps_down: bool,
    pub caps_on: bool,
}

impl Signals {
    pub fn start(hwnd: HWND) -> Self {
        let (endpoint, callback) = unsafe {
            let cb: IAudioEndpointVolumeCallback = VolumeCallback { hwnd: hwnd.0 as isize }.into();
            let ep = (|| -> Result<IAudioEndpointVolume> {
                let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
                let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
                let ep: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
                ep.RegisterControlChangeNotify(&cb)?;
                Ok(ep)
            })()
            .ok();
            (ep.clone(), ep.map(|_| cb))
        };
        let brightness = unsafe {
            RegisterPowerSettingNotification(HANDLE(hwnd.0), &GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS, DEVICE_NOTIFY_WINDOW_HANDLE).ok()
        };
        unsafe {
            let devices = [RAWINPUTDEVICE { usUsagePage: 0x01, usUsage: 0x06, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd }];
            let _ = RegisterRawInputDevices(&devices, std::mem::size_of::<RAWINPUTDEVICE>() as u32);
        }
        let caps_on = unsafe { GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0 };
        Self { endpoint, callback, brightness, brightness_primed: false, caps_down: false, caps_on }
    }

    pub fn stop(&mut self) {
        unsafe {
            if let (Some(ep), Some(cb)) = (self.endpoint.take(), self.callback.take()) {
                let _ = ep.UnregisterControlChangeNotify(&cb);
            }
            if let Some(h) = self.brightness.take() {
                let _ = UnregisterPowerSettingNotification(h);
            }
            let devices = [RAWINPUTDEVICE { usUsagePage: 0x01, usUsage: 0x06, dwFlags: RIDEV_REMOVE, hwndTarget: HWND::default() }];
            let _ = RegisterRawInputDevices(&devices, std::mem::size_of::<RAWINPUTDEVICE>() as u32);
        }
    }

    /// WM_INPUT: what a key event means for the island.
    pub fn raw_input(&mut self, lparam: LPARAM) -> Option<KeyEvent> {
        let mut raw = RAWINPUT::default();
        let mut size = std::mem::size_of::<RAWINPUT>() as u32;
        let got = unsafe {
            GetRawInputData(HRAWINPUT(lparam.0 as *mut _), RID_INPUT, Some(&mut raw as *mut _ as *mut _), &mut size, std::mem::size_of::<RAWINPUTHEADER>() as u32)
        };
        if got == u32::MAX || raw.header.dwType != RIM_TYPEKEYBOARD.0 {
            return None;
        }
        let kb = unsafe { raw.data.keyboard };
        let released = kb.Flags as u32 & RI_KEY_BREAK != 0;
        if kb.VKey != VK_CAPITAL.0 {
            return (released && is_layout_key(kb.VKey)).then_some(KeyEvent::LayoutKeyUp);
        }
        if released {
            self.caps_down = false;
            return None;
        }
        if self.caps_down {
            return None; // Auto-repeat while held.
        }
        self.caps_down = true;
        self.caps_on = !self.caps_on;
        Some(KeyEvent::Caps(self.caps_on))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyEvent {
    Caps(bool),
    /// A key used in layout-switch chords was released.
    LayoutKeyUp,
}

/// Shift, Ctrl, Alt, Win (either side) and Space.
pub fn is_layout_key(vk: u16) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0x20 | 0x5B | 0x5C | 0xA0..=0xA5)
}

/// Keyboard layout of the foreground window's thread, with that window.
pub fn foreground_layout() -> (isize, isize) {
    unsafe {
        let fg = GetForegroundWindow();
        let thread = GetWindowThreadProcessId(fg, None);
        (fg.0 as isize, windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout(thread).0 as isize)
    }
}

/// Brightness percent from a PBT_POWERSETTINGCHANGE payload.
pub fn brightness_from(lparam: LPARAM) -> Option<u32> {
    let setting = unsafe { (lparam.0 as *const POWERBROADCAST_SETTING).as_ref()? };
    if setting.PowerSetting != GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS || setting.DataLength < 4 {
        return None;
    }
    let value = unsafe { std::ptr::read_unaligned(setting.Data.as_ptr() as *const u32) };
    Some(value.min(100))
}

/// ("RU", "Русский") for a keyboard layout handle.
pub fn layout_name(hkl: isize) -> (String, String) {
    let langid = (hkl as usize & 0xFFFF) as u32;
    let get = |kind| unsafe {
        let mut buf = [0u16; 64];
        let n = GetLocaleInfoW(langid, kind, Some(&mut buf));
        if n > 1 { String::from_utf16_lossy(&buf[..n as usize - 1]) } else { String::new() }
    };
    let code = get(LOCALE_SISO639LANGNAME).to_uppercase();
    let mut name = get(LOCALE_SNATIVELANGUAGENAME);
    if let Some(first) = name.chars().next() {
        name = first.to_uppercase().collect::<String>() + &name[first.len_utf8()..];
    }
    (if code.is_empty() { format!("{langid:04X}") } else { code }, name)
}

#[allow(dead_code)]
const _: GUID = GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_keys() {
        for vk in [0x10, 0xA0, 0xA1, 0x12, 0xA4, 0x11, 0x5B, 0x20] {
            assert!(is_layout_key(vk), "{vk:#x}");
        }
        assert!(!is_layout_key(0x41)); // 'A'
        assert!(!is_layout_key(0x14)); // Caps Lock is handled on its own
    }

    #[test]
    fn layout_names() {
        let (code, name) = layout_name(0x0419_0419);
        assert_eq!(code, "RU");
        assert_eq!(name, "Русский");
        let (code, _) = layout_name(0x0409_0409);
        assert_eq!(code, "EN");
    }
}
