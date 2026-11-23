//! Ordered capture frames in physical desktop coordinates. The bounding box includes all real
//! pixels between frames; cropping, monitor gaps and clipping belong to the existing Layout path.
use crate::geometry::{Point, Rect};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameSet {
    rects: Vec<Rect>,
}

impl FrameSet {
    pub fn rects(&self) -> &[Rect] { &self.rects }
    pub fn is_empty(&self) -> bool { self.rects.is_empty() }
    pub fn len(&self) -> usize { self.rects.len() }

    /// Ignore empty rectangles and exact duplicates without changing order.
    pub fn add(&mut self, rect: Rect) {
        if !rect.is_empty() && !self.rects.contains(&rect) { self.rects.push(rect); }
    }

    /// A click removes the most recently added frame containing it, even without a window there.
    /// Otherwise add the window under that point, when one exists.
    pub fn toggle(&mut self, point: Point, candidate: Option<Rect>) {
        if let Some(i) = self.rects.iter().rposition(|r| r.contains(point)) {
            self.rects.remove(i);
        } else if let Some(rect) = candidate {
            self.add(rect);
        }
    }

    /// Parts can overlap: toggle their exact rectangle rather than a containing frame.
    pub fn toggle_rect(&mut self, rect: Rect) {
        if let Some(i)=self.rects.iter().position(|r| *r==rect) { self.rects.remove(i); }
        else { self.add(rect); }
    }

    pub fn remove_last(&mut self) -> Option<Rect> { self.rects.pop() }
    pub fn union(&self) -> Option<Rect> { self.rects.iter().copied().reduce(|a, b| a.union(&b)) }
    pub fn clear(&mut self) { self.rects.clear(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordered_add_deduplicates_and_backspace_removes_last() {
        let mut s = FrameSet::default();
        let a = Rect::new(-100, -50, 40, 60);
        let b = Rect::new(200, 80, 500, 300);
        s.add(a); s.add(b); s.add(a); s.add(Rect::default());
        assert_eq!(s.rects(), &[a, b]);
        assert_eq!(s.remove_last(), Some(b));
        assert_eq!(s.union(), Some(a));
        assert_eq!(s.remove_last(), Some(a));
        assert_eq!(s.remove_last(), None);
        assert!(s.is_empty()); assert_eq!(s.union(), None);
    }
    #[test]
    fn toggle_removes_at_point_before_considering_another_window() {
        let mut s = FrameSet::default();
        let a = Rect::new(0, 0, 100, 100);
        let b = Rect::new(20, 20, 200, 200);
        s.add(a); s.add(b);
        s.toggle(Point::new(30, 30), Some(a));
        assert_eq!(s.rects(), &[a]);
        s.toggle(Point::new(30, 30), None);
        assert!(s.is_empty());
        s.toggle(Point::new(30, 30), Some(b));
        assert_eq!(s.rects(), &[b]);
        s.toggle(Point::new(500, 500), None);
        assert_eq!(s.len(), 1);
    }
    #[test]
    fn same_window_twice_empties_set_and_edges_are_half_open() {
        let mut s = FrameSet::default();
        let a = Rect::new(0, 0, 100, 100);
        s.toggle(Point::new(10, 10), Some(a));
        s.toggle(Point::new(100, 10), None); // right edge is outside
        assert_eq!(s.len(), 1);
        s.toggle(Point::new(10, 10), Some(a));
        assert!(s.is_empty());
    }
    #[test]
    fn union_spans_monitors_and_keeps_the_existing_gap_pieces() {
        use crate::geometry::Layout;
        let l = Layout::new(vec![Rect::new(-200, -100, 0, 100), Rect::new(40, 0, 400, 300)]);
        let mut s = FrameSet::default();
        s.add(Rect::new(-150, -50, -10, 50)); s.add(Rect::new(60, 10, 200, 200));
        let u = s.union().unwrap();
        assert_eq!(u, Rect::new(-150, -50, 200, 200));
        let p = l.pieces(&u);
        assert_eq!(p.len(), 2);
        assert_eq!((p[0].dst_x, p[0].dst_y), (0, 0));
        assert_eq!((p[1].dst_x, p[1].dst_y), (190, 50));
        assert_eq!(p[0].src, Rect::new(50, 50, 200, 200));
        assert_eq!(p[1].src, Rect::new(0, 0, 160, 200));
        s.clear(); assert_eq!(s.union(), None);
    }
}

#[cfg(test)]
mod part_tests {
    use super::*;
    #[test]
    fn overlapping_parts_toggle_exact_rect_without_removing_parent() {
        let (a,b)=(Rect::new(0,0,300,300),Rect::new(50,50,250,250));
        let mut s=FrameSet::default(); s.toggle_rect(a); s.toggle_rect(b);
        assert_eq!(s.rects(),&[a,b]); s.toggle_rect(b); assert_eq!(s.rects(),&[a]);
    }
}
