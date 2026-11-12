use crate::{Error, Result};
use image::{ImageFormat, ImageReader, RgbaImage};
use std::{
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
};
pub const MAX_BYTES: usize = 32 << 20;
pub const MAX_PIXELS: u64 = 16_777_216;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
#[derive(Clone)]
pub struct Raster {
    pixels: RgbaImage,
}
fn dimensions(width: u32, height: u32) -> Result<()> {
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        Err(Error::Limit)
    } else {
        Ok(())
    }
}
impl Raster {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_BYTES {
            return Err(Error::Limit);
        }
        let format = image::guess_format(bytes).map_err(|_| Error::Image)?;
        if !matches!(
            format,
            ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
        ) {
            return Err(Error::Image);
        }
        let (w, h) = ImageReader::with_format(Cursor::new(bytes), format)
            .into_dimensions()
            .map_err(|_| Error::Image)?;
        dimensions(w, h)?;
        let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        limits.max_alloc = Some(128 << 20);
        reader.limits(limits);
        let pixels = reader.decode().map_err(|_| Error::Image)?.into_rgba8();
        Ok(Self { pixels })
    }
    pub fn load(path: &Path) -> Result<Self> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
        }
        let mut file = options.open(path).map_err(|_| Error::Image)?;
        let metadata = file.metadata().map_err(|_| Error::Image)?;
        if !metadata.is_file() || metadata.len() > MAX_BYTES as u64 {
            return Err(Error::Limit);
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Image)?;
        Self::decode(&bytes)
    }
    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self> {
        dimensions(width, height)?;
        if pixels.len() != width as usize * height as usize * 4 {
            return Err(Error::Image);
        }
        let pixels = RgbaImage::from_raw(width, height, pixels).ok_or(Error::Image)?;
        Ok(Self { pixels })
    }
    pub fn width(&self) -> u32 {
        self.pixels.width()
    }
    pub fn height(&self) -> u32 {
        self.pixels.height()
    }
    pub fn pixels(&self) -> &[u8] {
        self.pixels.as_raw()
    }
    fn bounds(&self, r: Rect) -> Result<()> {
        if r.width == 0
            || r.height == 0
            || r.x
                .checked_add(r.width)
                .is_none_or(|right| right > self.width())
            || r.y
                .checked_add(r.height)
                .is_none_or(|bottom| bottom > self.height())
        {
            Err(Error::Bounds)
        } else {
            Ok(())
        }
    }
    pub fn crop(&self, rect: Rect) -> Result<Self> {
        self.bounds(rect)?;
        Ok(Self {
            pixels: image::imageops::crop_imm(
                &self.pixels,
                rect.x,
                rect.y,
                rect.width,
                rect.height,
            )
            .to_image(),
        })
    }
    /// Mean-color blocks; this is visual pixelation, not guaranteed information erasure.
    pub fn pixelate(&mut self, rect: Rect, block: u32) -> Result<()> {
        self.bounds(rect)?;
        if !(2..=128).contains(&block) {
            return Err(Error::Bounds);
        }
        for top in (rect.y..rect.y + rect.height).step_by(block as usize) {
            for left in (rect.x..rect.x + rect.width).step_by(block as usize) {
                let bottom = (top + block).min(rect.y + rect.height);
                let right = (left + block).min(rect.x + rect.width);
                let mut sums = [0u64; 4];
                let count = u64::from(bottom - top) * u64::from(right - left);
                for y in top..bottom {
                    for x in left..right {
                        for (sum, value) in sums.iter_mut().zip(self.pixels.get_pixel(x, y).0) {
                            *sum += u64::from(value);
                        }
                    }
                }
                let color = image::Rgba(sums.map(|s| (s / count) as u8));
                for y in top..bottom {
                    for x in left..right {
                        self.pixels.put_pixel(x, y, color);
                    }
                }
            }
        }
        Ok(())
    }
    pub fn qr(&self) -> Result<Vec<String>> {
        let gray = image::DynamicImage::ImageRgba8(self.pixels.clone()).into_luma8();
        let mut prepared = rqrr::PreparedImage::prepare(gray);
        let grids = prepared.detect_grids();
        if grids.len() > 64 {
            return Err(Error::Limit);
        }
        let found = !grids.is_empty();
        let mut out = Vec::new();
        for grid in grids {
            if let Ok((_, text)) = grid.decode() {
                if !out.contains(&text) {
                    out.push(text);
                }
            }
        }
        if found && out.is_empty() {
            Err(Error::Qr)
        } else {
            Ok(out)
        }
    }
    pub fn png(&self) -> Result<Vec<u8>> {
        let mut out = LimitedWriter(Vec::new(), false);
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(
                self.pixels(),
                self.width(),
                self.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|_| if out.1 { Error::Limit } else { Error::Image })?;
        Ok(out.0)
    }
}
use image::ImageEncoder;
struct LimitedWriter(Vec<u8>, bool);
impl Write for LimitedWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if b.len() > MAX_BYTES - self.0.len() {
            self.1 = true;
            return Err(std::io::Error::other("image size limit"));
        }
        self.0.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub fn local_image_uri(uri: &str) -> Result<PathBuf> {
    if uri.len() > 8192
        || uri.chars().any(|c| c.is_control())
        || uri.to_ascii_lowercase().contains("%00")
    {
        return Err(Error::Image);
    }
    let url = url::Url::parse(uri).map_err(|_| Error::Image)?;
    if url.scheme() != "file"
        || url.host_str().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Image);
    }
    let path = url.to_file_path().map_err(|_| Error::Image)?;
    if !path.is_absolute() {
        return Err(Error::Image);
    }
    Ok(path)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_remote_and_ambiguous_image_uris() {
        assert!(local_image_uri(if cfg!(windows) {
            "file:///C:/tmp/synthetic%20image.png"
        } else {
            "file:///tmp/synthetic%20image.png"
        })
        .unwrap()
        .is_absolute());
        for uri in [
            "https://example.test/image.png",
            "file://evil.test/tmp/image.png",
            "file:///tmp/image.png?query=x",
            "file:///tmp/image.png#fragment",
            "file:///tmp/image%00.png",
        ] {
            assert_eq!(local_image_uri(uri), Err(Error::Image));
        }
    }
    #[test]
    fn crop_checks_overflow_and_pixelation_only_changes_selection() {
        let mut image =
            Raster::from_rgba(4, 4, (0..16).flat_map(|i| [i * 10, 0, 0, 255]).collect()).unwrap();
        assert!(image
            .crop(Rect {
                x: u32::MAX,
                y: 0,
                width: 2,
                height: 1
            })
            .is_err());
        assert!(image
            .crop(Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 1
            })
            .is_err());
        let original = image.pixels().to_vec();
        image
            .pixelate(
                Rect {
                    x: 1,
                    y: 1,
                    width: 2,
                    height: 2,
                },
                2,
            )
            .unwrap();
        assert_eq!(&image.pixels()[..20], &original[..20]);
        assert_eq!(image.pixels()[20], 75);
        assert_eq!(image.pixels()[24], 75);
        let crop = image
            .crop(Rect {
                x: 1,
                y: 1,
                width: 2,
                height: 2,
            })
            .unwrap();
        assert_eq!((crop.width(), crop.height()), (2, 2));
    }
    #[test]
    fn synthetic_png_roundtrip_and_qr() {
        let qr = qrcode::QrCode::new(b"synthetic offline capture").unwrap();
        let size = qr.width();
        let margin = 4;
        let scale = 5;
        let width = (size + 2 * margin) * scale;
        let mut pixels = vec![255; width * width * 4];
        for y in 0..size {
            for x in 0..size {
                if qr[(x, y)] == qrcode::Color::Dark {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let at =
                                (((y + margin) * scale + dy) * width + (x + margin) * scale + dx)
                                    * 4;
                            pixels[at..at + 3].fill(0);
                        }
                    }
                }
            }
        }
        let image = Raster::from_rgba(width as u32, width as u32, pixels).unwrap();
        let decoded = Raster::decode(&image.png().unwrap()).unwrap();
        assert_eq!(decoded.qr().unwrap(), vec!["synthetic offline capture"]);
        assert!(Raster::decode(b"not an image").is_err());
        assert!(Raster::from_rgba(8192, 8192, Vec::new()).is_err());
    }
}

#[cfg(test)]
mod limits_tests {
    use super::*;
    #[test]
    fn decoder_rejects_large_dimensions_and_rgba_length_mismatch() {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                &vec![255; 8193 * 4],
                8193,
                1,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        assert!(matches!(Raster::decode(&bytes), Err(Error::Limit)));
        assert!(matches!(
            Raster::from_rgba(1, 1, vec![255; 8]),
            Err(Error::Image)
        ));
        assert!(Raster::decode(b"GIF89a\x01\0\x01\0").is_err());
    }
}
