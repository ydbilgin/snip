//! The history window's grid: layout, scrolling, hit testing and drawing, in physical pixels.

use crate::history::Entry;
use crate::i18n::{Lang, S};
use crate::icons::{self, Icon};
use crate::raster::{Canvas, Glyphs, IBox, Mask, Rgba};
use crate::theme::Theme;

/// Cell size in DIPs: a 16:10 thumbnail and one caption line.
pub const THUMB_W: f32 = crate::theme::HISTORY_THUMB_W;
pub const THUMB_H: f32 = crate::theme::HISTORY_THUMB_H;
pub const CAPTION: f32 = crate::theme::HISTORY_CAPTION;
pub const GAP: f32 = crate::theme::HISTORY_GAP;
pub const MARGIN: f32 = crate::theme::HISTORY_MARGIN;
pub const HEADER: f32 = crate::theme::HISTORY_HEADER;
pub const ACTION: f32 = crate::theme::HISTORY_ACTION;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Copy,
    Pin,
    Remove,
}

pub const ACTS: [Act; 3] = [Act::Copy, Act::Pin, Act::Remove];

impl Act {
    pub fn icon(self) -> Icon {
        match self {
            Act::Copy => Icon::Copy,
            Act::Pin => Icon::Pin,
            Act::Remove => Icon::Trash,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Cell(usize),
    Action(usize, Act),
    OpenFolder,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub w: i32,
    pub h: i32,
    pub scale: f32,
    pub cols: usize,
    pub count: usize,
    pub scroll: i32,
    /// Thumbnail size: columns stretch so the grid ends on both margins.
    pub cell_w: i32,
    pub cell_h: i32,
    pub folder_hover: bool,
}

impl Grid {
    pub fn new(w: i32, h: i32, scale: f32, count: usize) -> Grid {
        let s = |d: f32| (d * scale).round() as i32;
        let avail = w - 2 * s(MARGIN) + s(GAP);
        let cols = (avail / (s(THUMB_W) + s(GAP))).max(1) as usize;
        let cell_w = ((avail - cols as i32 * s(GAP)) / cols as i32).max(s(THUMB_W).min(1));
        let cell_h = (cell_w as f32 * THUMB_H / THUMB_W).round() as i32;
        Grid { w, h, scale, cols, count, scroll: 0, cell_w, cell_h, folder_hover: false }
    }

    fn s(&self, d: f32) -> i32 {
        (d * self.scale).round() as i32
    }

    pub fn rows(&self) -> usize {
        self.count.div_ceil(self.cols)
    }

    pub fn row_h(&self) -> i32 {
        self.cell_h + self.s(CAPTION) + self.s(GAP)
    }

    /// Height of all rows (content below the header).
    pub fn content_h(&self) -> i32 {
        self.rows() as i32 * self.row_h() + 2 * self.s(MARGIN)
    }

    pub fn max_scroll(&self) -> i32 {
        (self.content_h() - (self.h - self.s(HEADER))).max(0)
    }

    pub fn scroll_by(&mut self, dy: i32) {
        self.scroll = (self.scroll + dy).clamp(0, self.max_scroll());
    }

    /// The thumbnail box of item `i` in window coordinates (may be off screen).
    pub fn thumb_box(&self, i: usize) -> IBox {
        let (r, c) = ((i / self.cols) as i32, (i % self.cols) as i32);
        let x = self.s(MARGIN) + c * (self.cell_w + self.s(GAP));
        let y = self.s(HEADER) + self.s(MARGIN) + r * self.row_h() - self.scroll;
        IBox::xywh(x, y, self.cell_w, self.cell_h)
    }

    pub fn action_box(&self, i: usize, a: Act) -> IBox {
        let t = self.thumb_box(i);
        let k = ACTS.iter().position(|x| *x == a).unwrap_or(0) as i32;
        let sz = self.s(ACTION);
        let pad = self.s(crate::theme::HISTORY_ACTION_PAD);
        IBox::xywh(t.x1 - pad - (3 - k) * sz - (2 - k) * self.s(crate::theme::HISTORY_ACTION_GAP), t.y0 + pad, sz, sz)
    }

    pub fn folder_box(&self) -> IBox {
        let (w, h) = (self.s(crate::theme::HISTORY_FOLDER_W), self.s(crate::theme::HISTORY_FOLDER_H));
        IBox::xywh(self.w - self.s(MARGIN) - w, (self.s(HEADER) - h) / 2, w, h)
    }

    pub fn hit(&self, x: i32, y: i32, hovered: Option<usize>) -> Option<Hit> {
        if self.folder_box().contains(x, y) {
            return Some(Hit::OpenFolder);
        }
        if y < self.s(HEADER) {
            return None;
        }
        if let Some(i) = hovered {
            for a in ACTS {
                if self.action_box(i, a).contains(x, y) {
                    return Some(Hit::Action(i, a));
                }
            }
        }
        (0..self.count).find(|&i| {
            let b = self.thumb_box(i);
            IBox::new(b.x0, b.y0, b.x1, b.y1 + self.s(CAPTION)).contains(x, y)
        }).map(Hit::Cell)
    }

    /// Scroll so item `i` is fully visible.
    pub fn reveal(&mut self, i: usize) {
        let b = self.thumb_box(i);
        let top = self.s(HEADER) + self.s(MARGIN);
        if b.y0 < top {
            self.scroll_by(b.y0 - top - self.s(GAP));
        } else if b.y1 + self.s(CAPTION) > self.h {
            self.scroll_by(b.y1 + self.s(CAPTION) - self.h + self.s(GAP));
        }
    }

    /// Arrow-key movement in the grid.
    pub fn step(&self, i: Option<usize>, dx: i32, dy: i32) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        let Some(i) = i else { return Some(0) };
        let j = i as i64 + dx as i64 + dy as i64 * self.cols as i64;
        Some(j.clamp(0, self.count as i64 - 1) as usize)
    }
}

pub struct Item<'a> {
    pub entry: &'a Entry,
    pub thumb: Option<&'a Canvas>,
    pub missing: bool,
}

/// The day before "YYYY-MM-DD", in the same form.
fn day_before(today: &str) -> Option<String> {
    let (y, m, d): (i32, u32, u32) = (today.get(0..4)?.parse().ok()?, today.get(5..7)?.parse().ok()?, today.get(8..10)?.parse().ok()?);
    if d > 1 {
        return Some(format!("{y:04}-{m:02}-{:02}", d - 1));
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let (py, pm) = if m > 1 { (y, m - 1) } else { (y - 1, 12) };
    let last = match pm {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    Some(format!("{py:04}-{pm:02}-{last:02}"))
}

/// A capture's time in words: "Bugün 17:52", "Dün 09:03", "7 Kas 17:52".
pub fn when_label(time: &str, today: &str, lang: Lang) -> String {
    let date = time.get(0..10).unwrap_or("");
    let clock = time.get(11..16).unwrap_or("");
    if !date.is_empty() && date == today {
        return format!("{} {clock}", S::Today.get(lang));
    }
    if !date.is_empty() && day_before(today).as_deref() == Some(date) {
        return format!("{} {clock}", S::Yesterday.get(lang));
    }
    let month = time.get(5..7).and_then(|m| m.parse::<usize>().ok()).filter(|m| (1..=12).contains(m));
    let day = time.get(8..10).and_then(|d| d.parse::<u32>().ok());
    match (month, day) {
        (Some(m), Some(d)) => {
            let names = if lang == Lang::Tr { crate::i18n::MONTHS_TR } else { crate::i18n::MONTHS_EN };
            format!("{d} {} {clock}", names[m - 1])
        }
        _ => time.to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render(g: &Grid, t: &Theme, lang: Lang, items: &[Item], hover: Option<usize>, focus: Option<usize>, today: &str, glyphs: &dyn Glyphs) -> Canvas {
    let mut cv = Canvas::new(g.w as u32, g.h as u32, t.window);
    let s = |d: f32| (d * g.scale).round() as i32;
    let font = t.font * g.scale;
    let small = t.font_small * g.scale;
    let top = s(HEADER);
    // Cells first, then the header on top (cells scroll under it).
    for (i, it) in items.iter().enumerate() {
        let b = g.thumb_box(i);
        if b.y1 + s(CAPTION) < top || b.y0 > g.h {
            continue;
        }
        let radius = t.control_radius() * g.scale;
        let mut base = Mask::new(b);
        base.rounded_box(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32, radius);
        cv.blend_mask(&base, t.card);
        if let Some(th) = it.thumb {
            let k = (b.w() as f32 / th.w as f32).min(b.h() as f32 / th.h as f32);
            let (nw, nh) = (((th.w as f32 * k).round() as u32).max(1), ((th.h as f32 * k).round() as u32).max(1));
            let img = if (nw, nh) == (th.w, th.h) { th.clone() } else { th.resample_box(nw, nh) };
            cv.draw(&img, b.x0 + (b.w() - nw as i32) / 2, b.y0 + (b.h() - nh as i32) / 2);
            // Round the picture's corners too: the window's ground over what lies outside.
            let mut corners = Mask::new(b);
            for (c, v) in corners.cov.iter_mut().zip(base.cov.iter()) {
                *c = 255 - *v;
            }
            cv.blend_mask(&corners, t.window);
        }
        let ring = if focus == Some(i) { t.accent } else if hover == Some(i) { t.muted } else { t.border };
        let bw = if focus == Some(i) { s(crate::theme::HISTORY_FOCUS_WIDTH) } else { s(crate::theme::BORDER_WIDTH) }.max(1);
        let mut m = Mask::new(IBox::new(b.x0 - bw, b.y0 - bw, b.x1 + bw, b.y1 + bw));
        m.rounded_box_outline(b.x0 as f32 - bw as f32 + 1.0, b.y0 as f32 - bw as f32 + 1.0, b.x1 as f32 + bw as f32 - 1.0, b.y1 as f32 + bw as f32 - 1.0, radius, bw as f32);
        cv.blend_mask(&m, ring);
        // Caption: when on the left, the size on the right, each in one place.
        let cap_y = b.y1 + s(crate::theme::HISTORY_CAPTION_GAP);
        cv.text(glyphs, &when_label(&it.entry.time, today, lang), small, false, b.x0, cap_y, t.text);
        let (size_text, size_color) = if it.missing { (S::FileMissing.get(lang).to_string(), t.danger) } else { (format!("{}\u{00D7}{}", it.entry.w, it.entry.h), t.muted) };
        let sm = glyphs.text(&size_text, small, false);
        cv.text_mask(&sm, b.x1 - sm.w as i32, cap_y, size_color);
        if hover == Some(i) {
            for a in ACTS {
                let ab = g.action_box(i, a);
                let mut bg = Mask::new(ab);
                let rad = t.control_radius() * g.scale;
                bg.rounded_box(ab.x0 as f32, ab.y0 as f32, ab.x1 as f32, ab.y1 as f32, rad);
                cv.blend_mask(&bg, t.bg);
                // A 1-DIP border so the button shows on a white screenshot too.
                let mut ring = Mask::new(ab);
                ring.rounded_box_outline(ab.x0 as f32, ab.y0 as f32, ab.x1 as f32, ab.y1 as f32, rad, (crate::theme::BORDER_WIDTH * g.scale).max(1.0));
                cv.blend_mask(&ring, t.border);
                let isz = crate::theme::HISTORY_ACTION_ICON * g.scale;
                icons::draw(&mut cv, a.icon(), ab.x0 as f32 + (ab.w() as f32 - isz) / 2.0, ab.y0 as f32 + (ab.h() as f32 - isz) / 2.0, isz, if a == Act::Remove { t.danger } else { t.text });
            }
        }
    }
    cv.fill_box(IBox::new(0, 0, g.w, top), t.window);
    // The line shows only when something has scrolled under the header.
    if g.scroll > 0 {
        cv.fill_box(IBox::new(0, top - s(crate::theme::BORDER_WIDTH).max(1), g.w, top), t.border);
    }
    let title = glyphs.text(S::History.get(lang), crate::theme::TITLE_FONT * g.scale, true);
    let title_y = (top - title.h as i32) / 2;
    cv.text_mask(&title, s(MARGIN), title_y, t.text);
    let count = glyphs.text(&format!("{} {}", g.count, S::HistoryCount.get(lang)), small, false);
    if g.count > 0 {
        cv.text_mask(&count, s(MARGIN) + title.w as i32 + s(crate::theme::HISTORY_COUNT_GAP), title_y + title.ascent as i32 - count.ascent as i32, t.muted);
    }
    let fb = g.folder_box();
    let mut fm = Mask::new(fb);
    fm.rounded_box(fb.x0 as f32, fb.y0 as f32, fb.x1 as f32, fb.y1 as f32, t.control_radius() * g.scale);
    cv.blend_mask(&fm, if g.folder_hover { t.hover } else { t.card });
    let mut fo = Mask::new(fb);
    fo.rounded_box_outline(fb.x0 as f32, fb.y0 as f32, fb.x1 as f32, fb.y1 as f32, t.control_radius() * g.scale, (crate::theme::BORDER_WIDTH * g.scale).max(1.0));
    cv.blend_mask(&fo, t.border);
    let label = glyphs.text(S::OpenFolderShort.get(lang), font, false);
    let isz = crate::theme::HISTORY_FOLDER_ICON * g.scale;
    let inner_gap = crate::theme::LABEL_PAD_X * g.scale;
    let start = fb.x0 as f32 + (fb.w() as f32 - isz - inner_gap - label.w as f32) / 2.0;
    icons::draw(&mut cv, Icon::Folder, start, fb.y0 as f32 + (fb.h() as f32 - isz) / 2.0, isz, t.text);
    cv.text_mask(&label, (start + isz + inner_gap) as i32, fb.y0 + (fb.h() - label.h as i32) / 2, t.text);
    if g.count == 0 {
        // Empty: what this window is for and how to fill it.
        let isz = crate::theme::HISTORY_EMPTY_ICON * g.scale;
        let gap = s(crate::theme::HISTORY_EMPTY_GAP);
        let head = glyphs.text(S::HistoryEmpty.get(lang), font * crate::theme::TEXT_LINE_SCALE, true);
        let hint = glyphs.text(S::HistoryEmptyHint.get(lang), font, false);
        let total = isz as i32 + gap + head.h as i32 + gap / 2 + hint.h as i32;
        let mut y = top + (g.h - top - total) / 2;
        icons::draw(&mut cv, Icon::Region, (g.w as f32 - isz) / 2.0, y as f32, isz, t.muted);
        y += isz as i32 + gap;
        cv.text_mask(&head, (g.w - head.w as i32) / 2, y, t.text);
        y += head.h as i32 + gap / 2;
        cv.text_mask(&hint, (g.w - hint.w as i32) / 2, y, t.muted);
    }
    if g.max_scroll() > 0 {
        let track = g.h - top;
        let thumb_h = ((track as f32 * track as f32 / (g.content_h() as f32)).max(crate::theme::HISTORY_SCROLL_MIN * g.scale)) as i32;
        let y = top + ((track - thumb_h) as f32 * g.scroll as f32 / g.max_scroll() as f32) as i32;
        let x = g.w - s(crate::theme::HISTORY_SCROLL_RIGHT);
        let mut m = Mask::new(IBox::xywh(x, y, s(crate::theme::HISTORY_SCROLL_WIDTH), thumb_h));
        m.rounded_box(x as f32, y as f32, (x + s(crate::theme::HISTORY_SCROLL_WIDTH)) as f32, (y + thumb_h) as f32, crate::theme::HISTORY_SCROLL_RADIUS * g.scale);
        cv.blend_mask(&m, t.muted.with_alpha(crate::theme::HISTORY_SCROLL_ALPHA));
    }
    cv
}

/// Pick the colour of the "Copied" note: plain text, accent colour.
pub fn note_color(t: &Theme) -> Rgba {
    t.accent
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::BoxFont;
    use crate::theme::{Look, Mode};

    #[test]
    fn columns_follow_the_width_and_scale() {
        assert_eq!(Grid::new(900, 600, 1.0, 50).cols, 5);
        assert_eq!(Grid::new(900, 600, 1.5, 50).cols, 3);
        assert_eq!(Grid::new(100, 600, 1.0, 50).cols, 1, "never zero");
    }

    #[test]
    fn hits_find_cells_and_actions() {
        let g = Grid::new(900, 600, 1.0, 12);
        for i in 0..12 {
            let b = g.thumb_box(i);
            if b.y1 > g.h {
                continue;
            }
            assert_eq!(g.hit(b.x0 + 5, b.y1 - 5, None), Some(Hit::Cell(i)));
            for a in ACTS {
                let ab = g.action_box(i, a);
                assert!(b.contains(ab.x0, ab.y0) && b.contains(ab.x1 - 1, ab.y1 - 1), "{a:?} inside the thumbnail");
                assert_eq!(g.hit(ab.x0 + 1, ab.y0 + 1, Some(i)), Some(Hit::Action(i, a)));
                assert_eq!(g.hit(ab.x0 + 1, ab.y0 + 1, None), Some(Hit::Cell(i)), "actions only on the hovered cell");
            }
        }
        assert_eq!(g.hit(5, 5, None), None, "header");
        let f = g.folder_box();
        assert_eq!(g.hit(f.x0 + 1, f.y0 + 1, None), Some(Hit::OpenFolder));
    }

    #[test]
    fn scrolling_is_clamped_and_reveal_brings_items_into_view() {
        let mut g = Grid::new(900, 400, 1.0, 50);
        assert!(g.max_scroll() > 0);
        g.scroll_by(-100);
        assert_eq!(g.scroll, 0);
        g.scroll_by(1_000_000);
        assert_eq!(g.scroll, g.max_scroll());
        g.reveal(0);
        assert!(g.thumb_box(0).y0 >= (HEADER) as i32);
        g.reveal(49);
        let b = g.thumb_box(49);
        assert!(b.y1 + CAPTION as i32 <= g.h);
        let small = Grid::new(900, 2000, 1.0, 3);
        assert_eq!(small.max_scroll(), 0);
    }

    #[test]
    fn captions_say_when_in_words() {
        assert_eq!(when_label("2026-11-07 17:52:12", "2026-11-07", Lang::Tr), "Bugün 17:52");
        assert_eq!(when_label("2026-11-06 09:03:00", "2026-11-07", Lang::Tr), "Dün 09:03");
        assert_eq!(when_label("2026-10-31 09:03:00", "2026-11-01", Lang::En), "Yesterday 09:03");
        assert_eq!(when_label("2025-12-31 23:59:00", "2026-01-01", Lang::Tr), "Dün 23:59");
        assert_eq!(when_label("2024-02-29 08:00:00", "2024-03-01", Lang::En), "Yesterday 08:00");
        assert_eq!(when_label("2026-11-05 17:52:12", "2026-11-07", Lang::Tr), "5 Kas 17:52");
        assert_eq!(when_label("2026-02-14 10:00:00", "2026-11-07", Lang::En), "14 Feb 10:00");
        assert_eq!(when_label("broken", "2026-11-07", Lang::En), "broken");
    }

    #[test]
    fn the_grid_ends_on_both_margins() {
        let g = Grid::new(900, 600, 1.0, 12);
        assert_eq!(g.thumb_box(0).x0, MARGIN as i32);
        let right = g.w - g.thumb_box(g.cols - 1).x1;
        assert!((right - MARGIN as i32).abs() < g.cols as i32, "right margin {right}");
    }

    #[test]
    fn arrow_steps_stay_in_range() {
        let g = Grid::new(900, 600, 1.0, 12); // 5 columns
        assert_eq!(g.step(None, 1, 0), Some(0));
        assert_eq!(g.step(Some(0), 0, 1), Some(5));
        assert_eq!(g.step(Some(11), 1, 0), Some(11));
        assert_eq!(g.step(Some(1), 0, -1), Some(0));
        assert_eq!(Grid::new(900, 600, 1.0, 0).step(None, 1, 0), None);
    }

    #[test]
    fn renders_empty_and_full_states() {
        let t = Theme::new(Look::Graphite, Mode::Dark);
        let g = Grid::new(600, 400, 1.0, 0);
        let empty = render(&g, &t, Lang::Tr, &[], None, None, "2026-11-07", &BoxFont);
        assert_eq!((empty.w, empty.h), (600, 400));
        let e = Entry { id: 1, time: "2026-11-04 19:50:12".into(), path: "x".into(), owned: false, w: 1280, h: 720, source: "snip".into() };
        let th = Canvas::new(200, 113, Rgba::rgb(200, 0, 0));
        let items = [Item { entry: &e, thumb: Some(&th), missing: false }];
        let g = Grid::new(600, 400, 1.0, 1);
        let full = render(&g, &t, Lang::En, &items, Some(0), Some(0), "2026-11-07", &BoxFont);
        let b = g.thumb_box(0);
        assert_eq!(full.get(b.x0 + 10, b.y0 + b.h() / 2), Rgba::rgb(200, 0, 0));
    }
}
