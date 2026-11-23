//! History thumbnails: a tiny raw format, so the history window shows 50 of them without
//! decoding a PNG each (and without loading an image codec).
//!
//! Layout: `SNT1`, width (u32 LE), height (u32 LE), then width x height x 3 bytes BGR, top-down.

use crate::raster::{Canvas, Rgba};

pub const MAGIC: &[u8; 4] = b"SNT1";
/// Thumbnails fit in this box (pixels): sharp at 125 % in a 160 x 100 DIP cell.
pub const MAX_W: u32 = crate::theme::HISTORY_THUMB_PIXELS.0;
pub const MAX_H: u32 = crate::theme::HISTORY_THUMB_PIXELS.1;

pub fn make(img: &Canvas) -> Canvas {
    img.downscale_to_fit(MAX_W, MAX_H)
}

pub fn encode(t: &Canvas) -> Vec<u8> {
    let mut v = Vec::with_capacity(12 + (t.w * t.h * 3) as usize);
    v.extend_from_slice(MAGIC);
    v.extend_from_slice(&t.w.to_le_bytes());
    v.extend_from_slice(&t.h.to_le_bytes());
    for p in t.px.as_chunks::<4>().0 {
        v.extend_from_slice(&p[..3]);
    }
    v
}

pub fn decode(b: &[u8]) -> Option<Canvas> {
    if b.len() < 12 || &b[..4] != MAGIC {
        return None;
    }
    let w = u32::from_le_bytes(b[4..8].try_into().ok()?);
    let h = u32::from_le_bytes(b[8..12].try_into().ok()?);
    if w == 0 || h == 0 || w > 4096 || h > 4096 || b.len() != 12 + (w * h * 3) as usize {
        return None;
    }
    let mut c = Canvas::new(w, h, Rgba::default());
    for (i, p) in b[12..].as_chunks::<3>().0.iter().enumerate() {
        c.px[i * 4..i * 4 + 4].copy_from_slice(&[p[0], p[1], p[2], 255]);
    }
    Some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_size_limit() {
        let mut img = Canvas::new(1920, 1080, Rgba::rgb(10, 20, 30));
        img.set(0, 0, Rgba::rgb(200, 100, 50));
        let t = make(&img);
        assert_eq!((t.w, t.h), (200, 113));
        let b = encode(&t);
        assert_eq!(b.len(), 12 + 200 * 113 * 3);
        assert_eq!(decode(&b), Some(t));
    }

    #[test]
    fn broken_files_are_refused() {
        let good = encode(&Canvas::new(2, 2, Rgba::rgb(1, 2, 3)));
        let mut wrong_magic = good.clone();
        wrong_magic[0] = b'X';
        let cases: Vec<Vec<u8>> = vec![vec![], good[..11].to_vec(), good[..good.len() - 1].to_vec(), wrong_magic];
        assert!(!cases.is_empty());
        for c in cases {
            assert_eq!(decode(&c), None);
        }
        assert!(decode(&good).is_some());
    }
}
