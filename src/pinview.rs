//! A pinned capture: an always-on-top window showing the image. Zoom with the wheel (around the
//! cursor), move by dragging, Esc or double-click closes, right-click for the menu.

pub const ZOOMS: [f32; 11] = [0.25, 0.33, 0.5, 0.67, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0];
pub const OPACITY: [u8; 4] = crate::theme::PIN_OPACITY;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pin {
    pub img_w: u32,
    pub img_h: u32,
    /// Index into ZOOMS.
    pub zoom: usize,
    /// Index into OPACITY.
    pub opacity: usize,
    /// Window top-left (physical, desktop coordinates).
    pub x: i32,
    pub y: i32,
    /// Border width in pixels (1 DIP).
    pub border: i32,
}

impl Pin {
    pub fn new(img_w: u32, img_h: u32, x: i32, y: i32, border: i32) -> Pin {
        // Placed so the image sits exactly where it was captured.
        Pin { img_w, img_h, zoom: 5, opacity: 0, x: x - border, y: y - border, border }
    }

    pub fn factor(&self) -> f32 {
        ZOOMS[self.zoom]
    }

    pub fn image_size(&self) -> (i32, i32) {
        (((self.img_w as f32 * self.factor()).round() as i32).max(1), ((self.img_h as f32 * self.factor()).round() as i32).max(1))
    }

    pub fn window_size(&self) -> (i32, i32) {
        let (w, h) = self.image_size();
        (w + 2 * self.border, h + 2 * self.border)
    }

    /// Zoom one step in (`dir` > 0) or out, keeping the image point under the cursor (window
    /// position `cx`, `cy`, desktop coordinates) where it is. Returns whether anything changed.
    pub fn zoom_at(&mut self, dir: i32, cx: i32, cy: i32) -> bool {
        let nz = if dir > 0 { (self.zoom + 1).min(ZOOMS.len() - 1) } else { self.zoom.saturating_sub(1) };
        if nz == self.zoom {
            return false;
        }
        let k = ZOOMS[nz] / ZOOMS[self.zoom];
        let ix = (cx - self.x - self.border) as f32;
        let iy = (cy - self.y - self.border) as f32;
        self.x = cx - self.border - (ix * k).round() as i32;
        self.y = cy - self.border - (iy * k).round() as i32;
        self.zoom = nz;
        true
    }

    pub fn reset_zoom(&mut self) {
        self.zoom = 5;
    }

    pub fn next_opacity(&mut self) -> u8 {
        self.opacity = (self.opacity + 1) % OPACITY.len();
        OPACITY[self.opacity]
    }

    pub fn alpha(&self) -> u8 {
        OPACITY[self.opacity]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_the_capture_position_at_100_percent() {
        let p = Pin::new(400, 300, -200, -100, 1);
        assert_eq!(p.factor(), 1.0);
        assert_eq!((p.x, p.y), (-201, -101));
        assert_eq!(p.window_size(), (402, 302));
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let mut p = Pin::new(400, 300, 0, 0, 1);
        let (cx, cy) = (101, 51);
        let (ix0, iy0) = ((cx - p.x - p.border) as f32, (cy - p.y - p.border) as f32);
        assert!(p.zoom_at(1, cx, cy));
        assert_eq!(p.factor(), 1.25);
        let ix = (cx - p.x - p.border) as f32 / p.factor();
        let iy = (cy - p.y - p.border) as f32 / p.factor();
        assert!((ix - ix0).abs() <= 1.0 && (iy - iy0).abs() <= 1.0, "{ix},{iy} vs {ix0},{iy0}");
        for _ in 0..20 {
            p.zoom_at(1, cx, cy);
        }
        assert_eq!(p.factor(), 4.0);
        assert!(!p.zoom_at(1, cx, cy), "stops at 400 %");
        for _ in 0..20 {
            p.zoom_at(-1, cx, cy);
        }
        assert_eq!(p.factor(), 0.25);
        assert_eq!(p.image_size(), (100, 75));
    }

    #[test]
    fn opacity_cycles() {
        let mut p = Pin::new(1, 1, 0, 0, 1);
        let seen: Vec<u8> = (0..5).map(|_| p.next_opacity()).collect();
        assert_eq!(seen, vec![217, 166, 115, 255, 217]);
    }
}
