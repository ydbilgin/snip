//! The frame around a selection or a hovered window, as filled bars in desktop pixels.
//!
//! Rule (a 2026-11-08 bug report: frames spilled onto neighbouring screens): a frame is drawn inside the monitor that
//! holds it. It used to be stroked outside the rectangle, so a whole-monitor hover or a drag to the
//! edge put a one-pixel line on the neighbouring monitor. Now:
//! - a monitor that the selection does not overlap gets no frame (touching an edge is not overlap);
//! - the frame sits just outside the selection, but where that would leave the monitor, the side
//!   moves inside the selection, against the monitor's edge;
//! - every bar is clipped to the monitor, right and bottom exclusive.
//!
//! A window that hangs a few pixels over a neighbour (a maximized window is 8 px past its monitor)
//! is first snapped to the monitor that holds it (`Layout::snap_window`).

use crate::geometry::Rect;

/// A filled bar of the frame. `layer` 0 is the one next to the selection, 1 the one outside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bar {
    pub rect: Rect,
    pub layer: u8,
}

/// Where one layer's four bands lie: each is `[lo, hi)` across its own axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bands {
    pub left: (i32, i32),
    pub right: (i32, i32),
    pub top: (i32, i32),
    pub bottom: (i32, i32),
}

/// The bands of the layer that lies `offset..offset+thick` outside `sel`, out of `total` pixels of
/// frame in all. A side whose band would leave `m` moves inside the selection instead.
pub fn bands(m: Rect, sel: Rect, offset: i32, thick: i32, total: i32) -> Bands {
    let out_lo = |edge: i32, lo_limit: i32| {
        let b = (edge - offset - thick, edge - offset);
        if edge >= lo_limit && b.0 < lo_limit { (edge + total - offset - thick, edge + total - offset) } else { b }
    };
    let out_hi = |edge: i32, hi_limit: i32| {
        let b = (edge + offset, edge + offset + thick);
        if edge <= hi_limit && b.1 > hi_limit { (edge - total + offset, edge - total + offset + thick) } else { b }
    };
    Bands {
        left: out_lo(sel.left, m.left),
        right: out_hi(sel.right, m.right),
        top: out_lo(sel.top, m.top),
        bottom: out_hi(sel.bottom, m.bottom),
    }
}

fn clipped(r: Rect, m: Rect, layer: u8, out: &mut Vec<Bar>) {
    if let Some(rect) = r.intersect(&m) {
        out.push(Bar { rect, layer });
    }
}

/// The two-layer frame (`one` pixels each) of `sel` as drawn on monitor `m`. Empty when `sel` does
/// not overlap `m`.
pub fn selection_frame(m: Rect, sel: Rect, one: i32) -> Vec<Bar> {
    let mut out = Vec::new();
    if sel.intersect(&m).is_none() {
        return out;
    }
    let total = 2 * one;
    for layer in 0..2u8 {
        let b = bands(m, sel, layer as i32 * one, one, total);
        let (x0, x1) = (b.left.0, b.right.1);
        let (y0, y1) = (b.top.0, b.bottom.1);
        clipped(Rect::new(x0, y0, x1, b.top.1), m, layer, &mut out);
        clipped(Rect::new(x0, b.bottom.0, x1, y1), m, layer, &mut out);
        clipped(Rect::new(x0, b.top.1, b.left.1, b.bottom.0), m, layer, &mut out);
        clipped(Rect::new(b.right.0, b.top.1, x1, b.bottom.0), m, layer, &mut out);
    }
    out
}

/// The eight corner bars of the text frame (`th` thick, `len` long), drawn on monitor `m`.
pub fn corner_bars(m: Rect, sel: Rect, th: i32, len: i32) -> Vec<Rect> {
    let mut out = Vec::new();
    if sel.intersect(&m).is_none() {
        return out;
    }
    let b = bands(m, sel, 0, th, th);
    let (l, t, r, bt) = (b.left.0, b.top.0, b.right.1, b.bottom.1);
    for bar in [
        Rect::new(l, t, l + len, t + th),
        Rect::new(l, t, l + th, t + len),
        Rect::new(r - len, t, r, t + th),
        Rect::new(r - th, t, r, t + len),
        Rect::new(l, bt - th, l + len, bt),
        Rect::new(l, bt - len, l + th, bt),
        Rect::new(r - len, bt - th, r, bt),
        Rect::new(r - th, bt - len, r, bt),
    ] {
        if let Some(c) = bar.intersect(&m) {
            out.push(c);
        }
    }
    out
}

/// A handle belongs to the monitor that holds its centre (right and bottom exclusive).
pub fn handle_belongs(m: Rect, center_x: i32, center_y: i32) -> bool {
    center_x >= m.left && center_x < m.right && center_y >= m.top && center_y < m.bottom
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Layout, Point};

    /// A four-monitor layout (7040 x 2880): top, left, main, right.
    fn four_monitors() -> Layout {
        crate::geometry::his_layout_now()
    }

    fn pixels(bars: &[Bar]) -> std::collections::HashSet<(i32, i32)> {
        let mut s = std::collections::HashSet::new();
        for b in bars {
            for y in b.rect.top..b.rect.bottom {
                for x in b.rect.left..b.rect.right {
                    s.insert((x, y));
                }
            }
        }
        s
    }

    /// Every frame pixel of `sel` on every monitor lies in that monitor, and a monitor the
    /// selection does not overlap gets none.
    fn check(l: &Layout, sel: Rect, one: i32) {
        for m in &l.monitors {
            let bars = selection_frame(*m, sel, one);
            for b in &bars {
                assert!(b.rect.intersect(m) == Some(b.rect), "bar {:?} leaves monitor {:?} for sel {:?}", b.rect, m, sel);
            }
            if sel.intersect(m).is_none() {
                assert!(bars.is_empty(), "frame on {m:?} though sel {sel:?} does not overlap it");
            }
            for c in corner_bars(*m, sel, 3, 14) {
                assert!(c.intersect(m) == Some(c), "corner {:?} leaves {:?}", c, m);
            }
        }
    }

    #[test]
    fn whole_monitor_hover_stays_on_its_own_monitor_on_all_four() {
        let l = four_monitors();
        for (i, m) in l.monitors.iter().enumerate() {
            for one in [1, 2, 3] {
                check(&l, *m, one);
                for (j, o) in l.monitors.iter().enumerate() {
                    let px = pixels(&selection_frame(*o, *m, one));
                    if i != j {
                        assert!(px.is_empty(), "monitor {i}'s frame drew {} px on monitor {j}", px.len());
                    } else {
                        // All four sides are visible, hugging the monitor's own edge pixels.
                        for (x, y) in [(m.left, m.top + 50), (m.right - 1, m.top + 50), (m.left + 50, m.top), (m.left + 50, m.bottom - 1)] {
                            assert!(px.contains(&(x, y)), "monitor {i} one={one}: edge pixel ({x},{y}) not framed");
                        }
                        assert!(!px.contains(&(m.left + 50, m.top + 2 * one + 5)), "frame is thicker than {} px", 2 * one);
                    }
                }
            }
        }
    }

    #[test]
    fn the_cases_in_his_screenshots() {
        let l = four_monitors();
        // s-hover-ekran2: main monitor hover must not paint y=-1, x=-1 or x=2560.
        let main = l.monitors[2];
        for (j, o) in l.monitors.iter().enumerate() {
            let px = pixels(&selection_frame(*o, main, 1));
            for bad in [(500, -1), (-1, 500), (2560, 500), (-1, 0), (2560, 0)] {
                assert!(!px.contains(&bad), "monitor {j} paints {bad:?}");
            }
        }
        // t-surukle-ekran2: a drag 1280..2559 inclusive on the main monitor, right = 2560 exclusive.
        let drag = Rect::new(1280, 400, 2560, 1000);
        assert!(pixels(&selection_frame(l.monitors[3], drag, 1)).is_empty(), "nothing on the right monitor");
        let px = pixels(&selection_frame(main, drag, 1));
        assert!(px.contains(&(2559, 700)) && px.contains(&(2558, 700)), "frame hugs the edge inside");
        assert!(!px.iter().any(|(x, _)| *x >= 2560));
        // t-surukle-ekran1: the same on the left monitor, border must not reach x=0 of the main.
        let drag1 = Rect::new(-960, 300, 0, 900);
        assert!(pixels(&selection_frame(main, drag1, 1)).is_empty());
        assert!(pixels(&selection_frame(l.monitors[1], drag1, 1)).contains(&(-1, 500)));
    }

    #[test]
    fn edges_corners_and_spans() {
        let l = four_monitors();
        let sels = [
            Rect::new(0, 0, 100, 100),                // corner of the main monitor
            Rect::new(2460, 1340, 2560, 1440),        // opposite corner
            Rect::new(-300, 200, 300, 600),           // spans left and main
            Rect::new(2000, 200, 3000, 600),          // spans main and right
            Rect::new(2559, 10, 2561, 20),            // two pixels across the seam
            Rect::new(-1920, -1440, 5120, 1440),      // the whole desktop
            Rect::new(1, 1, 2559, 1439),              // one pixel in
            Rect::new(0, 0, 1, 1),                    // a single pixel
            Rect::new(-606, -1440, 2834, 0),          // the top monitor
        ];
        for sel in sels {
            for one in [1, 2] {
                check(&l, sel, one);
            }
        }
        // A seam-spanning selection: each side's monitor shows its part, the seam itself has no line.
        let span = Rect::new(2000, 200, 3000, 600);
        let main_px = pixels(&selection_frame(l.monitors[2], span, 1));
        assert!(!main_px.contains(&(2559, 400)), "no vertical line at the seam on the main monitor");
        let right_px = pixels(&selection_frame(l.monitors[3], span, 1));
        assert!(!right_px.contains(&(2560, 400)), "none at the seam on the right monitor");
        assert!(right_px.contains(&(2999, 400)) || right_px.contains(&(3000, 400)));
    }

    #[test]
    fn a_maximized_window_snaps_to_its_monitor() {
        let l = four_monitors();
        // 8 px of invisible border past every side of the main monitor.
        let win = Rect::new(-8, -8, 2568, 1448);
        let snapped = l.snap_window(&win);
        assert_eq!(snapped, Rect::new(0, 0, 2560, 1440));
        for m in &l.monitors {
            let px = pixels(&selection_frame(*m, snapped, 1));
            if *m != l.monitors[2] {
                assert!(px.is_empty());
            }
        }
        // Same on each of a four-monitor layout.
        for m in &l.monitors {
            let w = Rect::new(m.left - 8, m.top - 8, m.right + 8, m.bottom + 8);
            assert_eq!(l.snap_window(&w), *m, "{m:?}");
        }
        // A window really spanning two monitors keeps spanning.
        let span = Rect::new(2000, 200, 3000, 900);
        assert_eq!(l.snap_window(&span), span);
        // A window on one monitor is unchanged.
        let inside = Rect::new(100, 100, 900, 700);
        assert_eq!(l.snap_window(&inside), inside);
        // A window off the desktop keeps what clip gives.
        assert_eq!(l.snap_window(&Rect::new(9000, 9000, 9100, 9100)), Rect::new(9000, 9000, 9100, 9100));
    }

    #[test]
    fn handles_belong_to_one_monitor() {
        let l = four_monitors();
        // A handle centred on the seam x = 2560 belongs to the right monitor only.
        let n = l.monitors.iter().filter(|m| handle_belongs(**m, 2560, 700)).count();
        assert_eq!(n, 1);
        assert!(handle_belongs(l.monitors[3], 2560, 700));
        assert!(handle_belongs(l.monitors[2], 2559, 700));
        assert!(l.monitor_at(Point::new(2560, 700)) == Some(3));
    }

    #[test]
    fn corner_bars_stay_inside_and_hug_the_edge() {
        let m = Rect::new(0, 0, 2560, 1440);
        let bars = corner_bars(m, Rect::new(0, 0, 2560, 1440), 3, 14);
        assert_eq!(bars.len(), 8);
        for b in &bars {
            assert!(b.intersect(&m) == Some(*b));
        }
        assert!(bars.iter().any(|b| b.left == 0 && b.top == 0));
        assert!(bars.iter().any(|b| b.right == 2560 && b.bottom == 1440));
    }
}
