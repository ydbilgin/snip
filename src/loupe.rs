//! The magnifier: while the editor picks an area, and while the colour picker is out, a small
//! panel next to the cursor shows the pixels around it enlarged, with the middle pixel framed and
//! its position and colour written below. It makes a pixel-exact edge and an exact colour easy.

use crate::geometry::{Point, Rect};
use crate::raster::{Canvas, Glyphs, IBox, Mask, Pt, Rgba};
use crate::theme::{self, Theme};

/// The desktop pixels the magnifier shows for a cursor at `p` (`LOUPE_PIXELS` square, centred).
pub fn sample_rect(p: Point) -> Rect {
    let n = theme::LOUPE_PIXELS as i32;
    Rect::from_xywh(p.x - n / 2, p.y - n / 2, n, n)
}

/// Copy the part of `sample_rect(p)` that lies on the desktop (`got`, cropped from the frozen
/// screen as `px`) into a full square; off-desktop pixels stay transparent.
pub fn square(p: Point, got: Rect, px: &Canvas) -> Canvas {
    let want = sample_rect(p);
    let n = theme::LOUPE_PIXELS;
    let mut out = Canvas::new(n, n, Rgba::default());
    out.draw(px, got.left - want.left, got.top - want.top);
    out
}

/// The text under the enlarged pixels: the position, then the colour as hex.
pub fn readout(p: Point, c: Rgba) -> (String, String) {
    (format!("{}, {}", p.x, p.y), if c.a == 0 { String::from("-") } else { c.with_alpha(255).to_hex() })
}

/// The panel for a cursor at `p`, from the square `sample` that `square` made.
pub fn render(sample: &Canvas, p: Point, t: &Theme, scale: f32, g: &dyn Glyphs) -> Canvas {
    let s = |d: f32| (d * scale).round() as i32;
    let n = sample.w as i32;
    let cell = s(theme::LOUPE_ZOOM).max(2);
    let pad = s(t.pad).max(1);
    let grid = n * cell;
    let read_h = s(theme::LOUPE_READOUT);
    let (w, h) = (grid + 2 * pad, grid + 2 * pad + read_h);
    let mut cv = Canvas::new(w as u32, h as u32, Rgba::default());
    // The panel: the theme's border, then its ground.
    let r = t.radius * scale;
    let mut m = Mask::new(cv.bounds());
    m.rounded_box(0.0, 0.0, w as f32, h as f32, r);
    cv.blend_mask(&m, t.border);
    let bw = (theme::BORDER_WIDTH * scale).round().max(1.0);
    let mut inner = Mask::new(cv.bounds());
    inner.rounded_box(bw, bw, w as f32 - bw, h as f32 - bw, (r - bw).max(0.0));
    cv.blend_mask(&inner, t.bg);
    // The pixels, each a `cell` square; off the desktop a quiet checker.
    for y in 0..n {
        for x in 0..n {
            let c = sample.get(x, y);
            let b = IBox::xywh(pad + x * cell, pad + y * cell, cell, cell);
            let c = if c.a == 0 { if (x + y) % 2 == 0 { t.surface } else { t.hover } } else { c.with_alpha(255) };
            cv.fill_box(b, c);
        }
    }
    // A faint grid so single pixels read as pixels.
    let line = Rgba::rgb(128, 128, 128).with_alpha(38);
    for i in 1..n {
        cv.fill_box(IBox::xywh(pad + i * cell, pad, 1, grid), line);
        cv.fill_box(IBox::xywh(pad, pad + i * cell, grid, 1), line);
    }
    // The middle pixel: a dark and a light ring, readable on any colour.
    let mid = n / 2;
    let c0 = Pt::new((pad + mid * cell) as f32, (pad + mid * cell) as f32);
    let th = (theme::BORDER_WIDTH * scale).max(1.0);
    let mut outer = Mask::new(cv.bounds());
    outer.box_outline(c0.x - th, c0.y - th, c0.x + cell as f32 + th, c0.y + cell as f32 + th, th);
    cv.blend_mask(&outer, Rgba::rgb(0, 0, 0));
    let mut ring = Mask::new(cv.bounds());
    ring.box_outline(c0.x, c0.y, c0.x + cell as f32, c0.y + cell as f32, th);
    cv.blend_mask(&ring, Rgba::rgb(255, 255, 255));
    // The readout: position on the left, a swatch and the hex on the right.
    let colour = sample.get(mid, mid);
    let (pos, hex) = readout(p, colour);
    let font = t.font_small * scale;
    let y_mid = pad + grid + read_h / 2;
    let a = g.text(&pos, font, false);
    let b = g.text(&hex, font, true);
    let (sw, gap) = (s(10.0), s(6.0));
    // What fits, in this order of need: the hex, the position, the swatch.
    let room = w - 2 * pad - 2 * s(2.0);
    let with_pos = a.w as i32 + gap + b.w as i32 <= room;
    let with_swatch = with_pos && a.w as i32 + 2 * gap + sw + b.w as i32 <= room;
    if with_pos {
        cv.text_mask(&a, pad + s(2.0), y_mid - a.h as i32 / 2, t.muted);
    }
    let bx = w - pad - s(2.0) - b.w as i32;
    cv.text_mask(&b, bx, y_mid - b.h as i32 / 2, t.text);
    if colour.a != 0 && with_swatch {
        let sx = bx - gap - sw;
        let mut d = Mask::new(IBox::xywh(sx - 1, y_mid - sw / 2 - 1, sw + 2, sw + 2));
        d.rounded_box(sx as f32, (y_mid - sw / 2) as f32, (sx + sw) as f32, (y_mid + sw / 2) as f32, 2.0 * scale);
        cv.blend_mask(&d, colour.with_alpha(255));
        let mut o = Mask::new(IBox::xywh(sx - 1, y_mid - sw / 2 - 1, sw + 2, sw + 2));
        o.rounded_box_outline(sx as f32, (y_mid - sw / 2) as f32, (sx + sw) as f32, (y_mid + sw / 2) as f32, 2.0 * scale, th);
        cv.blend_mask(&o, t.border);
    }
    cv
}

/// Where the panel goes: below and right of the cursor, flipped to the other side when the
/// monitor `mon` has no room, never over the cursor itself.
pub fn place(p: Point, w: i32, h: i32, mon: Rect, offset: i32) -> Point {
    let x = if p.x + offset + w <= mon.right { p.x + offset } else { p.x - offset - w };
    let y = if p.y + offset + h <= mon.bottom { p.y + offset } else { p.y - offset - h };
    Point::new(x.clamp(mon.left, (mon.right - w).max(mon.left)), y.clamp(mon.top, (mon.bottom - h).max(mon.top)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::BoxFont;
    use crate::theme::{Look, Mode};

    fn theme() -> Theme {
        Theme::new(Look::Graphite, Mode::Dark)
    }

    #[test]
    fn the_sample_is_centred_on_the_cursor() {
        let r = sample_rect(Point::new(100, 50));
        assert_eq!((r.width(), r.height()), (15, 15));
        assert!(r.contains(Point::new(100, 50)));
        assert_eq!((r.left + r.right) / 2, 100 + (r.left + r.right) % 2 / 2);
        assert_eq!(r.left, 93);
    }

    #[test]
    fn off_desktop_pixels_stay_empty() {
        // The cursor at the desktop's top-left corner: only the lower-right quarter exists.
        let p = Point::new(0, 0);
        let got = Rect::new(0, 0, 8, 8);
        let px = Canvas::new(8, 8, Rgba::rgb(10, 200, 30));
        let sq = square(p, got, &px);
        assert_eq!(sq.get(0, 0).a, 0);
        assert_eq!(sq.get(7, 7), Rgba::rgb(10, 200, 30), "the middle pixel is the cursor's");
        assert_eq!(readout(p, sq.get(0, 0)).1, "-");
    }

    #[test]
    fn the_middle_pixel_is_enlarged_and_named() {
        let mut sample = Canvas::new(15, 15, Rgba::rgb(255, 255, 255));
        sample.set(7, 7, Rgba::rgb(0xE5, 0x38, 0x3B));
        let t = theme();
        let cv = render(&sample, Point::new(640, 360), &t, 1.0, &BoxFont);
        let pad = t.pad as i32;
        let cell = theme::LOUPE_ZOOM as i32;
        assert_eq!(cv.w as i32, 15 * cell + 2 * pad);
        // Inside the middle cell (past its rings) the colour is the pixel's.
        let cx = pad + 7 * cell + cell / 2;
        assert_eq!(cv.get(cx, cx), Rgba::rgb(0xE5, 0x38, 0x3B));
        assert_eq!(readout(Point::new(640, 360), sample.get(7, 7)), ("640, 360".to_string(), "#E5383B".to_string()));
        assert_eq!(cv.get(0, 0).a, 0, "rounded corner");
    }

    #[test]
    fn placement_flips_at_the_monitor_edge() {
        let mon = Rect::new(0, 0, 1920, 1080);
        assert_eq!(place(Point::new(100, 100), 130, 150, mon, 24), Point::new(124, 124));
        let p = place(Point::new(1900, 1070), 130, 150, mon, 24);
        assert_eq!(p, Point::new(1900 - 24 - 130, 1070 - 24 - 150));
        let q = place(Point::new(5, 5), 130, 150, Rect::new(0, 0, 100, 100), 24);
        assert!(q.x >= 0 && q.y >= 0, "a tiny monitor still keeps it on screen: {q:?}");
    }
}
