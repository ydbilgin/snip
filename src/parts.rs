//! Physical-pixel candidate filtering. Level zero is always the frozen whole window.
use crate::geometry::Rect;

pub fn levels(window: Rect, candidates: &[Rect]) -> Vec<Rect> {
    let mut kept = vec![window];
    for candidate in candidates {
        let Some(r) = candidate.intersect(&window) else {
            continue;
        };
        if r.width() < 48 || r.height() < 48 || r.area() * 2 < candidate.area() {
            continue;
        }
        let parent = kept.last().unwrap();
        if [
            (r.left, parent.left),
            (r.top, parent.top),
            (r.right, parent.right),
            (r.bottom, parent.bottom),
        ]
        .iter()
        .all(|(a, b)| (i64::from(*a) - i64::from(*b)).abs() <= 4)
        {
            continue;
        }
        kept.push(r);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discord_columns_and_message_keep_order() {
        let w = Rect::from_xywh(-100, 20, 1519, 900);
        let chat = Rect::from_xywh(180, 100, 970, 766);
        let message = Rect::from_xywh(188, 200, 954, 189);
        assert_eq!(
            levels(w, &[w, chat, chat.offset(2, -2), message]),
            vec![w, chat, message]
        );
    }
    #[test]
    fn chrome_clips_live_rects_and_rejects_mostly_outside_and_small() {
        let w = Rect::new(0, 0, 1000, 800);
        assert_eq!(
            levels(
                w,
                &[
                    Rect::new(-10, 100, 700, 700),
                    Rect::new(-900, 100, 100, 700),
                    Rect::new(10, 10, 57, 500),
                    Rect::new(10, 10, 500, 57),
                    Rect::new(1000, 0, 1500, 800),
                    Rect::new(0, 0, 0, 0),
                ]
            ),
            vec![w, Rect::new(0, 100, 700, 700)]
        );
    }
    #[test]
    fn inclusive_thresholds_and_parent_tolerance() {
        let w = Rect::new(0, 0, 100, 100);
        assert_eq!(
            levels(w, &[Rect::new(4, 4, 96, 96), Rect::new(-48, 0, 48, 48)]),
            vec![w, Rect::new(0, 0, 48, 48)]
        );
        assert_eq!(levels(w, &[Rect::new(-49, 0, 48, 48)]), vec![w]);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Uia,
    Edges,
    None,
}
impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Self::Uia => "uia",
            Self::Edges => "edges",
            Self::None => "none",
        }
    }
}
/// An early edge result stays hidden until UIA gives no useful result or 400 ms elapse.
pub fn choose(uia: Option<&[Rect]>, edges: &[Rect], elapsed_ms: u128) -> Source {
    if uia.is_some_and(|v| v.len() > 1) {
        Source::Uia
    } else if edges.len() > 1 && (uia.is_some() || elapsed_ms >= 400) {
        Source::Edges
    } else {
        Source::None
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    #[test]
    fn uia_priority_nothing_useful_timeout_and_late_replacement() {
        let whole = Rect::new(0, 0, 1000, 800);
        let panes = [whole, Rect::new(0, 0, 300, 800)];
        assert_eq!(choose(None, &panes, 399), Source::None);
        assert_eq!(choose(None, &panes, 400), Source::Edges);
        assert_eq!(choose(Some(&[whole]), &panes, 10), Source::Edges);
        assert_eq!(choose(Some(&panes), &panes, 900), Source::Uia);
        assert_eq!(choose(Some(&[whole]), &[whole], 900), Source::None);
    }
}

/// Plain overlay interaction state; independent of Windows input sampling.
#[derive(Default, Debug)]
pub struct Mode {
    latched: bool,
    held: bool,
    tap: bool,
    wheel_entered: bool,
    window: Option<isize>,
    depth: Option<usize>,
    pending_out: usize,
}
impl Mode {
    pub fn active(&self) -> bool {
        self.latched || self.held
    }
    pub fn alt(&mut self, down: bool, eligible: bool) {
        if down {
            if !self.held && eligible {
                if !self.active() {
                    self.depth = None;
                    self.pending_out = 0;
                }
                self.held = true;
                self.tap = true;
            }
        } else {
            if self.held && self.tap && eligible {
                self.latched = !self.latched;
                self.wheel_entered = false;
            }
            self.held = false;
            self.tap = false;
        }
    }
    pub fn interrupt(&mut self) {
        self.tap = false;
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn window(&mut self, hwnd: Option<isize>) {
        if self.window != hwnd {
            self.window = hwnd;
            self.depth = None;
            self.pending_out = 0;
        }
    }
    pub fn index(&self, len: usize) -> usize {
        self.depth
            .unwrap_or(len.saturating_sub(1).saturating_sub(self.pending_out))
            .min(len.saturating_sub(1))
    }
    /// Anchor the initial innermost choice once useful geometry arrives. Later hovers in the
    /// same window keep that outer-based depth even if the candidate chain gets longer.
    pub fn resolve(&mut self, len: usize) {
        if len > 1 && self.depth.is_none() {
            self.depth = Some(self.index(len));
            self.pending_out = 0;
        }
    }
    pub fn wheel(&mut self, delta: i32, len: usize) {
        self.interrupt();
        if delta == 0 {
            return;
        }
        if !self.active() {
            if delta > 0 {
                return;
            }
            self.latched = true;
            self.wheel_entered = true;
            self.depth = Some(0);
        }
        if len <= 1 && self.depth.is_none() {
            if delta > 0 {
                self.pending_out = self.pending_out.saturating_add(1);
            } else {
                self.pending_out = self.pending_out.saturating_sub(1);
            }
            return;
        }
        let current = if len > 1 {
            self.index(len)
        } else {
            self.depth.unwrap_or(0)
        };
        if delta > 0 {
            if current == 0 && self.wheel_entered {
                self.latched = false;
                self.wheel_entered = false;
            }
            self.depth = Some(current.saturating_sub(1));
        } else {
            // Preserve intent even while levels are still pending.
            self.depth = Some(if len > 1 {
                (current + 1).min(len - 1)
            } else {
                current + 1
            });
        }
    }
}

#[cfg(test)]
mod mode_tests {
    use super::*;
    #[test]
    fn alt_tap_hold_click_and_interrupted_tap() {
        let mut m = Mode::default();
        m.alt(true, true);
        assert!(m.active());
        assert_eq!(m.index(5), 4);
        m.alt(false, true);
        assert!(m.active());
        m.alt(true, true);
        m.alt(false, true);
        assert!(!m.active());
        m.alt(true, true);
        m.interrupt();
        m.alt(false, true);
        assert!(!m.active());
        m.alt(true, false);
        m.alt(false, true);
        assert!(!m.active()); // Alt during drag
    }
    #[test]
    fn wheel_depth_stays_outer_based_resets_on_window_change_and_only_wheel_can_exit() {
        let mut m = Mode::default();
        m.window(Some(1));
        m.wheel(-120, 5);
        assert!(m.active());
        assert_eq!(m.index(5), 1);
        m.wheel(-120, 5);
        assert_eq!(m.index(5), 2);
        assert_eq!(m.index(8), 2);
        m.window(Some(1));
        assert_eq!(m.index(8), 2);
        m.wheel(120, 5);
        m.wheel(120, 5);
        assert!(m.active());
        assert_eq!(m.index(5), 0);
        m.wheel(120, 5);
        assert!(!m.active());
        m.alt(true, true);
        m.alt(false, true);
        m.window(Some(2));
        assert_eq!(m.index(5), 4);
        for _ in 0..10 {
            m.wheel(120, 5);
        }
        assert!(m.active());
        m.reset();
        assert!(!m.active());
    }
    #[test]
    fn default_depth_anchors_once_and_pending_wheel_up_is_relative() {
        let mut m = Mode::default();
        m.window(Some(1));
        m.alt(true, true);
        m.alt(false, true);
        m.wheel(120, 1);
        m.resolve(5);
        assert_eq!(m.index(5), 3);
        m.window(Some(1));
        m.resolve(8);
        assert_eq!(m.index(8), 3);
        m.window(Some(2));
        m.resolve(8);
        assert_eq!(m.index(8), 7);
    }
    #[test]
    fn wheel_before_answer_retains_first_inner_level() {
        let mut m = Mode::default();
        m.wheel(-120, 1);
        assert_eq!(m.index(6), 1);
        m.wheel(-120, 1);
        assert_eq!(m.index(6), 2);
        m.alt(false, true);
        assert!(m.active());
    }
}
