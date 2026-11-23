//! The delay timer: "snip in 3 s" / "snip in 5 s". A countdown badge shows the seconds left and
//! hides itself before the screen is frozen, so it never ends up in the capture.

use crate::raster::{Canvas, Glyphs, Mask, Pt, Rgba};
use crate::theme::Theme;

pub const CHOICES: [u32; 2] = [3, 5];

/// Accepts 3 and 5 only (the tray offers these; the command line refuses others).
pub fn parse_seconds(s: &str) -> Option<u32> {
    s.trim().trim_end_matches('s').parse::<u32>().ok().filter(|n| CHOICES.contains(n))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Countdown {
    pub start_ms: u64,
    pub seconds: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    /// Show this many seconds.
    Show(u32),
    /// Hide the badge now; the freeze follows on the next tick (one frame later).
    Hide,
    Fire,
}

/// How long before the freeze the badge disappears (lets the compositor remove it from screen).
pub const HIDE_MS: u64 = 120;

impl Countdown {
    pub fn new(start_ms: u64, seconds: u32) -> Self {
        Self { start_ms, seconds }
    }

    pub fn due_ms(&self) -> u64 {
        self.start_ms + self.seconds as u64 * 1000
    }

    pub fn tick(&self, now_ms: u64) -> Tick {
        let due = self.due_ms();
        if now_ms >= due {
            Tick::Fire
        } else if now_ms + HIDE_MS >= due {
            Tick::Hide
        } else {
            Tick::Show(((due - now_ms) as f32 / 1000.0).ceil() as u32)
        }
    }
}

/// The badge: a disc with the number, `d` DIPs across.
pub fn render_badge(n: u32, t: &Theme, scale: f32, g: &dyn Glyphs) -> Canvas {
    let d = (crate::theme::COUNTDOWN_SIZE * scale).round();
    let mut cv = Canvas::new(d as u32, d as u32, Rgba::default());
    let c = Pt::new(d / 2.0, d / 2.0);
    let mut m = Mask::new(cv.bounds());
    m.disc(c, d / 2.0);
    cv.blend_mask(&m, t.border);
    let mut inner = Mask::new(cv.bounds());
    inner.disc(c, d / 2.0 - (crate::theme::BORDER_WIDTH * scale).max(1.0));
    cv.blend_mask(&inner, t.bg);
    let tm = g.text(&n.to_string(), crate::theme::COUNTDOWN_FONT * scale, true);
    cv.text_mask(&tm, ((d - tm.w as f32) / 2.0).round() as i32, ((d - tm.h as f32) / 2.0).round() as i32, t.text);
    cv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_shows_hides_then_fires() {
        let c = Countdown::new(1000, 3);
        let cases = [(1000, Tick::Show(3)), (1999, Tick::Show(3)), (2000, Tick::Show(2)), (3500, Tick::Show(1)), (3880, Tick::Hide), (3999, Tick::Hide), (4000, Tick::Fire), (9000, Tick::Fire)];
        assert!(!cases.is_empty());
        for (now, want) in cases {
            assert_eq!(c.tick(now), want, "at {now}");
        }
    }

    #[test]
    fn only_three_and_five_seconds() {
        assert_eq!(parse_seconds("3"), Some(3));
        assert_eq!(parse_seconds("5s"), Some(5));
        for bad in ["0", "4", "10", "", "x"] {
            assert_eq!(parse_seconds(bad), None, "{bad}");
        }
    }

    #[test]
    fn badge_is_round_and_scales() {
        let t = Theme::new(crate::theme::Look::Graphite, crate::theme::Mode::Light);
        let b = render_badge(3, &t, 1.5, &crate::raster::BoxFont);
        assert_eq!((b.w, b.h), (84, 84));
        assert_eq!(b.get(0, 0).a, 0);
        assert_eq!(b.get(42, 6).a, 255);
    }
}
