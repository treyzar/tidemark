//! Decode arbitrary image bytes to the 160x160 RGBA cover, bounded against decoder bombs.

use crate::error::{Error, Result};
use crate::source::{id_from_bytes, Cover, COVER_PX};
use image::imageops::FilterType;
use image::{ImageReader, Limits};
use std::io::Cursor;

const MAX_DIMENSION: u32 = 4096;
const MAX_ALLOC: u64 = 96 * 1024 * 1024;

pub fn decode_to_cover(bytes: Vec<u8>) -> Result<Cover> {
    let mut reader = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(Error::Io)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let mut rgba = reader.decode()?.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return Err(Error::Dimensions(w, h));
    }
    if w > MAX_DIMENSION || h > MAX_DIMENSION {
        return Err(Error::Dimensions(w, h));
    }
    let side = w.min(h);
    let square =
        image::imageops::crop(&mut rgba, (w - side) / 2, (h - side) / 2, side, side).to_image();
    let scaled = image::imageops::resize(&square, COVER_PX, COVER_PX, FilterType::CatmullRom);
    Ok(Cover {
        id: id_from_bytes(&bytes),
        pixels: scaled.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, ImageFormat, Rgba};

    fn encode_png(img: &ImageBuffer<Rgba<u8>, Vec<u8>>) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img.clone())
            .write_to(&mut buf, ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    fn encode_jpeg(img: &ImageBuffer<Rgba<u8>, Vec<u8>>) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img.clone())
            .write_to(&mut buf, ImageFormat::Jpeg)
            .unwrap();
        buf.into_inner()
    }

    fn solid(w: u32, h: u32, c: [u8; 3]) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
        ImageBuffer::from_fn(w, h, |_, _| Rgba([c[0], c[1], c[2], 255]))
    }

    fn close(actual: u8, want: u8) -> bool {
        (i16::from(actual) - i16::from(want)).abs() <= 24
    }

    #[test]
    fn png_becomes_a_160x160_rgba_cover() {
        let png = encode_png(&solid(200, 100, [10, 20, 30]));
        let cover = decode_to_cover(png.clone()).unwrap();
        assert_eq!(cover.pixels.len(), (COVER_PX * COVER_PX * 4) as usize);
        for px in cover.pixels.as_chunks::<4>().0 {
            assert!(close(px[0], 10) && close(px[1], 20) && close(px[2], 30) && px[3] == 255);
        }
        let other = decode_to_cover(png).unwrap();
        assert_eq!(cover.id, other.id, "same bytes must give the same id");
    }

    #[test]
    fn jpeg_becomes_a_160x160_rgba_cover() {
        let jpeg = encode_jpeg(&solid(300, 120, [200, 100, 0]));
        let cover = decode_to_cover(jpeg).unwrap();
        assert_eq!(cover.pixels.len(), (COVER_PX * COVER_PX * 4) as usize);
        assert!(close(cover.pixels[0], 200));
        assert!(close(cover.pixels[1], 100));
    }

    #[test]
    fn id_is_the_sha256_prefix_of_the_source_bytes() {
        use sha2::Digest;
        let bytes = b"anything at all";
        let want = {
            let h = sha2::Sha256::digest(bytes);
            u64::from_be_bytes(h[..8].try_into().unwrap())
        };
        assert_eq!(id_from_bytes(bytes), want);
        assert_ne!(id_from_bytes(b"other"), want);
    }

    #[test]
    fn non_square_covers_take_the_centered_square() {
        // rows: red, blue, blue -> centered 3x2 crop keeps red on top, blue below.
        let img = ImageBuffer::from_fn(3, 3, |_, y| {
            if y == 0 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            }
        });
        let png = encode_png(&img);
        let cover = decode_to_cover(png).unwrap();
        let at = |x: u32, y: u32| {
            let i = ((y * COVER_PX + x) * 4) as usize;
            (cover.pixels[i], cover.pixels[i + 1], cover.pixels[i + 2])
        };
        let (r, g, b) = at(80, 2);
        assert!(
            r > 200 && g < 60 && b < 60,
            "top must stay red: {r} {g} {b}"
        );
        let (r, g, b) = at(80, 157);
        assert!(
            b > 200 && r < 60 && g < 60,
            "bottom must stay blue: {r} {g} {b}"
        );
    }

    #[test]
    fn non_image_bytes_are_rejected() {
        assert!(decode_to_cover(b"definitely not an image".to_vec()).is_err());
        assert!(decode_to_cover(Vec::new()).is_err());
    }

    #[test]
    fn bomb_dimensions_fail_on_decoder_limits() {
        let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let mut chunk = 13u32.to_be_bytes().to_vec();
        chunk.extend_from_slice(b"IHDR");
        chunk.extend_from_slice(&100_000u32.to_be_bytes());
        chunk.extend_from_slice(&100_000u32.to_be_bytes());
        chunk.extend_from_slice(&[8, 2, 0, 0, 0]);
        let crc = crc32(&chunk[4..]);
        chunk.extend_from_slice(&crc.to_be_bytes());
        png.extend_from_slice(&chunk);
        let res = decode_to_cover(png);
        assert!(res.is_err(), "100000x100000 must fail on decoder limits");
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
}
