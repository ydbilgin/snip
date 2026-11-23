//! Annotations and their undo/redo history, and the renderer that burns them into the image.
//!
//! Shapes live in desktop coordinates (physical pixels, like the selection), so the selection can
//! still be moved or resized after drawing. `render` crops to whatever the selection is at the end.

use crate::raster::{Canvas, Glyphs, IBox, Mask, Pt, Rgba};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObscureMode {
    Pixelate,
    Blur,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Arrow { a: Pt, b: Pt, color: Rgba, width: f32 },
    Rect { a: Pt, b: Pt, color: Rgba, width: f32 },
    Line { a: Pt, b: Pt, color: Rgba, width: f32 },
    Pen { pts: Vec<Pt>, color: Rgba, width: f32 },
    Marker { pts: Vec<Pt>, color: Rgba, width: f32 },
    /// `at` is the top-left of the first line; `size` the em size in pixels.
    Text { at: Pt, text: String, color: Rgba, size: f32 },
    /// A numbered badge centred at `at` with radius `r`.
    Step { at: Pt, n: u32, color: Rgba, r: f32 },
    /// `strength`: block size (pixelate) or radius (blur) in pixels.
    Obscure { a: Pt, b: Pt, mode: ObscureMode, strength: f32 },
}

impl Shape {
    /// Pixel bounds of everything the shape may change (for partial redraws and tests).
    pub fn bounds(&self) -> IBox {
        match self {
            Shape::Arrow { a, b, width, .. } => IBox::around(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y), arrow_head(*width) + width),
            Shape::Rect { a, b, width, .. } | Shape::Line { a, b, width, .. } => IBox::around(a.x, a.y, b.x, b.y, width / 2.0 + 1.0),
            Shape::Pen { pts, width, .. } | Shape::Marker { pts, width, .. } => {
                let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for p in pts {
                    x0 = x0.min(p.x);
                    y0 = y0.min(p.y);
                    x1 = x1.max(p.x);
                    y1 = y1.max(p.y);
                }
                if pts.is_empty() { IBox::default() } else { IBox::around(x0, y0, x1, y1, width / 2.0 + 1.0) }
            }
            Shape::Text { at, text, size, .. } => {
                let lines = text.split('\n').count().max(1) as f32;
                let longest = text.split('\n').map(|l| l.chars().count()).max().unwrap_or(0) as f32;
                IBox::around(at.x, at.y, at.x + longest * size * crate::theme::ANNOTATION_TEXT_WIDTH_ESTIMATE,
                    at.y + lines * size * crate::theme::ANNOTATION_LINE_SCALE, crate::theme::ANNOTATION_TEXT_BOUNDS_PAD)
            }
            Shape::Step { at, r, .. } => IBox::around(at.x, at.y, at.x, at.y, r + 1.0),
            Shape::Obscure { a, b, .. } => IBox::around(a.x, a.y, b.x, b.y, 0.0),
        }
    }

    /// Boxes that together hold every pixel the shape changes, with far less area than `bounds`
    /// on a long diagonal or a large frame. `from`: for a stroke, only what the points from this
    /// index on add. Boxes may overlap.
    pub fn cover(&self, from: usize) -> Vec<IBox> {
        const STEP: f32 = 128.0;
        let along = |a: Pt, b: Pt, pad: f32, v: &mut Vec<IBox>| {
            let n = (a.dist(b) / STEP).ceil().max(1.0) as usize;
            let at = |k: usize| a.plus(b.minus(a).scale(k as f32 / n as f32));
            for k in 0..n {
                let (p, q) = (at(k), at(k + 1));
                v.push(IBox::around(p.x.min(q.x), p.y.min(q.y), p.x.max(q.x), p.y.max(q.y), pad));
            }
        };
        let mut v = Vec::new();
        match self {
            Shape::Arrow { a, b, width, .. } => {
                along(*a, *b, width / 2.0 + 1.0, &mut v);
                let head = arrow_head(*width) * (1.0 + crate::theme::ARROW_HEAD_HALF_SCALE) + width + 1.0;
                v.push(IBox::around(b.x, b.y, b.x, b.y, head));
            }
            Shape::Line { a, b, width, .. } => along(*a, *b, width / 2.0 + 1.0, &mut v),
            Shape::Rect { a, b, width, .. } => {
                let pad = width / 2.0 + 1.0;
                let (x0, x1, y0, y1) = (a.x.min(b.x), a.x.max(b.x), a.y.min(b.y), a.y.max(b.y));
                v.push(IBox::around(x0, y0, x1, y0, pad));
                v.push(IBox::around(x0, y1, x1, y1, pad));
                v.push(IBox::around(x0, y0, x0, y1, pad));
                v.push(IBox::around(x1, y0, x1, y1, pad));
            }
            Shape::Pen { pts, width, .. } | Shape::Marker { pts, width, .. } => {
                let pad = width / 2.0 + 1.0;
                match &pts[from.saturating_sub(1).min(pts.len())..] {
                    [p] if from == 0 => v.push(IBox::around(p.x, p.y, p.x, p.y, pad)),
                    tail => {
                        for w in tail.windows(2) {
                            v.push(IBox::around(w[0].x.min(w[1].x), w[0].y.min(w[1].y), w[0].x.max(w[1].x), w[0].y.max(w[1].y), pad));
                        }
                    }
                }
            }
            _ => v.push(self.bounds()),
        }
        v
    }

    /// Too small to keep (a click with a drag tool).
    pub fn is_degenerate(&self) -> bool {
        match self {
            Shape::Arrow { a, b, .. } | Shape::Line { a, b, .. } => a.dist(*b) < 3.0,
            Shape::Rect { a, b, .. } | Shape::Obscure { a, b, .. } => (a.x - b.x).abs() < 3.0 || (a.y - b.y).abs() < 3.0,
            Shape::Pen { pts, .. } | Shape::Marker { pts, .. } => pts.is_empty(),
            Shape::Text { text, .. } => text.trim().is_empty(),
            Shape::Step { .. } => false,
        }
    }
}

/// Length of the arrow head for a shaft `width` wide.
pub fn arrow_head(width: f32) -> f32 {
    (width * crate::theme::ARROW_HEAD_WIDTH_SCALE + crate::theme::ARROW_HEAD_EXTRA).max(crate::theme::ARROW_HEAD_MIN)
}

#[derive(Clone, Debug, PartialEq)]
enum Op {
    Add(Shape),
    Replace { index: usize, before: Shape, after: Shape },
    Remove {
        index: usize,
        before: Shape,
    },
}

/// The shapes and an undo/redo history. A new edit clears the redo stack.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Doc {
    shapes: Vec<Shape>,
    undo: Vec<Op>,
    redo: Vec<Op>,
}

impl Doc {
    pub fn shapes(&self) -> &[Shape] {
        &self.shapes
    }

    pub fn add(&mut self, s: Shape) -> bool {
        if s.is_degenerate() {
            return false;
        }
        self.shapes.push(s.clone());
        self.undo.push(Op::Add(s));
        self.redo.clear();
        true
    }

    /// Change shape `index` (text edits). A change to a degenerate shape is refused.
    pub fn replace(&mut self, index: usize, after: Shape) -> bool {
        let Some(before) = self.shapes.get(index).cloned() else { return false };
        if before == after || after.is_degenerate() {
            return false;
        }
        self.shapes[index] = after.clone();
        self.undo.push(Op::Replace { index, before, after });
        self.redo.clear();
        true
    }

    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.shapes.len() {
            return false;
        }
        let before = self.shapes.remove(index);
        self.undo.push(Op::Remove { index, before });
        self.redo.clear();
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(op) = self.undo.pop() else {
            return false;
        };
        match &op {
            Op::Add(_) => {
                self.shapes.pop();
            }
            Op::Replace { index, before, .. } => self.shapes[*index] = before.clone(),
            Op::Remove { index, before } => self.shapes.insert(*index, before.clone()),
        }
        self.redo.push(op);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(op) = self.redo.pop() else {
            return false;
        };
        match &op {
            Op::Add(s) => self.shapes.push(s.clone()),
            Op::Replace { index, after, .. } => self.shapes[*index] = after.clone(),
            Op::Remove { index, .. } => {
                self.shapes.remove(*index);
            }
        }
        self.undo.push(op);
        true
    }

    /// The number the next step badge gets: one more than the highest badge now in the image
    /// (an undone badge frees its number).
    pub fn next_step(&self) -> u32 {
        self.shapes.iter().filter_map(|s| if let Shape::Step { n, .. } = s { Some(*n) } else { None }).max().unwrap_or(0) + 1
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }
}

/// Snap `b` so the line from `a` is horizontal, vertical or at 45 degrees (Shift).
pub fn snap45(a: Pt, b: Pt) -> Pt {
    let d = b.minus(a);
    let len = d.len();
    if len < 1e-3 {
        return b;
    }
    let ang = d.y.atan2(d.x);
    let step = std::f32::consts::FRAC_PI_4;
    let snapped = (ang / step).round() * step;
    Pt::new(a.x + len * snapped.cos(), a.y + len * snapped.sin())
}

/// Make the box from `a` to `b` a square (Shift), keeping the drag direction.
pub fn square(a: Pt, b: Pt) -> Pt {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let s = dx.abs().max(dy.abs());
    Pt::new(a.x + s * dx.signum(), a.y + s * dy.signum())
}

/// Text colour for a step badge of colour `c` (white or near-black, whichever reads better).
pub fn badge_text(c: Rgba) -> Rgba {
    let white = crate::theme::BADGE_LIGHT_TEXT;
    let ink = crate::theme::BADGE_DARK_TEXT;
    // White unless it is clearly hard to read (yellow, orange, white): white on red reads better
    // than the near-tie in contrast numbers suggests.
    if c.contrast(white) >= 3.0 { white } else { ink }
}

/// Actual text hit bounds: use the same glyphs and line height as final rendering.
pub fn text_bounds(shape: &Shape, glyphs: &dyn Glyphs) -> Option<IBox> {
    let Shape::Text { at, text, size, .. } = shape else {
        return None;
    };
    let width = text
        .split('\n')
        .map(|line| glyphs.text(line, *size, false).w)
        .max()
        .unwrap_or(1)
        .max(1);
    let height = (glyphs.line_height(*size) * text.split('\n').count() as f32)
        .ceil()
        .max(1.0) as i32;
    Some(IBox::xywh(
        at.x.round() as i32,
        at.y.round() as i32,
        width as i32,
        height,
    ))
}

/// Draw `shapes` onto `img`, whose top-left pixel is at desktop position `origin`.
pub fn render(img: &mut Canvas, origin: (i32, i32), shapes: &[Shape], glyphs: &dyn Glyphs) {
    render_clipped(img, origin, shapes, None, glyphs);
}

/// Like `render`, touching only the box `clip` (image coordinates). Inside it the result is the
/// same pixels a full render gives. Text and obscure areas ignore the clip.
pub fn render_in(img: &mut Canvas, origin: (i32, i32), shape: &Shape, clip: IBox, glyphs: &dyn Glyphs) {
    render_clipped(img, origin, std::slice::from_ref(shape), Some(clip), glyphs);
}

fn render_clipped(img: &mut Canvas, origin: (i32, i32), shapes: &[Shape], clip: Option<IBox>, glyphs: &dyn Glyphs) {
    let o = Pt::new(origin.0 as f32, origin.1 as f32);
    let l = |p: &Pt| p.minus(o);
    for s in shapes {
        let mut local =
            s.bounds()
                .intersect(&IBox::xywh(origin.0, origin.1, img.w as i32, img.h as i32));
        if let Some(c) = clip {
            local = local.intersect(&IBox::new(c.x0 + origin.0, c.y0 + origin.1, c.x1 + origin.0, c.y1 + origin.1));
        }
        // Text bounds are estimates: actual glyph widths depend on the chosen font. A wide
        // glyph can overlap the image even when that estimate does not.
        if local.is_empty() && !matches!(s, Shape::Text { .. }) {
            continue;
        }
        let mbox = IBox::new(local.x0 - origin.0, local.y0 - origin.1, local.x1 - origin.0, local.y1 - origin.1);
        match s {
            Shape::Arrow { a, b, color, width } => {
                let mut m = Mask::new(mbox);
                arrow_mask(&mut m, l(a), l(b), *width);
                img.blend_mask(&m, *color);
            }
            Shape::Rect { a, b, color, width } => {
                let mut m = Mask::new(mbox);
                let (a, b) = (l(a), l(b));
                m.box_outline(a.x, a.y, b.x, b.y, *width);
                img.blend_mask(&m, *color);
            }
            Shape::Line { a, b, color, width } => {
                let mut m = Mask::new(mbox);
                m.capsule(l(a), l(b), *width);
                img.blend_mask(&m, *color);
            }
            Shape::Pen { pts, color, width } => {
                let mut m = Mask::new(mbox);
                m.polyline(&pts.iter().map(l).collect::<Vec<_>>(), *width);
                img.blend_mask(&m, *color);
            }
            Shape::Marker { pts, color, width } => {
                let mut m = Mask::new(mbox);
                m.polyline(&pts.iter().map(l).collect::<Vec<_>>(), *width);
                img.multiply_mask(&m, *color);
            }
            Shape::Text { at, text, color, size } => {
                let p = l(at);
                let line_h = glyphs.line_height(*size).round() as i32;
                for (i, line) in text.split('\n').enumerate() {
                    if !line.is_empty() {
                        img.text(glyphs, line, *size, false, p.x.round() as i32, p.y.round() as i32 + i as i32 * line_h, *color);
                    }
                }
            }
            Shape::Step { at, n, color, r } => {
                let c = l(at);
                let mut m = Mask::new(mbox);
                m.disc(c, *r);
                img.blend_mask(&m, *color);
                let label = n.to_string();
                let t = glyphs.text(&label, r * crate::theme::STEP_FONT_SCALE, true);
                let x = (c.x - t.w as f32 / 2.0).round() as i32;
                let y = (c.y - t.h as f32 / 2.0).round() as i32;
                img.text_mask(&t, x, y, badge_text(*color));
            }
            Shape::Obscure { a, b, mode, strength } => {
                let (a, b) = (l(a), l(b));
                let r = IBox::around(a.x, a.y, b.x, b.y, 0.0);
                match mode {
                    ObscureMode::Pixelate => img.pixelate(r, strength.round().max(2.0) as u32),
                    ObscureMode::Blur => img.blur(r, strength.round().max(1.0) as u32),
                }
            }
        }
    }
}

fn arrow_mask(m: &mut Mask, a: Pt, b: Pt, width: f32) {
    let d = b.minus(a);
    let len = d.len();
    if len < 1e-3 {
        return;
    }
    let u = d.scale(1.0 / len);
    let head = arrow_head(width).min(len);
    let half = head * crate::theme::ARROW_HEAD_HALF_SCALE;
    let base = b.minus(u.scale(head));
    let n = Pt::new(-u.y, u.x);
    m.convex(&[b, base.plus(n.scale(half)), base.minus(n.scale(half))]);
    // The shaft ends inside the head so its round cap never pokes out of the tip.
    m.capsule(a, b.minus(u.scale(head * crate::theme::ARROW_SHAFT_END_SCALE)), width);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::BoxFont;

    const RED: Rgba = Rgba::rgb(229, 56, 59);
    const WHITE: Rgba = Rgba::rgb(255, 255, 255);

    fn line(x0: f32, x1: f32) -> Shape {
        Shape::Line { a: Pt::new(x0, 10.0), b: Pt::new(x1, 10.0), color: RED, width: 4.0 }
    }

    #[test]
    fn undo_redo_walk_back_and_forth() {
        let mut d = Doc::default();
        assert!(!d.can_undo() && !d.can_redo());
        assert!(d.add(line(0.0, 50.0)));
        assert!(d.add(line(0.0, 60.0)));
        assert_eq!(d.shapes().len(), 2);
        assert!(d.undo());
        assert_eq!(d.shapes(), &[line(0.0, 50.0)]);
        assert!(d.redo());
        assert_eq!(d.shapes().len(), 2);
        assert!(d.undo() && d.undo());
        assert!(d.is_empty());
        assert!(!d.undo(), "nothing left to undo");
        assert!(d.redo());
        // A new edit drops the redo stack.
        assert!(d.add(line(0.0, 70.0)));
        assert!(!d.can_redo());
        assert_eq!(d.shapes(), &[line(0.0, 50.0), line(0.0, 70.0)]);
    }

    #[test]
    fn degenerate_shapes_are_not_added() {
        let mut d = Doc::default();
        let cases = [
            line(10.0, 11.0),
            Shape::Rect { a: Pt::new(0.0, 0.0), b: Pt::new(100.0, 1.0), color: RED, width: 2.0 },
            Shape::Text { at: Pt::new(0.0, 0.0), text: "  \n ".into(), color: RED, size: 20.0 },
            Shape::Pen { pts: vec![], color: RED, width: 2.0 },
        ];
        assert!(!cases.is_empty());
        for c in cases {
            assert!(!d.add(c.clone()), "{c:?}");
        }
        assert!(!d.can_undo());
    }

    #[test]
    fn replace_is_undoable() {
        let mut d = Doc::default();
        let t = |s: &str| Shape::Text { at: Pt::new(5.0, 5.0), text: s.into(), color: RED, size: 20.0 };
        d.add(t("ab"));
        assert!(d.replace(0, t("abc")));
        assert!(!d.replace(0, t("abc")), "no change, no history entry");
        assert!(!d.replace(9, t("x")), "no such shape");
        d.undo();
        assert_eq!(d.shapes(), &[t("ab")]);
        d.redo();
        assert_eq!(d.shapes(), &[t("abc")]);
    }

    #[test]
    fn step_numbers_follow_undo() {
        let mut d = Doc::default();
        let step = |n| Shape::Step { at: Pt::new(20.0, 20.0), n, color: RED, r: 12.0 };
        assert_eq!(d.next_step(), 1);
        d.add(step(d.next_step()));
        d.add(step(d.next_step()));
        assert_eq!(d.next_step(), 3);
        d.undo();
        assert_eq!(d.next_step(), 2);
    }

    #[test]
    fn snapping() {
        let a = Pt::new(0.0, 0.0);
        let s = snap45(a, Pt::new(100.0, 10.0));
        assert!((s.y).abs() < 1e-3 && (s.x - 100.5).abs() < 0.1);
        let s = snap45(a, Pt::new(50.0, 45.0));
        assert!((s.x - s.y).abs() < 1e-3, "45 degrees: {s:?}");
        assert_eq!(square(a, Pt::new(-30.0, 10.0)), Pt::new(-30.0, 30.0));
        assert_eq!(square(a, Pt::new(30.0, 0.0)), Pt::new(30.0, 30.0));
        assert_eq!(square(a, Pt::new(0.0, -30.0)), Pt::new(30.0, -30.0));
    }

    #[test]
    fn text_with_wide_glyphs_is_not_culled_by_estimated_bounds() {
        struct WideFont;
        impl Glyphs for WideFont {
            fn text(&self, _: &str, _: f32, _: bool) -> crate::raster::TextMask {
                crate::raster::TextMask { w: 30, h: 10, ascent: 10, cov: vec![255; 300] }
            }
        }
        let mut img = Canvas::new(10, 10, WHITE);
        let text = Shape::Text { at: Pt::new(-20.0, 0.0), text: "W".into(), color: RED, size: 10.0 };
        render(&mut img, (0, 0), &[text], &WideFont);
        assert_eq!(img.get(0, 0), RED);
    }

    #[test]
    fn render_uses_desktop_coordinates() {
        // The image's top-left is at desktop (-100, -50) (a monitor left of and above the primary).
        let mut img = Canvas::new(200, 100, WHITE);
        let s = Shape::Rect { a: Pt::new(-90.0, -40.0), b: Pt::new(-10.0, 0.0), color: RED, width: 4.0 };
        render(&mut img, (-100, -50), &[s], &BoxFont);
        assert_eq!(img.get(10, 30), RED, "left edge at x=-90 -> 10");
        assert_eq!(img.get(50, 30), WHITE, "inside stays clear");
        assert_eq!(img.get(150, 30), WHITE);
    }

    #[test]
    fn arrow_has_a_head_wider_than_its_shaft() {
        let mut img = Canvas::new(200, 60, WHITE);
        render(&mut img, (0, 0), &[Shape::Arrow { a: Pt::new(10.0, 30.0), b: Pt::new(190.0, 30.0), color: RED, width: 4.0 }], &BoxFont);
        let painted = |x: i32| (0..60).filter(|&y| img.get(x, y) != WHITE).count();
        assert!(painted(50) <= 6, "shaft: {}", painted(50));
        assert!(painted(180) > 8, "head: {}", painted(180));
        assert_eq!(img.get(195, 30), WHITE, "nothing past the tip");
    }

    #[test]
    fn marker_keeps_dark_text_dark() {
        let mut img = Canvas::new(40, 20, WHITE);
        img.fill_box(IBox::new(0, 0, 20, 20), Rgba::rgb(0, 0, 0));
        let m = Shape::Marker { pts: vec![Pt::new(0.0, 10.0), Pt::new(40.0, 10.0)], color: crate::theme::MARKER_DEFAULT, width: 12.0 };
        render(&mut img, (0, 0), &[m], &BoxFont);
        assert_eq!(img.get(5, 10), Rgba::rgb(0, 0, 0));
        assert_eq!(img.get(30, 10), crate::theme::MARKER_DEFAULT);
    }

    #[test]
    fn obscure_changes_only_its_box() {
        let mut img = Canvas::new(40, 40, WHITE);
        for y in 0..40 {
            for x in 0..40 {
                if (x + y) % 2 == 0 {
                    img.set(x, y, Rgba::rgb(0, 0, 0));
                }
            }
        }
        let before = img.clone();
        let s = Shape::Obscure { a: Pt::new(10.0, 10.0), b: Pt::new(30.0, 30.0), mode: ObscureMode::Pixelate, strength: 10.0 };
        render(&mut img, (0, 0), &[s], &BoxFont);
        assert_eq!(img.get(5, 5), before.get(5, 5));
        let c = img.get(15, 15);
        assert!(c.r > 100 && c.r < 155, "checkerboard averaged to grey: {c:?}");
        assert_eq!(img.get(10, 10), img.get(19, 19), "one block, one colour");
    }

    #[test]
    fn step_badge_text_contrasts() {
        assert_eq!(badge_text(RED), WHITE);
        assert_eq!(badge_text(Rgba::rgb(255, 214, 10)), Rgba::rgb(17, 17, 17));
        let mut img = Canvas::new(60, 60, WHITE);
        render(&mut img, (0, 0), &[Shape::Step { at: Pt::new(30.0, 30.0), n: 7, color: RED, r: 16.0 }], &BoxFont);
        assert_eq!(img.get(30, 16), RED, "disc");
        assert_eq!(img.get(30, 30), WHITE, "the number's box font glyph in white");
    }

    #[test]
    fn shapes_outside_the_image_are_skipped() {
        let mut img = Canvas::new(10, 10, WHITE);
        let before = img.clone();
        render(&mut img, (0, 0), &[line(100.0, 200.0)], &BoxFont);
        assert_eq!(img, before);
    }
}
