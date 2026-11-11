//! App dock on the Home view: pinned programs/shortcuts from the config,
//! their 48 px icons, launching, and picking a new app to pin.

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use windows::core::{w, Interface, Result, HSTRING, PWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, DestroyIcon, PrivateExtractIconsW, ASFW_ANY, HICON, SW_SHOWNORMAL};

pub const ICON_PX: u32 = 48;

/// Apps pinned on first run; ones missing on this PC are skipped.
pub fn default_apps() -> Vec<String> {
    [
        "%SystemRoot%\\explorer.exe",
        "%SystemRoot%\\System32\\notepad.exe",
        "%SystemRoot%\\System32\\Taskmgr.exe",
        "%SystemRoot%\\System32\\cmd.exe",
    ]
    .map(String::from)
    .to_vec()
}

#[derive(Debug, Clone)]
pub struct App {
    /// As written in the config (may contain %VARS%).
    pub entry: String,
    pub path: String,
    pub id: u64,
    /// Premultiplied BGRA, ICON_PX square.
    pub icon: Option<Arc<Vec<u8>>>,
}

pub fn expand(entry: &str) -> String {
    let src = HSTRING::from(entry);
    let mut buf = vec![0u16; 1024];
    let n = unsafe { ExpandEnvironmentStringsW(&src, Some(&mut buf)) } as usize;
    if n == 0 || n > buf.len() {
        return entry.to_string();
    }
    String::from_utf16_lossy(&buf[..n - 1])
}

/// Stable icon id for a file path (shared by the dock and app notices).
pub fn path_id(path: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.to_lowercase().hash(&mut h);
    h.finish()
}

pub fn load(entries: &[String]) -> Vec<App> {
    entries
        .iter()
        .filter_map(|entry| {
            let path = expand(entry);
            if !std::path::Path::new(&path).exists() {
                return None;
            }
            Some(App { entry: entry.clone(), id: path_id(&path), icon: icon(&path).ok(), path })
        })
        .collect()
}

/// Where a file's icon lives: shortcuts point at their icon location (or
/// their target), programs carry their own.
fn icon_source(path: &str) -> (String, i32) {
    if !path.to_ascii_lowercase().ends_with(".lnk") {
        return (path.to_string(), 0);
    }
    let resolved = (|| unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        link.cast::<IPersistFile>().ok()?.Load(&HSTRING::from(path), STGM_READ).ok()?;
        let mut buf = [0u16; 260];
        let mut index = 0i32;
        if link.GetIconLocation(&mut buf, &mut index).is_ok() && buf[0] != 0 {
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            return Some((expand(&String::from_utf16_lossy(&buf[..len])), index));
        }
        let mut buf = [0u16; 260];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        (len > 0).then(|| (String::from_utf16_lossy(&buf[..len]), 0))
    })();
    resolved.unwrap_or_else(|| (path.to_string(), 0))
}

/// A 48 px icon read straight from the file. Deliberately avoids the
/// shell's system image list, which costs ~12 MB of memory to load.
pub fn icon(path: &str) -> Result<Arc<Vec<u8>>> {
    let (file, index) = icon_source(path);
    let mut name = [0u16; 260];
    for (dst, src) in name.iter_mut().zip(file.encode_utf16().take(259)) {
        *dst = src;
    }
    unsafe {
        let mut icons = [HICON::default(); 1];
        let got = PrivateExtractIconsW(&name, index, ICON_PX as i32, ICON_PX as i32, Some(&mut icons), None, 0);
        if got == 0 || got == u32::MAX || icons[0].is_invalid() {
            return Err(windows::core::Error::empty());
        }
        let hicon = icons[0];
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let result = (|| {
            let bitmap = wic.CreateBitmapFromHICON(hicon)?;
            let scaler = wic.CreateBitmapScaler()?;
            scaler.Initialize(&bitmap, ICON_PX, ICON_PX, WICBitmapInterpolationModeFant)?;
            let converter = wic.CreateFormatConverter()?;
            converter.Initialize(&scaler, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
            let mut pixels = vec![0u8; (ICON_PX * ICON_PX * 4) as usize];
            converter.CopyPixels(std::ptr::null(), ICON_PX * 4, &mut pixels)?;
            Ok(Arc::new(pixels))
        })();
        let _ = DestroyIcon(hicon);
        result
    }
}

/// Opens the app. The island just received the click, so it may hand
/// foreground rights to the new process.
pub fn launch(path: &str) {
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        ShellExecuteW(None, w!("open"), &HSTRING::from(path), None, None, SW_SHOWNORMAL);
    }
}

/// Standard "Open" dialog for a program or shortcut.
pub fn pick(owner: HWND) -> Option<String> {
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let filters = [COMDLG_FILTERSPEC { pszName: w!("Программы и ярлыки"), pszSpec: w!("*.exe;*.lnk") }];
        dialog.SetFileTypes(&filters).ok()?;
        dialog.SetTitle(w!("Добавить в док")).ok()?;
        dialog.Show(Some(owner)).ok()?;
        let item = dialog.GetResult().ok()?;
        let name: PWSTR = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        path
    }
}
