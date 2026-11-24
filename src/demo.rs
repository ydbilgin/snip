//! The README films: snip's own screens driven by a script over made-up screens (apps on a
//! desktop, a game drawn from flat shapes) and written as GIFs. Each film is a story: a game
//! moment, a bug report, code copied from a paused video, a clip, a pinned reference. Nothing here opens a window, reads the screen or sends input. The editor is the real core
//! state machine fed with scripted pointer and key events; the plain, text and recording overlays
//! are drawn from the same core pieces the overlay window blits (mode bar, hints, size labels,
//! selection frame, result card, recording control), in the order the overlay draws them.
//! `snip --demo-gif DIR` writes the films with the real Windows font and the copy-code film reads
//! its words with the Windows OCR; the tests run the same scripts with the box font.
//!
//! A film frame is the screen (1040 x 650 DIPs at 150 %, so 1560 x 975 pixels) and, under it, a
//! strip that says what is happening and which keys are pressed. The strip is narration, not app.

use std::io::Write;

use crate::beautify;
use crate::dpi::{Monitor, Monitors};
use crate::editor::{CursorKind, Editor, Effect, Phase, Prefs};
use crate::geometry::{Point, Rect};
use crate::hookstate::VK_RETURN;
use crate::i18n::Lang;
use crate::raster::{Canvas, Glyphs, IBox, Mask, Pt, Rgba};
use crate::textcore::TextOutcome;
use crate::theme::{self, Look, Mode, Theme};
use crate::toolbar::{self, Bar, BarState, Item};

/// The screen in DIPs.
pub const W: i32 = 1040;
pub const H: i32 = 650;
pub const TASKBAR: i32 = 48;
/// The narration strip under the screen, in DIPs.
pub const STRIP: i32 = 56;
/// The films' monitor: 150 %, a common laptop scale. README shows them at half size, so they stay
/// sharp on a high-density screen.
pub const DPI: u32 = 144;
/// Hundredths of a second per frame (12.5 frames a second).
pub const DELAY: u16 = 8;

const INK: Rgba = Rgba::rgb(0x23, 0x26, 0x2B);
const SOFT: Rgba = Rgba::rgb(0x6A, 0x70, 0x78);
const RULE: Rgba = Rgba::rgb(0xE2, 0xE3, 0xE0);
const PAGE: Rgba = Rgba::rgb(0xFB, 0xFB, 0xF9);
const SIDE: Rgba = Rgba::rgb(0xF1, 0xF1, 0xEE);
const BLUE: Rgba = Rgba::rgb(0x2F, 0x6F, 0xDB);
const WALL: Rgba = Rgba::rgb(0x4E, 0x5B, 0x67);
const BAR_BG: Rgba = Rgba::rgb(0xEE, 0xEF, 0xF0);
const RESULT_BG: Rgba = Rgba::rgb(0x1C, 0x1C, 0x1F);
const STRIP_BG: Rgba = Rgba::rgb(0x16, 0x17, 0x1A);
const STRIP_TEXT: Rgba = Rgba::rgb(0xE4, 0xE4, 0xE7);
const CAP_BG: Rgba = Rgba::rgb(0xF4, 0xF4, 0xF5);
const CAP_EDGE: Rgba = Rgba::rgb(0xA1, 0xA1, 0xAA);
/// The recording bars' colour (record_win.rs paints them with this brush).
const REC_BAR: Rgba = Rgba::rgb(0xC0, 0x30, 0x30);

/// The two windows on the made-up desktop, in DIPs: a notebook and, on top of it, a chart.
pub const WIN_A: (i32, i32, i32, i32) = (36, 30, 676, 574);
pub const WIN_B: (i32, i32, i32, i32) = (620, 96, 1004, 430);
/// The notebook brought to the front and widened (the pin film).
pub const NOTES_WIDE: (i32, i32, i32, i32) = (36, 30, 1004, 574);
/// The invoice with the wrong total (the bug-report film).
pub const INVOICE: (i32, i32, i32, i32) = (150, 40, 890, 556);
/// The issue form the bug report is pasted into, and its picture box.
pub const ISSUE: (i32, i32, i32, i32) = (110, 24, 930, 594);
const ISSUE_DROP: (i32, i32, i32, i32) = (140, 252, 900, 536);
/// The paused tutorial video and the terminal beside it (the copy-code film).
pub const VIDEO: (i32, i32, i32, i32) = (30, 26, 640, 520);
pub const TERM: (i32, i32, i32, i32) = (650, 230, 1010, 592);
/// The chart's bars: colour and height in DIPs.
const BARS: [(Rgba, i32); 5] = [
    (Rgba::rgb(0x2F, 0x6F, 0xDB), 120),
    (Rgba::rgb(0x2D, 0xA4, 0x4E), 170),
    (Rgba::rgb(0xF2, 0x8C, 0x28), 95),
    (Rgba::rgb(0x82, 0x50, 0xDF), 145),
    (Rgba::rgb(0xE5, 0x38, 0x3B), 70),
];
const CHART_X: i32 = 664;
const CHART_STEP: i32 = 62;
const CHART_BASE: i32 = 400;
/// The notebook's paragraph.
pub const PARAGRAPH: [&str; 3] = ["Snip freezes every screen at once, so menus", "and tooltips stay put while you pick the area.", "Copy the text in any picture with Win+Shift+T."];
const CHECKS: [&str; 4] = ["Magnifier while selecting", "Select and move shapes", "Spotlight and ellipse", "Picture frame"];
/// What the pin film types into the notebook, line by line, looking at the pinned chart.
pub const NOTE: [&str; 2] = ["Tuesday was the busiest day of the week,", "Friday the quietest."];
/// The commands on the paused tutorial's slide (the copy-code film reads them).
pub const CODE: [&str; 3] = ["git pull origin main", "npm install", "npm run build"];
/// The game frame the editor film freezes: the hero just before the pit.
pub const GAME_STILL_T: u32 = 8;
/// Coins the hero has before the run.
const COINS: u32 = 1284;

/// Which made-up screen a film shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Place {
    /// A notebook and a chart on the desktop.
    #[default]
    Work,
    /// The notebook in front, widened, over the chart.
    Notes,
    /// A game, full screen, at frame `Desk::t`.
    Game,
    /// An invoice whose total is wrong.
    Invoice,
    /// A bug tracker's "new issue" form.
    Issue,
    /// A paused tutorial video and a terminal.
    Video,
}

/// What can change on the made-up screen while a film runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Desk {
    pub place: Place,
    pub checks: [bool; 4],
    /// The game's frame: where the hero is on the run.
    pub t: u32,
    /// Characters of `NOTE` typed into the notebook.
    pub typed: usize,
    /// Lines pasted into the terminal.
    pub terminal: Vec<String>,
}

impl Default for Desk {
    fn default() -> Self {
        Desk { place: Place::Work, checks: [true, true, false, false], t: 0, typed: 0, terminal: Vec::new() }
    }
}

impl Desk {
    pub fn at(place: Place) -> Desk {
        Desk { place, ..Desk::default() }
    }
    pub fn game(t: u32) -> Desk {
        Desk { place: Place::Game, t, ..Desk::default() }
    }
}

/// The windows a place shows, top first (DIPs): what a click on the overlay can pick.
fn windows_of(place: Place) -> Vec<(i32, i32, i32, i32)> {
    match place {
        Place::Work => vec![WIN_B, WIN_A],
        Place::Notes => vec![NOTES_WIDE, WIN_B],
        Place::Game => vec![],
        Place::Invoice => vec![INVOICE],
        Place::Issue => vec![ISSUE],
        Place::Video => vec![TERM, VIDEO],
    }
}

fn windows_px(place: Place, k: f32) -> Vec<Rect> {
    windows_of(place).iter().map(|r| Rect::new(px(r.0, k), px(r.1, k), px(r.2, k), px(r.3, k))).collect()
}

fn px(v: i32, k: f32) -> i32 {
    (v as f32 * k).round() as i32
}

/// A window's colours: light apps, or the dark video player and terminal.
#[derive(Clone, Copy)]
struct Skin {
    page: Rgba,
    bar: Rgba,
    ink: Rgba,
    rule: Rgba,
}

const LIGHT: Skin = Skin { page: PAGE, bar: SIDE, ink: INK, rule: RULE };
const DARK: Skin = Skin { page: Rgba::rgb(0x1C, 0x1C, 0x1C), bar: Rgba::rgb(0x20, 0x20, 0x20), ink: Rgba::rgb(0xE6, 0xE6, 0xE6), rule: Rgba::rgb(0x2E, 0x2E, 0x2E) };

/// Draws in DIPs on a canvas of pixels.
struct Pen<'a> {
    cv: Canvas,
    k: f32,
    g: &'a dyn Glyphs,
}

impl Pen<'_> {
    fn b(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> IBox {
        IBox::new(px(x0, self.k), px(y0, self.k), px(x1, self.k), px(y1, self.k))
    }
    fn fill(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Rgba) {
        let b = self.b(x0, y0, x1, y1);
        self.cv.fill_box(b, c);
    }
    fn round(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, r: f32, c: Rgba) {
        let b = self.b(x0, y0, x1, y1);
        let mut m = Mask::new(b);
        m.rounded_box(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32, r * self.k);
        self.cv.blend_mask(&m, c);
    }
    /// A rounded box with a 1-DIP edge.
    #[allow(clippy::too_many_arguments)]
    fn field(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, r: f32, edge: Rgba, c: Rgba) {
        self.round(x0, y0, x1, y1, r, edge);
        let b = self.b(x0, y0, x1, y1);
        let one = self.k.round().max(1.0);
        let mut m = Mask::new(b);
        m.rounded_box(b.x0 as f32 + one, b.y0 as f32 + one, b.x1 as f32 - one, b.y1 as f32 - one, (r * self.k - one).max(0.0));
        self.cv.blend_mask(&m, c);
    }
    fn disc(&mut self, x: f32, y: f32, r: f32, c: Rgba) {
        let mut m = Mask::new(self.cv.bounds());
        m.disc(Pt::new(x * self.k, y * self.k), r * self.k);
        self.cv.blend_mask(&m, c);
    }
    /// A filled convex polygon, points in DIPs.
    fn poly(&mut self, pts: &[(f32, f32)], c: Rgba) {
        let v: Vec<Pt> = pts.iter().map(|(x, y)| Pt::new(x * self.k, y * self.k)).collect();
        let mut m = Mask::new(self.cv.bounds());
        m.convex(&v);
        self.cv.blend_mask(&m, c);
    }
    fn line(&mut self, a: (f32, f32), b: (f32, f32), w: f32, c: Rgba) {
        let mut m = Mask::new(self.cv.bounds());
        m.capsule(Pt::new(a.0 * self.k, a.1 * self.k), Pt::new(b.0 * self.k, b.1 * self.k), w * self.k);
        self.cv.blend_mask(&m, c);
    }
    /// A four-pointed sparkle.
    fn star(&mut self, x: f32, y: f32, r: f32, c: Rgba) {
        let n = r * 0.28;
        self.poly(&[(x, y - r), (x + n, y), (x, y + r), (x - n, y)], c);
        self.poly(&[(x - r, y), (x, y - n), (x + r, y), (x, y + n)], c);
    }
    fn text(&mut self, s: &str, size: f32, bold: bool, x: i32, y: i32, c: Rgba) {
        let (xx, yy) = (px(x, self.k), px(y, self.k));
        self.cv.text(self.g, s, size * self.k, bold, xx, yy, c);
    }
    fn width(&self, s: &str, size: f32, bold: bool) -> i32 {
        (self.g.text(s, size * self.k, bold).w as f32 / self.k).round() as i32
    }
    /// Text whose right edge is at `xr`.
    fn text_right(&mut self, s: &str, size: f32, bold: bool, xr: i32, y: i32, c: Rgba) {
        let w = self.g.text(s, size * self.k, bold).w as i32;
        let yy = px(y, self.k);
        self.cv.text(self.g, s, size * self.k, bold, px(xr, self.k) - w, yy, c);
    }
    /// A Windows 11 window: soft shadow, 8-DIP corners and a 1-DIP edge; the title bar in the
    /// window's own colour, the app's icon and title at the left, minimize, maximize and close at
    /// the right (46-DIP buttons, 10-DIP glyphs drawn with 1-DIP lines).
    fn window(&mut self, r: (i32, i32, i32, i32), title: &str, bar: i32, icon: Rgba, s: Skin) {
        let b = self.b(r.0, r.1, r.2, r.3);
        let (sh, mx, my) = toolbar::render_shadow(b.w(), b.h(), &Theme::new(Look::Graphite, Mode::Light), 1.6 * self.k);
        self.cv.draw(&sh, b.x0 - mx, b.y0 - my);
        self.round(r.0, r.1, r.2, r.3, 8.0, s.page);
        self.round(r.0, r.1, r.2, r.1 + bar, 8.0, s.bar);
        self.fill(r.0, r.1 + bar - 8, r.2, r.1 + bar, s.bar);
        self.fill(r.0, r.1 + bar, r.2, r.1 + bar + 1, s.rule);
        let one = self.k;
        let mut edge = Mask::new(b);
        edge.rounded_box_outline(b.x0 as f32 + one / 2.0, b.y0 as f32 + one / 2.0, b.x1 as f32 - one / 2.0, b.y1 as f32 - one / 2.0, 8.0 * self.k - one / 2.0, one);
        self.cv.blend_mask(&edge, Rgba::rgb(0, 0, 0).with_alpha(36));
        let cy = r.1 + bar / 2;
        self.round(r.0 + 14, cy - 8, r.0 + 30, cy + 8, 4.0, icon);
        self.round(r.0 + 19, cy - 3, r.0 + 25, cy + 3, 1.5, Rgba::rgb(255, 255, 255).with_alpha(170));
        self.text(title, 13.0, false, r.0 + 42, cy - 9, s.ink);
        let k = self.k;
        let (y, h) = (cy as f32 * k, 5.0 * k);
        let at = |n: i32| (r.2 - 23 - 46 * n) as f32 * k;
        let mut m = Mask::new(self.cv.bounds());
        let x = at(2);
        m.polyline(&[Pt::new(x - h, y), Pt::new(x + h, y)], one);
        let x = at(1);
        m.rounded_box_outline(x - h, y - h, x + h, y + h, 1.5 * k, one);
        let x = at(0);
        m.polyline(&[Pt::new(x - h, y - h), Pt::new(x + h, y + h)], one);
        m.polyline(&[Pt::new(x + h, y - h), Pt::new(x - h, y + h)], one);
        self.cv.blend_mask(&m, s.ink);
    }
}

/// The made-up screen at `k` pixels per DIP. Plain shapes and text, so every run draws the same
/// pixels; the game is original art made of flat shapes.
pub fn scene(g: &dyn Glyphs, k: f32, desk: &Desk) -> Canvas {
    let mut p = Pen { cv: Canvas::new(px(W, k) as u32, px(H, k) as u32, WALL), k, g };
    if desk.place == Place::Game {
        game(&mut p, desk.t);
        return p.cv;
    }
    taskbar(&mut p);
    match desk.place {
        Place::Work => {
            notebook(&mut p, WIN_A, desk);
            chart(&mut p);
        }
        Place::Notes => {
            chart(&mut p);
            notebook(&mut p, NOTES_WIDE, desk);
        }
        Place::Invoice => invoice(&mut p),
        Place::Issue => issue(&mut p),
        Place::Video => {
            video(&mut p);
            terminal(&mut p, &desk.terminal);
        }
        Place::Game => {}
    }
    p.cv
}

/// The taskbar: a light bar, a few plain app squares in the middle, the time on the right.
fn taskbar(p: &mut Pen<'_>) {
    p.fill(0, H - TASKBAR, W, H, BAR_BG);
    p.fill(0, H - TASKBAR, W, H - TASKBAR + 1, Rgba::rgb(0xD6, 0xD7, 0xD9));
    let apps = [Rgba::rgb(0x2F, 0x6F, 0xDB), Rgba::rgb(0xE8, 0xB8, 0x4A), Rgba::rgb(0x3C, 0x8D, 0x6E), Rgba::rgb(0x6A, 0x70, 0xC8), Rgba::rgb(0x2B, 0x2F, 0x36)];
    let x0 = W / 2 - (apps.len() as i32 * 40) / 2;
    for (i, c) in apps.iter().enumerate() {
        let x = x0 + i as i32 * 40 + 6;
        p.round(x, H - TASKBAR + 10, x + 28, H - TASKBAR + 38, 6.0, *c);
        p.round(x + 9, H - TASKBAR + 19, x + 19, H - TASKBAR + 29, 2.0, Rgba::rgb(255, 255, 255).with_alpha(150));
    }
    p.text("14:20", 13.0, false, W - 58, H - TASKBAR + 15, INK);
}

/// The notebook: a list, a paragraph, a checklist, a button and, in the pin film, typed notes.
fn notebook(p: &mut Pen<'_>, a: (i32, i32, i32, i32), desk: &Desk) {
    p.window(a, "Release notes - Notebook", 40, Rgba::rgb(0x2F, 0x6F, 0xDB), LIGHT);
    let side = (a.0, a.1 + 41, a.0 + 170, a.3);
    p.fill(side.0, side.1, side.2, side.3 - 8, SIDE);
    p.round(side.0, side.3 - 16, side.2, side.3, 8.0, SIDE);
    p.fill(side.0 + 8, side.3 - 16, side.2, side.3, SIDE);
    p.fill(side.2, side.1, side.2 + 1, side.3, RULE);
    for (i, item) in ["Inbox", "Release notes", "Design review", "Roadmap", "Archive"].iter().enumerate() {
        let y = side.1 + 18 + i as i32 * 34;
        if i == 1 {
            p.round(side.0 + 10, y - 7, side.2 - 10, y + 24, 6.0, Rgba::rgb(0xE2, 0xE9, 0xF8));
        }
        p.text(item, 14.0, i == 1, side.0 + 24, y, if i == 1 { BLUE } else { INK });
    }
    let x = side.2 + 32;
    p.text("Version 2.0", 26.0, true, x, a.1 + 58, INK);
    p.text("Tuesday, 14:20  -  4 changes, 2 waiting", 13.0, false, x, a.1 + 98, SOFT);
    for (i, line) in PARAGRAPH.iter().enumerate() {
        p.text(line, 15.0, false, x, a.1 + 132 + i as i32 * 24, INK);
    }
    for (i, label) in CHECKS.iter().enumerate() {
        let y = a.1 + 222 + i as i32 * 30;
        let done = desk.checks[i];
        p.round(x, y, x + 16, y + 16, 4.0, if done { BLUE } else { RULE });
        if done {
            crate::icons::draw(&mut p.cv, crate::icons::Icon::Check, (x as f32) * p.k, (y as f32) * p.k, 16.0 * p.k, Rgba::rgb(255, 255, 255));
        }
        p.text(label, 15.0, false, x + 28, y - 1, if done { SOFT } else { INK });
    }
    p.round(x, a.1 + 360, x + 128, a.1 + 396, 6.0, BLUE);
    p.text("Publish", 15.0, true, x + 36, a.1 + 368, Rgba::rgb(255, 255, 255));
    p.text("Draft saved", 13.0, false, x + 146, a.1 + 370, SOFT);
    p.text("Reviewer: dana.lee@example.com", 13.0, false, x, a.1 + 424, SOFT);
    // Typed notes, with the caret after the last character.
    let mut left = desk.typed;
    let mut caret = None;
    for (i, line) in NOTE.iter().enumerate() {
        let n = left.min(line.chars().count());
        left -= n;
        let shown: String = line.chars().take(n).collect();
        let y = a.1 + 456 + i as i32 * 26;
        p.text(&shown, 16.0, false, x, y, INK);
        if n > 0 || i == 0 {
            caret = Some((x + p.width(&shown, 16.0, false) + 2, y));
        }
    }
    if desk.place == Place::Notes
        && let Some((cx, cy)) = caret
    {
        p.fill(cx, cy + 1, cx + 1, cy + 21, INK);
    }
}

/// The chart window.
fn chart(p: &mut Pen<'_>) {
    let b = WIN_B;
    p.window(b, "Stats", 36, Rgba::rgb(0x3C, 0x8D, 0x6E), LIGHT);
    p.text("Captures per day", 13.0, true, b.0 + 28, b.1 + 56, SOFT);
    for (i, (c, h)) in BARS.iter().enumerate() {
        let bx = CHART_X + i as i32 * CHART_STEP;
        p.round(bx, CHART_BASE - h, bx + 36, CHART_BASE, 3.0, *c);
    }
    p.fill(CHART_X - 8, CHART_BASE, CHART_X + 4 * CHART_STEP + 44, CHART_BASE + 1, RULE);
    for (i, d) in ["Mon", "Tue", "Wed", "Thu", "Fri"].iter().enumerate() {
        p.text(d, 12.0, false, CHART_X + i as i32 * CHART_STEP + 6, CHART_BASE + 6, SOFT);
    }
}

/// An invoice whose items add up to 245.00 while the total says 254.00.
fn invoice(p: &mut Pen<'_>) {
    let a = INVOICE;
    p.window(a, "Invoice 2041 - Ledger", 40, Rgba::rgb(0xE0, 0x7A, 0x34), LIGHT);
    let (x, xr) = (a.0 + 40, a.2 - 40);
    p.text("Invoice #2041", 24.0, true, x, a.1 + 52, INK);
    p.text("Customer: dana.lee@example.com", 14.0, false, x, a.1 + 94, SOFT);
    p.text("Issued 24 Nov 2026  -  Due 8 Dec 2026", 13.0, false, x, a.1 + 120, SOFT);
    p.text("Item", 12.0, true, x, a.1 + 166, SOFT);
    p.text("Qty", 12.0, true, x + 410, a.1 + 166, SOFT);
    p.text_right("Amount", 12.0, true, xr, a.1 + 166, SOFT);
    p.fill(x, a.1 + 188, xr, a.1 + 189, RULE);
    for (i, (item, qty, amount)) in [("Design review", "2 h", "120.00"), ("Screenshot pack", "1", "80.00"), ("Support call", "1 h", "45.00")].iter().enumerate() {
        let y = a.1 + 200 + i as i32 * 38;
        p.text(item, 15.0, false, x, y, INK);
        p.text(qty, 15.0, false, x + 410, y, INK);
        p.text_right(amount, 15.0, false, xr, y, INK);
        p.fill(x, y + 28, xr, y + 29, RULE);
    }
    p.text("Total", 16.0, true, x, a.1 + 322, INK);
    p.text_right("254.00", 16.0, true, xr, a.1 + 322, INK);
    p.round(x, a.1 + 380, x + 140, a.1 + 416, 6.0, BLUE);
    p.text("Send invoice", 14.0, true, x + 22, a.1 + 389, Rgba::rgb(255, 255, 255));
    p.text("Save draft", 13.0, false, x + 160, a.1 + 390, SOFT);
}

/// A bug tracker's form; the pasted picture goes in its box (drawn by the film).
fn issue(p: &mut Pen<'_>) {
    let a = ISSUE;
    let white = Rgba::rgb(255, 255, 255);
    p.window(a, "New issue - Tracker", 40, Rgba::rgb(0x6A, 0x70, 0xC8), LIGHT);
    let (x0, x1) = (ISSUE_DROP.0, ISSUE_DROP.2);
    p.text("Title", 12.0, true, x0, a.1 + 58, SOFT);
    p.field(x0, a.1 + 76, x1, a.1 + 110, 5.0, RULE, white);
    p.text("Invoice total is 9.00 too high", 14.0, false, x0 + 12, a.1 + 84, INK);
    p.text("Description", 12.0, true, x0, a.1 + 124, SOFT);
    p.field(x0, a.1 + 142, x1, a.1 + 196, 5.0, RULE, white);
    p.text("The three items add up to 245.00; the total says 254.00.", 14.0, false, x0 + 12, a.1 + 150, INK);
    p.text("Screenshot", 12.0, true, x0, a.1 + 210, SOFT);
    let d = ISSUE_DROP;
    p.field(d.0, d.1, d.2, d.3, 5.0, RULE, Rgba::rgb(0xF4, 0xF5, 0xF7));
    let hint = "Paste a picture here (Ctrl+V)";
    let w = p.width(hint, 14.0, false);
    p.text(hint, 14.0, false, (d.0 + d.2 - w) / 2, (d.1 + d.3) / 2 - 10, SOFT);
    p.round(x0, d.3 + 14, x0 + 110, d.3 + 46, 6.0, BLUE);
    p.text("Submit", 14.0, true, x0 + 30, d.3 + 21, white);
}

/// A paused tutorial: a slide with three commands, the player's controls under it.
fn video(p: &mut Pen<'_>) {
    let a = VIDEO;
    p.window(a, "Build setup, part 2 - Media Player", 36, Rgba::rgb(0xE0, 0x46, 0x3E), DARK);
    p.fill(a.0 + 1, a.1 + 37, a.2 - 1, 430, Rgba::rgb(0x0E, 0x0F, 0x12));
    p.round(52, 80, 618, 414, 6.0, Rgba::rgb(0x1E, 0x22, 0x30));
    p.text("Step 2: get the code", 18.0, true, 80, 100, Rgba::rgb(0xF2, 0xF2, 0xF2));
    p.round(80, 140, 590, 290, 6.0, Rgba::rgb(0x12, 0x15, 0x1E));
    for (i, line) in CODE.iter().enumerate() {
        p.text(line, 17.0, false, 100, 160 + i as i32 * 40, Rgba::rgb(0xE6, 0xE6, 0xE6));
    }
    p.text("Then open the folder in your editor.", 14.0, false, 80, 316, Rgba::rgb(0x9A, 0xA0, 0xB4));
    p.fill(60, 448, 610, 452, Rgba::rgb(0x4A, 0x4A, 0x4A));
    p.fill(60, 448, 262, 452, Rgba::rgb(0x4C, 0x9A, 0xFF));
    p.disc(262.0, 450.0, 6.0, Rgba::rgb(255, 255, 255));
    p.text("4:12 / 9:30", 12.0, false, 60, 466, Rgba::rgb(0xCC, 0xCC, 0xCC));
    p.poly(&[(327.0, 472.0), (327.0, 496.0), (347.0, 484.0)], Rgba::rgb(255, 255, 255));
}

/// A terminal: a prompt and whatever was pasted into it.
fn terminal(p: &mut Pen<'_>, lines: &[String]) {
    let a = TERM;
    let ink = Rgba::rgb(0xCC, 0xCC, 0xCC);
    p.window(a, "Terminal", 36, Rgba::rgb(0x2B, 0x2F, 0x36), DARK);
    p.fill(a.0 + 1, a.1 + 37, a.2 - 1, a.3 - 8, Rgba::rgb(0x0C, 0x0C, 0x0C));
    let (x, mut y) = (a.0 + 16, a.1 + 54);
    for line in lines {
        p.text(&format!("PS> {line}"), 13.0, false, x, y, ink);
        y += 24;
    }
    p.text("PS>", 13.0, false, x, y, ink);
    let cx = x + p.width("PS>", 13.0, false) + 6;
    p.fill(cx, y + 2, cx + 8, y + 18, ink);
}

/// Where the hero is at frame `t` of the run (feet, DIPs): running, jumping the pit, landing.
pub fn hero_at(t: u32) -> (f32, f32) {
    let t = t as f32;
    if t < 14.0 {
        (420.0 + t * 8.0, 510.0)
    } else if t < 34.0 {
        let s = (t - 14.0) / 20.0;
        (532.0 + s * 168.0, 510.0 - 480.0 * s * (1.0 - s))
    } else {
        ((700.0 + (t - 34.0) * 6.0).min(790.0), 510.0)
    }
}

/// The coins over the pit: where they float and the frame the hero takes each.
const PIT_COINS: [(f32, f32, u32); 3] = [(574.0, 396.0, 19), (616.0, 366.0, 24), (658.0, 396.0, 29)];

/// The coins taken by frame `t`.
pub fn coins_at(t: u32) -> u32 {
    COINS + 10 * PIT_COINS.iter().filter(|c| t >= c.2).count() as u32
}

/// A game, full screen: dusk sky, mountains, a pit with coins over it, a chest with a rare find on
/// a ledge, the hero, the HUD. Original flat shapes, no real game's art.
fn game(p: &mut Pen<'_>, t: u32) {
    let rgb = Rgba::rgb;
    for (y0, y1, c) in [(0, 110, rgb(0x1D, 0x27, 0x47)), (110, 190, rgb(0x27, 0x33, 0x5A)), (190, 250, rgb(0x33, 0x40, 0x6B)), (250, 300, rgb(0x46, 0x50, 0x7A)), (300, 650, rgb(0x5E, 0x64, 0x88))] {
        p.fill(0, y0, W, y1, c);
    }
    for (x, y) in [(60.0, 40.0), (150.0, 90.0), (240.0, 30.0), (330.0, 120.0), (420.0, 60.0), (560.0, 72.0), (610.0, 100.0), (700.0, 46.0), (980.0, 70.0), (1010.0, 160.0), (90.0, 170.0), (380.0, 190.0)] {
        p.disc(x, y, 1.6, rgb(0xE8, 0xE6, 0xF0).with_alpha(200));
    }
    p.disc(860.0, 140.0, 50.0, rgb(0xF2, 0xD7, 0xA0));
    let far = rgb(0x3A, 0x44, 0x70);
    for m in [[(0.0, 360.0), (140.0, 220.0), (300.0, 360.0)], [(220.0, 360.0), (400.0, 190.0), (560.0, 360.0)], [(480.0, 360.0), (650.0, 240.0), (800.0, 360.0)], [(700.0, 360.0), (880.0, 210.0), (1040.0, 360.0)]] {
        p.poly(&m, far);
    }
    p.fill(0, 359, W, 470, far);
    let near = rgb(0x2C, 0x35, 0x58);
    for m in [[(-40.0, 470.0), (120.0, 330.0), (300.0, 470.0)], [(380.0, 470.0), (560.0, 350.0), (740.0, 470.0)], [(760.0, 470.0), (940.0, 320.0), (1120.0, 470.0)]] {
        p.poly(&m, near);
    }
    p.fill(0, 469, W, 520, near);
    // The ground, the pit in it.
    p.fill(0, 520, W, H, rgb(0x3B, 0x2F, 0x2A));
    p.fill(0, 508, W, 522, rgb(0x5C, 0x8A, 0x3C));
    p.fill(560, 508, 660, H, rgb(0x14, 0x18, 0x26));
    // The ledge with the chest and its find.
    p.round(700, 380, 900, 402, 4.0, rgb(0x6B, 0x5E, 0x55));
    p.round(700, 376, 900, 385, 3.0, rgb(0x6F, 0xA0, 0x46));
    p.round(770, 342, 822, 380, 3.0, rgb(0xB8, 0x74, 0x2E));
    p.round(770, 342, 822, 354, 3.0, rgb(0x8F, 0x55, 0x20));
    p.round(792, 352, 800, 364, 1.5, rgb(0xF2, 0xC1, 0x4E));
    for (x, y, r) in [(758.0, 334.0, 9.0), (834.0, 328.0, 7.0), (810.0, 322.0, 6.0)] {
        p.star(x, y, r, rgb(0xFF, 0xE0, 0x8A));
    }
    p.field(690, 280, 910, 314, 5.0, rgb(0xD9, 0xA4, 0x41), rgb(0x1A, 0x1D, 0x2B));
    p.text("Ember Lantern  -  Legendary", 13.0, true, 704, 288, rgb(0xF2, 0xC1, 0x4E));
    // A slime by the path.
    let bob = if (t / 4).is_multiple_of(2) { 0 } else { 2 };
    p.round(364, 492 + bob, 396, 512, 12.0, rgb(0x5F, 0xBF, 0x6A));
    p.disc(374.0, 500.0 + bob as f32, 2.4, rgb(0x1D, 0x22, 0x35));
    p.disc(386.0, 500.0 + bob as f32, 2.4, rgb(0x1D, 0x22, 0x35));
    for (x, y, at) in PIT_COINS {
        if t < at {
            p.disc(x, y, 7.0, rgb(0xF2, 0xC1, 0x4E));
            p.disc(x, y, 4.0, rgb(0xD9, 0xA2, 0x3A));
        }
    }
    // The hero.
    let (hx, hy) = hero_at(t);
    let (hx, hy) = (hx.round() as i32, hy.round() as i32);
    let step = if hy == 510 && (t / 2) % 2 == 1 { 2 } else { 0 };
    p.fill(hx - 9, hy - 2, hx - 3, hy + 6 - step, rgb(0x3B, 0x2F, 0x2A));
    p.fill(hx + 3, hy - 2, hx + 9, hy + 6 - 2 + step, rgb(0x3B, 0x2F, 0x2A));
    p.round(hx - 12, hy - 34, hx + 12, hy, 6.0, rgb(0xC9, 0x50, 0x3A));
    p.fill(hx - 12, hy - 16, hx + 12, hy - 12, rgb(0x3B, 0x2F, 0x2A));
    p.disc(hx as f32, hy as f32 - 44.0, 11.0, rgb(0xF0, 0xCF, 0xAE));
    p.disc(hx as f32 + 4.0, hy as f32 - 45.0, 1.8, rgb(0x1D, 0x22, 0x35));
    p.line((hx as f32 + 14.0, hy as f32 - 20.0), (hx as f32 + 28.0, hy as f32 - 46.0), 3.0, rgb(0xD8, 0xDC, 0xE6));
    p.line((hx as f32 + 10.0, hy as f32 - 24.0), (hx as f32 + 19.0, hy as f32 - 18.0), 3.0, rgb(0x8F, 0x55, 0x20));
    // The HUD: hearts, coins, the place, four skill slots.
    for i in 0..4 {
        let (x, y) = (30.0 + i as f32 * 28.0, 30.0);
        let c = if i < 3 { rgb(0xE0, 0x46, 0x3E) } else { rgb(0x4A, 0x4F, 0x68) };
        p.disc(x - 4.5, y, 5.5, c);
        p.disc(x + 4.5, y, 5.5, c);
        p.poly(&[(x - 9.8, y + 1.5), (x + 9.8, y + 1.5), (x, y + 12.0)], c);
    }
    p.disc(930.0, 34.0, 10.0, rgb(0xF2, 0xC1, 0x4E));
    p.disc(930.0, 34.0, 6.0, rgb(0xD9, 0xA2, 0x3A));
    let n = coins_at(t);
    p.text(&format!("{},{:03}", n / 1000, n % 1000), 18.0, true, 948, 22, rgb(0xF5, 0xF1, 0xE6));
    let place = "Level 3  -  North Cave";
    let w = p.width(place, 13.0, false);
    p.text(place, 13.0, false, (W - w) / 2, 22, rgb(0xCF, 0xCB, 0xE0));
    for (i, c) in [rgb(0xE0, 0x46, 0x3E), rgb(0x4C, 0x9A, 0xFF), rgb(0xF2, 0xC1, 0x4E), rgb(0x8B, 0xD0, 0x6A)].iter().enumerate() {
        let x = 420 + i as i32 * 52;
        p.field(x, 586, x + 44, 630, 6.0, rgb(0x55, 0x5B, 0x7A), rgb(0x1A, 0x1D, 0x2B));
        p.disc(x as f32 + 22.0, 608.0, 10.0, *c);
        p.text(&(i + 1).to_string(), 10.0, true, x + 5, 588, rgb(0xCF, 0xCB, 0xE0));
    }
}

/// The mouse pointer at `k`, drawn like the system arrow (white, dark edge) or the cross.
fn draw_cursor(cv: &mut Canvas, p: Point, kind: CursorKind, k: f32) {
    let (x, y) = (p.x as f32, p.y as f32);
    match kind {
        CursorKind::Cross => {
            let r = 10.0 * k;
            for (w, c) in [(3.0 * k.max(1.0), Rgba::rgb(255, 255, 255)), (k.max(1.0), Rgba::rgb(0, 0, 0))] {
                let mut m = Mask::new(IBox::around(x - r - 2.0, y - r - 2.0, x + r + 2.0, y + r + 2.0, 2.0));
                m.bar(Pt::new(x + 0.5 - r, y + 0.5), Pt::new(x + 0.5 + r, y + 0.5), w);
                m.bar(Pt::new(x + 0.5, y + 0.5 - r), Pt::new(x + 0.5, y + 0.5 + r), w);
                cv.blend_mask(&m, c);
            }
        }
        _ => {
            let pts = [(0.0, 0.0), (0.0, 17.0), (4.2, 13.2), (7.0, 19.5), (9.8, 18.3), (7.0, 12.0), (12.5, 12.0)];
            let v: Vec<Pt> = pts.iter().map(|(a, b)| Pt::new(x + a * k, y + b * k)).collect();
            // Even-odd fill of the outline, then the outline itself.
            let b = IBox::around(x, y, x + 13.0 * k, y + 20.0 * k, 2.0).intersect(&cv.bounds());
            for py in b.y0..b.y1 {
                for px in b.x0..b.x1 {
                    let q = Pt::new(px as f32 + 0.5, py as f32 + 0.5);
                    let mut inside = false;
                    for i in 0..v.len() {
                        let (a, c) = (v[i], v[(i + 1) % v.len()]);
                        if (a.y > q.y) != (c.y > q.y) && q.x < a.x + (q.y - a.y) * (c.x - a.x) / (c.y - a.y) {
                            inside = !inside;
                        }
                    }
                    if inside {
                        cv.set(px, py, Rgba::rgb(255, 255, 255));
                    }
                }
            }
            let mut m = Mask::new(b);
            let mut closed = v.clone();
            closed.push(v[0]);
            m.polyline(&closed, 1.1 * k);
            cv.blend_mask(&m, Rgba::rgb(0, 0, 0));
        }
    }
}

/// Write frames as a looping GIF as they come: each frame only where it changed since the last
/// written one, a still stretch as one longer frame, a local palette per frame. One full frame is
/// kept in memory, not the film.
pub struct GifOut<Wr: Write> {
    enc: gif::Encoder<Wr>,
    prev: Option<Canvas>,
    pending: Option<(IBox, u16, Canvas)>,
}

impl<Wr: Write> GifOut<Wr> {
    pub fn new(out: Wr, w: u32, h: u32) -> Result<Self, String> {
        let mut enc = gif::Encoder::new(out, w as u16, h as u16, &[]).map_err(|e| e.to_string())?;
        enc.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;
        Ok(GifOut { enc, prev: None, pending: None })
    }

    fn flush(&mut self) -> Result<(), String> {
        let Some((b, delay, part)) = self.pending.take() else { return Ok(()) };
        let mut rgba: Vec<u8> = part.px.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        let mut fr = gif::Frame::from_rgba_speed(part.w as u16, part.h as u16, &mut rgba, 10);
        fr.left = b.x0 as u16;
        fr.top = b.y0 as u16;
        fr.delay = delay;
        fr.dispose = gif::DisposalMethod::Keep;
        self.enc.write_frame(&fr).map_err(|e| e.to_string())
    }

    pub fn push(&mut self, f: &Canvas) -> Result<(), String> {
        let changed = match &self.prev {
            None => Some(f.bounds()),
            Some(p) => changed_box(p, f),
        };
        match changed {
            None => {
                if let Some(p) = self.pending.as_mut() {
                    p.1 += DELAY;
                }
            }
            Some(b) => {
                self.flush()?;
                self.pending = Some((b, DELAY, f.crop(b)));
                self.prev = Some(f.clone());
            }
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<Wr, String> {
        self.flush()?;
        self.enc.into_inner().map_err(|e| e.to_string())
    }
}

/// What the screen shows.
#[derive(Clone, Debug, PartialEq)]
enum Stage {
    /// The desktop, nothing of snip on it (or a pin, or a card).
    Desktop,
    /// The editor overlay (`Editor` runs it).
    Editor,
    /// The plain snip overlay: mode bar, hint, the window under the cursor or the dragged area.
    Plain,
    /// The text overlay: guide lines, hint, a yellow frame.
    Text,
    /// Picking the area to record.
    RecordPick,
    /// Recording: red bars around the area and the control beside it.
    Recording { area: Rect, from: usize },
    /// A finished picture, shown on its own over the darkened desktop.
    Result(Canvas),
}

/// One film: the editor, the made-up desktop and where the frames go.
pub struct Film<'a> {
    pub e: Editor,
    g: &'a dyn Glyphs,
    pub k: f32,
    desk: Desk,
    scene: Canvas,
    stage: Stage,
    cursor: Point,
    /// Plain, text and record overlays: the pressed point and the current one while dragging.
    drag: Option<(Point, Point)>,
    /// The windows of the desktop, top first (pixels).
    windows: Vec<Rect>,
    caption: Option<(Vec<String>, String)>,
    shift: bool,
    card: Option<Canvas>,
    pin: Option<(Canvas, Point)>,
    /// The last picture copied (Ctrl+C in the editor, a plain snip), and where it was pasted.
    clip: Option<Canvas>,
    pub pasted: Option<Canvas>,
    out: Option<GifOut<Box<dyn Write>>>,
    pub err: Option<String>,
    /// Frames shot so far.
    pub count: usize,
    pub first: Option<Canvas>,
    pub last: Option<Canvas>,
    /// Frames kept by name (`mark`), screen only, without the strip.
    pub marks: Vec<(String, Canvas)>,
    /// The text the OCR gave the text film.
    pub read: Option<String>,
    ocr: Option<Ocr<'a>>,
}

/// Reads the text in a picture (the Windows OCR in `snip --demo-gif`, a stand-in in the tests).
pub type Ocr<'a> = &'a dyn Fn(&Canvas) -> Result<String, String>;

fn ease(k: f32) -> f32 {
    k * k * (3.0 - 2.0 * k)
}

impl<'a> Film<'a> {
    /// A film at `dpi` (96 in the tests, `DPI` for the README) writing to `out`.
    pub fn new(g: &'a dyn Glyphs, dpi: u32, start: (i32, i32), out: Option<Box<dyn Write>>) -> Result<Film<'a>, String> {
        let k = dpi as f32 / 96.0;
        let (sw, sh) = (px(W, k), px(H, k));
        let mons = Monitors::new(vec![Monitor::new(Rect::from_xywh(0, 0, sw, sh), dpi)]);
        let windows = windows_px(Place::Work, k);
        let cursor = Point::new(px(start.0, k), px(start.1, k));
        let mut e = Editor::new(mons, windows.clone(), Theme::new(Look::Graphite, Mode::Dark), &Prefs::default(), cursor);
        e.lang = Lang::En;
        // Text capture is on by default, so the bar offers "copy text" as it does after install.
        e.text_action = true;
        let out = match out {
            Some(w) => Some(GifOut::new(w, sw as u32, (sh + px(STRIP, k)) as u32)?),
            None => None,
        };
        let desk = Desk::default();
        let first = scene(g, k, &desk);
        Ok(Film {
            e,
            g,
            k,
            desk,
            scene: first,
            stage: Stage::Desktop,
            cursor,
            drag: None,
            windows,
            caption: None,
            shift: false,
            card: None,
            pin: None,
            clip: None,
            pasted: None,
            out,
            err: None,
            count: 0,
            first: None,
            last: None,
            marks: Vec::new(),
            read: None,
            ocr: None,
        })
    }

    pub fn with_ocr(mut self, ocr: Ocr<'a>) -> Self {
        self.ocr = Some(ocr);
        self
    }

    fn p(&self, x: i32, y: i32) -> Point {
        Point::new(px(x, self.k), px(y, self.k))
    }

    fn theme(&self) -> Theme {
        self.e.theme
    }

    fn screen(&self) -> Rect {
        Rect::from_xywh(0, 0, px(W, self.k), px(H, self.k))
    }

    fn work(&self) -> Rect {
        Rect::from_xywh(0, 0, px(W, self.k), px(H - TASKBAR, self.k))
    }

    /// What the strip under the screen says: the keys pressed (as caps), then the words.
    pub fn caption(&mut self, keys: &[&str], text: &str) {
        self.caption = Some((keys.iter().map(|s| s.to_string()).collect(), text.to_string()));
    }

    /// Change what the screen shows; a new place brings its own windows to pick.
    pub fn set_desk(&mut self, desk: Desk) {
        if desk != self.desk {
            if desk.place != self.desk.place {
                self.windows = windows_px(desk.place, self.k);
                self.e.windows = self.windows.clone();
            }
            self.desk = desk;
            self.scene = scene(self.g, self.k, &self.desk);
        }
    }

    /// The game runs on for `n` frames.
    pub fn play(&mut self, n: usize) {
        for _ in 0..n {
            let mut d = self.desk.clone();
            d.t += 1;
            self.set_desk(d);
            self.shot();
        }
    }

    /// Ctrl+V in another app: the copied picture lands in the issue form.
    pub fn paste_picture(&mut self) {
        self.pasted = self.clip.clone();
        self.shot();
    }

    /// Ctrl+V in the terminal: the lines the OCR read, one per prompt.
    pub fn paste_text(&mut self) {
        self.card = None;
        if let Some(t) = self.read.clone() {
            let mut d = self.desk.clone();
            d.terminal = t.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
            self.set_desk(d);
        }
        self.shot();
    }

    /// Type the notes, `per` characters a frame.
    pub fn type_note(&mut self, per: usize) {
        let all: usize = NOTE.iter().map(|l| l.chars().count()).sum();
        while self.desk.typed < all {
            let mut d = self.desk.clone();
            d.typed = (d.typed + per).min(all);
            self.set_desk(d);
            self.shot();
        }
    }

    /// Draw the current state as one frame.
    pub fn shot(&mut self) {
        let f = self.render();
        if let Some(out) = self.out.as_mut()
            && let Err(e) = out.push(&f)
        {
            self.err.get_or_insert(e);
        }
        if self.first.is_none() {
            self.first = Some(f.clone());
        }
        self.last = Some(f);
        self.count += 1;
    }

    /// Keep the current screen (no strip) as a still.
    pub fn mark(&mut self, name: &str) {
        let f = self.render();
        let s = f.crop(IBox::new(0, 0, px(W, self.k), px(H, self.k)));
        self.marks.push((name.to_string(), s));
    }

    pub fn hold(&mut self, n: usize) {
        for _ in 0..n {
            self.shot();
        }
    }

    fn at(&self) -> Point {
        if self.stage == Stage::Editor { self.e.cursor } else { self.cursor }
    }

    /// Move the pointer to `to` (DIPs) over `n` frames, eased; the overlay sees every step.
    pub fn glide(&mut self, to: (i32, i32), n: usize) {
        let from = self.at();
        let to = self.p(to.0, to.1);
        for i in 1..=n.max(1) {
            let k = ease(i as f32 / n.max(1) as f32);
            let p = Point::new(from.x + ((to.x - from.x) as f32 * k).round() as i32, from.y + ((to.y - from.y) as f32 * k).round() as i32);
            self.move_to(p);
            self.shot();
        }
    }

    fn move_to(&mut self, p: Point) {
        self.cursor = p;
        if self.stage == Stage::Editor {
            let eff = self.e.on_move(p, self.shift);
            self.apply(eff);
        }
        if let Some((a, _)) = self.drag {
            self.drag = Some((a, p));
        }
    }

    pub fn down(&mut self) {
        if self.stage == Stage::Editor {
            let p = self.e.cursor;
            let eff = self.e.on_down_mods(p, self.shift, false, self.g);
            self.apply(eff);
        } else {
            self.drag = Some((self.cursor, self.cursor));
        }
        self.shot();
    }

    pub fn up(&mut self) {
        match self.stage.clone() {
            Stage::Editor => {
                let p = self.e.cursor;
                let eff = self.e.on_up(p, self.shift);
                self.apply(eff);
            }
            Stage::Plain => {
                let sel = self.plain_sel();
                self.drag = None;
                if let Some(r) = sel {
                    self.take(r);
                }
            }
            Stage::Text => {
                let sel = self.drag.filter(|(a, b)| !crate::geometry::is_click(*a, *b)).map(|(a, b)| drag_rect(a, b));
                self.drag = None;
                if let Some(r) = sel {
                    self.read_text(r);
                }
            }
            Stage::RecordPick => {
                let sel = self.plain_sel();
                self.drag = None;
                if let Some(r) = sel {
                    self.stage = Stage::Recording { area: r, from: self.count };
                }
            }
            _ => {
                self.drag = None;
            }
        }
        self.shot();
    }

    pub fn click(&mut self) {
        self.down();
        self.up();
    }

    pub fn drag(&mut self, from: (i32, i32), to: (i32, i32), travel: usize, steps: usize) {
        self.glide(from, travel);
        self.down();
        self.glide(to, steps);
        self.up();
    }

    /// A key; in the editor it goes to the editor, on the plain overlay Enter takes the screen.
    pub fn key(&mut self, vk: u32, ctrl: bool) {
        match self.stage {
            Stage::Editor => {
                let eff = self.e.on_key(vk, ctrl, self.shift);
                self.apply(eff);
            }
            Stage::Plain if vk == VK_RETURN => {
                let r = self.screen();
                self.take(r);
            }
            _ => {}
        }
        self.shot();
    }

    pub fn type_text(&mut self, s: &str) {
        for c in s.chars() {
            let eff = self.e.on_char(c);
            self.apply(eff);
            self.shot();
        }
    }

    pub fn press(&mut self, it: Item, travel: usize) {
        if let Some((x, y)) = self.e.item_center(it) {
            let (x, y) = ((x as f32 / self.k).round() as i32, (y as f32 / self.k).round() as i32);
            self.glide((x, y), travel);
            self.hold(2);
            self.click();
        }
    }

    /// The editor hotkey: the screen freezes and the editor starts picking the area.
    pub fn open_editor(&mut self) {
        let c = self.cursor;
        let mons = self.e.mons.clone();
        let mut e = Editor::new(mons, self.windows.clone(), self.e.theme, &Prefs::default(), c);
        e.lang = Lang::En;
        e.text_action = true;
        self.e = e;
        self.stage = Stage::Editor;
    }

    /// The editor with an area already picked (the films that start mid-edit), no frames.
    pub fn editor_on(&mut self, a: (i32, i32), b: (i32, i32)) {
        self.cursor = self.p(a.0, a.1);
        self.open_editor();
        let (a, b) = (self.p(a.0, a.1), self.p(b.0, b.1));
        self.e.on_move(a, false);
        self.e.on_down(a, false);
        self.e.on_move(b, false);
        self.e.on_up(b, false);
    }

    /// Draw on the picked area without frames (setting the stage).
    pub fn quick(&mut self, a: (i32, i32), b: (i32, i32)) {
        let (a, b) = (self.p(a.0, a.1), self.p(b.0, b.1));
        self.e.on_down(a, false);
        self.e.on_move(b, false);
        self.e.on_up(b, false);
    }

    pub fn plain(&mut self) {
        self.stage = Stage::Plain;
        self.card = None;
    }

    pub fn text_overlay(&mut self) {
        self.stage = Stage::Text;
        self.card = None;
    }

    pub fn record_pick(&mut self) {
        self.stage = Stage::RecordPick;
        self.card = None;
    }

    /// The recorder's hotkey again: recording stops, the bars go.
    pub fn stop_recording(&mut self) {
        self.stage = Stage::Desktop;
    }

    pub fn desktop(&mut self) {
        self.stage = Stage::Desktop;
    }

    /// PrtScn: every screen as one picture, no overlay; the card says so.
    pub fn all_screens(&mut self) {
        let s = self.screen();
        let body = crate::cardview::all_screens_body(s.width() as u32, s.height() as u32, 1, Lang::En);
        let content = crate::cardview::CardContent { mark: crate::cardview::Mark::Done, title: crate::i18n::S::ToastAll.get(Lang::En).to_string(), meta: None, body, tip: None };
        self.card = Some(crate::cardview::render(&content, &self.theme(), self.k, self.g));
    }

    /// Pick up the pin and move it by `d` DIPs over `n` frames.
    pub fn drag_pin(&mut self, d: (i32, i32), n: usize) {
        let Some((_, start)) = self.pin.clone() else { return };
        let c0 = self.cursor;
        for i in 1..=n {
            let k = ease(i as f32 / n as f32);
            let (dx, dy) = ((px(d.0, self.k) as f32 * k).round() as i32, (px(d.1, self.k) as f32 * k).round() as i32);
            self.cursor = Point::new(c0.x + dx, c0.y + dy);
            if let Some(p) = self.pin.as_mut() {
                p.1 = Point::new(start.x + dx, start.y + dy);
            }
            self.shot();
        }
    }

    fn plain_sel(&self) -> Option<Rect> {
        if let Some((a, b)) = self.drag
            && !crate::geometry::is_click(a, b)
        {
            return Some(drag_rect(a, b));
        }
        let k = crate::geometry::window_at(&self.windows, self.cursor)?;
        self.windows[k].intersect(&self.screen())
    }

    /// A plain snip finished: the picture goes to the clipboard; the film shows it on its own.
    fn take(&mut self, r: Rect) {
        let img = self.scene.crop(IBox::new(r.left, r.top, r.right, r.bottom));
        self.clip = Some(img.clone());
        self.stage = Stage::Result(img);
    }

    /// The text overlay finished: OCR reads the frozen pixels under the area, the card shows it.
    fn read_text(&mut self, r: Rect) {
        let crop = self.scene.crop(IBox::new(r.left, r.top, r.right, r.bottom));
        self.stage = Stage::Desktop;
        let outcome = match self.ocr.map(|read| read(&crop)) {
            Some(Ok(text)) if !text.is_empty() => TextOutcome::Copied(text),
            Some(Ok(_)) => TextOutcome::NoText,
            Some(Err(e)) => {
                self.err.get_or_insert(format!("ocr: {e}"));
                TextOutcome::Error
            }
            None => TextOutcome::NoEngine,
        };
        if let TextOutcome::Copied(t) = &outcome {
            self.read = Some(t.clone());
        }
        if let Some(c) = crate::cardview::content(&outcome, Lang::En) {
            self.card = Some(crate::cardview::render(&c, &self.theme(), self.k, self.g));
        }
    }

    fn apply(&mut self, eff: Effect) {
        match eff {
            Effect::PickColor { at, .. } => {
                let c = self.scene.get(at.x.clamp(0, self.scene.w as i32 - 1), at.y.clamp(0, self.scene.h as i32 - 1));
                let eff = self.e.picked_color(c);
                self.apply(eff);
            }
            Effect::Copy => {
                if let Some(img) = self.finished() {
                    self.clip = Some(img.clone());
                    self.stage = Stage::Result(img);
                }
            }
            Effect::Pin => {
                if let (Some(img), Some(sel)) = (self.finished(), self.e.sel) {
                    // The pin window: the picture where it was taken, with a 1-DIP border.
                    let b = crate::dpi::px(theme::BORDER_WIDTH, (self.k * 96.0).round() as u32).max(1);
                    let mut cv = Canvas::new(img.w + 2 * b as u32, img.h + 2 * b as u32, self.theme().border);
                    cv.draw(&img, b, b);
                    self.pin = Some((cv, Point::new(sel.left - b, sel.top - b)));
                    self.stage = Stage::Desktop;
                }
            }
            _ => {}
        }
    }

    /// The editor's finished picture, as Copy, Save and Pin produce it.
    fn finished(&self) -> Option<Canvas> {
        let sel = self.e.sel?;
        let crop = self.scene.crop(IBox::new(sel.left, sel.top, sel.right, sel.bottom));
        let img = self.e.compose_doc(&crop, self.g);
        Some(if self.e.frame { beautify::apply(&img, self.e.scale()) } else { img })
    }

    fn shadowed(&self, cv: &mut Canvas, item: &Canvas, at: Point) {
        let (s, mx, my) = toolbar::render_shadow(item.w as i32, item.h as i32, &self.theme(), self.k);
        cv.draw(&s, at.x - mx, at.y - my);
        cv.draw(item, at.x, at.y);
    }

    /// The frozen screen, dimmed, with `sel` shown bright and framed (plain, text and record).
    fn dimmed_with(&self, cv: &mut Canvas, alpha: u8, sel: Option<Rect>, text: bool) {
        cv.fill_box(cv.bounds(), theme::DIM_COLOR.with_alpha(alpha));
        let Some(sel) = sel else { return };
        let t = self.theme();
        let b = IBox::new(sel.left, sel.top, sel.right, sel.bottom);
        cv.draw(&self.scene.crop(b), sel.left, sel.top);
        let one = crate::dpi::px(theme::BORDER_WIDTH, (self.k * 96.0).round() as u32).max(1);
        for bar in crate::frame::selection_frame(self.screen(), sel, one) {
            let r = bar.rect;
            let c = match (bar.layer, text) {
                (0, true) => theme::MARKER,
                (0, false) => t.sel_inner,
                (_, true) => theme::MARKER_INK.with_alpha(153),
                (_, false) => t.sel_outer.with_alpha((theme::SEL_OUTER_ALPHA * 255.0) as u8),
            };
            cv.fill_box(IBox::new(r.left, r.top, r.right, r.bottom), c);
        }
        if text {
            let (len, th) = (px(14, self.k), px(3, self.k).max(2));
            for r in crate::frame::corner_bars(self.screen(), sel, th, len) {
                cv.fill_box(IBox::new(r.left, r.top, r.right, r.bottom), theme::MARKER);
            }
        }
    }

    /// The size label above the dragged area's corner, as the overlay places it.
    fn size_label(&self, cv: &mut Canvas, r: Rect, text: bool) {
        let s = format!("{} \u{00D7} {}", r.width(), r.height());
        let c = if text {
            let s = if crate::textcore::selection_usable(r.width(), r.height()) { s } else { crate::i18n::S::TooSmall.get(Lang::En).to_string() };
            toolbar::render_text_label(&s, self.k, self.g)
        } else {
            toolbar::render_label(&s, &self.theme(), self.k, self.g)
        };
        let gap = (theme::SELECTION_LABEL_GAP * self.k).round() as i32;
        let y = if r.top - gap - c.h as i32 >= 0 { r.top - gap - c.h as i32 } else { r.top + gap };
        cv.draw(&c, r.left.max(0).min(px(W, self.k) - c.w as i32), y);
    }

    fn render(&self) -> Canvas {
        let (sw, sh) = (px(W, self.k), px(H, self.k));
        let mut cv = Canvas::new(sw as u32, (sh + px(STRIP, self.k)) as u32, STRIP_BG);
        let screen = self.render_screen();
        cv.draw(&screen, 0, 0);
        self.draw_strip(&mut cv);
        cv
    }

    fn render_screen(&self) -> Canvas {
        let t = self.theme();
        let k = self.k;
        let mut cv = self.scene.clone();
        let mut cursor = Some(CursorKind::Cross);
        match &self.stage {
            Stage::Result(img) => {
                // The picture on its own, as it would land in a chat or a document.
                cv.fill_box(cv.bounds(), RESULT_BG);
                let fit = img.downscale_to_fit((cv.w as i32 - px(180, k)) as u32, (cv.h as i32 - px(120, k)) as u32);
                let at = Point::new((cv.w as i32 - fit.w as i32) / 2, (cv.h as i32 - fit.h as i32) / 2);
                self.shadowed(&mut cv, &fit, at);
                cursor = None;
            }
            Stage::Desktop => {
                if let (Some(img), Place::Issue) = (&self.pasted, self.desk.place) {
                    let d = ISSUE_DROP;
                    let pad = 12;
                    let fit = img.downscale_to_fit(px(d.2 - d.0 - 2 * pad, k) as u32, px(d.3 - d.1 - 2 * pad, k) as u32);
                    let (x, y) = ((px(d.0 + d.2, k) - fit.w as i32) / 2, (px(d.1 + d.3, k) - fit.h as i32) / 2);
                    let one = k.round().max(1.0) as i32;
                    cv.fill_box(IBox::new(x - one, y - one, x + fit.w as i32 + one, y + fit.h as i32 + one), RULE);
                    cv.draw(&fit, x, y);
                }
                if let Some((pin, at)) = &self.pin {
                    cv.draw(pin, at.x, at.y);
                }
                cursor = Some(CursorKind::Arrow);
            }
            Stage::Recording { area, from } => {
                let ctl = crate::record::render_control(((self.count - from) as u64) * DELAY as u64 * 10, "Stop", &t, k, self.g);
                let gap = (theme::SELECTION_LABEL_GAP * k).round() as i32;
                let (bars, panel) = crate::record::indicator(*area, self.screen(), (ctl.w as i32, ctl.h as i32), gap);
                for b in bars {
                    cv.fill_box(IBox::new(b.left, b.top, b.right, b.bottom), REC_BAR);
                }
                if let Some(p) = panel {
                    cv.draw(&ctl, p.left, p.top);
                }
                cursor = Some(CursorKind::Arrow);
            }
            Stage::Plain | Stage::RecordPick => {
                let record = self.stage == Stage::RecordPick;
                let sel = self.plain_sel();
                self.dimmed_with(&mut cv, (theme::PLAIN_DIM_DEFAULT * 255.0).round() as u8, sel, false);
                let top = px(theme::MODE_BAR_TOP as i32, k);
                if record {
                    let hint = toolbar::render_record_hint(Lang::En, &t, k, self.g);
                    self.shadowed(&mut cv, &hint, Point::new((px(W, k) - hint.w as i32) / 2, top));
                } else {
                    let bar = Bar::layout(&toolbar::mode_slots_with(true), &t, k);
                    let at = Point::new((px(W, k) - bar.w) / 2, top);
                    let st = BarState { active: vec![Item::ModeRegion], ..Default::default() };
                    self.shadowed(&mut cv, &toolbar::render_bar(&bar, &t, &st), at);
                    if self.drag.is_none() {
                        let hint = toolbar::render_plain_hint(Lang::En, &t, k, self.g, cv.w as i32 - 2 * top);
                        let y = at.y + bar.h + (theme::SELECTION_LABEL_GAP * k).round() as i32;
                        self.shadowed(&mut cv, &hint, Point::new((px(W, k) - hint.w as i32) / 2, y));
                    }
                }
                if let Some((a, b)) = self.drag
                    && !crate::geometry::is_click(a, b)
                {
                    self.size_label(&mut cv, drag_rect(a, b), false);
                }
            }
            Stage::Text => {
                let sel = self.drag.filter(|(a, b)| !crate::geometry::is_click(*a, *b)).map(|(a, b)| drag_rect(a, b));
                self.dimmed_with(&mut cv, (theme::PLAIN_DIM_DEFAULT * 255.0).round() as u8, sel, true);
                if self.drag.is_none() {
                    let one = crate::dpi::px(theme::BORDER_WIDTH, (k * 96.0).round() as u32).max(1);
                    let g = Rgba::rgb(255, 255, 255).with_alpha(72);
                    cv.fill_box(IBox::new(self.cursor.x, 0, self.cursor.x + one, cv.h as i32), g);
                    cv.fill_box(IBox::new(0, self.cursor.y, cv.w as i32, self.cursor.y + one), g);
                    use crate::i18n::S;
                    let hint = toolbar::render_text_hint(S::TextHintDrag.get(Lang::En), &[(crate::cursorfence::CROSSING_KEY_NAME, S::TextHintCross.get(Lang::En)), ("Esc", S::TextHintCancel.get(Lang::En))], &t, k, self.g);
                    self.shadowed(&mut cv, &hint, Point::new((px(W, k) - hint.w as i32) / 2, px(theme::MODE_BAR_TOP as i32, k)));
                }
                if let Some(r) = sel {
                    self.size_label(&mut cv, r, true);
                }
            }
            Stage::Editor => {
                let e = &self.e;
                cv.fill_box(cv.bounds(), theme::DIM_COLOR.with_alpha(t.dim_alpha));
                let mon = self.screen();
                if let Some(sel) = e.shown_sel() {
                    let b = IBox::new(sel.left, sel.top, sel.right, sel.bottom);
                    let crop = self.scene.crop(b);
                    let img = if e.phase == Phase::Editing && e.sel == Some(sel) { e.compose(&crop, self.g, true) } else { crop };
                    cv.draw(&img, sel.left, sel.top);
                    let one = crate::dpi::px(theme::BORDER_WIDTH, (k * 96.0).round() as u32).max(1);
                    for bar in crate::frame::selection_frame(mon, sel, one) {
                        let r = bar.rect;
                        let c = if bar.layer == 0 { t.sel_inner } else { t.sel_outer.with_alpha((theme::SEL_OUTER_ALPHA * 255.0) as u8) };
                        cv.fill_box(IBox::new(r.left, r.top, r.right, r.bottom), c);
                    }
                    for h in e.handle_rects() {
                        let c = Pt::new((h.left + h.right) as f32 / 2.0, (h.top + h.bottom) as f32 / 2.0);
                        let mut d = Mask::new(cv.bounds());
                        d.disc(c, theme::HANDLE_RADIUS * e.scale());
                        cv.blend_mask(&d, theme::HANDLE_FILL);
                        let mut r = Mask::new(cv.bounds());
                        r.ring(c, theme::HANDLE_RADIUS * e.scale(), one as f32);
                        cv.blend_mask(&r, t.handle_ring());
                    }
                }
                let v = e.view(self.g);
                for (floating, item) in [(true, v.bar), (true, v.palette), (true, v.size_menu), (false, v.label), (false, v.tooltip)] {
                    let Some((at, c)) = item else { continue };
                    if floating {
                        self.shadowed(&mut cv, &c, at);
                    } else {
                        cv.draw(&c, at.x, at.y);
                    }
                }
                if e.wants_loupe() {
                    let p = e.cursor;
                    let want = crate::loupe::sample_rect(p);
                    if let Some(got) = want.intersect(&mon) {
                        let pxs = self.scene.crop(IBox::new(got.left, got.top, got.right, got.bottom));
                        let sq = crate::loupe::square(p, got, &pxs);
                        let l = crate::loupe::render(&sq, p, &t, e.scale(), self.g);
                        let at = crate::loupe::place(p, l.w as i32, l.h as i32, mon, (theme::LOUPE_OFFSET * e.scale()).round() as i32);
                        self.shadowed(&mut cv, &l, at);
                    }
                }
                cursor = Some(e.cursor_kind(e.cursor, false));
            }
        }
        if let Some(card) = &self.card {
            let (x, y) = crate::cardview::place(self.work(), card.w as i32, card.h as i32, px(theme::CARD_MARGIN as i32, k));
            cv.draw(card, x, y);
        }
        if let Some(kind) = cursor {
            draw_cursor(&mut cv, self.at(), kind, k);
        }
        cv
    }

    /// The narration strip: key caps, then the words, centred.
    fn draw_strip(&self, cv: &mut Canvas) {
        let Some((keys, text)) = &self.caption else { return };
        let k = self.k;
        let s = |d: f32| (d * k).round() as i32;
        let y0 = px(H, k);
        let mid = y0 + px(STRIP, k) / 2;
        let caps: Vec<_> = keys.iter().map(|key| self.g.text(key, 13.0 * k, true)).collect();
        let words = self.g.text(text, 15.0 * k, false);
        let (cap_pad, cap_h, cap_gap) = (s(9.0), s(26.0), s(5.0));
        let caps_w: i32 = caps.iter().map(|c| c.w as i32 + 2 * cap_pad).sum::<i32>() + cap_gap * (caps.len() as i32 - 1).max(0);
        let gap = if caps.is_empty() { 0 } else { s(14.0) };
        let total = caps_w + gap + words.w as i32;
        let mut x = (cv.w as i32 - total) / 2;
        for c in &caps {
            let b = IBox::new(x, mid - cap_h / 2, x + c.w as i32 + 2 * cap_pad, mid + cap_h / 2);
            let mut m = Mask::new(b);
            m.rounded_box(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32, 5.0 * k);
            cv.blend_mask(&m, CAP_EDGE);
            let mut m = Mask::new(b);
            m.rounded_box(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32 - s(2.0) as f32, 5.0 * k);
            cv.blend_mask(&m, CAP_BG);
            cv.text_mask(c, b.x0 + cap_pad, b.y0 + (cap_h - s(2.0) - c.h as i32) / 2, INK);
            x = b.x1 + cap_gap;
        }
        if !caps.is_empty() {
            x += gap - cap_gap;
        }
        cv.text_mask(&words, x, mid - words.h as i32 / 2, STRIP_TEXT);
    }
}

/// Inclusive drag, as the overlay takes it: the press and the release pixel are both inside.
fn drag_rect(a: Point, b: Point) -> Rect {
    let r = Rect::from_drag(a, b);
    Rect::new(r.left, r.top, r.right + 1, r.bottom + 1)
}

/// Story 1, a game: a rare find mid-run. The editor key freezes the frame, the area is picked with
/// the magnifier, circled and labelled, then copied for the group chat.
pub fn game_film(f: &mut Film<'_>) {
    f.set_desk(Desk::game(GAME_STILL_T));
    f.caption(&[], "A rare find in the middle of a run");
    f.hold(10);
    f.caption(&["Pause"], "Pause: the game frame freezes");
    f.hold(4);
    f.open_editor();
    f.hold(3);
    f.caption(&[], "Drag over the moment; the magnifier shows every pixel");
    f.glide((440, 262), 10);
    f.down();
    // On the chest's lock: the magnifier shows it pixel by pixel.
    f.glide((796, 359), 12);
    f.hold(2);
    f.mark("magnifier");
    f.glide((930, 540), 8);
    f.up();
    f.hold(3);
    f.caption(&["I"], "Take the coin's gold for the marks");
    f.key('I' as u32, false);
    f.glide((616, 360), 8);
    f.hold(2);
    f.click();
    f.caption(&["E"], "Circle it");
    f.key('E' as u32, false);
    f.drag((750, 316), (846, 394), 6, 8);
    f.caption(&["T"], "Say where it is");
    f.key('T' as u32, false);
    f.glide((458, 292), 5);
    f.click();
    f.type_text("Level 3, behind the pit");
    f.key(VK_RETURN, false);
    f.hold(3);
    f.caption(&["Ctrl", "C"], "Copy: ready to paste in the group chat");
    f.hold(2);
    f.key('C' as u32, true);
    f.hold(18);
}

/// Story 2, a bug report: a wrong total. The window is picked with one click, marked, the
/// customer's email pixelated, then pasted into the issue form.
pub fn bug_film(f: &mut Film<'_>) {
    f.set_desk(Desk::at(Place::Invoice));
    f.caption(&[], "The total on this invoice is wrong");
    f.hold(10);
    f.caption(&["Pause"], "Snip and edit");
    f.hold(4);
    f.open_editor();
    f.hold(2);
    f.caption(&[], "Click the window: all of it is picked");
    f.glide((600, 300), 10);
    f.hold(4);
    f.click();
    f.hold(3);
    // The arrow is the editor's first tool, ready without a key.
    f.caption(&[], "Point at the problem with an arrow");
    f.drag((660, 470), (786, 384), 6, 8);
    f.caption(&["T"], "Say what is wrong");
    f.key('T' as u32, false);
    f.glide((466, 458), 5);
    f.click();
    f.type_text("Should be 245.00");
    f.key(VK_RETURN, false);
    f.caption(&["B"], "Pixelate the customer's email");
    f.key('B' as u32, false);
    f.drag((186, 130), (420, 156), 6, 8);
    f.hold(4);
    f.mark("editor");
    f.caption(&["Ctrl", "C"], "Copy");
    f.key('C' as u32, true);
    f.hold(6);
    f.set_desk(Desk::at(Place::Issue));
    f.desktop();
    f.caption(&[], "Open a new issue");
    f.hold(4);
    f.glide((520, 400), 10);
    f.click();
    f.caption(&["Ctrl", "V"], "Paste: the problem marked, the email hidden");
    f.paste_picture();
    f.hold(24);
}

/// Story 3, a paused tutorial: commands shown as a picture, copied with the OCR and pasted into
/// the terminal instead of typed out.
pub fn code_film(f: &mut Film<'_>) {
    f.set_desk(Desk::at(Place::Video));
    f.caption(&[], "A video shows the commands, but you cannot select them");
    f.hold(12);
    f.caption(&["Win", "Shift", "T"], "Copy text: drag over it");
    f.hold(3);
    f.text_overlay();
    f.hold(3);
    f.glide((88, 146), 10);
    f.down();
    f.glide((584, 284), 14);
    f.hold(2);
    f.up();
    f.caption(&[], "Read by the OCR built into Windows, on this PC; now on the clipboard");
    f.hold(24);
    f.caption(&[], "Click the terminal");
    f.glide((820, 420), 8);
    f.click();
    f.caption(&["Ctrl", "V"], "Paste: no typing it out by hand");
    f.paste_text();
    f.hold(22);
}

/// Story 4, a clip: the next seconds of the run, recorded; the bars and the timer stay outside.
pub fn clip_film(f: &mut Film<'_>) {
    f.set_desk(Desk::game(0));
    f.caption(&["Ctrl", "Shift", "F9"], "Record the next few seconds");
    f.hold(6);
    f.record_pick();
    f.hold(3);
    f.caption(&[], "Pick the area");
    f.glide((330, 250), 10);
    f.down();
    f.glide((960, 560), 14);
    f.up();
    f.caption(&[], "Recording: the bars and the timer stay outside the clip");
    f.play(50);
    f.caption(&["Ctrl", "Shift", "F9"], "Stop: a silent MP4, and a GIF made from it");
    f.stop_recording();
    f.hold(18);
}

/// Story 5, a reference kept in view: the chart is pinned, the notebook comes to the front and
/// the notes are written looking at the pin.
pub fn pin_film(f: &mut Film<'_>) {
    f.caption(&[], "Writing about a chart in another window?");
    f.hold(8);
    f.caption(&["Pause"], "Snip and edit");
    f.hold(3);
    f.open_editor();
    f.hold(2);
    f.caption(&[], "Click the chart window");
    f.glide((820, 300), 10);
    f.hold(3);
    f.click();
    f.hold(3);
    f.caption(&["Ctrl", "P"], "Pin it: it stays on top of every window");
    f.key('P' as u32, true);
    f.hold(5);
    f.glide((820, 200), 6);
    f.drag_pin((22, 40), 10);
    f.caption(&[], "Back to the notes: the chart stays in view");
    f.glide((300, 420), 10);
    f.click();
    f.set_desk(Desk { place: Place::Notes, ..Desk::default() });
    f.hold(4);
    f.type_note(2);
    f.hold(14);
}

/// Film 6: the plain snip takes a window with one click and the screen with Enter; PrtScn takes
/// every screen with no overlay at all.
pub fn modes_film(f: &mut Film<'_>) {
    f.caption(&["Win", "Shift", "S"], "The plain snip: the screen freezes");
    f.hold(6);
    f.plain();
    f.hold(8);
    f.caption(&[], "Point at a window and it lights up");
    f.glide((300, 470), 12);
    f.hold(4);
    f.glide((820, 300), 12);
    f.hold(5);
    f.caption(&[], "Click: the window goes to the clipboard and into a PNG");
    f.click();
    f.hold(14);
    f.desktop();
    f.caption(&["Win", "Shift", "S"], "");
    f.hold(3);
    f.plain();
    f.caption(&["Enter"], "Enter: the whole screen");
    f.hold(6);
    f.key(VK_RETURN, false);
    f.hold(12);
    f.desktop();
    f.caption(&["PrtScn"], "All screens as one picture, no overlay");
    f.hold(4);
    f.all_screens();
    f.hold(22);
}

/// The bounding box of the pixels that differ between two frames of the same size.
pub fn changed_box(a: &Canvas, b: &Canvas) -> Option<IBox> {
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    let w = a.w as usize;
    for (i, (p, q)) in a.px.as_chunks::<4>().0.iter().zip(b.px.as_chunks::<4>().0.iter()).enumerate() {
        if p != q {
            let (x, y) = ((i % w) as i32, (i / w) as i32);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    (x0 <= x1).then(|| IBox::new(x0, y0, x1, y1))
}

/// A still: `item` on the wallpaper of the films' screen, centred, with a 1-DIP border, rounded
/// corners and the floating-panel shadow. Every still has the screen's size.
fn on_wall(item: &Canvas, k: f32) -> Canvas {
    let (w, h) = (px(W, k), px(H, k));
    let mut cv = Canvas::new(w as u32, h as u32, WALL);
    let t = Theme::new(Look::Graphite, Mode::Dark);
    let (x, y) = ((w - item.w as i32) / 2, (h - item.h as i32) / 2);
    let (s, mx, my) = toolbar::render_shadow(item.w as i32, item.h as i32, &t, k);
    cv.draw(&s, x - mx, y - my);
    let r = t.radius * k;
    let b = IBox::xywh(x, y, item.w as i32, item.h as i32);
    let mut edge = Mask::new(b);
    edge.rounded_box(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32, r);
    cv.blend_mask(&edge, t.border);
    // The content inside the border, its corners cut round.
    let one = k.round().max(1.0);
    let mut inner = Mask::new(b);
    inner.rounded_box(b.x0 as f32 + one, b.y0 as f32 + one, b.x1 as f32 - one, b.y1 as f32 - one, (r - one).max(0.0));
    for yy in b.y0..b.y1 {
        for xx in b.x0..b.x1 {
            let a = inner.at(xx, yy);
            if a > 0 {
                cv.blend(xx, yy, item.get(xx - x, yy - y).with_alpha(255), a as f32 / 255.0);
            }
        }
    }
    cv
}

/// The Settings window's content (first tab, defaults, English), at 125 % so all of it fits the
/// still, which has the films' screen size.
pub fn settings_still(g: &dyn Glyphs) -> Canvas {
    use crate::settingsview::{self, ViewState};
    let sc = 1.25;
    let t = Theme::new(Look::Graphite, Mode::Dark);
    let (lw, lh) = ((settingsview::WIDTH * sc).round() as i32, (settingsview::height_dip() * sc).round() as i32);
    let l = settingsview::layout(lw, lh, sc, Lang::En, g, &t, settingsview::TAB_SNIP);
    let st = ViewState { recording: None, hover: None, message: None, folder_text: "Screenshots (Windows default)", autostart: false, language_text: "Automatic (Windows language)" };
    let cv = settingsview::render(&l, &t, Lang::En, &crate::config::Config::default(), &st, g);
    on_wall(&cv, DPI as f32 / 96.0)
}

/// The History window at 150 %, with captures cut from the films' screens.
pub fn history_still(g: &dyn Glyphs) -> Canvas {
    use crate::history::Entry;
    use crate::historyview::{self, Grid};
    let k = DPI as f32 / 96.0;
    let t = Theme::new(Look::Graphite, Mode::Dark);
    type Cut = (Desk, ((i32, i32), (i32, i32)), &'static str);
    let cuts: [Cut; 6] = [
        (Desk::game(GAME_STILL_T), ((440, 262), (930, 540)), "editor"),
        (Desk::at(Place::Invoice), ((INVOICE.0, INVOICE.1), (INVOICE.2, INVOICE.3)), "editor"),
        (Desk::at(Place::Video), ((52, 80), (618, 414)), "snip"),
        (Desk::default(), ((WIN_B.0, WIN_B.1), (WIN_B.2, WIN_B.3)), "snip"),
        (Desk::game(40), ((0, 0), (W, H)), "snip"),
        (Desk::default(), ((WIN_A.0, WIN_A.1), (WIN_A.2, WIN_A.3)), "snip"),
    ];
    let times = ["2026-11-24 14:20:31", "2026-11-24 14:18:02", "2026-11-24 11:05:47", "2026-11-23 17:52:12", "2026-11-23 09:12:40", "2026-11-22 16:10:12"];
    let thumbs: Vec<Canvas> = cuts.iter().map(|(d, ((x0, y0), (x1, y1)), _)| crate::thumb::make(&scene(g, k, d).crop(IBox::new(px(*x0, k), px(*y0, k), px(*x1, k), px(*y1, k))))).collect();
    let entries: Vec<Entry> = cuts.iter().zip(times).enumerate().map(|(i, ((_, ((x0, y0), (x1, y1)), src), tm))| Entry { id: i as u64 + 1, time: tm.into(), path: "x".into(), owned: false, w: px(x1 - x0, k) as u32, h: px(y1 - y0, k) as u32, source: (*src).into() }).collect();
    let items: Vec<historyview::Item> = entries.iter().zip(&thumbs).map(|(e, th)| historyview::Item { entry: e, thumb: Some(th), missing: false }).collect();
    let grid = Grid::new(px(620, k), px(400, k), k, items.len());
    let cv = historyview::render(&grid, &t, Lang::En, &items, Some(0), None, "2026-11-24", g);
    on_wall(&cv, k)
}

/// Named stills: (file stem, picture).
pub type Stills = Vec<(String, Canvas)>;

/// One film's script.
pub type Script = fn(&mut Film<'_>);

/// The README films, stories first: file name, where the pointer starts (DIPs), the script.
pub const FILMS: [(&str, (i32, i32), Script); 6] = [
    ("snip-game.gif", (560, 330), game_film),
    ("snip-bug-report.gif", (960, 600), bug_film),
    ("snip-copy-code.gif", (560, 560), code_film),
    ("snip-game-clip.gif", (700, 600), clip_film),
    ("snip-pin.gif", (560, 330), pin_film),
    ("snip-modes.gif", (840, 520), modes_film),
];

/// Write every film into `dir`, one at a time; returns a line per film and the stills (name, picture).
pub fn write_all(dir: &std::path::Path, g: &dyn Glyphs, ocr: Ocr<'_>) -> Result<(Vec<String>, Stills), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut lines = Vec::new();
    let mut stills = Vec::new();
    for (name, start, script) in FILMS {
        let path = dir.join(name);
        let file = std::io::BufWriter::new(std::fs::File::create(&path).map_err(|e| e.to_string())?);
        let mut f = Film::new(g, DPI, start, Some(Box::new(file)))?.with_ocr(ocr);
        script(&mut f);
        if let Some(e) = f.err.take() {
            return Err(format!("{name}: {e}"));
        }
        let out = f.out.take().ok_or("no output")?;
        let mut w = out.finish()?;
        w.flush().map_err(|e| e.to_string())?;
        drop(w);
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let seconds = f.count as f32 * DELAY as f32 / 100.0;
        let (fw, fh) = f.last.as_ref().map(|c| (c.w, c.h)).unwrap_or((0, 0));
        lines.push(format!("{name}: {fw}x{fh}, {} frames, {seconds:.1} s, {bytes} bytes", f.count));
        if let Some(t) = &f.read {
            lines.push(format!("  OCR read: {:?}", t));
        }
        stills.append(&mut f.marks);
    }
    stills.push(("settings".into(), settings_still(g)));
    stills.push(("history".into(), history_still(g)));
    Ok((lines, stills))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::Shape;
    use crate::raster::BoxFont;
    use crate::toolbar::Tool;

    fn run(script: Script, start: (i32, i32)) -> Film<'static> {
        static READ: fn(&Canvas) -> Result<String, String> = |_| Ok(CODE.join("\r\n"));
        let mut f = Film::new(&BoxFont, 96, start, None).unwrap().with_ocr(&READ);
        script(&mut f);
        assert!(f.err.is_none(), "{:?}", f.err);
        f
    }

    fn seconds(f: &Film<'_>) -> f32 {
        f.count as f32 * DELAY as f32 / 100.0
    }

    /// The area the editing tests pick: the notebook's page and the chart.
    const AREA: ((i32, i32), (i32, i32)) = ((224, 70), (1004, 478));

    fn r(w: (i32, i32, i32, i32)) -> Rect {
        Rect::new(w.0, w.1, w.2, w.3)
    }

    #[test]
    fn every_film_runs_6_to_15_seconds() {
        for (name, start, script) in FILMS {
            let f = run(script, start);
            let s = seconds(&f);
            assert!((6.0..=15.0).contains(&s), "{name}: {s} s");
            let last = f.last.as_ref().unwrap();
            assert_eq!((last.w as i32, last.h as i32), (W, H + STRIP), "{name}");
            assert_ne!(f.first.as_ref().unwrap(), last, "{name} moves");
        }
    }

    #[test]
    fn the_game_film_circles_and_labels_the_find_then_copies_it() {
        let f = run(game_film, (560, 330));
        assert_eq!(f.marks.len(), 1);
        assert_eq!((f.marks[0].1.w as i32, f.marks[0].1.h as i32), (W, H), "stills have no strip");
        assert!(matches!(f.stage, Stage::Result(_)));
        assert!(matches!(f.e.doc.shapes(), [Shape::Ellipse { .. }, Shape::Text { .. }]), "{:?}", f.e.doc.shapes());
        assert_eq!(f.e.color, Rgba::rgb(0xF2, 0xC1, 0x4E), "the coin's gold");
        assert_eq!(f.e.sel, Some(Rect::new(440, 262, 931, 541)));
    }

    #[test]
    fn the_bug_film_picks_the_window_hides_the_email_and_pastes_the_picture() {
        let f = run(bug_film, (960, 600));
        assert_eq!(f.e.sel, Some(r(INVOICE)), "one click took the whole window");
        assert_eq!(f.e.doc.shapes().len(), 3, "arrow, text, pixelate: {:?}", f.e.doc.shapes());
        assert!(matches!(f.e.doc.shapes()[2], Shape::Obscure { .. }));
        assert_eq!(f.desk.place, Place::Issue);
        let pasted = f.pasted.as_ref().expect("pasted");
        assert_eq!((pasted.w as i32, pasted.h as i32), (INVOICE.2 - INVOICE.0, INVOICE.3 - INVOICE.1));
        // The pasted picture is the editor's: its email line is no longer the scene's pixels.
        let plain = scene(&BoxFont, 1.0, &Desk::at(Place::Invoice));
        let (x, y) = (200, 142);
        assert_ne!(pasted.get(x - INVOICE.0, y - INVOICE.1), plain.get(x, y));
    }

    #[test]
    fn the_code_film_pastes_what_the_ocr_read_into_the_terminal() {
        let f = run(code_film, (560, 560));
        assert_eq!(f.read.as_deref(), Some(CODE.join("\r\n").as_str()));
        assert_eq!(f.desk.terminal, CODE.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(f.card.is_none(), "the card has gone when the paste lands");
        // Without a reader the card says so and nothing is pasted.
        let mut f = Film::new(&BoxFont, 96, (560, 560), None).unwrap();
        code_film(&mut f);
        assert!(f.read.is_none() && f.desk.terminal.is_empty());
    }

    #[test]
    fn the_clip_film_plays_the_game_inside_the_bars() {
        let mut f = Film::new(&BoxFont, 96, (700, 600), None).unwrap();
        f.set_desk(Desk::game(0));
        f.record_pick();
        f.drag((330, 250), (960, 560), 2, 3);
        let Stage::Recording { area, .. } = f.stage.clone() else { panic!("recording") };
        assert_eq!(area, Rect::new(330, 250, 961, 561));
        f.play(30);
        let frame = f.render_screen();
        let now = scene(&BoxFont, 1.0, &Desk::game(30));
        assert_eq!(frame.get(500, 400), now.get(500, 400), "inside: the game as it is now");
        assert_eq!(frame.get(area.left - 1, area.top + 20), REC_BAR);
        assert_ne!(scene(&BoxFont, 1.0, &Desk::game(0)), now, "the game moves");
        assert_eq!(coins_at(0), COINS);
        assert_eq!(coins_at(30), COINS + 30, "three coins over the pit");
        let f = run(clip_film, (700, 600));
        assert_eq!(f.desk.t, 50);
        assert_eq!(f.stage, Stage::Desktop);
    }

    #[test]
    fn the_hero_clears_the_pit() {
        for t in 0..60 {
            let (x, y) = hero_at(t);
            if (560.0 - 9.0..=660.0 + 9.0).contains(&x) {
                assert!(y < 509.0, "frame {t}: over the pit at {x}, {y}");
            }
        }
    }

    #[test]
    fn the_pin_film_keeps_the_chart_on_top_of_the_notes() {
        let f = run(pin_film, (560, 330));
        let (pin, at) = f.pin.clone().expect("pinned");
        assert_eq!((pin.w as i32, pin.h as i32), (WIN_B.2 - WIN_B.0 + 2, WIN_B.3 - WIN_B.1 + 2));
        assert_eq!(f.desk.place, Place::Notes);
        assert_eq!(f.desk.typed, NOTE.iter().map(|l| l.chars().count()).sum::<usize>());
        let frame = f.render_screen();
        assert_eq!(frame.get(at.x + 40, at.y + 60), pin.get(40, 60), "the pin is over the notebook");
    }

    #[test]
    fn the_modes_film_takes_the_chart_window_then_the_screen_then_shows_the_card() {
        let mut f = Film::new(&BoxFont, 96, (840, 520), None).unwrap();
        f.plain();
        f.glide((820, 300), 2);
        assert_eq!(f.plain_sel(), Some(r(WIN_B)), "the chart, on top");
        f.glide((300, 470), 2);
        assert_eq!(f.plain_sel(), Some(r(WIN_A)));
        f.click();
        assert!(matches!(&f.stage, Stage::Result(img) if (img.w as i32, img.h as i32) == (WIN_A.2 - WIN_A.0, WIN_A.3 - WIN_A.1)));
        let f = run(modes_film, (840, 520));
        assert!(f.card.is_some(), "PrtScn shows its card");
    }

    #[test]
    fn every_place_draws_something_of_its_own() {
        let places = [Place::Work, Place::Notes, Place::Game, Place::Invoice, Place::Issue, Place::Video];
        let shots: Vec<Canvas> = places.iter().map(|p| scene(&BoxFont, 1.0, &Desk::at(*p))).collect();
        for i in 0..shots.len() {
            for j in i + 1..shots.len() {
                assert_ne!(shots[i], shots[j], "{:?} and {:?}", places[i], places[j]);
            }
        }
    }

    #[test]
    fn the_script_drives_the_real_editor() {
        let mut f = Film::new(&BoxFont, 96, (500, 300), None).unwrap();
        f.open_editor();
        assert!(f.e.wants_loupe(), "selecting: magnifier on");
        f.drag(AREA.0, AREA.1, 4, 4);
        assert_eq!(f.e.sel, Some(Rect::new(224, 70, 1005, 479)));
        f.e.on_key('E' as u32, false, false);
        f.drag((704, 214), (786, 412), 2, 3);
        assert!(matches!(f.e.doc.shapes(), [Shape::Ellipse { .. }]));
        f.e.on_key('I' as u32, false, false);
        f.glide((744, 330), 2);
        f.click();
        assert_eq!(f.e.color, BARS[1].0, "the green bar");
        assert_eq!(f.e.tool, Some(Tool::Ellipse));
    }

    #[test]
    fn films_at_150_percent_are_twice_the_readme_width_and_scale_their_script() {
        let mut f = Film::new(&BoxFont, DPI, (500, 300), None).unwrap();
        f.open_editor();
        f.drag(AREA.0, AREA.1, 2, 2);
        assert_eq!(f.e.sel, Some(Rect::new(336, 105, 1507, 718)));
        f.hold(1);
        let last = f.last.as_ref().unwrap();
        assert_eq!((last.w, last.h), (1560, 1059));
    }

    #[test]
    fn stills_have_the_screen_size() {
        for s in [settings_still(&BoxFont), history_still(&BoxFont)] {
            assert_eq!((s.w as i32, s.h as i32), (px(W, 1.5), px(H, 1.5)));
        }
    }

    #[test]
    fn a_gif_keeps_only_what_changed() {
        let a = Canvas::new(40, 30, Rgba::rgb(10, 20, 30));
        let mut b = a.clone();
        b.set(5, 6, Rgba::rgb(200, 0, 0));
        b.set(9, 8, Rgba::rgb(200, 0, 0));
        assert_eq!(changed_box(&a, &b), Some(IBox::new(5, 6, 10, 9)));
        assert_eq!(changed_box(&a, &a), None);
        let mut g = GifOut::new(Vec::new(), 40, 30).unwrap();
        for f in [&a, &a, &b, &a] {
            g.push(f).unwrap();
        }
        let out = g.finish().unwrap();
        assert_eq!(&out[..6], b"GIF89a");
        // Three written frames (the repeat merges into the first), each with an image descriptor.
        assert!(out.iter().filter(|&&x| x == 0x2C).count() >= 3);
    }
}
