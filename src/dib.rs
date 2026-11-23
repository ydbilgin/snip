//! Clipboard image bytes: CF_DIBV5 (32-bit BGRA, BI_RGB) and CF_DIB (24-bit, the most widely read
//! form). Both are bottom-up device-independent bitmaps. The input is top-down BGRA with a row
//! stride of `width * 4`; alpha is written as 255 (a screenshot is opaque).
//!
//! CF_DIBV5 is BI_RGB with every mask 0, not BI_BITFIELDS: the `image` crate's BMP decoder
//! (0.25.9 and 0.25.10, used by arboard 3.6.1 and so by Codex CLI) reads 12 more bytes of masks
//! after a BITFIELDS V5 header and fails with "failed to fill whole buffer"; arboard then says the
//! image "could not be converted to the appropriate format" (seen 2026-11-13). arboard also turns
//! BI_RGB with an alpha mask of 0xFF000000 back into BITFIELDS, so the alpha mask stays 0 too.

pub const BITMAPV5HEADER_SIZE: usize = 124;
pub const BITMAPINFOHEADER_SIZE: usize = 40;
const BI_RGB: u32 = 0;
const LCS_SRGB: u32 = 0x7352_4742; // 'sRGB'
const LCS_GM_IMAGES: u32 = 4;
const PELS_PER_METER_96DPI: i32 = 3780;

fn check(width: u32, height: u32, bgra: &[u8]) {
    assert!(width > 0 && height > 0, "empty image");
    assert_eq!(bgra.len(), width as usize * height as usize * 4, "BGRA buffer size");
}

/// Stride of a 24-bit DIB row (padded to 4 bytes).
pub fn stride24(width: u32) -> usize {
    (width as usize * 3 + 3) & !3
}

pub fn dibv5_bytes(width: u32, height: u32, bgra: &[u8]) -> Vec<u8> {
    check(width, height, bgra);
    let row = width as usize * 4;
    let image = row * height as usize;
    let mut v = Vec::with_capacity(BITMAPV5HEADER_SIZE + image);
    let put32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    put32(&mut v, BITMAPV5HEADER_SIZE as u32);
    v.extend_from_slice(&(width as i32).to_le_bytes());
    v.extend_from_slice(&(height as i32).to_le_bytes()); // positive: bottom-up
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&32u16.to_le_bytes());
    put32(&mut v, BI_RGB);
    put32(&mut v, image as u32);
    v.extend_from_slice(&PELS_PER_METER_96DPI.to_le_bytes());
    v.extend_from_slice(&PELS_PER_METER_96DPI.to_le_bytes());
    put32(&mut v, 0); // ClrUsed
    put32(&mut v, 0); // ClrImportant
    // Masks: none with BI_RGB (B, G, R, unused byte; see the module note).
    put32(&mut v, 0); // red mask
    put32(&mut v, 0); // green mask
    put32(&mut v, 0); // blue mask
    put32(&mut v, 0); // alpha mask
    put32(&mut v, LCS_SRGB);
    v.extend_from_slice(&[0u8; 36]); // CIEXYZTRIPLE endpoints
    put32(&mut v, 0); // gamma red
    put32(&mut v, 0); // gamma green
    put32(&mut v, 0); // gamma blue
    put32(&mut v, LCS_GM_IMAGES);
    put32(&mut v, 0); // ProfileData
    put32(&mut v, 0); // ProfileSize
    put32(&mut v, 0); // Reserved
    debug_assert_eq!(v.len(), BITMAPV5HEADER_SIZE);
    for y in (0..height as usize).rev() {
        let src = &bgra[y * row..(y + 1) * row];
        for px in src.as_chunks::<4>().0 {
            v.extend_from_slice(&[px[0], px[1], px[2], 255]);
        }
    }
    v
}

pub fn dib24_bytes(width: u32, height: u32, bgra: &[u8]) -> Vec<u8> {
    check(width, height, bgra);
    let row = width as usize * 4;
    let stride = stride24(width);
    let image = stride * height as usize;
    let mut v = Vec::with_capacity(BITMAPINFOHEADER_SIZE + image);
    v.extend_from_slice(&(BITMAPINFOHEADER_SIZE as u32).to_le_bytes());
    v.extend_from_slice(&(width as i32).to_le_bytes());
    v.extend_from_slice(&(height as i32).to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&24u16.to_le_bytes());
    v.extend_from_slice(&BI_RGB.to_le_bytes());
    v.extend_from_slice(&(image as u32).to_le_bytes());
    v.extend_from_slice(&PELS_PER_METER_96DPI.to_le_bytes());
    v.extend_from_slice(&PELS_PER_METER_96DPI.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    debug_assert_eq!(v.len(), BITMAPINFOHEADER_SIZE);
    let pad = stride - width as usize * 3;
    for y in (0..height as usize).rev() {
        let src = &bgra[y * row..(y + 1) * row];
        for px in src.as_chunks::<4>().0 {
            v.extend_from_slice(&px[..3]);
        }
        v.extend(std::iter::repeat_n(0u8, pad));
    }
    v
}

/// CF_HDROP bytes for one file: a DROPFILES header (files at offset 20, wide names) then the path
/// in UTF-16 and two NULs.
pub fn hdrop_bytes(path: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(20 + (path.len() + 2) * 2);
    for x in [20u32, 0, 0, 0, 1] {
        v.extend_from_slice(&x.to_le_bytes()); // pFiles, pt.x, pt.y, fNC, fWide
    }
    v.extend(path.encode_utf16().chain([0, 0]).flat_map(u16::to_le_bytes));
    v
}

/// Top-down BGRA to top-down BGR (for the 24-bit PNG).
pub fn bgra_to_bgr(bgra: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(bgra.len() / 4 * 3);
    for px in bgra.as_chunks::<4>().0 {
        v.extend_from_slice(&px[..3]);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u32_at(v: &[u8], o: usize) -> u32 {
        u32::from_le_bytes(v[o..o + 4].try_into().unwrap())
    }
    fn i32_at(v: &[u8], o: usize) -> i32 {
        i32::from_le_bytes(v[o..o + 4].try_into().unwrap())
    }
    fn u16_at(v: &[u8], o: usize) -> u16 {
        u16::from_le_bytes(v[o..o + 2].try_into().unwrap())
    }

    /// 3x2 image, top row: red green blue; bottom row: white black gray. Alpha 0 on purpose.
    fn sample() -> (u32, u32, Vec<u8>) {
        let px = [
            [0, 0, 255, 0], [0, 255, 0, 0], [255, 0, 0, 0],
            [255, 255, 255, 0], [0, 0, 0, 0], [128, 128, 128, 0],
        ];
        (3, 2, px.iter().flatten().copied().collect())
    }

    #[test]
    fn dibv5_header_fields() {
        let (w, h, b) = sample();
        let v = dibv5_bytes(w, h, &b);
        assert_eq!(u32_at(&v, 0), 124);
        assert_eq!(i32_at(&v, 4), 3);
        assert_eq!(i32_at(&v, 8), 2, "positive height = bottom-up");
        assert_eq!(u16_at(&v, 12), 1);
        assert_eq!(u16_at(&v, 14), 32);
        assert_eq!(u32_at(&v, 16), BI_RGB, "BITFIELDS breaks arboard/image (module note)");
        assert_eq!(u32_at(&v, 20), 3 * 2 * 4);
        for o in [40, 44, 48, 52] {
            assert_eq!(u32_at(&v, o), 0, "mask at {o}: an alpha mask makes arboard switch to BITFIELDS");
        }
        assert_eq!(u32_at(&v, 56), LCS_SRGB);
        assert_eq!(u32_at(&v, 108), LCS_GM_IMAGES);
        assert_eq!(v.len(), 124 + 24);
    }

    #[test]
    fn dibv5_pixels_are_bottom_up_and_opaque() {
        let (w, h, b) = sample();
        let v = dibv5_bytes(w, h, &b);
        let px = &v[124..];
        // First stored row is the bottom row: white, black, gray.
        assert_eq!(&px[0..4], &[255, 255, 255, 255]);
        assert_eq!(&px[4..8], &[0, 0, 0, 255]);
        assert_eq!(&px[8..12], &[128, 128, 128, 255]);
        // Second stored row is the top row: red (B=0,G=0,R=255), green, blue.
        assert_eq!(&px[12..16], &[0, 0, 255, 255]);
        assert_eq!(&px[20..24], &[255, 0, 0, 255]);
    }

    #[test]
    fn dib24_header_padding_and_pixels() {
        let (w, h, b) = sample();
        let v = dib24_bytes(w, h, &b);
        assert_eq!(u32_at(&v, 0), 40);
        assert_eq!(u16_at(&v, 14), 24);
        assert_eq!(u32_at(&v, 16), BI_RGB);
        assert_eq!(stride24(3), 12); // 9 bytes padded to 12
        assert_eq!(u32_at(&v, 20), 24);
        assert_eq!(v.len(), 40 + 24);
        let px = &v[40..];
        assert_eq!(&px[0..9], &[255, 255, 255, 0, 0, 0, 128, 128, 128]);
        assert_eq!(&px[9..12], &[0, 0, 0], "row padding");
        assert_eq!(&px[12..21], &[0, 0, 255, 0, 255, 0, 255, 0, 0]);
    }

    /// arboard 3.6.1's `maybe_tweak_header` then `BmpDecoder::new_without_file_header`, as Codex CLI
    /// reads an image off the clipboard. With BITFIELDS this failed (2026-11-13).
    fn arboard_read(mut v: Vec<u8>) -> Result<(u32, u32, Vec<u8>), String> {
        use image::ImageDecoder;
        let (bits, comp, alpha) = (u16_at(&v, 14), u32_at(&v, 16), u32_at(&v, 52));
        if bits == 32 && comp == 0 && alpha == 0xFF00_0000 {
            v[16..20].copy_from_slice(&3u32.to_le_bytes());
        }
        let d = image::codecs::bmp::BmpDecoder::new_without_file_header(std::io::Cursor::new(&v)).map_err(|e| e.to_string())?;
        let (w, h) = d.dimensions();
        let img = image::DynamicImage::from_decoder(d).map_err(|e| e.to_string())?.into_rgba8();
        Ok((w, h, img.into_raw()))
    }

    #[test]
    fn codex_cli_can_read_the_dibv5() {
        let (w, h, b) = sample();
        let (rw, rh, rgba) = arboard_read(dibv5_bytes(w, h, &b)).expect("arboard reads it");
        assert_eq!((rw, rh), (3, 2));
        // Top row first, RGBA: red, green, blue; then white, black, gray. Opaque.
        assert_eq!(&rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&rgba[8..12], &[0, 0, 255, 255]);
        assert_eq!(&rgba[16..20], &[0, 0, 0, 255]);
        assert_eq!(&rgba[20..24], &[128, 128, 128, 255]);
        let big = vec![9u8; 149 * 44 * 4];
        assert!(arboard_read(dibv5_bytes(149, 44, &big)).is_ok(), "the nick's size, odd width");
    }

    #[test]
    fn hdrop_holds_one_wide_path_and_two_nuls() {
        let v = hdrop_bytes("C:\\a\\ö.jpg");
        assert_eq!((u32_at(&v, 0), u32_at(&v, 16)), (20, 1));
        let units: Vec<u16> = v[20..].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        assert_eq!(String::from_utf16(&units[..units.len() - 2]).unwrap(), "C:\\a\\ö.jpg");
        assert_eq!(&units[units.len() - 2..], &[0, 0]);
    }

    #[test]
    fn stride24_cases() {
        let cases = [(1, 4), (2, 8), (3, 12), (4, 12), (5, 16), (2560, 7680), (1921, 5764)];
        assert!(!cases.is_empty());
        for (w, s) in cases {
            assert_eq!(stride24(w), s, "width {w}");
        }
    }

    #[test]
    fn bgr_conversion() {
        assert_eq!(bgra_to_bgr(&[1, 2, 3, 4, 5, 6, 7, 8]), vec![1, 2, 3, 5, 6, 7]);
    }

    #[test]
    #[should_panic(expected = "empty image")]
    fn zero_size_image_is_refused() {
        dibv5_bytes(0, 0, &[]);
    }

    #[test]
    fn large_selection_sizes_add_up() {
        let (w, h) = (2560u32, 1440u32);
        let b = vec![7u8; (w * h * 4) as usize];
        assert_eq!(dibv5_bytes(w, h, &b).len(), 124 + 2560 * 1440 * 4);
        assert_eq!(dib24_bytes(w, h, &b).len(), 40 + 7680 * 1440);
    }
}
