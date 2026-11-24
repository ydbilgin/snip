//! A small software renderer over top-down BGRA pixels.
//!
//! Everything snip draws itself goes through here: the annotations in the editor (the same code
//! makes the preview and the saved PNG, so what you see is what you get), the toolbar, the icons,
//! the history grid and the settings window. Text is the one thing it does not rasterise; a
//! `Glyphs` implementation supplies coverage masks for it (GDI on Windows, a box font in tests).
//!
//! Shapes are drawn as coverage masks from signed distances (a pixel's centre is at x + 0.5), then
//! blended once, so overlapping parts of one stroke never double up.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// `#RRGGBB` or `#RRGGBBAA`, case-insensitive.
    pub fn from_hex(s: &str) -> Option<Rgba> {
        let h = s.strip_prefix('#')?;
        if !(h.len() == 6 || h.len() == 8) || !h.is_ascii() {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        Some(Rgba { r: byte(0)?, g: byte(2)?, b: byte(4)?, a: if h.len() == 8 { byte(6)? } else { 255 } })
    }

    pub fn to_hex(self) -> String {
        if self.a == 255 { format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b) } else { format!("#{:02X}{:02X}{:02X}{:02X}", self.r, self.g, self.b, self.a) }
    }

    /// Relative luminance (sRGB, WCAG 2), 0..1.
    pub fn luminance(self) -> f32 {
        let lin = |c: u8| {
            let c = c as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// WCAG contrast ratio between two opaque colours (1..21).
    pub fn contrast(self, o: Rgba) -> f32 {
        let (a, b) = (self.luminance(), o.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Pt {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    pub fn minus(self, o: Pt) -> Pt {
        Pt::new(self.x - o.x, self.y - o.y)
    }
    pub fn plus(self, o: Pt) -> Pt {
        Pt::new(self.x + o.x, self.y + o.y)
    }
    pub fn scale(self, k: f32) -> Pt {
        Pt::new(self.x * k, self.y * k)
    }
    pub fn dot(self, o: Pt) -> f32 {
        self.x * o.x + self.y * o.y
    }
    pub fn len(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn dist(self, o: Pt) -> f32 {
        self.minus(o).len()
    }
}

/// An integer pixel box, half-open, in canvas coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct IBox {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl IBox {
    pub const fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x0, y0, x1, y1 }
    }
    pub fn xywh(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self::new(x, y, x + w, y + h)
    }
    pub fn w(&self) -> i32 {
        (self.x1 - self.x0).max(0)
    }
    pub fn h(&self) -> i32 {
        (self.y1 - self.y0).max(0)
    }
    pub fn is_empty(&self) -> bool {
        self.w() == 0 || self.h() == 0
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }
    pub fn translate(&self, dx: i32, dy: i32) -> IBox {
        IBox::new(self.x0 + dx, self.y0 + dy, self.x1 + dx, self.y1 + dy)
    }
    pub fn intersect(&self, o: &IBox) -> IBox {
        let r = IBox::new(self.x0.max(o.x0), self.y0.max(o.y0), self.x1.min(o.x1), self.y1.min(o.y1));
        if r.x0 < r.x1 && r.y0 < r.y1 { r } else { IBox::default() }
    }
    /// The pixel box that holds a float box, grown by `pad` pixels.
    pub fn around(x0: f32, y0: f32, x1: f32, y1: f32, pad: f32) -> IBox {
        IBox::new((x0.min(x1) - pad).floor() as i32, (y0.min(y1) - pad).floor() as i32, (x0.max(x1) + pad).ceil() as i32, (y0.max(y1) + pad).ceil() as i32)
    }
}

/// Top-down BGRA pixels, straight (not premultiplied) alpha. Screenshots are opaque (alpha 255);
/// UI layers may be transparent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Canvas {
    pub fn new(w: u32, h: u32, fill: Rgba) -> Canvas {
        let mut px = Vec::with_capacity(w as usize * h as usize * 4);
        for _ in 0..w as usize * h as usize {
            px.extend_from_slice(&[fill.b, fill.g, fill.r, fill.a]);
        }
        Canvas { w, h, px }
    }

    pub fn from_bgra(w: u32, h: u32, px: Vec<u8>) -> Canvas {
        assert_eq!(px.len(), w as usize * h as usize * 4, "BGRA buffer size");
        Canvas { w, h, px }
    }

    pub fn bounds(&self) -> IBox {
        IBox::new(0, 0, self.w as i32, self.h as i32)
    }

    fn idx(&self, x: i32, y: i32) -> usize {
        (y as usize * self.w as usize + x as usize) * 4
    }

    pub fn get(&self, x: i32, y: i32) -> Rgba {
        let i = self.idx(x, y);
        Rgba { b: self.px[i], g: self.px[i + 1], r: self.px[i + 2], a: self.px[i + 3] }
    }

    pub fn set(&mut self, x: i32, y: i32, c: Rgba) {
        let i = self.idx(x, y);
        self.px[i..i + 4].copy_from_slice(&[c.b, c.g, c.r, c.a]);
    }

    /// Source-over with straight alpha, `k` = extra coverage 0..1.
    pub fn blend(&mut self, x: i32, y: i32, c: Rgba, k: f32) {
        let a = c.a as f32 / 255.0 * k.clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        let i = self.idx(x, y);
        let da = self.px[i + 3] as f32 / 255.0;
        let oa = a + da * (1.0 - a);
        if oa <= 0.0 {
            return;
        }
        let mix = |s: u8, d: u8| ((s as f32 * a + d as f32 * da * (1.0 - a)) / oa).round().clamp(0.0, 255.0) as u8;
        self.px[i] = mix(c.b, self.px[i]);
        self.px[i + 1] = mix(c.g, self.px[i + 1]);
        self.px[i + 2] = mix(c.r, self.px[i + 2]);
        self.px[i + 3] = (oa * 255.0).round() as u8;
    }

    /// Multiply (a highlighter): white under the ink takes the ink's colour, black stays black.
    pub fn multiply(&mut self, x: i32, y: i32, c: Rgba, k: f32) {
        let a = c.a as f32 / 255.0 * k.clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        let i = self.idx(x, y);
        let m = |s: u8, d: u8| {
            let prod = d as f32 * s as f32 / 255.0;
            (d as f32 + (prod - d as f32) * a).round().clamp(0.0, 255.0) as u8
        };
        self.px[i] = m(c.b, self.px[i]);
        self.px[i + 1] = m(c.g, self.px[i + 1]);
        self.px[i + 2] = m(c.r, self.px[i + 2]);
    }

    pub fn fill_box(&mut self, b: IBox, c: Rgba) {
        let b = b.intersect(&self.bounds());
        for y in b.y0..b.y1 {
            for x in b.x0..b.x1 {
                if c.a == 255 {
                    self.set(x, y, c);
                } else {
                    self.blend(x, y, c, 1.0);
                }
            }
        }
    }

    /// Copy `src` with its top-left at (`dx`, `dy`), source-over.
    pub fn draw(&mut self, src: &Canvas, dx: i32, dy: i32) {
        let b = IBox::xywh(dx, dy, src.w as i32, src.h as i32).intersect(&self.bounds());
        for y in b.y0..b.y1 {
            for x in b.x0..b.x1 {
                let c = src.get(x - dx, y - dy);
                if c.a == 255 {
                    self.set(x, y, c);
                } else if c.a > 0 {
                    self.blend(x, y, c, 1.0);
                }
            }
        }
    }

    pub fn blend_mask(&mut self, m: &Mask, c: Rgba) {
        self.apply_mask(m, |cv, x, y, k| cv.blend(x, y, c, k));
    }

    pub fn multiply_mask(&mut self, m: &Mask, c: Rgba) {
        self.apply_mask(m, |cv, x, y, k| cv.multiply(x, y, c, k));
    }

    fn apply_mask(&mut self, m: &Mask, mut f: impl FnMut(&mut Canvas, i32, i32, f32)) {
        let b = m.b.intersect(&self.bounds());
        for y in b.y0..b.y1 {
            for x in b.x0..b.x1 {
                let k = m.at(x, y);
                if k > 0 {
                    f(self, x, y, k as f32 / 255.0);
                }
            }
        }
    }

    /// Replace each `block` x `block` cell inside `r` by its average colour.
    pub fn pixelate(&mut self, r: IBox, block: u32) {
        let r = r.intersect(&self.bounds());
        let block = block.max(2) as i32;
        let mut by = r.y0;
        while by < r.y1 {
            let mut bx = r.x0;
            while bx < r.x1 {
                let cell = IBox::new(bx, by, (bx + block).min(r.x1), (by + block).min(r.y1));
                let (mut sb, mut sg, mut sr, mut n) = (0u64, 0u64, 0u64, 0u64);
                for y in cell.y0..cell.y1 {
                    for x in cell.x0..cell.x1 {
                        let c = self.get(x, y);
                        sb += c.b as u64;
                        sg += c.g as u64;
                        sr += c.r as u64;
                        n += 1;
                    }
                }
                let avg = Rgba::rgb((sr / n) as u8, (sg / n) as u8, (sb / n) as u8);
                for y in cell.y0..cell.y1 {
                    for x in cell.x0..cell.x1 {
                        let a = self.get(x, y).a;
                        self.set(x, y, avg.with_alpha(a));
                    }
                }
                bx += block;
            }
            by += block;
        }
    }

    /// Three box-blur passes (close to a Gaussian) of `radius` inside `r`. Pixels outside `r` are
    /// neither read nor written: the edge is clamped.
    pub fn blur(&mut self, r: IBox, radius: u32) {
        let r = r.intersect(&self.bounds());
        if r.is_empty() || radius == 0 {
            return;
        }
        let (w, h) = (r.w() as usize, r.h() as usize);
        let mut buf: Vec<[f32; 3]> = Vec::with_capacity(w * h);
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                let c = self.get(x, y);
                buf.push([c.b as f32, c.g as f32, c.r as f32]);
            }
        }
        let rad = radius as i32;
        let mut tmp = buf.clone();
        for _ in 0..3 {
            box_pass(&buf, &mut tmp, w, h, rad, true);
            box_pass(&tmp, &mut buf, w, h, rad, false);
        }
        for (i, v) in buf.iter().enumerate() {
            let (x, y) = (r.x0 + (i % w) as i32, r.y0 + (i / w) as i32);
            let a = self.get(x, y).a;
            self.set(x, y, Rgba { b: v[0].round() as u8, g: v[1].round() as u8, r: v[2].round() as u8, a });
        }
    }

    /// Box-filter downscale to fit inside `max_w` x `max_h`, keeping the aspect ratio. Never upscales.
    pub fn downscale_to_fit(&self, max_w: u32, max_h: u32) -> Canvas {
        let k = (max_w as f64 / self.w as f64).min(max_h as f64 / self.h as f64).min(1.0);
        let nw = ((self.w as f64 * k).round() as u32).max(1);
        let nh = ((self.h as f64 * k).round() as u32).max(1);
        self.resample_box(nw, nh)
    }

    /// Area-average resample to exactly `nw` x `nh` (any direction).
    pub fn resample_box(&self, nw: u32, nh: u32) -> Canvas {
        let mut out = Canvas::new(nw, nh, Rgba::default());
        let sx = self.w as f64 / nw as f64;
        let sy = self.h as f64 / nh as f64;
        for oy in 0..nh {
            let y0 = oy as f64 * sy;
            let y1 = y0 + sy;
            for ox in 0..nw {
                let x0 = ox as f64 * sx;
                let x1 = x0 + sx;
                let mut acc = [0f64; 4];
                let mut wsum = 0f64;
                let mut y = y0.floor() as i64;
                while (y as f64) < y1 && y < self.h as i64 {
                    let wy = (y1.min(y as f64 + 1.0) - y0.max(y as f64)).max(0.0);
                    let mut x = x0.floor() as i64;
                    while (x as f64) < x1 && x < self.w as i64 {
                        let wx = (x1.min(x as f64 + 1.0) - x0.max(x as f64)).max(0.0);
                        let wgt = wx * wy;
                        let i = self.idx(x as i32, y as i32);
                        for (c, a) in acc.iter_mut().enumerate() {
                            *a += self.px[i + c] as f64 * wgt;
                        }
                        wsum += wgt;
                        x += 1;
                    }
                    y += 1;
                }
                let i = out.idx(ox as i32, oy as i32);
                for (d, a) in out.px[i..i + 4].iter_mut().zip(acc) {
                    *d = (a / wsum.max(1e-9)).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        out
    }

    /// Premultiplied copy (for Direct2D bitmaps and layered windows).
    pub fn premultiplied(&self) -> Vec<u8> {
        let mut v = self.px.clone();
        for p in v.as_chunks_mut::<4>().0 {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
        v
    }

    /// A sub-rectangle as a new canvas (clipped to the canvas).
    /// Copy the pixels of `b` from `src` (a canvas of the same size), clipped to both.
    pub fn copy_box_from(&mut self, src: &Canvas, b: IBox) {
        if self.w != src.w || self.h != src.h {
            return;
        }
        let b = b.intersect(&self.bounds());
        if b.is_empty() {
            return;
        }
        let stride = self.w as usize * 4;
        let (x0, x1) = (b.x0 as usize * 4, b.x1 as usize * 4);
        for y in b.y0 as usize..b.y1 as usize {
            let row = y * stride;
            self.px[row + x0..row + x1].copy_from_slice(&src.px[row + x0..row + x1]);
        }
    }

    pub fn crop(&self, b: IBox) -> Canvas {
        let b = b.intersect(&self.bounds());
        let mut px = Vec::with_capacity(b.w() as usize * b.h() as usize * 4);
        for y in b.y0..b.y1 {
            let s = self.idx(b.x0, y);
            px.extend_from_slice(&self.px[s..s + b.w() as usize * 4]);
        }
        Canvas { w: b.w() as u32, h: b.h() as u32, px }
    }
}

fn box_pass(src: &[[f32; 3]], dst: &mut [[f32; 3]], w: usize, h: usize, r: i32, horizontal: bool) {
    let (n, lines) = if horizontal { (w, h) } else { (h, w) };
    let at = |line: usize, i: i32| -> usize {
        let i = i.clamp(0, n as i32 - 1) as usize;
        if horizontal { line * w + i } else { i * w + line }
    };
    let k = 1.0 / (2 * r + 1) as f32;
    for line in 0..lines {
        let mut acc = [0f32; 3];
        for i in -r..=r {
            let s = src[at(line, i)];
            for c in 0..3 {
                acc[c] += s[c];
            }
        }
        for i in 0..n as i32 {
            let o = at(line, i);
            dst[o] = [acc[0] * k, acc[1] * k, acc[2] * k];
            let add = src[at(line, i + r + 1)];
            let sub = src[at(line, i - r)];
            for c in 0..3 {
                acc[c] += add[c] - sub[c];
            }
        }
    }
}

/// Coverage (0..255) over a pixel box. Shapes add to it with `max`, so a stroke that crosses
/// itself has the same density everywhere.
#[derive(Clone, Debug)]
pub struct Mask {
    pub b: IBox,
    pub cov: Vec<u8>,
}

fn cov_from_sd(sd: f32) -> u8 {
    // sd: signed distance from the pixel centre to the edge, negative inside.
    ((0.5 - sd).clamp(0.0, 1.0) * 255.0).round() as u8
}

impl Mask {
    pub fn new(b: IBox) -> Mask {
        Mask { b, cov: vec![0; b.w() as usize * b.h() as usize] }
    }

    pub fn at(&self, x: i32, y: i32) -> u8 {
        if !self.b.contains(x, y) {
            return 0;
        }
        self.cov[((y - self.b.y0) * self.b.w() + (x - self.b.x0)) as usize]
    }

    fn put(&mut self, x: i32, y: i32, v: u8) {
        if v == 0 || !self.b.contains(x, y) {
            return;
        }
        let i = ((y - self.b.y0) * self.b.w() + (x - self.b.x0)) as usize;
        if v > self.cov[i] {
            self.cov[i] = v;
        }
    }

    /// Run `sd` (signed distance at a pixel centre) over `area` and keep the coverage.
    pub fn shape(&mut self, area: IBox, sd: impl Fn(Pt) -> f32) {
        let a = area.intersect(&self.b);
        for y in a.y0..a.y1 {
            for x in a.x0..a.x1 {
                self.put(x, y, cov_from_sd(sd(Pt::new(x as f32 + 0.5, y as f32 + 0.5))));
            }
        }
    }

    /// A line from `a` to `b` with round ends, `width` wide.
    pub fn capsule(&mut self, a: Pt, b: Pt, width: f32) {
        let r = width / 2.0;
        let area = IBox::around(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y), r + 1.0);
        self.shape(area, |p| dist_to_segment(p, a, b) - r);
    }

    /// A line with flat ends (the stroke stops exactly at `a` and `b`).
    pub fn bar(&mut self, a: Pt, b: Pt, width: f32) {
        let d = b.minus(a);
        let len = d.len();
        if len < 1e-3 {
            return;
        }
        let u = d.scale(1.0 / len);
        let n = Pt::new(-u.y, u.x).scale(width / 2.0);
        self.convex(&[a.plus(n), b.plus(n), b.minus(n), a.minus(n)]);
    }

    pub fn polyline(&mut self, pts: &[Pt], width: f32) {
        match pts {
            [] => {}
            [p] => self.disc(*p, width / 2.0),
            _ => {
                for w in pts.windows(2) {
                    self.capsule(w[0], w[1], width);
                }
            }
        }
    }

    pub fn disc(&mut self, c: Pt, r: f32) {
        let area = IBox::around(c.x, c.y, c.x, c.y, r + 1.0);
        self.shape(area, |p| p.dist(c) - r);
    }

    /// A circle outline of radius `r` (to the middle of the stroke).
    pub fn ring(&mut self, c: Pt, r: f32, width: f32) {
        let area = IBox::around(c.x, c.y, c.x, c.y, r + width / 2.0 + 1.0);
        self.shape(area, |p| (p.dist(c) - r).abs() - width / 2.0);
    }

    /// A filled convex polygon (any winding).
    pub fn convex(&mut self, pts: &[Pt]) {
        if pts.len() < 3 {
            return;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in pts {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        // Orientation from the signed area, so the edge normals point outwards.
        let mut area2 = 0.0;
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            area2 += a.x * b.y - b.x * a.y;
        }
        let sign = if area2 >= 0.0 { 1.0 } else { -1.0 };
        let edges: Vec<(Pt, Pt)> = (0..pts.len())
            .filter_map(|i| {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                let d = b.minus(a);
                let l = d.len();
                (l > 1e-6).then(|| (a, Pt::new(d.y, -d.x).scale(sign / l)))
            })
            .collect();
        self.shape(IBox::around(x0, y0, x1, y1, 1.0), |p| edges.iter().map(|(a, n)| p.minus(*a).dot(*n)).fold(f32::MIN, f32::max));
    }

    /// The outline of a box (to the middle of the stroke), square outer corners.
    pub fn box_outline(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32) {
        let (c, half) = box_center_half(x0, y0, x1, y1);
        let area = IBox::around(x0, y0, x1, y1, width / 2.0 + 1.0);
        self.shape(area, |p| {
            let d = sd_box(p.minus(c), half);
            let inner = -d - width / 2.0; // inside the hole: positive
            let outer = sd_box_square(p.minus(c), Pt::new(half.x + width / 2.0, half.y + width / 2.0));
            outer.max(inner)
        });
    }

    /// The outline of the ellipse with centre `c` and radii `r` (to the middle of the stroke).
    pub fn ellipse_outline(&mut self, c: Pt, r: Pt, width: f32) {
        let area = IBox::around(c.x - r.x, c.y - r.y, c.x + r.x, c.y + r.y, width / 2.0 + 1.0);
        self.shape(area, |p| sd_ellipse(p.minus(c), r).abs() - width / 2.0);
    }

    /// A filled box with rounded corners of radius `r`.
    pub fn rounded_box(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) {
        let (c, half) = box_center_half(x0, y0, x1, y1);
        let r = r.min(half.x).min(half.y).max(0.0);
        self.shape(IBox::around(x0, y0, x1, y1, 1.0), |p| sd_box(p.minus(c), Pt::new(half.x - r, half.y - r)) - r);
    }

    /// A rounded box outline, `width` wide, drawn inside the box edge.
    pub fn rounded_box_outline(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32, width: f32) {
        let (c, half) = box_center_half(x0, y0, x1, y1);
        let r = r.min(half.x).min(half.y).max(0.0);
        self.shape(IBox::around(x0, y0, x1, y1, 1.0), |p| {
            let d = sd_box(p.minus(c), Pt::new(half.x - r, half.y - r)) - r;
            d.max(-(d + width))
        });
    }
}

fn box_center_half(x0: f32, y0: f32, x1: f32, y1: f32) -> (Pt, Pt) {
    let (l, r) = (x0.min(x1), x0.max(x1));
    let (t, b) = (y0.min(y1), y0.max(y1));
    (Pt::new((l + r) / 2.0, (t + b) / 2.0), Pt::new((r - l) / 2.0, (b - t) / 2.0))
}

/// Exact signed distance to an axis-aligned box centred at the origin.
pub fn sd_box(p: Pt, half: Pt) -> f32 {
    let q = Pt::new(p.x.abs() - half.x, p.y.abs() - half.y);
    let outside = Pt::new(q.x.max(0.0), q.y.max(0.0)).len();
    outside + q.x.max(q.y).min(0.0)
}

/// Signed distance to an axis-aligned ellipse centred at the origin with radii `r`: the implicit
/// function divided by its gradient, which is exact on the curve and close to it within a stroke
/// width. A flat ellipse (a radius under half a pixel) is a line.
pub fn sd_ellipse(p: Pt, r: Pt) -> f32 {
    let (rx, ry) = (r.x.max(0.5), r.y.max(0.5));
    let f = (p.x * p.x) / (rx * rx) + (p.y * p.y) / (ry * ry) - 1.0;
    let gx = 2.0 * p.x / (rx * rx);
    let gy = 2.0 * p.y / (ry * ry);
    let g = (gx * gx + gy * gy).sqrt();
    if g < 1e-6 { -rx.min(ry) } else { f / g }
}

/// Chebyshev distance to a box: the outside corners stay square.
fn sd_box_square(p: Pt, half: Pt) -> f32 {
    (p.x.abs() - half.x).max(p.y.abs() - half.y)
}

pub fn dist_to_segment(p: Pt, a: Pt, b: Pt) -> f32 {
    let ab = b.minus(a);
    let l2 = ab.dot(ab);
    if l2 < 1e-12 {
        return p.dist(a);
    }
    let t = (p.minus(a).dot(ab) / l2).clamp(0.0, 1.0);
    p.dist(a.plus(ab.scale(t)))
}

/// A rendered piece of text: coverage over `w` x `h`, with the baseline `ascent` pixels down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextMask {
    pub w: u32,
    pub h: u32,
    pub ascent: u32,
    pub cov: Vec<u8>,
}

/// Turns text into coverage. Windows: GDI with grayscale antialiasing (`gdi.rs`); tests: `BoxFont`.
pub trait Glyphs {
    /// `px`: the em size in pixels. Multi-line text is split on '\n' by the caller.
    fn text(&self, s: &str, px: f32, bold: bool) -> TextMask;

    /// Use the actual font's line height, also used by the native Windows edit control.
    fn line_height(&self, px: f32) -> f32 {
        self.text("Mg", px, false).h as f32
    }
}

/// A test font: every character is a filled box 0.6 em wide and 0.7 em high, spaces are empty.
/// Deterministic on every OS, so renderer tests do not depend on installed fonts.
pub struct BoxFont;

impl Glyphs for BoxFont {
    fn text(&self, s: &str, px: f32, _bold: bool) -> TextMask {
        let cw = (px * 0.6).round().max(1.0) as u32;
        let h = (px * 1.25).round().max(1.0) as u32;
        let ascent = (px * 0.95).round() as u32;
        let n = s.chars().count() as u32;
        let w = (cw * n).max(1);
        let mut cov = vec![0u8; (w * h) as usize];
        let top = ascent.saturating_sub((px * 0.7).round() as u32);
        for (i, ch) in s.chars().enumerate() {
            if ch == ' ' {
                continue;
            }
            for y in top..ascent.min(h) {
                for x in (i as u32 * cw + 1)..((i as u32 + 1) * cw).saturating_sub(1).max(i as u32 * cw + 1) {
                    cov[(y * w + x) as usize] = 255;
                }
            }
        }
        TextMask { w, h, ascent, cov }
    }
}

impl Canvas {
    /// Blend a text mask with its top-left at (`x`, `y`).
    pub fn text_mask(&mut self, t: &TextMask, x: i32, y: i32, c: Rgba) {
        let m = Mask { b: IBox::xywh(x, y, t.w as i32, t.h as i32), cov: t.cov.clone() };
        self.blend_mask(&m, c);
    }

    /// Draw one line of text with its top-left at (`x`, `y`); returns the text's width.
    #[allow(clippy::too_many_arguments)]
    pub fn text(&mut self, g: &dyn Glyphs, s: &str, px: f32, bold: bool, x: i32, y: i32, c: Rgba) -> u32 {
        if s.is_empty() {
            return 0;
        }
        let t = g.text(s, px, bold);
        self.text_mask(&t, x, y, c);
        t.w
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Rgba = Rgba::rgb(255, 255, 255);
    const RED: Rgba = Rgba::rgb(255, 0, 0);

    #[test]
    fn ellipse_distance_is_exact_on_a_circle_and_zero_on_the_curve() {
        let r = Pt::new(20.0, 20.0);
        for p in [Pt::new(25.0, 0.0), Pt::new(0.0, 18.0), Pt::new(-14.0, 14.0)] {
            assert!((sd_ellipse(p, r) - (p.len() - 20.0)).abs() < 0.6, "{p:?}");
        }
        let e = Pt::new(40.0, 10.0);
        for k in 0..12 {
            let a = k as f32 * 0.5;
            let on = Pt::new(e.x * a.cos(), e.y * a.sin());
            assert!(sd_ellipse(on, e).abs() < 1e-3, "{on:?}");
        }
        assert!(sd_ellipse(Pt::new(0.0, 0.0), e) < 0.0, "the centre is inside");
    }

    #[test]
    fn hex_round_trip_and_errors() {
        assert_eq!(Rgba::from_hex("#E5484D"), Some(Rgba::rgb(0xE5, 0x48, 0x4D)));
        assert_eq!(Rgba::from_hex("#e5484d80").unwrap().a, 0x80);
        assert_eq!(Rgba::rgb(1, 2, 3).to_hex(), "#010203");
        let bad = ["E5484D", "#E548", "#GG0000", "#ççç", ""];
        assert!(!bad.is_empty());
        for b in bad {
            assert_eq!(Rgba::from_hex(b), None, "{b}");
        }
    }

    #[test]
    fn contrast_of_black_on_white_is_21() {
        let c = Rgba::rgb(0, 0, 0).contrast(WHITE);
        assert!((c - 21.0).abs() < 0.01, "{c}");
    }

    #[test]
    fn capsule_covers_its_middle_and_not_far_away() {
        let mut m = Mask::new(IBox::new(0, 0, 40, 20));
        m.capsule(Pt::new(5.0, 10.0), Pt::new(35.0, 10.0), 5.0);
        assert_eq!(m.at(20, 9), 255);
        assert_eq!(m.at(20, 10), 255);
        assert_eq!(m.at(20, 15), 0);
        assert_eq!(m.at(39, 10), 0, "beyond the round cap");
        // The edge pixel is partly covered (antialiased).
        let edge = m.at(20, 7);
        assert!(edge > 0 && edge < 255, "{edge}");
    }

    #[test]
    fn crossing_stroke_does_not_double_its_alpha() {
        let mut c = Canvas::new(30, 30, WHITE);
        let mut m = Mask::new(c.bounds());
        m.polyline(&[Pt::new(2.0, 15.0), Pt::new(28.0, 15.0), Pt::new(15.0, 2.0), Pt::new(15.0, 28.0)], 4.0);
        c.blend_mask(&m, RED.with_alpha(128));
        // The crossing point and a single-stroke point get the same colour.
        assert_eq!(c.get(15, 15), c.get(5, 15));
    }

    #[test]
    fn convex_polygon_winding_does_not_matter() {
        let tri = [Pt::new(2.0, 2.0), Pt::new(18.0, 2.0), Pt::new(10.0, 18.0)];
        let mut a = Mask::new(IBox::new(0, 0, 20, 20));
        let mut b = Mask::new(IBox::new(0, 0, 20, 20));
        a.convex(&tri);
        b.convex(&[tri[2], tri[1], tri[0]]);
        assert_eq!(a.cov, b.cov);
        assert_eq!(a.at(10, 6), 255);
        assert_eq!(a.at(1, 17), 0);
    }

    #[test]
    fn box_outline_is_hollow_with_square_corners() {
        let mut m = Mask::new(IBox::new(0, 0, 40, 40));
        m.box_outline(10.0, 10.0, 30.0, 30.0, 4.0);
        assert_eq!(m.at(20, 20), 0, "hollow");
        assert_eq!(m.at(10, 20), 255, "left edge");
        assert_eq!(m.at(8, 8), 255, "outer corner is square");
        assert_eq!(m.at(6, 6), 0);
    }

    #[test]
    fn rounded_box_cuts_its_corners() {
        let mut m = Mask::new(IBox::new(0, 0, 40, 40));
        m.rounded_box(0.0, 0.0, 40.0, 40.0, 10.0);
        assert_eq!(m.at(20, 20), 255);
        assert_eq!(m.at(0, 0), 0);
        assert_eq!(m.at(0, 20), 255);
    }

    #[test]
    fn multiply_keeps_black_and_tints_white() {
        let mut c = Canvas::new(2, 1, WHITE);
        c.set(1, 0, Rgba::rgb(0, 0, 0));
        let yellow = Rgba::rgb(255, 220, 0);
        c.multiply(0, 0, yellow, 1.0);
        c.multiply(1, 0, yellow, 1.0);
        assert_eq!(c.get(0, 0), yellow);
        assert_eq!(c.get(1, 0), Rgba::rgb(0, 0, 0));
    }

    #[test]
    fn pixelate_averages_each_block() {
        let mut c = Canvas::new(4, 2, WHITE);
        c.set(0, 0, Rgba::rgb(0, 0, 0));
        c.set(1, 1, Rgba::rgb(0, 0, 0));
        c.pixelate(c.bounds(), 2);
        // Left block: two black, two white -> 127 grey; right block stays white.
        assert_eq!(c.get(0, 0), Rgba::rgb(127, 127, 127));
        assert_eq!(c.get(1, 1), Rgba::rgb(127, 127, 127));
        assert_eq!(c.get(3, 1), WHITE);
    }

    #[test]
    fn blur_stays_inside_its_box() {
        let mut c = Canvas::new(20, 1, WHITE);
        for x in 0..10 {
            c.set(x, 0, Rgba::rgb(0, 0, 0));
        }
        let before = c.clone();
        c.blur(IBox::new(5, 0, 15, 1), 2);
        assert_eq!(c.get(4, 0), before.get(4, 0), "outside left untouched");
        assert_eq!(c.get(15, 0), before.get(15, 0), "outside right untouched");
        let mid = c.get(10, 0).r;
        assert!(mid > 0 && mid < 255, "the edge inside is softened: {mid}");
    }

    #[test]
    fn downscale_keeps_aspect_and_averages() {
        let mut c = Canvas::new(400, 200, WHITE);
        c.fill_box(IBox::new(0, 0, 200, 200), Rgba::rgb(0, 0, 0));
        let t = c.downscale_to_fit(100, 100);
        assert_eq!((t.w, t.h), (100, 50));
        assert_eq!(t.get(10, 10), Rgba::rgb(0, 0, 0));
        assert_eq!(t.get(90, 10), WHITE);
        let small = Canvas::new(10, 10, WHITE).downscale_to_fit(100, 100);
        assert_eq!((small.w, small.h), (10, 10), "never upscales");
    }

    #[test]
    fn premultiplied_scales_colour_by_alpha() {
        let c = Canvas::new(1, 1, Rgba { r: 200, g: 100, b: 50, a: 128 });
        assert_eq!(c.premultiplied(), vec![25, 50, 100, 128]);
    }

    #[test]
    fn crop_clips_to_the_canvas() {
        let mut c = Canvas::new(10, 10, WHITE);
        c.set(9, 9, RED);
        let k = c.crop(IBox::new(8, 8, 20, 20));
        assert_eq!((k.w, k.h), (2, 2));
        assert_eq!(k.get(1, 1), RED);
    }

    #[test]
    fn box_font_is_deterministic_and_draws() {
        let t = BoxFont.text("ab c", 20.0, false);
        assert_eq!(t.w, 12 * 4);
        assert!(t.cov.contains(&255));
        let mut c = Canvas::new(60, 30, WHITE);
        let w = c.text(&BoxFont, "ab", 20.0, false, 0, 0, RED);
        assert_eq!(w, 24);
        assert_eq!(c.get(5, 12), RED);
    }
}
