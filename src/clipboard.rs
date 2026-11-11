//! Windows clipboard access for the history: reads what was just copied
//! (text, image, files), honours the "do not record" markers that password
//! managers set, and puts a history item back on the clipboard.

use crate::clip_history::{self, Entry, MAX_TEXT_CHARS};
use std::hash::{Hash, Hasher};
use windows::core::{w, Result, GUID, PCWSTR};
use windows::Win32::Foundation::{GENERIC_WRITE, HANDLE, HGLOBAL, HWND};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{CF_DIB, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

/// Keeps the clipboard open for the lifetime of the guard.
struct Open;

impl Open {
    fn new(hwnd: HWND) -> Option<Self> {
        // Another app may hold it for a moment right after copying.
        for _ in 0..5 {
            if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
                return Some(Open);
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        None
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

fn format(name: PCWSTR) -> u32 {
    unsafe { RegisterClipboardFormatW(name) }
}

fn available(fmt: u32) -> bool {
    unsafe { IsClipboardFormatAvailable(fmt) }.is_ok()
}

/// Bytes of a clipboard format, copied out of its global memory.
fn bytes_of(fmt: u32) -> Option<Vec<u8>> {
    unsafe {
        let handle = GetClipboardData(fmt).ok()?;
        let global = HGLOBAL(handle.0);
        let size = GlobalSize(global);
        let ptr = GlobalLock(global) as *const u8;
        if ptr.is_null() {
            return None;
        }
        let out = std::slice::from_raw_parts(ptr, size).to_vec();
        let _ = GlobalUnlock(global);
        Some(out)
    }
}

/// Password managers and other apps mark secrets as not-for-history.
fn excluded() -> bool {
    if available(format(w!("ExcludeClipboardContentFromMonitorProcessing"))) || available(format(w!("Clipboard Viewer Ignore"))) {
        return true;
    }
    let can_include = format(w!("CanIncludeInClipboardHistory"));
    if available(can_include) {
        if let Some(b) = bytes_of(can_include) {
            if b.len() >= 4 && u32::from_le_bytes([b[0], b[1], b[2], b[3]]) == 0 {
                return true;
            }
        }
    }
    false
}

/// What is on the clipboard now, as a history entry (images are written to
/// the history folder as PNG). None when empty, unsupported or excluded.
pub fn read(hwnd: HWND) -> Option<Entry> {
    let _open = Open::new(hwnd)?;
    if excluded() {
        return None;
    }
    if available(CF_HDROP.0 as u32) {
        let paths = read_files()?;
        return (!paths.is_empty()).then_some(Entry::Files { paths });
    }
    if available(CF_UNICODETEXT.0 as u32) {
        let raw = bytes_of(CF_UNICODETEXT.0 as u32)?;
        let units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
        if units.len() > MAX_TEXT_CHARS * 2 {
            return None;
        }
        return Some(Entry::Text { text: String::from_utf16_lossy(&units) });
    }
    if available(CF_DIB.0 as u32) {
        let dib = bytes_of(CF_DIB.0 as u32)?;
        return save_dib_as_png(&dib).ok();
    }
    None
}

fn read_files() -> Option<Vec<String>> {
    unsafe {
        let handle = GetClipboardData(CF_HDROP.0 as u32).ok()?;
        let drop = HDROP(handle.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let len = DragQueryFileW(drop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(drop, i, Some(&mut buf));
            out.push(String::from_utf16_lossy(&buf[..len]));
        }
        Some(out)
    }
}

fn wic() -> Result<IWICImagingFactory> {
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
}

/// Wraps a CF_DIB (BITMAPINFOHEADER + pixels) into a BMP file in memory.
pub fn dib_to_bmp(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |o: usize| Some(u32::from_le_bytes(dib.get(o..o + 4)?.try_into().ok()?));
    let header_size = u32_at(0)? as usize;
    let bit_count = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?);
    let compression = u32_at(16)?;
    let colors_used = u32_at(32)? as usize;
    let palette = if bit_count <= 8 { (if colors_used == 0 { 1usize << bit_count } else { colors_used }) * 4 } else { 0 };
    // BI_BITFIELDS with a plain 40-byte header has 3 masks after it.
    let masks = if compression == 3 && header_size == 40 { 12 } else { 0 };
    let offset = 14 + header_size + palette + masks;
    let total = 14 + dib.len();
    let mut bmp = Vec::with_capacity(total);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(total as u32).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&(offset as u32).to_le_bytes());
    bmp.extend_from_slice(dib);
    Some(bmp)
}

fn save_dib_as_png(dib: &[u8]) -> Result<Entry> {
    let bmp = dib_to_bmp(dib).ok_or_else(windows::core::Error::empty)?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    dib.hash(&mut h);
    let name = format!("img-{:016x}.png", h.finish());
    let path = clip_history::dir().join(&name);
    let _ = std::fs::create_dir_all(clip_history::dir());
    unsafe {
        let wic = wic()?;
        let stream = wic.CreateStream()?;
        stream.InitializeFromMemory(&bmp)?;
        let decoder = wic.CreateDecoderFromStream(&stream, &GUID::zeroed(), WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let (mut width, mut height) = (0, 0);
        frame.GetSize(&mut width, &mut height)?;
        if !path.exists() {
            let out = wic.CreateStream()?;
            let wide: Vec<u16> = path.as_os_str().encode_wide_null();
            out.InitializeFromFilename(PCWSTR(wide.as_ptr()), GENERIC_WRITE.0)?;
            let encoder = wic.CreateEncoder(&GUID_ContainerFormatPng, &GUID::zeroed())?;
            encoder.Initialize(&out, WICBitmapEncoderNoCache)?;
            let mut target = None;
            encoder.CreateNewFrame(&mut target, std::ptr::null_mut())?;
            let target = target.ok_or_else(windows::core::Error::empty)?;
            target.Initialize(None)?;
            target.SetSize(width, height)?;
            // 32-bit DIBs usually carry no real alpha: store opaque BGR.
            let mut fmt = GUID_WICPixelFormat24bppBGR;
            target.SetPixelFormat(&mut fmt)?;
            let converter = wic.CreateFormatConverter()?;
            converter.Initialize(&frame, &fmt, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
            target.WriteSource(&converter, std::ptr::null())?;
            target.Commit()?;
            encoder.Commit()?;
        }
        Ok(Entry::Image { file: name, width, height })
    }
}

trait WideNull {
    fn encode_wide_null(&self) -> Vec<u16>;
}

impl WideNull for std::ffi::OsStr {
    fn encode_wide_null(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(Some(0)).collect()
    }
}

/// Copies `bytes` into movable global memory for SetClipboardData.
fn global(bytes: &[u8]) -> Option<HANDLE> {
    unsafe {
        let g = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).ok()?;
        let ptr = GlobalLock(g) as *mut u8;
        if ptr.is_null() {
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        let _ = GlobalUnlock(g);
        Some(HANDLE(g.0))
    }
}

/// CF_HDROP payload: DROPFILES header + double-null-terminated wide paths.
pub fn drop_files_bytes(paths: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&20u32.to_le_bytes()); // pFiles: offset of the list
    out.extend_from_slice(&[0u8; 8]); // pt
    out.extend_from_slice(&0u32.to_le_bytes()); // fNC
    out.extend_from_slice(&1u32.to_le_bytes()); // fWide
    for p in paths {
        for u in p.encode_utf16().chain(Some(0)) {
            out.extend_from_slice(&u.to_le_bytes());
        }
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// A bottom-up 32-bit CF_DIB from a PNG in the history folder.
fn png_to_dib(file: &str) -> Result<Vec<u8>> {
    unsafe {
        let wic = wic()?;
        let wide: Vec<u16> = clip_history::dir().join(file).as_os_str().encode_wide_null();
        let decoder = wic.CreateDecoderFromFilename(PCWSTR(wide.as_ptr()), None, windows::Win32::Foundation::GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h)?;
        let converter = wic.CreateFormatConverter()?;
        converter.Initialize(&frame, &GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
        let stride = w * 4;
        let mut pixels = vec![0u8; (stride * h) as usize];
        converter.CopyPixels(std::ptr::null(), stride, &mut pixels)?;
        let mut dib = Vec::with_capacity(40 + pixels.len());
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&(w as i32).to_le_bytes());
        dib.extend_from_slice(&(h as i32).to_le_bytes()); // positive: bottom-up
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        dib.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
        dib.extend_from_slice(&[0u8; 16]);
        for row in pixels.chunks_exact(stride as usize).rev() {
            dib.extend_from_slice(row);
        }
        Ok(dib)
    }
}

/// Puts a history entry back on the clipboard.
pub fn write(hwnd: HWND, entry: &Entry) -> bool {
    let (fmt, bytes) = match entry {
        Entry::Text { text } => {
            let mut b: Vec<u8> = text.encode_utf16().chain(Some(0)).flat_map(|u| u.to_le_bytes()).collect();
            b.shrink_to_fit();
            (CF_UNICODETEXT.0 as u32, b)
        }
        Entry::Files { paths } => (CF_HDROP.0 as u32, drop_files_bytes(paths)),
        Entry::Image { file, .. } => match png_to_dib(file) {
            Ok(dib) => (CF_DIB.0 as u32, dib),
            Err(_) => return false,
        },
    };
    let Some(_open) = Open::new(hwnd) else { return false };
    unsafe {
        if EmptyClipboard().is_err() {
            return false;
        }
        let Some(handle) = global(&bytes) else { return false };
        // On success the system owns the memory.
        SetClipboardData(fmt, Some(handle)).is_ok()
    }
}

/// Plain text on the clipboard (for pasting into the island's fields).
pub fn read_text(hwnd: HWND) -> Option<String> {
    let _open = Open::new(hwnd)?;
    let raw = bytes_of(CF_UNICODETEXT.0 as u32)?;
    let units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    Some(String::from_utf16_lossy(&units))
}

pub fn listen(hwnd: HWND) {
    unsafe {
        let _ = AddClipboardFormatListener(hwnd);
    }
}

pub fn stop_listening(hwnd: HWND) {
    unsafe {
        let _ = RemoveClipboardFormatListener(hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bmp_header_offsets() {
        // 40-byte header, 32 bpp, BI_RGB: pixels right after the header.
        let mut dib = vec![0u8; 40 + 16];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        let bmp = dib_to_bmp(&dib).unwrap();
        assert_eq!(&bmp[0..2], b"BM");
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 54);
        // BI_BITFIELDS adds 12 bytes of masks.
        dib[16..20].copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(u32::from_le_bytes(dib_to_bmp(&dib).unwrap()[10..14].try_into().unwrap()), 66);
        // 8 bpp with a full palette.
        dib[14..16].copy_from_slice(&8u16.to_le_bytes());
        dib[16..20].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(u32::from_le_bytes(dib_to_bmp(&dib).unwrap()[10..14].try_into().unwrap()), 14 + 40 + 1024);
    }

    #[test]
    fn hdrop_payload() {
        let b = drop_files_bytes(&["C:\\a".into(), "D:\\b".into()]);
        assert_eq!(u32::from_le_bytes(b[0..4].try_into().unwrap()), 20);
        assert_eq!(u32::from_le_bytes(b[16..20].try_into().unwrap()), 1);
        let list: Vec<u16> = b[20..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let expected: Vec<u16> = "C:\\a\0D:\\b\0\0".encode_utf16().collect();
        assert_eq!(list, expected);
    }
}
