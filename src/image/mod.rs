//! Inline images: decoded bitmaps, the per-pane store that owns them, and the tiles that cells
//! point at. Decoders are pure functions from bytes to a `Bitmap`.

pub mod anim;
pub mod base64;
pub mod iterm;
pub mod kitty;
pub mod media;
pub mod sixel;
pub mod store;
pub mod worker;

/// Largest decoded image, in pixels (128 MiB of RGBA); anything bigger is refused rather than
/// allocated. The check on `decode_image` happens on the header, before any pixel is decoded.
pub const MAX_PIXELS: usize = 32 * 1024 * 1024;
/// Longest side of a decoded image.
const MAX_SIDE: u32 = 16384;
/// Most memory a decoder may allocate for one image, transient buffers included.
const MAX_DECODE_ALLOC: u64 = 512 * 1024 * 1024;

/// Straight-alpha RGBA, 8 bits per channel, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

impl Bitmap {
    pub fn new(w: usize, h: usize) -> Bitmap {
        Bitmap { w, h, rgba: vec![0; w * h * 4] }
    }

    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.w + x) * 4;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }

    pub fn set(&mut self, x: usize, y: usize, px: [u8; 4]) {
        let i = (y * self.w + x) * 4;
        self.rgba[i..i + 4].copy_from_slice(&px);
    }

    /// Bytes of pixel data, for the store's memory budget.
    pub fn bytes(&self) -> usize {
        self.rgba.len()
    }
}

/// Decode a PNG, JPEG or GIF (first frame) into a bitmap.
pub fn decode_image(data: &[u8]) -> Result<Bitmap, String> {
    let reader = || image::ImageReader::new(std::io::Cursor::new(data)).with_guessed_format();
    let (w, h) = reader()
        .map_err(|e| format!("image decode: {e}"))?
        .into_dimensions()
        .map_err(|e| format!("image decode: {e}"))?;
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || (w as usize).saturating_mul(h as usize) > MAX_PIXELS {
        return Err(format!("image size {w}x{h} refused"));
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    let mut reader = reader().map_err(|e| format!("image decode: {e}"))?;
    reader.limits(limits);
    let img = reader.decode().map_err(|e| format!("image decode: {e}"))?;
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w == 0 || h == 0 || w.saturating_mul(h) > MAX_PIXELS {
        return Err(format!("image size {w}x{h} refused"));
    }
    Ok(Bitmap { w, h, rgba: img.into_rgba8().into_raw() })
}

/// Decode what a program sent as an inline image whatever its format is. Raster formats (PNG, JPEG,
/// GIF and the others the decoder recognizes by content) come first; a Sixel stream, which is what
/// `imgcat` sends when it is given a `.six` file, is read by the Sixel decoder. The formats
/// identify themselves, so the order matters only for the error that is reported.
pub fn decode_any(data: &[u8]) -> Result<Bitmap, String> {
    match decode_image(data) {
        Ok(bitmap) => Ok(bitmap),
        Err(raster) => match sixel_body(data) {
            Some(body) => sixel::decode(body).map_err(|e| format!("not a raster image ({raster}); as Sixel: {e}")),
            None => Err(raster),
        },
    }
}

/// The part of a Sixel stream after `DCS params q`, without its terminator; `None` when `data`
/// does not start like one.
fn sixel_body(data: &[u8]) -> Option<&[u8]> {
    let rest = data.strip_prefix(b"\x1bP").or_else(|| data.strip_prefix(&[0x90]))?;
    let q = rest.iter().take(40).position(|&b| b == b'q')?;
    if !rest[..q].iter().all(|b| b.is_ascii_digit() || *b == b';') {
        return None;
    }
    let body = &rest[q + 1..];
    Some(body.strip_suffix(b"\x1b\\").or_else(|| body.strip_suffix(&[0x9c])).unwrap_or(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sixel_file_sent_as_an_inline_image_is_read_as_sixel() {
        let six = b"\x1bP0;1;0q\"1;1;4;6#0;2;100;0;0#0!4~-\x1b\\";
        let b = decode_any(six).unwrap();
        assert_eq!((b.w, b.h), (4, 6));
        assert_eq!(b.pixel(0, 0), [255, 0, 0, 255]);
        let bare = decode_any(b"\x1bPq#0;2;0;100;0#0!3~\x1b\\").unwrap();
        assert_eq!(bare.pixel(1, 0), [0, 255, 0, 255]);
    }

    #[test]
    fn decode_any_still_reads_rasters_and_still_refuses_garbage_with_the_raster_error() {
        assert_eq!(decode_any(&png_2x1()).unwrap().w, 2);
        assert!(decode_any(b"not an image").unwrap_err().contains("image decode"));
        assert!(decode_any(b"\x1bPnot sixel\x1b\\").is_err());
        assert!(decode_any(b"\x1bP1;2xq...").is_err(), "parameters are digits and semicolons only");
        assert!(decode_any(&[]).is_err());
    }

    fn png_2x1() -> Vec<u8> {
        let mut out = Vec::new();
        let img = image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 128]).unwrap();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn png_decodes_to_rgba() {
        let b = decode_image(&png_2x1()).unwrap();
        assert_eq!((b.w, b.h), (2, 1));
        assert_eq!(b.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(b.pixel(1, 0), [0, 0, 255, 128]);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode_image(b"not an image").is_err());
        assert!(decode_image(&[]).is_err());
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    fn png_header(w: u32, h: u32) -> Vec<u8> {
        let mut ihdr = b"IHDR".to_vec();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        out.extend_from_slice(&ihdr);
        out.extend_from_slice(&crc32(&ihdr).to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(b"IDAT");
        out.extend_from_slice(&crc32(b"IDAT").to_be_bytes());
        out
    }

    #[test]
    fn a_header_that_claims_a_huge_image_is_refused_before_anything_is_allocated() {
        for (w, h) in [(60_000, 60_000), (16_385, 1), (1, 16_385), (8192, 8192)] {
            let err = decode_image(&png_header(w, h)).unwrap_err();
            assert!(err.contains("refused"), "{w}x{h}: {err}");
        }
        assert!(decode_image(&png_header(u32::MAX, u32::MAX)).is_err());
    }

    #[test]
    fn a_header_within_the_limits_but_without_data_is_a_decode_error() {
        let err = decode_image(&png_header(100, 100)).unwrap_err();
        assert!(err.contains("image decode"), "{err}");
    }
}
