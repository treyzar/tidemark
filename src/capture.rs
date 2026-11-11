//! Screen pixels: a snapshot of the whole virtual screen (all monitors, in
//! physical pixels), cropping, pixelation for hidden parts, and the BGRA
//! image the editor and OCR work on.

use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN};

/// Top-down BGRA pixels, alpha = 255.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub pixels: Vec<u8>,
}

/// The whole virtual screen; (x, y) is its top-left in screen coordinates.
pub struct Shot {
    pub x: i32,
    pub y: i32,
    pub image: Image,
}

impl Image {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.w + x) * 4) as usize;
        [self.pixels[i], self.pixels[i + 1], self.pixels[i + 2], self.pixels[i + 3]]
    }

    /// The part inside (x, y, w, h), clamped to the image.
    pub fn crop(&self, x: i32, y: i32, w: i32, h: i32) -> Image {
        let x0 = x.clamp(0, self.w as i32) as u32;
        let y0 = y.clamp(0, self.h as i32) as u32;
        let x1 = (x + w).clamp(0, self.w as i32) as u32;
        let y1 = (y + h).clamp(0, self.h as i32) as u32;
        let (cw, ch) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
        let mut pixels = Vec::with_capacity((cw * ch * 4) as usize);
        for row in y0..y1 {
            let start = ((row * self.w + x0) * 4) as usize;
            pixels.extend_from_slice(&self.pixels[start..start + (cw * 4) as usize]);
        }
        Image { w: cw, h: ch, pixels }
    }

    /// Block-averaged copy, (w/block) x (h/block): drawn back stretched with
    /// nearest-neighbour sampling it hides what was there.
    pub fn pixelate(&self, block: u32) -> Image {
        let block = block.max(1);
        let (bw, bh) = (self.w.div_ceil(block).max(1), self.h.div_ceil(block).max(1));
        let mut pixels = Vec::with_capacity((bw * bh * 4) as usize);
        for by in 0..bh {
            for bx in 0..bw {
                let mut sum = [0u32; 3];
                let mut n = 0u32;
                for y in by * block..((by + 1) * block).min(self.h) {
                    for x in bx * block..((bx + 1) * block).min(self.w) {
                        let p = self.pixel(x, y);
                        for c in 0..3 {
                            sum[c] += p[c] as u32;
                        }
                        n += 1;
                    }
                }
                let n = n.max(1);
                pixels.extend_from_slice(&[(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8, 255]);
            }
        }
        Image { w: bw, h: bh, pixels }
    }

    /// Gray levels for QR decoding.
    pub fn luma(&self) -> Vec<u8> {
        self.pixels.chunks_exact(4).map(|p| ((p[2] as u32 * 299 + p[1] as u32 * 587 + p[0] as u32 * 114) / 1000) as u8).collect()
    }

    /// Bottom-up 32-bit CF_DIB payload.
    pub fn to_dib(&self) -> Vec<u8> {
        let stride = (self.w * 4) as usize;
        let mut dib = Vec::with_capacity(40 + self.pixels.len());
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&(self.w as i32).to_le_bytes());
        dib.extend_from_slice(&(self.h as i32).to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&0u32.to_le_bytes());
        dib.extend_from_slice(&(self.pixels.len() as u32).to_le_bytes());
        dib.extend_from_slice(&[0u8; 16]);
        if stride > 0 {
            for row in self.pixels.chunks_exact(stride).rev() {
                dib.extend_from_slice(row);
            }
        }
        dib
    }
}

pub fn virtual_screen() -> RECT {
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        RECT { left: x, top: y, right: x + GetSystemMetrics(SM_CXVIRTUALSCREEN), bottom: y + GetSystemMetrics(SM_CYVIRTUALSCREEN) }
    }
}

/// Copies the screen area `r` (screen coordinates) into an Image.
pub fn grab(r: RECT) -> Option<Image> {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w <= 0 || h <= 0 {
        return None;
    }
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let result = match CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bmp) if !bits.is_null() => {
                let old = SelectObject(mem, bmp.into());
                let ok = BitBlt(mem, 0, 0, w, h, Some(screen), r.left, r.top, SRCCOPY | CAPTUREBLT).is_ok();
                let mut pixels = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize).to_vec();
                for p in pixels.chunks_exact_mut(4) {
                    p[3] = 255;
                }
                SelectObject(mem, old);
                let _ = DeleteObject(bmp.into());
                ok.then_some(Image { w: w as u32, h: h as u32, pixels })
            }
            _ => None,
        };
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        result
    }
}

pub fn shoot() -> Option<Shot> {
    let r = virtual_screen();
    Some(Shot { x: r.left, y: r.top, image: grab(r)? })
}

/// The monitor under the cursor.
pub fn cursor_monitor() -> RECT {
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let m = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(m, &mut info);
        info.rcMonitor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(w: u32, h: u32) -> Image {
        let mut pixels = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let v = if (x + y) % 2 == 0 { 0 } else { 200 };
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
        Image { w, h, pixels }
    }

    #[test]
    fn crop_and_clamp() {
        let img = checker(10, 6);
        let c = img.crop(2, 1, 4, 3);
        assert_eq!((c.w, c.h), (4, 3));
        assert_eq!(c.pixel(0, 0), img.pixel(2, 1));
        let edge = img.crop(8, 4, 10, 10);
        assert_eq!((edge.w, edge.h), (2, 2));
        assert_eq!(img.crop(20, 20, 5, 5).w, 0);
    }

    #[test]
    fn pixelate_averages_blocks() {
        let img = checker(4, 4);
        let p = img.pixelate(2);
        assert_eq!((p.w, p.h), (2, 2));
        assert_eq!(p.pixel(0, 0), [100, 100, 100, 255]);
        // Partial edge blocks still average what is there.
        assert_eq!(checker(5, 5).pixelate(2).w, 3);
    }

    #[test]
    fn dib_is_bottom_up() {
        let img = Image { w: 1, h: 2, pixels: vec![1, 1, 1, 255, 2, 2, 2, 255] };
        let dib = img.to_dib();
        assert_eq!(dib.len(), 40 + 8);
        assert_eq!(&dib[40..44], &[2, 2, 2, 255]); // last row first
        assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), 2);
    }

    #[test]
    fn luma_weights() {
        let img = Image { w: 2, h: 1, pixels: vec![0, 0, 255, 255, 255, 255, 255, 255] };
        assert_eq!(img.luma(), vec![76, 255]);
    }
}
