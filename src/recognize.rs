//! Reading screenshots: text via the built-in Windows OCR engine (offline,
//! languages from the user's profile) and QR codes via `rqrr`. OCR runs on
//! its own MTA thread and reports back with a posted message.

use crate::capture::Image;
use std::sync::{Arc, Mutex};
use windows::core::Result;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_OCR: u32 = WM_APP + 15;

/// Small crops OCR badly; scale them up with nearest-neighbour sampling.
pub fn upscale(img: &Image, factor: u32) -> Image {
    let (w, h) = (img.w * factor, img.h * factor);
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            pixels.extend_from_slice(&img.pixel(x / factor, y / factor));
        }
    }
    Image { w, h, pixels }
}

fn ocr(img: &Image) -> Result<String> {
    let img = if img.h < 60 || img.w < 60 { upscale(img, 3) } else if img.h < 160 { upscale(img, 2) } else { img.clone() };
    let buffer = CryptographicBuffer::CreateFromByteArray(&img.pixels)?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, img.w as i32, img.h as i32)?;
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()?;
    let result = engine.RecognizeAsync(&bitmap)?.get()?;
    let lines: Vec<String> = result.Lines()?.into_iter().filter_map(|l| l.Text().ok()).map(|t| t.to_string()).collect();
    Ok(lines.join("\n"))
}

/// Recognizes text on a background thread; posts WM_OCR when done.
pub fn spawn_ocr(hwnd: HWND, img: Image, slot: Arc<Mutex<Option<String>>>) {
    let hwnd_raw = hwnd.0 as isize;
    let _ = std::thread::Builder::new().name("ocr".into()).spawn(move || {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let text = ocr(&img).unwrap_or_default();
        *slot.lock().unwrap() = Some(text);
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd_raw as *mut _)), WM_OCR, WPARAM(0), LPARAM(0));
        }
    });
}

/// Contents of the first QR code found, if any.
pub fn qr(img: &Image) -> Option<String> {
    let luma = img.luma();
    let w = img.w as usize;
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(img.w as usize, img.h as usize, |x, y| luma[y * w + x]);
    prepared.detect_grids().into_iter().find_map(|g| g.decode().ok().map(|(_, content)| content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upscale_repeats_pixels() {
        let img = Image { w: 2, h: 1, pixels: vec![1, 1, 1, 255, 9, 9, 9, 255] };
        let up = upscale(&img, 2);
        assert_eq!((up.w, up.h), (4, 2));
        assert_eq!(up.pixel(1, 1), [1, 1, 1, 255]);
        assert_eq!(up.pixel(2, 0), [9, 9, 9, 255]);
    }

    #[test]
    fn no_qr_in_noise() {
        let img = Image { w: 40, h: 40, pixels: (0..40 * 40).flat_map(|i| { let v = (i * 37 % 255) as u8; [v, v, v, 255] }).collect() };
        assert_eq!(qr(&img), None);
    }

    /// Live: `cargo test recognize::live -- --ignored --nocapture` OCRs the
    /// top-left of the primary screen.
    #[test]
    #[ignore]
    fn live_ocr() {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let img = crate::capture::grab(windows::Win32::Foundation::RECT { left: 0, top: 0, right: 900, bottom: 300 }).unwrap();
        let t = std::time::Instant::now();
        let text = ocr(&img).unwrap();
        println!("OCR {:?}: {} chars in {} lines", t.elapsed(), text.chars().count(), text.lines().count());
    }
}
