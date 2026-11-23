//! GDI helpers: text coverage for the renderer (`Glyphs`), and putting a `Canvas` on a window.
//!
//! Text is drawn white on black with natural-width ClearType and the three colour channels are
//! averaged into one gray coverage: it is blended onto arbitrary pixels, where colour fringes would
//! show, but it keeps ClearType's even spacing and soft stems (plain GDI grayscale fits each letter
//! to whole pixels and looks uneven at UI sizes).

use std::cell::RefCell;
use std::collections::HashMap;

use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::core::PCWSTR;

use crate::raster::{Canvas, Glyphs, TextMask};

pub struct GdiGlyphs {
    dc: HDC,
    /// A 32-bit bitmap stays selected: on the DC's default one-bit bitmap Windows measures text with
    /// other (narrower) widths than it then draws, which cut the end of small text.
    base: HBITMAP,
    base_prev: HGDIOBJ,
    fonts: RefCell<HashMap<(u32, bool), HFONT>>,
    face: Vec<u16>,
    gamma: [u8; 256],
}

impl Default for GdiGlyphs {
    fn default() -> Self {
        Self::new()
    }
}

impl GdiGlyphs {
    pub fn new() -> GdiGlyphs {
        let dc = unsafe { CreateCompatibleDC(None) };
        let mut gamma = [0u8; 256];
        for (i, v) in gamma.iter_mut().enumerate() {
            *v = ((i as f32 / 255.0).powf(crate::theme::TEXT_GAMMA) * 255.0).round() as u8;
        }
        let b = bmi(1, 1);
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let base = unsafe { CreateDIBSection(Some(dc), &b, DIB_RGB_COLORS, &mut bits, None, 0) }.unwrap_or_default();
        let base_prev = unsafe { SelectObject(dc, base.into()) };
        GdiGlyphs { dc, base, base_prev, fonts: RefCell::new(HashMap::new()), face: crate::theme::FONT_FAMILY.encode_utf16().chain(Some(0)).collect(), gamma }
    }

    pub(crate) fn font(&self, px: f32, bold: bool) -> HFONT {
        let key = ((px * 4.0).round() as u32, bold);
        *self.fonts.borrow_mut().entry(key).or_insert_with(|| unsafe {
            CreateFontW(
                -(px.round() as i32).max(1),
                0,
                0,
                0,
                if bold { crate::theme::FONT_WEIGHT_SEMIBOLD } else { crate::theme::FONT_WEIGHT_NORMAL },
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_TT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                FONT_QUALITY(6),
                (DEFAULT_PITCH.0 | FF_SWISS.0) as u32,
                PCWSTR(self.face.as_ptr()),
            )
        })
    }

    /// Metrics only: native input resizing must not rasterize a bitmap per keystroke.
    pub(crate) fn measure(&self, s: &str, px: f32) -> (i32, i32) {
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let old = SelectObject(self.dc, self.font(px, false).into());
            let mut tm = TEXTMETRICW::default();
            let _ = GetTextMetricsW(self.dc, &mut tm);
            let mut size = SIZE::default();
            if !wide.is_empty() {
                let _ = GetTextExtentPoint32W(self.dc, &wide, &mut size);
            }
            SelectObject(self.dc, old);
            (size.cx.max(0), tm.tmHeight.max(1))
        }
    }
}

pub(crate) fn text_overhang(px: f32) -> i32 {
    (px / crate::theme::TEXT_GLYPH_OVERHANG_DIVISOR).ceil() as i32
}

impl Drop for GdiGlyphs {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.base_prev);
            let _ = DeleteObject(self.base.into());
            for (_, f) in self.fonts.borrow_mut().drain() {
                let _ = DeleteObject(f.into());
            }
            let _ = DeleteDC(self.dc);
        }
    }
}

impl Glyphs for GdiGlyphs {
    fn line_height(&self, px: f32) -> f32 {
        self.measure("", px).1 as f32
    }

    fn text(&self, s: &str, px: f32, bold: bool) -> TextMask {
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let old = SelectObject(self.dc, self.font(px, bold).into());
            let mut tm = TEXTMETRICW::default();
            let _ = GetTextMetricsW(self.dc, &mut tm);
            let mut sz = SIZE::default();
            let _ = GetTextExtentPoint32W(self.dc, &wide, &mut sz);
            // A little room for overhangs (italic-like glyph tails, antialiasing).
            let pad = text_overhang(px);
            let (w, h) = ((sz.cx + 2 * pad).max(1), tm.tmHeight.max(1));
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let Ok(dib) = CreateDIBSection(Some(self.dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            else {
                SelectObject(self.dc, old);
                return TextMask { w: 1, h: 1, ascent: 1, cov: vec![0] };
            };
            let prev = SelectObject(self.dc, dib.into());
            std::ptr::write_bytes(bits as *mut u8, 0, (w * h * 4) as usize);
            SetBkMode(self.dc, TRANSPARENT);
            SetTextColor(self.dc, COLORREF(0x00FF_FFFF));
            let _ = TextOutW(self.dc, pad, 0, &wide);
            let _ = GdiFlush();
            let px_slice = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
            let cov: Vec<u8> = px_slice.as_chunks::<4>().0.iter().map(|p| self.gamma[(p[0] as usize + p[1] as usize + p[2] as usize) / 3]).collect();
            SelectObject(self.dc, prev);
            let _ = DeleteObject(dib.into());
            SelectObject(self.dc, old);
            TextMask { w: w as u32, h: h as u32, ascent: tm.tmAscent.max(0) as u32, cov }
        }
    }
}

fn bmi(w: i32, h: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
        ..Default::default()
    }
}

/// Paint an opaque canvas into a window's client area at (0, 0) (call between BeginPaint/EndPaint).
pub fn blit(dc: HDC, cv: &Canvas) {
    let b = bmi(cv.w as i32, cv.h as i32);
    unsafe {
        SetDIBitsToDevice(dc, 0, 0, cv.w, cv.h, 0, 0, 0, cv.h, cv.px.as_ptr() as *const _, &b, DIB_RGB_COLORS);
    }
}

/// Show `cv` (straight alpha) as the whole content of a layered window at (`x`, `y`), with an
/// extra constant opacity `alpha` (255 = as is).
pub fn update_layered(hwnd: HWND, cv: &Canvas, x: i32, y: i32, alpha: u8) -> bool {
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let b = bmi(cv.w as i32, cv.h as i32);
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let ok = match CreateDIBSection(Some(mem), &b, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(dib) => {
                let pm = cv.premultiplied();
                std::ptr::copy_nonoverlapping(pm.as_ptr(), bits as *mut u8, pm.len());
                let old = SelectObject(mem, dib.into());
                let pos = POINT { x, y };
                let size = SIZE { cx: cv.w as i32, cy: cv.h as i32 };
                let src = POINT { x: 0, y: 0 };
                let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
                let r = windows::Win32::UI::WindowsAndMessaging::UpdateLayeredWindow(
                    hwnd,
                    Some(screen),
                    Some(&pos),
                    Some(&size),
                    Some(mem),
                    Some(&src),
                    COLORREF(0),
                    Some(&blend),
                    windows::Win32::UI::WindowsAndMessaging::ULW_ALPHA,
                );
                SelectObject(mem, old);
                let _ = DeleteObject(dib.into());
                r.is_ok()
            }
            Err(_) => false,
        };
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        ok
    }
}
