//! Long straight separators in frozen BGRA pixels. No OS calls and no image contents retained.
use crate::geometry::{Point, Rect};

const THRESHOLD: u16 = 60;
#[derive(Clone, Debug)]
pub struct Pane {
    pub rect: Rect,
    pub children: Vec<Pane>,
}
impl Pane {
    pub fn chain(&self, point: Point) -> Vec<Rect> {
        if !self.rect.contains(point) {
            return Vec::new();
        }
        let mut chain = vec![self.rect];
        if let Some(child) = self.children.iter().find(|c| c.rect.contains(point)) {
            chain.extend(child.chain(point));
        }
        chain
    }
}
#[derive(Clone, Debug)]
pub struct Split {
    pub root: Pane,
    pub separators: usize,
}
struct Image {
    pixels: Vec<[u8; 3]>,
    width: usize,
    scale: i32,
    origin: Rect,
}
impl Image {
    fn pixel(&self, x: i32, y: i32) -> [u8; 3] {
        self.pixels[y as usize * self.width + x as usize]
    }
    fn diff(a: [u8; 3], b: [u8; 3]) -> u16 {
        a.into_iter()
            .zip(b)
            .map(|(x, y)| x.abs_diff(y) as u16)
            .sum()
    }
    fn physical(&self, r: Rect) -> Rect {
        Rect::new(
            self.origin.left + r.left * self.scale,
            self.origin.top + r.top * self.scale,
            (self.origin.left + r.right * self.scale).min(self.origin.right),
            (self.origin.top + r.bottom * self.scale).min(self.origin.bottom),
        )
    }
    fn split(&self, r: Rect, depth: usize, count: &mut usize) -> Pane {
        let mut pane = Pane {
            rect: self.physical(r),
            children: Vec::new(),
        };
        if depth == 3 {
            return pane;
        }
        let min = (48 + self.scale - 1) / self.scale;
        let mut best = None;
        for vertical in [true, false] {
            let (start, end, along0, along1) = if vertical {
                (r.left, r.right, r.top, r.bottom)
            } else {
                (r.top, r.bottom, r.left, r.right)
            };
            let extent = along1 - along0;
            for cut in start + min..=end - min {
                let physical_extent = if vertical {
                    pane.rect.width()
                } else {
                    pane.rect.height()
                };
                let first_side = (cut - start) * self.scale;
                // A ceil-sized downsample block can end one pixel beyond an odd window edge.
                if first_side < 48 || physical_extent - first_side < 48 {
                    continue;
                }
                let mut hits = 0;
                let mut run = 0;
                let mut longest = 0;
                let mut strength = 0u64;
                let mut misses = 0;
                for a in along0..along1 {
                    let px = |b| {
                        if vertical {
                            self.pixel(b, a)
                        } else {
                            self.pixel(a, b)
                        }
                    };
                    let before = px(cut - 1);
                    let step = Self::diff(before, px(cut + 1));
                    let mut d = step;
                    if d <= THRESHOLD {
                        let ink = px(cut);
                        let left = Self::diff(ink, before);
                        if left > THRESHOLD {
                            for thickness in 1..=3 {
                                if cut + thickness >= end {
                                    break;
                                }
                                if thickness > 1
                                    && Self::diff(ink, px(cut + thickness - 1)) > THRESHOLD / 2
                                {
                                    break;
                                }
                                let stripe = left.min(Self::diff(ink, px(cut + thickness)));
                                d = d.max(stripe);
                                if d > THRESHOLD {
                                    break;
                                }
                            }
                        }
                    }
                    if d > THRESHOLD {
                        hits += 1;
                        run += 1;
                        longest = longest.max(run);
                        strength += u64::from(d);
                    } else {
                        run = 0;
                        misses += 1;
                        // Exact early rejection: even all remaining pixels cannot reach 85%.
                        if misses * 100 > extent * 15 {
                            break;
                        }
                    }
                }
                if hits * 100 < extent * 85 || longest * 100 < extent * 60 {
                    continue;
                }
                let score = strength * 1000 / extent as u64;
                if best.is_none_or(|(s, _, _)| score > s) {
                    best = Some((score, vertical, cut));
                }
            }
        }
        if let Some((_, vertical, cut)) = best {
            *count += 1;
            let (a, b) = if vertical {
                (
                    Rect::new(r.left, r.top, cut, r.bottom),
                    Rect::new(cut, r.top, r.right, r.bottom),
                )
            } else {
                (
                    Rect::new(r.left, r.top, r.right, cut),
                    Rect::new(r.left, cut, r.right, r.bottom),
                )
            };
            pane.children = vec![
                self.split(a, depth + 1, count),
                self.split(b, depth + 1, count),
            ];
        }
        pane
    }
}
/// Invalid/short input yields the whole window and no separators. Stride may contain padding.
pub fn split(bgra: &[u8], stride: usize, window: Rect) -> Split {
    let empty = || Split {
        root: Pane {
            rect: window,
            children: Vec::new(),
        },
        separators: 0,
    };
    let (w, h) = (window.width() as usize, window.height() as usize);
    if w == 0
        || h == 0
        || stride < w.saturating_mul(4)
        || stride.checked_mul(h).is_none_or(|n| n > bgra.len())
    {
        return empty();
    }
    let scale = if w > 1600 { 2 } else { 1 };
    let (sw, sh) = (w.div_ceil(scale), h.div_ceil(scale));
    let mut pixels = Vec::with_capacity(sw * sh);
    for y in 0..sh {
        for x in 0..sw {
            let mut sums = [0u16; 3];
            let mut n = 0;
            for dy in 0..scale {
                for dx in 0..scale {
                    let (sx, sy) = (x * scale + dx, y * scale + dy);
                    if sx >= w || sy >= h {
                        continue;
                    }
                    let at = sy * stride + sx * 4;
                    for c in 0..3 {
                        sums[c] += u16::from(bgra[at + c]);
                    }
                    n += 1;
                }
            }
            pixels.push(sums.map(|s| (s / n) as u8));
        }
    }
    let image = Image {
        pixels,
        width: sw,
        scale: scale as i32,
        origin: window,
    };
    let mut separators = 0;
    let root = image.split(
        Rect::from_xywh(0, 0, sw as i32, sh as i32),
        0,
        &mut separators,
    );
    Split { root, separators }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(w: usize, h: usize, f: impl Fn(usize, usize) -> u8) -> Vec<u8> {
        let mut v = vec![255; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let at = (y * w + x) * 4;
                v[at..at + 3].fill(f(x, y));
            }
        }
        v
    }
    #[test]
    fn whatsapp_columns_header_bubbles_and_negative_origin() {
        let v = fixture(1000, 800, |x, y| {
            if x == 300 {
                200
            } else if x < 300 {
                35
            } else if y < 100 {
                90
            } else if (400..650).contains(&x) && (250..330).contains(&y) {
                110
            } else {
                10
            }
        });
        let s = split(&v, 4000, Rect::from_xywh(-100, 20, 1000, 800));
        let c = s.root.chain(Point::new(700, 500));
        assert_eq!(s.separators, 2);
        assert_eq!(c.len(), 3);
        assert!(
            (c[1].left - 200).abs() <= 1 && (c[2].top - 120).abs() <= 1,
            "{c:?}"
        );
        assert_eq!(s.root.chain(Point::new(-50, 500)).len(), 2);
        assert!(s.root.chain(Point::new(900, 500)).is_empty());
    }
    #[test]
    fn thin_stripes_on_equal_colours_and_horizontal_stripes() {
        for thickness in 1..=3 {
            for vertical in [true, false] {
                let v = fixture(400, 300, |x, y| {
                    if (150..150 + thickness).contains(&if vertical { x } else { y }) {
                        240
                    } else {
                        30
                    }
                });
                assert_eq!(
                    split(&v, 1600, Rect::from_xywh(0, 0, 400, 300)).separators,
                    1
                );
            }
        }
    }
    #[test]
    fn glyph_blobs_photo_gradient_short_lines_and_bad_stride_have_no_separator() {
        for photo in [false, true] {
            let v = fixture(700, 500, |x, y| {
                if photo {
                    ((x + y) / 6) as u8
                } else if x % 19 < 7 && y % 17 < 10 {
                    180
                } else {
                    20
                }
            });
            assert_eq!(
                split(&v, 2800, Rect::from_xywh(0, 0, 700, 500)).separators,
                0
            );
        }
        let v = fixture(400, 300, |x, y| if x == 150 && y < 230 { 240 } else { 30 });
        assert_eq!(
            split(&v, 1600, Rect::from_xywh(0, 0, 400, 300)).separators,
            0
        );
        assert_eq!(split(&[], 2, Rect::from_xywh(0, 0, 400, 300)).separators, 0);
    }
    #[test]
    fn odd_downsample_bounds_keep_both_panes_at_least_48_physical_pixels() {
        let image = Image {
            pixels: vec![[30, 30, 30]; 801 * 75],
            width: 801,
            scale: 2,
            origin: Rect::new(0, 0, 1601, 149),
        };
        let mut image = image;
        // A stronger stripe at downsample row 52 must not leave a 47 px bottom pane.
        for x in 0..801 {
            image.pixels[52 * 801 + x] = [240; 3];
        }
        let mut count = 0;
        let tree = image.split(Rect::new(0, 0, 801, 75), 0, &mut count);
        fn check(p: &Pane) {
            for child in &p.children {
                assert!(
                    child.rect.width() >= 48 && child.rect.height() >= 48,
                    "{:?}",
                    child.rect
                );
                check(child);
            }
        }
        check(&tree);
    }
    #[test]
    fn one_core_2560_1440_budget_and_downsampling() {
        let v = fixture(2560, 1440, |x, y| {
            if x == 701 {
                250
            } else if x < 701 {
                30
            } else if y < 120 {
                100
            } else {
                10
            }
        });
        let t = std::time::Instant::now();
        let s = split(&v, 2560 * 4, Rect::from_xywh(0, 0, 2560, 1440));
        let ms = t.elapsed().as_secs_f64() * 1000.;
        println!(
            "edges 2560x1440 one core: {ms:.3} ms, {} separators",
            s.separators
        );
        assert!(s.separators >= 2);
        assert!(ms < 120., "edges exceeded the time budget: {ms:.3} ms");
    }
}
