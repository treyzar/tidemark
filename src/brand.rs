//! Shared embedded application identity. Keep technical IDs stable for upgrades.
use windows::core::PCWSTR;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{LoadIconW, HICON};

pub fn icon() -> HICON {
    unsafe {
        LoadIconW(
            GetModuleHandleW(None).ok().map(Into::into),
            PCWSTR(101usize as *const u16),
        )
        .unwrap_or_default()
    }
}
