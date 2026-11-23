//! The main window: what Snip does, one row per job. Each row says what the job does, its key, and
//! has one button that starts it. Below: what was taken lately. Drawing and hit testing only; the
//! window around it is in `uiwin.rs`.

use crate::i18n::{Lang, S};
use crate::icons::{self, Icon};
use crate::raster::{Canvas, Glyphs, IBox, Mask, Rgba};
use crate::settingsview::key_caps;
use crate::theme::{self, Theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    Snip,
    Edit,
    All,
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Settings,
    Start(Job),
    Delay(u32),
    /// The function is turned off: the row's button turns it on. true: screenshots.
    TurnOn(bool),
    StartTray,
    Thumb(usize),
    SeeAll,
    /// More texts are kept than the three shown: a menu of all of them.
    AllTexts,
    TextLine(usize),
}

pub struct RecentText {
    pub preview: String,
    pub when: String,
}

pub struct Data<'a> {
    pub snip_on: bool,
    pub text_on: bool,
    /// The key of each job as text; empty: no key.
    pub snip_key: &'a str,
    pub edit_key: &'a str,
    pub all_key: &'a str,
    pub text_key: &'a str,
    pub tray_running: bool,
    pub thumbs: &'a [&'a Canvas],
    pub texts: &'a [RecentText],
    /// More texts are kept than `texts` holds.
    pub more_texts: bool,
    pub keep_texts: bool,
    pub hover: Option<Part>,
    /// A short answer in the footer ("Kopyalandı").
    pub note: Option<&'a str>,
}

const MARGIN: f32 = 24.0;
const HEADER: f32 = 76.0;
const NOTICE: f32 = 48.0;
const HEADING: f32 = 30.0;
const ROW: f32 = 68.0;
const GROUP_GAP: f32 = 10.0;
const RECENT: f32 = 96.0;
const FOOTER: f32 = 38.0;
const CARD_PAD: f32 = 16.0;
const BUTTON_W: f32 = 104.0;
const BUTTON_H: f32 = 32.0;
const SMALL_BUTTON_W: f32 = 49.0;
const ICON: f32 = 20.0;
const TEXT_X: f32 = 52.0;
const THUMB_W: f32 = 96.0;
const THUMB_H: f32 = 62.0;
const THUMBS: usize = 3;
const TEXT_ROWS: usize = 3;

fn s(sc: f32, d: f32) -> i32 {
    (d * sc).round() as i32
}

/// One screen position of everything, from the width and whether the notice shows.
struct Geo {
    sc: f32,
    x0: i32,
    x1: i32,
    notice: Option<IBox>,
    image_heading: i32,
    rows: [IBox; 4],
    text_heading: i32,
    text_row: IBox,
    recent_heading: i32,
    recent_left: IBox,
    recent_right: IBox,
    footer: i32,
}

fn geo(w: i32, sc: f32, notice: bool) -> Geo {
    let x0 = s(sc, MARGIN);
    let x1 = w - x0;
    let mut y = s(sc, HEADER);
    let notice_box = notice.then(|| {
        let b = IBox::new(x0, y, x1, y + s(sc, NOTICE) - s(sc, 8.0));
        y += s(sc, NOTICE);
        b
    });
    let image_heading = y;
    y += s(sc, HEADING);
    let row = s(sc, ROW);
    let rows = [IBox::new(x0, y, x1, y + row), IBox::new(x0, y + row, x1, y + 2 * row), IBox::new(x0, y + 2 * row, x1, y + 3 * row), IBox::new(x0, y + 3 * row, x1, y + 4 * row)];
    y += 4 * row + s(sc, GROUP_GAP);
    let text_heading = y;
    y += s(sc, HEADING);
    let text_row = IBox::new(x0, y, x1, y + row);
    y += row + s(sc, GROUP_GAP);
    let recent_heading = y;
    y += s(sc, HEADING);
    let mid = (x0 + x1) / 2;
    let gap = s(sc, 6.0);
    let recent_left = IBox::new(x0, y, mid - gap, y + s(sc, RECENT));
    let recent_right = IBox::new(mid + gap, y, x1, y + s(sc, RECENT));
    y += s(sc, RECENT);
    Geo { sc, x0, x1, notice: notice_box, image_heading, rows, text_heading, text_row, recent_heading, recent_left, recent_right, footer: y }
}

pub fn height_dip(notice: bool) -> f32 {
    HEADER + if notice { NOTICE } else { 0.0 } + 2.0 * HEADING + 5.0 * ROW + 2.0 * GROUP_GAP + HEADING + RECENT + FOOTER
}

fn button_in(row: IBox, sc: f32) -> IBox {
    let right = row.x1 - s(sc, CARD_PAD);
    let mid = (row.y0 + row.y1) / 2;
    let half = s(sc, BUTTON_H) / 2;
    IBox::new(right - s(sc, BUTTON_W), mid - half, right, mid + half)
}

/// The boxes that answer to the mouse. `count` limits the thumbs and text lines to what exists.
pub fn parts(w: i32, sc: f32, d: &Data, lang: Lang, g: &dyn Glyphs, t: &Theme) -> Vec<(Part, IBox)> {
    let gm = geo(w, sc, !d.tray_running);
    let mut v = Vec::new();
    let top = (s(sc, HEADER) - s(sc, BUTTON_H)) / 2;
    let settings_w = g.text(S::Settings.get(lang), t.font * sc, false).w as i32 + s(sc, 24.0 + ICON);
    v.push((Part::Settings, IBox::new(gm.x1 - settings_w, top, gm.x1, top + s(sc, BUTTON_H))));
    if let Some(n) = gm.notice {
        let mid = (n.y0 + n.y1) / 2;
        let half = s(sc, 13.0);
        let right = n.x1 - s(sc, 10.0);
        v.push((Part::StartTray, IBox::new(right - s(sc, 76.0), mid - half, right, mid + half)));
    }
    if d.snip_on {
        v.push((Part::Start(Job::Snip), button_in(gm.rows[0], sc)));
        v.push((Part::Start(Job::All), button_in(gm.rows[1], sc)));
        v.push((Part::Start(Job::Edit), button_in(gm.rows[2], sc)));
        let b = button_in(gm.rows[3], sc);
        let one = s(sc, SMALL_BUTTON_W);
        v.push((Part::Delay(3), IBox::new(b.x0, b.y0, b.x0 + one, b.y1)));
        v.push((Part::Delay(5), IBox::new(b.x1 - one, b.y0, b.x1, b.y1)));
    } else {
        v.push((Part::TurnOn(true), button_in(gm.rows[0], sc)));
    }
    if d.text_on {
        v.push((Part::Start(Job::Text), button_in(gm.text_row, sc)));
    } else {
        v.push((Part::TurnOn(false), button_in(gm.text_row, sc)));
    }
    let see = g.text(S::SeeAll.get(lang), t.font_small * sc, false).w as i32 + s(sc, 16.0);
    let head_mid = gm.recent_heading + s(sc, HEADING) / 2;
    if !d.thumbs.is_empty() {
        v.push((Part::SeeAll, IBox::new(gm.recent_left.x1 - see, head_mid - s(sc, 12.0), gm.recent_left.x1, head_mid + s(sc, 12.0))));
    }
    if d.more_texts {
        v.push((Part::AllTexts, IBox::new(gm.recent_right.x1 - see, head_mid - s(sc, 12.0), gm.recent_right.x1, head_mid + s(sc, 12.0))));
    }
    for (i, b) in thumb_boxes(&gm).into_iter().enumerate().take(d.thumbs.len().min(THUMBS)) {
        v.push((Part::Thumb(i), b));
    }
    for (i, b) in text_boxes(&gm).into_iter().enumerate().take(d.texts.len().min(TEXT_ROWS)) {
        v.push((Part::TextLine(i), b));
    }
    v
}

fn thumb_boxes(gm: &Geo) -> Vec<IBox> {
    let sc = gm.sc;
    let b = gm.recent_left;
    let (tw, th) = (s(sc, THUMB_W), s(sc, THUMB_H));
    let free = b.w() - 2 * s(sc, CARD_PAD) - THUMBS as i32 * tw;
    let gap = (free / (THUMBS as i32 - 1)).max(0);
    let top = b.y0 + (b.h() - th) / 2;
    (0..THUMBS as i32).map(|i| { let x = b.x0 + s(sc, CARD_PAD) + i * (tw + gap); IBox::new(x, top, x + tw, top + th) }).collect()
}

fn text_boxes(gm: &Geo) -> Vec<IBox> {
    let sc = gm.sc;
    let b = gm.recent_right;
    let inset = s(sc, 6.0);
    let h = (b.h() - 2 * inset) / TEXT_ROWS as i32;
    (0..TEXT_ROWS as i32).map(|i| IBox::new(b.x0 + inset, b.y0 + inset + i * h, b.x1 - inset, b.y0 + inset + (i + 1) * h)).collect()
}

pub fn hit(list: &[(Part, IBox)], x: i32, y: i32) -> Option<Part> {
    list.iter().find(|(_, b)| b.contains(x, y)).map(|(p, _)| *p)
}

fn rounded(cv: &mut Canvas, b: IBox, r: f32, fill: Rgba) {
    let mut m = Mask::new(b);
    m.rounded_box(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32, r);
    cv.blend_mask(&m, fill);
}

fn ring(cv: &mut Canvas, b: IBox, r: f32, w: f32, color: Rgba) {
    let mut m = Mask::new(b);
    m.rounded_box_outline(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32, r, w);
    cv.blend_mask(&m, color);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Accent,
    Marker,
    Plain,
}

fn plain_ink(t: &Theme) -> Rgba {
    if t.look == theme::Look::Paper { t.text } else { t.on_raised }
}

#[allow(clippy::too_many_arguments)]
fn button(cv: &mut Canvas, b: IBox, label: &str, style: Style, hover: bool, t: &Theme, sc: f32, g: &dyn Glyphs) {
    let r = t.control_radius() * sc;
    let line = (theme::BORDER_WIDTH * sc).max(1.0);
    let ink = match style {
        Style::Accent => {
            rounded(cv, b, r, if hover { mix(t.accent, t.on_accent, 0.14) } else { t.accent });
            t.on_accent
        }
        Style::Marker => {
            rounded(cv, b, r, if hover { theme::MARKER_HOVER } else { theme::MARKER });
            theme::MARKER_INK
        }
        Style::Plain => {
            // Paper's raised colour is its accent: a plain button would read as the main one.
            let paper = t.look == theme::Look::Paper;
            rounded(cv, b, r, if hover { t.hover } else if paper { t.card } else { t.raised });
            ring(cv, b, r, line, t.border);
            plain_ink(t)
        }
    };
    let tm = g.text(label, t.font * sc, style != Style::Plain);
    cv.text_mask(&tm, b.x0 + (b.w() - tm.w as i32) / 2, b.y0 + (b.h() - tm.h as i32) / 2, ink);
}

fn mix(a: Rgba, b: Rgba, k: f32) -> Rgba {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
    Rgba::rgb(f(a.r, b.r), f(a.g, b.g), f(a.b, b.b))
}

fn caps(cv: &mut Canvas, text: &str, right: i32, mid: i32, t: &Theme, sc: f32, g: &dyn Glyphs) -> i32 {
    let mut x = right;
    let half = s(sc, 12.0);
    for cap in key_caps(text).iter().rev() {
        let tm = g.text(cap, t.font_small * sc, true);
        let w = tm.w as i32 + 2 * s(sc, 8.0);
        let b = IBox::new(x - w, mid - half, x, mid + half);
        let r = t.control_radius() * sc;
        rounded(cv, b, r, t.window);
        ring(cv, b, r, (theme::BORDER_WIDTH * sc).max(1.0), t.border);
        cv.text_mask(&tm, b.x0 + (w - tm.w as i32) / 2, mid - tm.h as i32 / 2, t.text);
        x -= w + s(sc, 4.0);
    }
    x
}

#[allow(clippy::too_many_arguments)]
fn job_row(cv: &mut Canvas, row: IBox, icon: Icon, icon_color: Rgba, title: &str, line: &str, key: &str, keyed: bool, off: bool, right_limit: i32, t: &Theme, sc: f32, g: &dyn Glyphs, lang: Lang) {
    let mid = (row.y0 + row.y1) / 2;
    let isz = s(sc, ICON) as f32;
    icons::draw(cv, icon, (row.x0 + s(sc, CARD_PAD)) as f32, mid as f32 - isz / 2.0, isz, if off { t.muted } else { icon_color });
    let tx = row.x0 + s(sc, TEXT_X);
    let (font, small) = (t.font * sc, t.font_small * sc);
    let mut caps_x = right_limit;
    if off {
        let tm = g.text(S::TurnedOff.get(lang), small, false);
        cv.text_mask(&tm, caps_x - tm.w as i32, mid - tm.h as i32 / 2, t.muted);
        caps_x -= tm.w as i32 + s(sc, 12.0);
    } else if !key.is_empty() {
        caps_x = caps(cv, key, caps_x, mid, t, sc, g) - s(sc, 8.0);
    } else if keyed {
        let tm = g.text(S::NoKeySet.get(lang), small, false);
        cv.text_mask(&tm, caps_x - tm.w as i32, mid - tm.h as i32 / 2, t.muted);
        caps_x -= tm.w as i32 + s(sc, 12.0);
    }
    let avail = caps_x - tx;
    let lines = crate::cardview::wrap_lines(line, 2, avail, &|x| g.text(x, small, false).w as i32);
    let title_px = g.text(title, font, true);
    let small_h = g.line_height(small) as i32;
    let total = title_px.h as i32 + lines.len() as i32 * small_h + s(sc, 1.0);
    let mut y = mid - total / 2;
    cv.text_mask(&title_px, tx, y, if off { t.muted } else { t.text });
    y += title_px.h as i32 + s(sc, 1.0);
    for l in &lines {
        cv.text(g, l, small, false, tx, y, t.muted);
        y += small_h;
    }
}

fn heading(cv: &mut Canvas, x: i32, top: i32, text: &str, t: &Theme, sc: f32, g: &dyn Glyphs) {
    let tm = g.text(text, t.font * sc, true);
    cv.text_mask(&tm, x, top + (s(sc, HEADING) - tm.h as i32) / 2, t.text);
}

fn card(cv: &mut Canvas, b: IBox, t: &Theme, sc: f32) {
    let r = t.radius * sc;
    rounded(cv, b, r, t.card);
    ring(cv, b, r, (theme::BORDER_WIDTH * sc).max(1.0), t.border);
}

pub fn render(w: i32, h: i32, sc: f32, d: &Data, lang: Lang, t: &Theme, g: &dyn Glyphs) -> Canvas {
    let mut cv = Canvas::new(w as u32, h as u32, t.window);
    let gm = geo(w, sc, !d.tray_running);
    let list = parts(w, sc, d, lang, g, t);
    let at = |p: Part| list.iter().find(|(q, _)| *q == p).map(|(_, b)| *b);
    let hover = |p: Part| d.hover == Some(p);
    let (font, small) = (t.font * sc, t.font_small * sc);

    // Header: the name, then in one sentence what it is.
    let title = g.text("Snip", theme::TITLE_FONT * sc, true);
    let line_h = g.line_height(font) as i32;
    let y = (s(sc, HEADER) - title.h as i32 - line_h) / 2;
    cv.text_mask(&title, gm.x0, y, t.text);
    let settings_x = at(Part::Settings).map(|b| b.x0).unwrap_or(gm.x1);
    let app_line = crate::cardview::wrap_lines(S::AppLine.get(lang), 1, settings_x - gm.x0 - s(sc, 12.0), &|x| g.text(x, font, false).w as i32);
    if let Some(l) = app_line.first() {
        cv.text(g, l, font, false, gm.x0, y + title.h as i32, t.muted);
    }
    if let Some(b) = at(Part::Settings) {
        let r = t.control_radius() * sc;
        rounded(&mut cv, b, r, if hover(Part::Settings) { t.hover } else { t.raised });
        ring(&mut cv, b, r, (theme::BORDER_WIDTH * sc).max(1.0), t.border);
        let isz = s(sc, 16.0) as f32;
        icons::draw(&mut cv, Icon::Settings, (b.x0 + s(sc, 9.0)) as f32, (b.y0 + b.y1) as f32 / 2.0 - isz / 2.0, isz, plain_ink(t));
        let tm = g.text(S::Settings.get(lang), font, false);
        cv.text_mask(&tm, b.x0 + s(sc, 30.0), b.y0 + (b.h() - tm.h as i32) / 2, t.on_raised);
    }

    if let Some(n) = gm.notice {
        card(&mut cv, n, t, sc);
        let tm = g.text(S::TrayNotRunning.get(lang), font, false);
        cv.text_mask(&tm, n.x0 + s(sc, CARD_PAD), (n.y0 + n.y1 - tm.h as i32) / 2, t.danger);
        if let Some(b) = at(Part::StartTray) {
            button(&mut cv, b, S::StartSnip.get(lang), Style::Plain, hover(Part::StartTray), t, sc, g);
        }
    }

    // Picture of the screen.
    heading(&mut cv, gm.x0, gm.image_heading, S::GroupImage.get(lang), t, sc, g);
    let all = IBox::new(gm.rows[0].x0, gm.rows[0].y0, gm.rows[3].x1, gm.rows[3].y1);
    card(&mut cv, all, t, sc);
    let one = s(sc, theme::BORDER_WIDTH).max(1);
    for r in &gm.rows[1..] {
        cv.fill_box(IBox::new(r.x0 + s(sc, CARD_PAD), r.y0, r.x1 - s(sc, CARD_PAD), r.y0 + one), t.border);
    }
    let right_limit = |row: IBox| button_in(row, sc).x0 - s(sc, 14.0);
    let on = !d.snip_on;
    job_row(&mut cv, gm.rows[0], Icon::Region, t.accent, S::FnSnip.get(lang), S::FnSnipLine.get(lang), d.snip_key, true, on, right_limit(gm.rows[0]), t, sc, g, lang);
    job_row(&mut cv, gm.rows[1], Icon::Screen, t.text, S::AllScreens.get(lang), S::FnAllLine.get(lang), d.all_key, true, on, right_limit(gm.rows[1]), t, sc, g, lang);
    job_row(&mut cv, gm.rows[2], Icon::Pen, t.text, S::FnEdit.get(lang), S::FnEditLine.get(lang), d.edit_key, true, on, right_limit(gm.rows[2]), t, sc, g, lang);
    job_row(&mut cv, gm.rows[3], Icon::Timer, t.text, S::FnDelay.get(lang), S::FnDelayLine.get(lang), "", false, on, right_limit(gm.rows[3]), t, sc, g, lang);
    if d.snip_on {
        if let Some(b) = at(Part::Start(Job::Snip)) {
            button(&mut cv, b, S::ChooseArea.get(lang), Style::Accent, hover(Part::Start(Job::Snip)), t, sc, g);
        }
        if let Some(b) = at(Part::Start(Job::All)) {
            button(&mut cv, b, S::TakeNow.get(lang), Style::Plain, hover(Part::Start(Job::All)), t, sc, g);
        }
        if let Some(b) = at(Part::Start(Job::Edit)) {
            button(&mut cv, b, S::ChooseArea.get(lang), Style::Plain, hover(Part::Start(Job::Edit)), t, sc, g);
        }
        for (secs, label) in [(3, S::Seconds3), (5, S::Seconds5)] {
            if let Some(b) = at(Part::Delay(secs)) {
                button(&mut cv, b, label.get(lang), Style::Plain, hover(Part::Delay(secs)), t, sc, g);
            }
        }
    } else if let Some(b) = at(Part::TurnOn(true)) {
        button(&mut cv, b, S::TurnOn.get(lang), Style::Plain, hover(Part::TurnOn(true)), t, sc, g);
    }

    // Text from the screen.
    heading(&mut cv, gm.x0, gm.text_heading, S::GroupText.get(lang), t, sc, g);
    card(&mut cv, gm.text_row, t, sc);
    let icon_color = if t.mode == theme::Mode::Dark { theme::MARKER } else { theme::MARKER_LINE };
    job_row(&mut cv, gm.text_row, Icon::TextCopy, icon_color, S::FnText.get(lang), S::FnTextLine.get(lang), d.text_key, true, !d.text_on, right_limit(gm.text_row), t, sc, g, lang);
    if d.text_on {
        if let Some(b) = at(Part::Start(Job::Text)) {
            button(&mut cv, b, S::ChooseText.get(lang), Style::Marker, hover(Part::Start(Job::Text)), t, sc, g);
        }
    } else if let Some(b) = at(Part::TurnOn(false)) {
        button(&mut cv, b, S::TurnOn.get(lang), Style::Plain, hover(Part::TurnOn(false)), t, sc, g);
    }

    // Lately.
    heading(&mut cv, gm.x0, gm.recent_heading, S::RecentSnips.get(lang), t, sc, g);
    heading(&mut cv, gm.recent_right.x0, gm.recent_heading, S::RecentTexts.get(lang), t, sc, g);
    for part in [Part::SeeAll, Part::AllTexts] {
        if let Some(b) = at(part) {
            if hover(part) {
                rounded(&mut cv, b, t.control_radius() * sc, t.hover);
            }
            let tm = g.text(S::SeeAll.get(lang), small, false);
            cv.text_mask(&tm, b.x0 + (b.w() - tm.w as i32) / 2, b.y0 + (b.h() - tm.h as i32) / 2, t.muted);
        }
    }
    card(&mut cv, gm.recent_left, t, sc);
    card(&mut cv, gm.recent_right, t, sc);
    if d.thumbs.is_empty() {
        empty_line(&mut cv, gm.recent_left, S::NoSnipsYet.get(lang), t, sc, g);
    }
    for (i, b) in thumb_boxes(&gm).into_iter().enumerate().take(d.thumbs.len().min(THUMBS)) {
        let th = d.thumbs[i];
        let cell = IBox::new(b.x0, b.y0, b.x1, b.y0 + s(sc, THUMB_H));
        rounded(&mut cv, cell, t.control_radius() * sc, t.window);
        let fit = th.downscale_to_fit(cell.w() as u32, cell.h() as u32);
        cv.draw(&fit, cell.x0 + (cell.w() - fit.w as i32) / 2, cell.y0 + (cell.h() - fit.h as i32) / 2);
        let line = if hover(Part::Thumb(i)) { t.accent } else { t.border };
        ring(&mut cv, cell, t.control_radius() * sc, (theme::BORDER_WIDTH * sc).max(1.0), line);
    }
    if d.texts.is_empty() {
        let line = if d.keep_texts { S::NoTextsYet } else { S::TextsOff };
        empty_line(&mut cv, gm.recent_right, line.get(lang), t, sc, g);
    }
    for (i, b) in text_boxes(&gm).into_iter().enumerate().take(d.texts.len().min(TEXT_ROWS)) {
        if hover(Part::TextLine(i)) {
            rounded(&mut cv, b, t.control_radius() * sc, t.hover);
        }
        let e = &d.texts[i];
        let when = g.text(&e.when, small, false);
        let (l, r) = (b.x0 + s(sc, 10.0), b.x1 - s(sc, 10.0));
        cv.text_mask(&when, r - when.w as i32, (b.y0 + b.y1 - when.h as i32) / 2, t.muted);
        let avail = r - when.w as i32 - s(sc, 12.0) - l;
        let lines = crate::cardview::wrap_lines(&e.preview, 1, avail, &|x| g.text(x, small, false).w as i32);
        if let Some(line) = lines.first() {
            let tm = g.text(line, small, false);
            cv.text_mask(&tm, l, (b.y0 + b.y1 - tm.h as i32) / 2, t.text);
        }
    }

    // Footer.
    let msg = d.note.unwrap_or(S::NoNetwork.get(lang));
    cv.text(g, msg, small, false, gm.x0, gm.footer + (s(sc, FOOTER) - g.line_height(small) as i32) / 2, if d.note.is_some() { t.text } else { t.muted });
    cv
}

fn empty_line(cv: &mut Canvas, b: IBox, text: &str, t: &Theme, sc: f32, g: &dyn Glyphs) {
    let small = t.font_small * sc;
    let avail = b.w() - 2 * s(sc, CARD_PAD);
    let lines = crate::cardview::wrap_lines(text, 3, avail, &|x| g.text(x, small, false).w as i32);
    let lh = g.line_height(small) as i32;
    let mut y = (b.y0 + b.y1 - lines.len() as i32 * lh) / 2;
    for l in &lines {
        cv.text(g, l, small, false, b.x0 + s(sc, CARD_PAD), y, t.muted);
        y += lh;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::BoxFont;
    use crate::theme::{Look, Mode};

    fn data<'a>(thumbs: &'a [&'a Canvas], texts: &'a [RecentText], snip_on: bool, text_on: bool, running: bool) -> Data<'a> {
        Data { snip_on, text_on, snip_key: "Win+Shift+S", edit_key: "Pause", all_key: "PrtScn", text_key: "Win+Shift+T", tray_running: running, thumbs, texts, more_texts: false, keep_texts: true, hover: None, note: None }
    }

    #[test]
    fn all_texts_is_offered_only_when_more_are_kept_than_shown() {
        let t = Theme::new(Look::Graphite, Mode::Light);
        let mut d = data(&[], &[], true, true, true);
        assert!(!parts(760, 1.0, &d, Lang::Tr, &BoxFont, &t).iter().any(|(p, _)| *p == Part::AllTexts));
        d.more_texts = true;
        let list = parts(760, 1.0, &d, Lang::Tr, &BoxFont, &t);
        let (_, b) = list.iter().find(|(p, _)| *p == Part::AllTexts).expect("offered");
        assert_eq!(hit(&list, b.x0 + 1, b.y0 + 1), Some(Part::AllTexts));
    }

    #[test]
    fn every_job_has_one_button_that_starts_it() {
        let t = Theme::new(Look::Graphite, Mode::Light);
        let d = data(&[], &[], true, true, true);
        let list = parts(760, 1.0, &d, Lang::Tr, &BoxFont, &t);
        for p in [Part::Settings, Part::Start(Job::Snip), Part::Start(Job::Edit), Part::Start(Job::All), Part::Start(Job::Text), Part::Delay(3), Part::Delay(5)] {
            let (_, b) = list.iter().find(|(q, _)| *q == p).unwrap_or_else(|| panic!("{p:?}"));
            assert_eq!(hit(&list, b.x0 + 1, b.y0 + 1), Some(p));
        }
        assert!(!list.iter().any(|(p, _)| matches!(p, Part::TurnOn(_) | Part::StartTray | Part::SeeAll)), "nothing to see yet: no See all");
    }

    #[test]
    fn a_function_turned_off_offers_only_turning_it_on_and_the_other_still_works() {
        let t = Theme::new(Look::Graphite, Mode::Dark);
        let d = data(&[], &[], false, true, true);
        let list = parts(760, 1.0, &d, Lang::Tr, &BoxFont, &t);
        assert!(list.iter().any(|(p, _)| *p == Part::TurnOn(true)));
        assert!(!list.iter().any(|(p, _)| matches!(p, Part::Start(Job::Snip) | Part::Start(Job::Edit) | Part::Start(Job::All) | Part::Delay(_))));
        assert!(list.iter().any(|(p, _)| *p == Part::Start(Job::Text)));
        let d = data(&[], &[], true, false, true);
        let list = parts(760, 1.0, &d, Lang::Tr, &BoxFont, &t);
        assert!(list.iter().any(|(p, _)| *p == Part::TurnOn(false)));
        assert!(!list.iter().any(|(p, _)| *p == Part::Start(Job::Text)));
        assert!(list.iter().any(|(p, _)| *p == Part::Start(Job::Snip)));
    }

    #[test]
    fn nothing_leaves_the_window_at_any_scale_or_state() {
        let th = Canvas::new(200, 125, Rgba::rgb(10, 20, 30));
        let thumbs = [&th, &th, &th, &th];
        let texts: Vec<RecentText> = (0..5).map(|i| RecentText { preview: format!("metin {i}"), when: "12:00".into() }).collect();
        for (sc, running) in [(1.0, true), (1.5, false), (2.0, true)] {
            let t = Theme::new(Look::Paper, Mode::Light);
            let d = data(&thumbs, &texts, true, true, running);
            let (w, h) = (((760.0 * sc) as i32), (height_dip(!running) * sc) as i32);
            for (p, b) in parts(w, sc, &d, Lang::Tr, &BoxFont, &t) {
                assert!(b.x0 >= 0 && b.x1 <= w && b.y1 <= h, "{p:?} {b:?} in {w}x{h}");
            }
            let cv = render(w, h, sc, &d, Lang::Tr, &t, &BoxFont);
            assert_eq!((cv.w as i32, cv.h as i32), (w, h));
        }
    }

    #[test]
    fn the_notice_appears_only_when_the_keys_would_not_work() {
        let t = Theme::new(Look::Graphite, Mode::Light);
        let d = data(&[], &[], true, true, false);
        assert!(parts(760, 1.0, &d, Lang::En, &BoxFont, &t).iter().any(|(p, _)| *p == Part::StartTray));
        assert!(height_dip(true) > height_dip(false));
    }

    #[test]
    fn the_text_button_is_yellow_and_the_snip_button_is_the_accent() {
        let t = Theme::new(Look::Graphite, Mode::Light);
        let d = data(&[], &[], true, true, true);
        let cv = render(760, height_dip(false) as i32, 1.0, &d, Lang::Tr, &t, &BoxFont);
        let list = parts(760, 1.0, &d, Lang::Tr, &BoxFont, &t);
        let at = |p: Part| list.iter().find(|(q, _)| *q == p).unwrap().1;
        let (a, b) = (at(Part::Start(Job::Text)), at(Part::Start(Job::Snip)));
        assert_eq!(cv.get(a.x0 + 6, a.y0 + 6), theme::MARKER);
        assert_eq!(cv.get(b.x0 + 6, b.y0 + 6), t.accent);
    }
}
