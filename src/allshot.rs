//! All screens as one picture: no overlay, no selection. The whole virtual desktop (the bounding
//! box of every monitor, negative coordinates included) is cut out of the frozen frames; the empty
//! corners of the box stay opaque black. It goes to the clipboard and into a PNG like any other
//! snip. The clipboard is behind `ImageSink`, so a test counts the writes and never opens the user's real
//! clipboard (an auditor overwrote it 12 times on 2026-11-07).

use windows::Win32::Foundation::HWND;

use crate::capture::Snapshot;
use crate::geometry::Rect;

/// The picture of every screen.
pub struct AllShot {
    pub w: u32,
    pub h: u32,
    /// Top-down BGRA, alpha 255.
    pub px: Vec<u8>,
    /// Where the desktop box lies, in desktop coordinates.
    pub bounds: Rect,
    pub screens: usize,
}

/// Cut the whole desktop out of the frozen frames.
pub fn take(snap: &Snapshot) -> Result<AllShot, String> {
    let bounds = snap.layout.bounds();
    if bounds.is_empty() {
        return Err("no monitor".into());
    }
    let (w, h, px) = snap.crop(&bounds).map_err(|e| format!("{:#010x} {}", e.code().0, e.message()))?;
    if (w as i32, h as i32) != (bounds.width(), bounds.height()) {
        return Err(format!("cut {w}x{h} but the desktop is {}x{}", bounds.width(), bounds.height()));
    }
    Ok(AllShot { w, h, px, bounds, screens: snap.monitors.len() })
}

/// Where a finished picture goes besides the file.
pub trait ImageSink {
    fn put(&mut self, w: u32, h: u32, bgra: &[u8]) -> Result<(), String>;
}

/// The real clipboard (CF_DIBV5 and CF_DIB).
pub struct RealClipboard(pub HWND);

impl ImageSink for RealClipboard {
    fn put(&mut self, w: u32, h: u32, bgra: &[u8]) -> Result<(), String> {
        crate::clipboard::set_image(self.0, w, h, bgra).map_err(|e| format!("{:#010x} {}", e.code().0, e.message()))
    }
}

/// The test clipboard: it does the work of the real one (both DIB forms, and the copy into the
/// global memory block each format needs) but opens nothing and keeps nothing.
#[derive(Default)]
pub struct CountingClipboard {
    pub puts: u32,
    pub bytes: u64,
    pub last_size: (u32, u32),
}

impl ImageSink for CountingClipboard {
    fn put(&mut self, w: u32, h: u32, bgra: &[u8]) -> Result<(), String> {
        let v5 = crate::dib::dibv5_bytes(w, h, bgra);
        let dib = crate::dib::dib24_bytes(w, h, bgra);
        // GlobalAlloc + memcpy of each format in the real path.
        let copies = (v5.clone(), dib.clone());
        self.puts += 1;
        self.bytes += (copies.0.len() + copies.1.len()) as u64;
        self.last_size = (w, h);
        Ok(())
    }
}

/// The text of the short confirmation card: "7040 × 2880 px, 4 screens".
pub fn describe(shot: &AllShot, lang: crate::i18n::Lang) -> String {
    format!("{} \u{00D7} {} px, {} {}", shot.w, shot.h, shot.screens, crate::i18n::S::ScreensWord.get(lang))
}
