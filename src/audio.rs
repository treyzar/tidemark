//! Per-app volume: finds the audio sessions on the default output device
//! that belong to the media app and reads/sets their own volume, leaving
//! every other sound alone.

use windows::core::{Interface, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::*;
use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};

/// Does the audio session of process `exe`/`aumid` belong to the SMTC app
/// `app_id`? SMTC reports desktop apps by exe name ("Spotify.exe") or a
/// short name ("Chrome", "MSEdge"), and Store apps by full AUMID.
pub fn matches(app_id: &str, exe: &str, aumid: Option<&str>) -> bool {
    let app = app_id.trim().to_ascii_lowercase();
    if app.is_empty() {
        return false;
    }
    if aumid.is_some_and(|a| a.to_ascii_lowercase() == app) {
        return true;
    }
    let exe = exe.to_ascii_lowercase();
    let stem = |s: &str| s.strip_suffix(".exe").unwrap_or(s).to_string();
    let exe_stem = stem(&exe);
    !exe_stem.is_empty() && (exe == app || exe_stem == stem(&app))
}

struct Process(HANDLE);

impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// (exe file name, AppUserModelId if packaged) of a process.
fn process_identity(pid: u32) -> Option<(String, Option<String>)> {
    unsafe {
        let p = Process(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        QueryFullProcessImageNameW(p.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        let exe = path.rsplit('\\').next().unwrap_or_default().to_string();
        let mut alen = buf.len() as u32;
        let aumid = (GetApplicationUserModelId(p.0, &mut alen, Some(PWSTR(buf.as_mut_ptr()))).is_ok() && alen > 1)
            .then(|| String::from_utf16_lossy(&buf[..alen as usize - 1]));
        Some((exe, aumid))
    }
}

fn sessions_for(app_id: &str) -> Vec<ISimpleAudioVolume> {
    let mut out = Vec::new();
    let Some(sessions) = (|| unsafe {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
        let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None).ok()?;
        manager.GetSessionEnumerator().ok()
    })() else {
        return out;
    };
    unsafe {
        let count = sessions.GetCount().unwrap_or(0);
        for i in 0..count {
            let Ok(control) = sessions.GetSession(i) else { continue };
            let Ok(control2) = control.cast::<IAudioSessionControl2>() else { continue };
            let Ok(pid) = control2.GetProcessId() else { continue };
            if pid == 0 {
                continue; // System sounds.
            }
            let Some((exe, aumid)) = process_identity(pid) else { continue };
            if matches(app_id, &exe, aumid.as_deref()) {
                if let Ok(v) = control.cast::<ISimpleAudioVolume>() {
                    out.push(v);
                }
            }
        }
    }
    out
}

/// The app's session volume (0..1), or None if it has no audio session.
pub fn app_volume(app_id: &str) -> Option<f32> {
    let first = sessions_for(app_id).into_iter().next()?;
    unsafe { first.GetMasterVolume().ok() }
}

/// Sets every audio session of the app (browsers have several).
pub fn set_app_volume(app_id: &str, volume: f32) {
    for v in sessions_for(app_id) {
        unsafe {
            let _ = v.SetMasterVolume(volume.clamp(0.0, 1.0), std::ptr::null());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn matches_smtc_app_ids_to_processes() {
        assert!(matches("Spotify.exe", "Spotify.exe", None));
        assert!(matches("Chrome", "chrome.exe", None));
        assert!(matches("MSEdge", "msedge.exe", None));
        assert!(matches(
            "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify",
            "Spotify.exe",
            Some("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify")
        ));
        assert!(!matches("Spotify.exe", "chrome.exe", None));
        assert!(!matches("", "chrome.exe", None));
        assert!(!matches("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic", "chrome.exe", None));
    }
}
