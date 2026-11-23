//! Desktop geometry in physical pixels (the process is per-monitor-v2 DPI aware).
//!
//! Coordinates are virtual-desktop coordinates: the primary monitor's top-left is (0, 0) and other
//! monitors may sit at negative x or y (one test layout has monitors at (-320, -1440) and (-1920, 218)).
//! Rectangles are half-open: `left <= x < right`, `top <= y < bottom`.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self { left, top, right, bottom }
    }

    pub const fn from_xywh(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { left: x, top: y, right: x + w, bottom: y + h }
    }

    /// The rectangle a drag from `a` to `b` covers, whichever way the drag went.
    pub fn from_drag(a: Point, b: Point) -> Self {
        Self { left: a.x.min(b.x), top: a.y.min(b.y), right: a.x.max(b.x), bottom: a.y.max(b.y) }
    }

    pub fn width(&self) -> i32 {
        (self.right - self.left).max(0)
    }

    pub fn height(&self) -> i32 {
        (self.bottom - self.top).max(0)
    }

    pub fn is_empty(&self) -> bool {
        self.width() == 0 || self.height() == 0
    }

    pub fn area(&self) -> i64 {
        self.width() as i64 * self.height() as i64
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.left && p.x < self.right && p.y >= self.top && p.y < self.bottom
    }

    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let r = Rect {
            left: self.left.max(o.left),
            top: self.top.max(o.top),
            right: self.right.min(o.right),
            bottom: self.bottom.min(o.bottom),
        };
        if r.left < r.right && r.top < r.bottom { Some(r) } else { None }
    }

    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        Rect {
            left: self.left.min(o.left),
            top: self.top.min(o.top),
            right: self.right.max(o.right),
            bottom: self.bottom.max(o.bottom),
        }
    }

    pub fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect { left: self.left + dx, top: self.top + dy, right: self.right + dx, bottom: self.bottom + dy }
    }

    /// Clamp a point into this rectangle (the last pixel row and column are inside).
    pub fn clamp_point(&self, p: Point) -> Point {
        Point { x: p.x.clamp(self.left, self.right - 1), y: p.y.clamp(self.top, self.bottom - 1) }
    }

    fn distance_sq(&self, p: Point) -> i64 {
        let dx = if p.x < self.left { self.left - p.x } else if p.x >= self.right { p.x - self.right + 1 } else { 0 };
        let dy = if p.y < self.top { self.top - p.y } else if p.y >= self.bottom { p.y - self.bottom + 1 } else { 0 };
        dx as i64 * dx as i64 + dy as i64 * dy as i64
    }
}

/// One part of a selection that lies on one monitor: copy `src` (monitor-local pixels) to
/// (`dst_x`, `dst_y`) in the output image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    pub monitor: usize,
    pub src: Rect,
    pub dst_x: i32,
    pub dst_y: i32,
}

/// The monitors as one desktop. The layout need not be a rectangle; gaps exist.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub monitors: Vec<Rect>,
}

impl Layout {
    pub fn new(monitors: Vec<Rect>) -> Self {
        Self { monitors }
    }

    /// Bounding box of all monitors (6400x2880 on the user's PC on 2026-10-08).
    pub fn bounds(&self) -> Rect {
        self.monitors.iter().fold(Rect::default(), |acc, m| acc.union(m))
    }

    pub fn monitor_at(&self, p: Point) -> Option<usize> {
        self.monitors.iter().position(|m| m.contains(p))
    }

    /// The monitor that holds `p`, or the nearest one when `p` is in a gap or off the desktop.
    pub fn nearest_monitor(&self, p: Point) -> Option<usize> {
        if let Some(i) = self.monitor_at(p) {
            return Some(i);
        }
        self.monitors.iter().enumerate().min_by_key(|(_, m)| m.distance_sq(p)).map(|(i, _)| i)
    }

    /// Move `p` onto the nearest monitor pixel (a drag cannot end in a gap or off the desktop).
    pub fn clamp_point(&self, p: Point) -> Point {
        match self.nearest_monitor(p) {
            Some(i) => self.monitors[i].clamp_point(p),
            None => p,
        }
    }

    /// Move the cursor by (dx, dy) for the arrow keys, staying on the desktop.
    pub fn nudge(&self, p: Point, dx: i32, dy: i32) -> Point {
        self.clamp_point(Point::new(p.x + dx, p.y + dy))
    }

    /// Split a selection into per-monitor pieces. Parts in gaps between monitors are in no piece
    /// (the output keeps them opaque black). The selection is first clipped to the bounds.
    pub fn pieces(&self, sel: &Rect) -> Vec<Piece> {
        let Some(sel) = sel.intersect(&self.bounds()) else { return Vec::new() };
        let mut out = Vec::new();
        for (i, m) in self.monitors.iter().enumerate() {
            if let Some(part) = sel.intersect(m) {
                out.push(Piece {
                    monitor: i,
                    src: part.offset(-m.left, -m.top),
                    dst_x: part.left - sel.left,
                    dst_y: part.top - sel.top,
                });
            }
        }
        out
    }

    /// The part of `sel` that is on monitor `i`, in that monitor's local pixels (for drawing).
    pub fn local_on(&self, i: usize, sel: &Rect) -> Option<Rect> {
        let m = self.monitors.get(i)?;
        sel.intersect(m).map(|r| r.offset(-m.left, -m.top))
    }

    /// A window's rectangle as a selection: a window hangs a few invisible pixels over its
    /// neighbours (a maximized one is 8 px past its monitor on every side), and a frame around it
    /// would spill onto them. Parts on a monitor that are only a sliver (32 px or less across) are
    /// dropped; a window that really spans monitors keeps spanning. Off the desktop: unchanged.
    pub fn snap_window(&self, w: &Rect) -> Rect {
        const SLIVER: i32 = 32;
        let parts: Vec<Rect> = self.monitors.iter().filter_map(|m| w.intersect(m)).collect();
        if parts.is_empty() {
            return *w;
        }
        let kept: Vec<&Rect> = parts.iter().filter(|p| p.width().min(p.height()) > SLIVER).collect();
        if kept.is_empty() {
            return *parts.iter().max_by_key(|p| p.area()).unwrap();
        }
        kept.iter().fold(*kept[0], |acc, p| acc.union(p))
    }

    /// Clip a selection to the desktop bounds (a window can hang off the desktop).
    pub fn clip(&self, sel: &Rect) -> Option<Rect> {
        sel.intersect(&self.bounds())
    }
}

/// The first window (in z-order, topmost first) whose bounds hold `p`.
pub fn window_at(windows: &[Rect], p: Point) -> Option<usize> {
    windows.iter().position(|w| w.contains(p))
}

/// A press and release closer than this many pixels is a click (window snip), not a drag.
pub const CLICK_SLOP: i32 = 3;

pub fn is_click(a: Point, b: Point) -> bool {
    (a.x - b.x).abs() <= CLICK_SLOP && (a.y - b.y).abs() <= CLICK_SLOP
}

/// A three-monitor layout measured on 2026-10-08, for tests and the self-test.
pub fn his_layout() -> Layout {
    Layout::new(vec![
        Rect::from_xywh(0, 0, 2560, 1440),        // DISPLAY1 Samsung, primary
        Rect::from_xywh(2560, 131, 1920, 1080),   // DISPLAY2 Samsung
        Rect::from_xywh(-320, -1440, 3440, 1440), // DISPLAY3 MSI, above
        Rect::from_xywh(-1920, 218, 1920, 1080),  // DISPLAY4 BenQ, left
    ])
}

/// A four-monitor layout measured on 2026-11-08 (from screenshots of the whole desktop): top, left, main
/// and right monitors; the box is 7040 x 2880 with a negative origin.
pub fn his_layout_now() -> Layout {
    Layout::new(vec![
        Rect::new(-606, -1440, 2834, 0),   // top, 3440 x 1440
        Rect::new(-1920, 0, 0, 1080),      // left, 1920 x 1080
        Rect::new(0, 0, 2560, 1440),       // main
        Rect::new(2560, 0, 5120, 1440),    // right
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_monitor_layout_is_7040_by_2880() {
        let b = his_layout_now().bounds();
        assert_eq!((b.left, b.top, b.width(), b.height()), (-1920, -1440, 7040, 2880));
    }

    #[test]
    fn bounds_of_his_layout_is_6400_by_2880() {
        let l = his_layout();
        assert_eq!(l.bounds(), Rect::new(-1920, -1440, 4480, 1440));
        assert_eq!(l.bounds().width(), 6400);
        assert_eq!(l.bounds().height(), 2880);
    }

    #[test]
    fn monitor_at_handles_negative_coordinates() {
        let l = his_layout();
        let cases = [
            (Point::new(0, 0), Some(0)),
            (Point::new(2559, 1439), Some(0)),
            (Point::new(2560, 131), Some(1)),
            (Point::new(2560, 130), None), // gap above DISPLAY2
            (Point::new(-320, -1440), Some(2)),
            (Point::new(-321, -1), None), // gap left of the MSI
            (Point::new(-1, -1), Some(2)),
            (Point::new(-1920, 218), Some(3)),
            (Point::new(-1, 1297), Some(3)),
            (Point::new(-1, 1298), None), // below the BenQ
            (Point::new(4480, 500), None),
        ];
        assert!(!cases.is_empty());
        for (p, want) in cases {
            assert_eq!(l.monitor_at(p), want, "point {p:?}");
        }
    }

    #[test]
    fn drag_is_normalised_in_every_direction() {
        let a = Point::new(-100, -50);
        let b = Point::new(30, 70);
        let want = Rect::new(-100, -50, 30, 70);
        let cases = [(a, b), (b, a), (Point::new(-100, 70), Point::new(30, -50)), (Point::new(30, -50), Point::new(-100, 70))];
        assert!(!cases.is_empty());
        for (x, y) in cases {
            assert_eq!(Rect::from_drag(x, y), want);
        }
        assert_eq!(want.width(), 130);
        assert_eq!(want.height(), 120);
    }

    #[test]
    fn single_monitor_selection_is_one_piece() {
        let l = his_layout();
        let p = l.pieces(&Rect::new(100, 100, 300, 250));
        assert_eq!(p, vec![Piece { monitor: 0, src: Rect::new(100, 100, 300, 250), dst_x: 0, dst_y: 0 }]);
    }

    #[test]
    fn cross_monitor_selection_splits_into_pieces_with_offsets() {
        let l = his_layout();
        // From the BenQ (left, x<0) across the primary and up into the MSI (y<0).
        let sel = Rect::new(-200, -100, 200, 300);
        let p = l.pieces(&sel);
        assert_eq!(p.len(), 3, "{p:?}");
        // Primary: x 0..200, y 0..300 -> local same, placed at (200, 100).
        assert!(p.contains(&Piece { monitor: 0, src: Rect::new(0, 0, 200, 300), dst_x: 200, dst_y: 100 }));
        // MSI: x -200..200, y -100..0 -> local x 120..520, y 1340..1440, placed at (0, 0).
        assert!(p.contains(&Piece { monitor: 2, src: Rect::new(120, 1340, 520, 1440), dst_x: 0, dst_y: 0 }));
        // BenQ: x -200..0, y 218..300 -> local x 1720..1920, y 0..82, placed at (0, 318).
        assert!(p.contains(&Piece { monitor: 3, src: Rect::new(1720, 0, 1920, 82), dst_x: 0, dst_y: 318 }));
        // The gap (x<0, 0<=y<218) is in no piece.
        let covered: i64 = p.iter().map(|q| q.src.area()).sum();
        assert!(covered < sel.area());
    }

    #[test]
    fn pieces_cover_every_monitor_for_the_whole_desktop() {
        let l = his_layout();
        let p = l.pieces(&l.bounds());
        assert_eq!(p.len(), 4);
        let covered: i64 = p.iter().map(|q| q.src.area()).sum();
        assert_eq!(covered, 12_787_200, "the four screens hold 12,787,200 pixels");
    }

    #[test]
    fn selection_off_the_desktop_has_no_pieces() {
        let l = his_layout();
        assert!(l.pieces(&Rect::new(5000, 5000, 5100, 5100)).is_empty());
        assert!(l.pieces(&Rect::new(10, 10, 10, 50)).is_empty(), "zero width");
    }

    #[test]
    fn clamp_moves_points_in_gaps_onto_the_nearest_monitor() {
        let l = his_layout();
        let cases = [
            (Point::new(-321, -10), Point::new(-320, -10)),  // left of MSI, nearest is MSI
            (Point::new(3000, 50), Point::new(3000, -1)),    // gap above DISPLAY2: the MSI's bottom row (51 px away) beats DISPLAY2 (81 px)
            (Point::new(9999, 500), Point::new(4479, 500)),  // far right
            (Point::new(-5000, -5000), Point::new(-320, -1440)), // far top-left goes to the MSI corner
            (Point::new(10, 10), Point::new(10, 10)),        // on a monitor: unchanged
        ];
        assert!(!cases.is_empty());
        for (p, want) in cases {
            assert_eq!(l.clamp_point(p), want, "point {p:?}");
        }
    }

    #[test]
    fn nudge_moves_one_pixel_and_stays_on_the_desktop() {
        let l = his_layout();
        assert_eq!(l.nudge(Point::new(5, 5), 1, 0), Point::new(6, 5));
        assert_eq!(l.nudge(Point::new(5, 5), 0, -10), Point::new(5, -5)); // into the MSI above
        assert_eq!(l.nudge(Point::new(4479, 500), 1, 0), Point::new(4479, 500)); // right edge
        assert_eq!(l.nudge(Point::new(-1920, 300), -1, 0), Point::new(-1920, 300)); // left edge
    }

    #[test]
    fn local_on_gives_monitor_local_drawing_rects() {
        let l = his_layout();
        let sel = Rect::new(-10, -10, 10, 10);
        assert_eq!(l.local_on(0, &sel), Some(Rect::new(0, 0, 10, 10)));
        assert_eq!(l.local_on(2, &sel), Some(Rect::new(310, 1430, 330, 1440)));
        assert_eq!(l.local_on(1, &sel), None);
        assert_eq!(l.local_on(9, &sel), None);
    }

    #[test]
    fn window_hit_test_takes_the_topmost() {
        let wins = [Rect::new(0, 0, 100, 100), Rect::new(-500, -500, 500, 500)];
        assert_eq!(window_at(&wins, Point::new(50, 50)), Some(0));
        assert_eq!(window_at(&wins, Point::new(-50, -50)), Some(1));
        assert_eq!(window_at(&wins, Point::new(600, 600)), None);
        assert_eq!(window_at(&[], Point::new(0, 0)), None);
    }

    #[test]
    fn window_hanging_off_the_desktop_is_clipped() {
        let l = his_layout();
        let w = Rect::new(4400, 100, 4600, 200);
        assert_eq!(l.clip(&w), Some(Rect::new(4400, 100, 4480, 200)));
        assert_eq!(l.clip(&Rect::new(5000, 0, 5100, 10)), None);
    }

    #[test]
    fn click_versus_drag() {
        assert!(is_click(Point::new(0, 0), Point::new(3, -3)));
        assert!(!is_click(Point::new(0, 0), Point::new(4, 0)));
        assert!(!is_click(Point::new(-10, -10), Point::new(-10, -14)));
    }
}
