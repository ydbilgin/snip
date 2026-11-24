//! The picture frame: the editor's Frame button puts the finished picture on a quiet ground with
//! rounded corners and a soft shadow, ready for a post, a README or a slide. It changes the copied,
//! saved and pinned picture only; the frozen screen and the annotations are untouched.

use crate::raster::{Canvas, IBox, Mask, Pt, Rgba, sd_box};
use crate::theme;

/// How the frame is laid out for a picture: padding on every side, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameLayout {
    pub pad: i32,
    pub radius: f32,
    pub blur: f32,
    pub drop: f32,
}

impl FrameLayout {
    /// The padding follows the picture's shorter side, within limits scaled by the monitor's
    /// `scale`, so a small snip does not drown in its frame and a large one does not look cramped.
    pub fn for_size(w: u32, h: u32, scale: f32) -> FrameLayout {
        let short = w.min(h) as f32;
        let pad = (short * theme::FRAME_PAD_SHARE).clamp(theme::FRAME_PAD_MIN * scale, theme::FRAME_PAD_MAX * scale).round() as i32;
        let radius = (theme::FRAME_RADIUS * scale).min(short / 4.0);
        FrameLayout { pad, radius, blur: theme::FRAME_SHADOW_BLUR * scale, drop: theme::FRAME_SHADOW_Y * scale }
    }

    pub fn outer(&self, w: u32, h: u32) -> (u32, u32) {
        (w + 2 * self.pad as u32, h + 2 * self.pad as u32)
    }
}

fn lerp(a: Rgba, b: Rgba, k: f32) -> Rgba {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
    Rgba::rgb(f(a.r, b.r), f(a.g, b.g), f(a.b, b.b))
}

/// `img` on the frame's ground; the result is opaque and `2 * pad` larger in each direction.
pub fn apply(img: &Canvas, scale: f32) -> Canvas {
    let lay = FrameLayout::for_size(img.w, img.h, scale);
    let (ow, oh) = lay.outer(img.w, img.h);
    let mut out = Canvas::new(ow, oh, theme::FRAME_TOP);
    // The ground: a vertical wash between two close slates (no banding worth dithering at 8 bit).
    for y in 0..oh as i32 {
        let c = lerp(theme::FRAME_TOP, theme::FRAME_BOTTOM, y as f32 / (oh.max(2) - 1) as f32);
        out.fill_box(IBox::new(0, y, ow as i32, y + 1), c);
    }
    let (x0, y0) = (lay.pad as f32, lay.pad as f32);
    let (x1, y1) = (x0 + img.w as f32, y0 + img.h as f32);
    // The shadow: a smooth falloff around the picture's rounded box, moved down a little.
    let half = Pt::new(img.w as f32 / 2.0 - lay.radius, img.h as f32 / 2.0 - lay.radius);
    let centre = Pt::new((x0 + x1) / 2.0, (y0 + y1) / 2.0 + lay.drop);
    let peak = theme::FRAME_SHADOW_ALPHA as f32;
    let blur = lay.blur.max(1.0);
    let inner = IBox::new(lay.pad + lay.radius.ceil() as i32, lay.pad + lay.radius.ceil() as i32, lay.pad + img.w as i32 - lay.radius.ceil() as i32, lay.pad + img.h as i32 - lay.radius.ceil() as i32);
    for y in 0..oh as i32 {
        for x in 0..ow as i32 {
            if inner.contains(x, y) {
                continue; // under the picture: covered below
            }
            let p = Pt::new(x as f32 + 0.5 - centre.x, y as f32 + 0.5 - centre.y);
            let d = sd_box(p, Pt::new(half.x.max(0.0), half.y.max(0.0))) - lay.radius;
            let k = (1.0 - (d + blur * 0.3) / blur).clamp(0.0, 1.0);
            let a = peak * k * k * (3.0 - 2.0 * k);
            if a >= 1.0 {
                out.blend(x, y, theme::DIM_COLOR.with_alpha(a as u8), 1.0);
            }
        }
    }
    // The picture, clipped to its rounded box with antialiased corners.
    let mut m = Mask::new(IBox::new(lay.pad, lay.pad, lay.pad + img.w as i32, lay.pad + img.h as i32));
    m.rounded_box(x0, y0, x1, y1, lay.radius);
    for y in 0..img.h as i32 {
        for x in 0..img.w as i32 {
            let k = m.at(x + lay.pad, y + lay.pad);
            if k == 255 {
                out.set(x + lay.pad, y + lay.pad, img.get(x, y).with_alpha(255));
            } else if k > 0 {
                out.blend(x + lay.pad, y + lay.pad, img.get(x, y).with_alpha(255), k as f32 / 255.0);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: u32, h: u32) -> Canvas {
        Canvas::new(w, h, Rgba::rgb(250, 250, 250))
    }

    #[test]
    fn padding_follows_the_short_side_within_limits() {
        assert_eq!(FrameLayout::for_size(100, 80, 1.0).pad, 28, "small: the minimum");
        assert_eq!(FrameLayout::for_size(1000, 800, 1.0).pad, 56, "7 % of 800");
        assert_eq!(FrameLayout::for_size(4000, 3000, 1.0).pad, 96, "large: the maximum");
        assert_eq!(FrameLayout::for_size(100, 80, 2.0).pad, 56, "limits scale with the monitor");
    }

    #[test]
    fn the_picture_sits_in_the_middle_with_rounded_corners() {
        let img = picture(300, 200);
        let out = apply(&img, 1.0);
        let pad = FrameLayout::for_size(300, 200, 1.0).pad;
        assert_eq!((out.w, out.h), (300 + 2 * pad as u32, 200 + 2 * pad as u32));
        assert_eq!(out.get(pad + 150, pad + 100), img.get(150, 100), "the middle is the picture");
        assert_ne!(out.get(pad, pad), img.get(0, 0), "the corner pixel is rounded off");
        assert!(out.px.chunks(4).all(|p| p[3] == 255), "the result is opaque");
    }

    #[test]
    fn the_shadow_falls_below_the_picture() {
        let out = apply(&picture(300, 200), 1.0);
        let pad = FrameLayout::for_size(300, 200, 1.0).pad;
        let mid = pad + 150;
        let above = out.get(mid, pad - 6).luminance();
        let below = out.get(mid, pad + 200 + 6).luminance();
        let ground = lerp(theme::FRAME_TOP, theme::FRAME_BOTTOM, (pad + 206) as f32 / (out.h - 1) as f32).luminance();
        assert!(below < ground, "darker than the ground under the picture");
        assert!(below < above || above < ground, "the shadow is heavier below");
    }

    #[test]
    fn the_ground_is_calm() {
        let top = theme::FRAME_TOP;
        let bottom = theme::FRAME_BOTTOM;
        assert!(top.contrast(bottom) < 1.6, "a wash, not a gradient show");
        // White and black pictures both stand out from it.
        assert!(Rgba::rgb(255, 255, 255).contrast(top) > 4.0);
        assert!(Rgba::rgb(0, 0, 0).contrast(bottom) > 1.2);
    }
}
