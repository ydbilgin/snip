//! Monitors with their scale, for mixed-DPI desktops.
//!
//! The process is per-monitor-v2 DPI aware, so Windows reports every rectangle in physical pixels
//! and the capture is 1:1. What changes per monitor is how big snip's own UI must be: a 32-DIP
//! toolbar button is 32 px on a 100 % monitor and 48 px on a 150 % one. Everything here works in
//! physical pixels in virtual-desktop coordinates (negative x and y are normal).

use crate::geometry::{Layout, Point, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub rect: Rect,
    /// The work area (without the taskbar).
    pub work: Rect,
    /// Effective DPI: 96 = 100 %, 144 = 150 %.
    pub dpi: u32,
}

impl Monitor {
    pub fn new(rect: Rect, dpi: u32) -> Self {
        Self { rect, work: rect, dpi }
    }

    pub fn scale(&self) -> f32 {
        scale(self.dpi)
    }

    /// DIPs to physical pixels on this monitor.
    pub fn px(&self, dip: f32) -> i32 {
        px(dip, self.dpi)
    }
}

pub fn scale(dpi: u32) -> f32 {
    if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 }
}

pub fn px(dip: f32, dpi: u32) -> i32 {
    (dip * scale(dpi)).round() as i32
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Monitors {
    pub list: Vec<Monitor>,
}

impl Monitors {
    pub fn new(list: Vec<Monitor>) -> Self {
        Self { list }
    }

    pub fn layout(&self) -> Layout {
        Layout::new(self.list.iter().map(|m| m.rect).collect())
    }

    /// The monitor that holds `p`, else the nearest one (MonitorFromPoint with MONITOR_DEFAULTTONEAREST).
    pub fn at(&self, p: Point) -> Option<&Monitor> {
        self.layout().nearest_monitor(p).map(|i| &self.list[i])
    }

    /// The monitor with the largest part of `r`; the nearest to its centre when it touches none
    /// (MonitorFromRect with MONITOR_DEFAULTTONEAREST).
    pub fn for_rect(&self, r: &Rect) -> Option<&Monitor> {
        let best = self.list.iter().enumerate().filter_map(|(i, m)| m.rect.intersect(r).map(|x| (i, x.area()))).max_by_key(|&(i, a)| (a, std::cmp::Reverse(i)));
        match best {
            Some((i, _)) => Some(&self.list[i]),
            None => self.at(Point::new(r.left + r.width() / 2, r.top + r.height() / 2)),
        }
    }

    /// A window of `w` x `h` DIPs centred on the work area of the monitor under `p`, in physical
    /// pixels on that monitor. Never larger than the work area minus a margin.
    pub fn centered_window(&self, p: Point, w_dip: f32, h_dip: f32) -> Option<Rect> {
        let m = self.at(p)?;
        let margin = m.px(crate::theme::WINDOW_WORK_MARGIN);
        let w = m.px(w_dip).min(m.work.width() - 2 * margin).max(1);
        let h = m.px(h_dip).min(m.work.height() - 2 * margin).max(1);
        let x = m.work.left + (m.work.width() - w) / 2;
        let y = m.work.top + (m.work.height() - h) / 2;
        Some(Rect::from_xywh(x, y, w, h))
    }
}

/// Where a floating panel of `w` x `h` physical pixels goes next to the selection `sel`:
/// below it, right-aligned; else above it; else inside its bottom-right corner. Always fully on
/// monitor `m` (the one the caller picked for the panel's scale), `gap` pixels from edges.
pub fn place_beside(sel: &Rect, w: i32, h: i32, m: &Monitor, gap: i32) -> Point {
    let area = m.rect;
    let clamp_x = |x: i32| x.clamp(area.left + gap, (area.right - gap - w).max(area.left + gap));
    let clamp_y = |y: i32| y.clamp(area.top + gap, (area.bottom - gap - h).max(area.top + gap));
    let x = clamp_x(sel.right - w);
    let below = sel.bottom + gap;
    if below + h <= area.bottom - gap {
        return Point::new(x, clamp_y(below));
    }
    let above = sel.top - gap - h;
    if above >= area.top + gap {
        return Point::new(x, clamp_y(above));
    }
    Point::new(clamp_x(sel.right - gap - w), clamp_y(sel.bottom - gap - h))
}

/// The user's layout with the scales of a mixed setup, for tests: a 150 % 4K monitor to the left at
/// negative x, the 100 % primary, and a 125 % monitor above it at negative y.
pub fn mixed_example() -> Monitors {
    Monitors::new(vec![
        Monitor::new(Rect::from_xywh(0, 0, 2560, 1440), 96),
        Monitor::new(Rect::from_xywh(-3840, -400, 3840, 2160), 144),
        Monitor::new(Rect::from_xywh(0, -1440, 2560, 1440), 120),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_and_px() {
        assert_eq!(scale(96), 1.0);
        assert_eq!(scale(144), 1.5);
        assert_eq!(scale(0), 1.0, "unknown DPI acts as 100 %");
        assert_eq!(px(32.0, 96), 32);
        assert_eq!(px(32.0, 120), 40);
        assert_eq!(px(32.0, 144), 48);
        assert_eq!(px(32.0, 192), 64);
        assert_eq!(px(1.0, 168), 2, "175 %: a 1-DIP line is 2 px, never 0");
    }

    #[test]
    fn monitor_lookup_with_negative_coordinates() {
        let ms = mixed_example();
        let cases = [
            (Point::new(10, 10), 96),
            (Point::new(-1, 0), 144),
            (Point::new(-3840, -400), 144),
            (Point::new(100, -1), 120),
            (Point::new(-5000, 0), 144),  // off the desktop: nearest
            (Point::new(1000, -2000), 120), // above everything
        ];
        assert!(!cases.is_empty());
        for (p, dpi) in cases {
            assert_eq!(ms.at(p).unwrap().dpi, dpi, "{p:?}");
        }
    }

    #[test]
    fn rect_goes_to_the_monitor_holding_most_of_it() {
        let ms = mixed_example();
        // 300 px on the 150 % monitor, 100 px on the primary.
        assert_eq!(ms.for_rect(&Rect::new(-300, 100, 100, 200)).unwrap().dpi, 144);
        assert_eq!(ms.for_rect(&Rect::new(-100, 100, 300, 200)).unwrap().dpi, 96);
        // Spanning the primary and the 125 % one above it, mostly above.
        assert_eq!(ms.for_rect(&Rect::new(10, -300, 50, 10)).unwrap().dpi, 120);
        assert_eq!(ms.for_rect(&Rect::new(9000, 9000, 9010, 9010)).unwrap().dpi, 96, "nowhere: nearest");
    }

    #[test]
    fn centered_window_is_scaled_for_its_monitor() {
        let ms = mixed_example();
        let a = ms.centered_window(Point::new(5, 5), 800.0, 600.0).unwrap();
        assert_eq!((a.width(), a.height()), (800, 600));
        assert_eq!(a.left, (2560 - 800) / 2);
        let b = ms.centered_window(Point::new(-5, 5), 800.0, 600.0).unwrap();
        assert_eq!((b.width(), b.height()), (1200, 900), "150 %");
        assert!(b.left >= -3840 && b.right <= 0);
        let big = ms.centered_window(Point::new(5, 5), 5000.0, 5000.0).unwrap();
        assert_eq!((big.width(), big.height()), (2560 - 48, 1440 - 48), "clipped to the work area");
    }

    #[test]
    fn panel_goes_below_then_above_then_inside() {
        let m = Monitor::new(Rect::from_xywh(-1920, 0, 1920, 1080), 96);
        let (w, h, gap) = (400, 40, 8);
        // Room below.
        let p = place_beside(&Rect::new(-1500, 100, -1000, 500), w, h, &m, gap);
        assert_eq!(p, Point::new(-1400, 508));
        // No room below, room above.
        let p = place_beside(&Rect::new(-1500, 300, -1000, 1070), w, h, &m, gap);
        assert_eq!(p, Point::new(-1400, 300 - 8 - 40));
        // Full height: inside, bottom-right.
        let p = place_beside(&Rect::new(-1920, 0, 0, 1080), w, h, &m, gap);
        assert_eq!(p, Point::new(-8 - 400, 1080 - 8 - 40));
        // A selection at the left edge narrower than the panel: clamped onto the monitor.
        let p = place_beside(&Rect::new(-1920, 100, -1800, 200), w, h, &m, gap);
        assert_eq!(p.x, -1920 + 8);
    }
}
