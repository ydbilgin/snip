//! The card that tells how a text capture ended: what it says, how long it stays and how it looks.
//! The window around it is `card.rs`. It never takes the keyboard.

use crate::geometry::Rect;
use crate::i18n::{Lang, S};
use crate::raster::{Canvas, Glyphs, IBox, Mask, Pt, Rgba};
use crate::textcore::{TextOutcome, measure, utf16_len};
use crate::theme::{self, Theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Done,
    Nothing,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardContent {
    pub mark: Mark,
    pub title: String,
    /// "2 satır, 99 karakter", right of the title.
    pub meta: Option<String>,
    pub body: String,
    pub tip: Option<String>,
}

/// What the card says. None: nothing is shown (a cancelled capture).
pub fn content(o: &TextOutcome, lang: Lang) -> Option<CardContent> {
    let s = |x: S| x.get(lang).to_string();
    let plain = |mark, title, body: S, tip: Option<S>| CardContent { mark, title: s(title), meta: None, body: s(body), tip: tip.map(s) };
    Some(match o {
        TextOutcome::Cancelled => return None,
        TextOutcome::Copied(text) => {
            let (lines, chars) = measure(text);
            let meta = S::CardMeta.get(lang).replace("{0}", &lines.to_string()).replace("{1}", &chars.to_string());
            CardContent { mark: Mark::Done, title: s(S::CardCopied), meta: Some(meta), body: text.clone(), tip: None }
        }
        TextOutcome::NoText => plain(Mark::Nothing, S::CardNoText, S::CardNoTextBody, Some(S::CardNoTextTip)),
        TextOutcome::ClipboardUnchanged => plain(Mark::Failed, S::CardUnchanged, S::CardUnchangedBody, Some(S::CardUnchangedTip)),
        TextOutcome::Error => plain(Mark::Failed, S::CardError, S::CardErrorBody, None),
        TextOutcome::Busy => plain(Mark::Nothing, S::CardBusy, S::CardBusyBody, None),
        TextOutcome::NoEngine => plain(Mark::Failed, S::CardNoEngine, S::CardNoEngineBody, Some(S::CardNoEngineTip)),
    })
}

/// How long the card stays (WinTextCapture's numbers): longer for longer text.
pub fn hold_ms(o: &TextOutcome) -> u32 {
    match o {
        TextOutcome::Copied(t) => 2400 + (utf16_len(t).saturating_mul(12) as u32).min(1600),
        TextOutcome::Busy => 1800,
        _ => 3600,
    }
}

/// Leaving the card with the mouse gives this much more time.
pub const LEAVE_MS: u32 = 1200;
pub const SLIDE_IN_MS: u32 = 170;
pub const FADE_OUT_MS: u32 = 180;
pub const SLIDE_DIP: f32 = 10.0;

/// Bottom right of the monitor's work area.
pub fn place(work: Rect, w: i32, h: i32, margin: i32) -> (i32, i32) {
    ((work.right - w - margin).max(work.left), (work.bottom - h - margin).max(work.top))
}

/// Lines for `text`: its own line breaks stay, a long line wraps at spaces, and when the text does
/// not fit in `max_lines` the last line ends in an ellipsis on a whole word. `width` measures.
pub fn wrap_lines(text: &str, max_lines: usize, max_w: i32, width: &dyn Fn(&str) -> i32) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cut = false;
    'source: for source in text.split('\n').map(|l| l.trim_end_matches('\r').trim()).filter(|l| !l.is_empty()) {
        let mut line = String::new();
        for word in source.split_whitespace() {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if width(&candidate) <= max_w {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
                if out.len() == max_lines {
                    cut = true;
                    break 'source;
                }
            }
            // A word wider than the whole line is cut by characters.
            let mut rest = String::new();
            for ch in word.chars() {
                let mut t = rest.clone();
                t.push(ch);
                if width(&t) > max_w && !rest.is_empty() {
                    out.push(std::mem::take(&mut rest));
                    if out.len() == max_lines {
                        cut = true;
                        break 'source;
                    }
                }
                rest.push(ch);
            }
            line = rest;
        }
        if !line.is_empty() {
            if out.len() == max_lines {
                cut = true;
                break 'source;
            }
            out.push(line);
        }
    }
    if cut && let Some(last) = out.last_mut() {
        let mut t = format!("{last}\u{2026}");
        while width(&t) > max_w {
            let body = t.trim_end_matches('\u{2026}');
            t = match body.rsplit_once(' ') {
                Some((head, _)) => format!("{head}\u{2026}"),
                None => {
                    let mut b = body.to_string();
                    b.pop();
                    if b.is_empty() {
                        break;
                    }
                    format!("{b}\u{2026}")
                }
            };
        }
        *last = t;
    }
    out
}

struct Metrics {
    w: i32,
    pad: i32,
    mark: i32,
    gap: i32,
    title_h: i32,
    line: i32,
    small: i32,
}

fn metrics(t: &Theme, scale: f32, g: &dyn Glyphs) -> Metrics {
    let s = |d: f32| (d * scale).round() as i32;
    Metrics {
        w: s(theme::CARD_WIDTH),
        pad: s(theme::CARD_PAD),
        mark: s(theme::CARD_MARK),
        gap: s(theme::CARD_MARK_GAP),
        title_h: g.line_height(t.font * scale) as i32,
        line: g.line_height(t.font * scale) as i32,
        small: g.line_height(t.font_small * scale) as i32,
    }
}

/// The card, on a see-through ground: rounded corners and a 1-DIP border.
pub fn render(c: &CardContent, t: &Theme, scale: f32, g: &dyn Glyphs) -> Canvas {
    let m = metrics(t, scale, g);
    let s = |d: f32| (d * scale).round() as i32;
    let font = t.font * scale;
    let small = t.font_small * scale;
    let text_x = m.pad + m.mark + m.gap;
    let inner = m.w - text_x - m.pad;
    let body = wrap_lines(&c.body, theme::CARD_BODY_LINES, inner, &|x| g.text(x, font, false).w as i32);
    let tip = c.tip.as_deref().map(|x| wrap_lines(x, 2, inner, &|y| g.text(y, small, false).w as i32)).unwrap_or_default();
    let mut h = m.pad + m.title_h.max(m.mark) + m.pad;
    if !body.is_empty() {
        h += s(4.0) + body.len() as i32 * m.line;
    }
    if !tip.is_empty() {
        h += s(6.0) + tip.len() as i32 * m.small;
    }
    let mut cv = Canvas::new(m.w as u32, h as u32, Rgba::rgb(0, 0, 0).with_alpha(0));
    let r = t.radius * scale;
    let b = cv.bounds();
    let mut mask = Mask::new(b);
    mask.rounded_box(0.0, 0.0, m.w as f32, h as f32, r);
    cv.blend_mask(&mask, t.border);
    let bw = (theme::BORDER_WIDTH * scale).round().max(1.0);
    let mut fill = Mask::new(b);
    fill.rounded_box(bw, bw, m.w as f32 - bw, h as f32 - bw, (r - bw).max(0.0));
    cv.blend_mask(&fill, t.bg);
    draw_mark(&mut cv, c.mark, m.pad, m.pad + (m.title_h.max(m.mark) - m.mark) / 2, m.mark, scale, t);
    let title = g.text(&c.title, font, true);
    let title_y = m.pad + (m.title_h.max(m.mark) - title.h as i32) / 2;
    cv.text_mask(&title, text_x, title_y, t.text);
    if let Some(meta) = &c.meta {
        let tm = g.text(meta, small, false);
        cv.text_mask(&tm, m.w - m.pad - tm.w as i32, title_y + (title.h as i32 - tm.h as i32) / 2, t.muted);
    }
    let mut y = m.pad + m.title_h.max(m.mark);
    if !body.is_empty() {
        y += s(4.0);
        for l in &body {
            cv.text(g, l, font, false, text_x, y, t.text);
            y += m.line;
        }
    }
    if !tip.is_empty() {
        y += s(6.0);
        for l in &tip {
            cv.text(g, l, small, false, text_x, y, t.text);
            y += m.small;
        }
    }
    cv
}

fn draw_mark(cv: &mut Canvas, mark: Mark, x: i32, y: i32, size: i32, scale: f32, t: &Theme) {
    let b = IBox::new(x, y, x + size, y + size);
    let c = Pt::new(x as f32 + size as f32 / 2.0, y as f32 + size as f32 / 2.0);
    let stroke = 2.0 * scale;
    let mut disc = Mask::new(b);
    disc.disc(c, size as f32 / 2.0);
    let k = size as f32 / 20.0;
    let at = |dx: f32, dy: f32| Pt::new(x as f32 + dx * k, y as f32 + dy * k);
    match mark {
        Mark::Done => {
            cv.blend_mask(&disc, theme::MARKER);
            let mut m = Mask::new(b);
            m.polyline(&[at(5.5, 10.5), at(8.8, 13.8), at(14.5, 6.8)], stroke);
            cv.blend_mask(&m, theme::MARKER_INK);
        }
        Mark::Nothing => {
            cv.blend_mask(&disc, t.muted);
            let mut m = Mask::new(b);
            m.polyline(&[at(6.0, 10.0), at(14.0, 10.0)], stroke);
            cv.blend_mask(&m, t.bg);
        }
        Mark::Failed => {
            cv.blend_mask(&disc, t.danger);
            let mut m = Mask::new(b);
            m.polyline(&[at(10.0, 5.0), at(10.0, 11.5)], stroke);
            m.disc(at(10.0, 14.8), 1.3 * scale);
            cv.blend_mask(&m, Rgba::rgb(255, 255, 255));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::BoxFont;
    use crate::theme::{Look, Mode};

    fn width(s: &str) -> i32 {
        s.chars().count() as i32 * 10
    }

    #[test]
    fn every_outcome_says_something_plain_except_cancel() {
        for lang in [Lang::En, Lang::Tr] {
            assert_eq!(content(&TextOutcome::Cancelled, lang), None);
            for o in [TextOutcome::Copied("a\r\nb".into()), TextOutcome::NoText, TextOutcome::ClipboardUnchanged, TextOutcome::Error, TextOutcome::Busy, TextOutcome::NoEngine] {
                let c = content(&o, lang).unwrap();
                assert!(!c.title.is_empty() && !c.body.is_empty(), "{o:?}");
            }
        }
        let c = content(&TextOutcome::Copied("Şirket\r\nGörüşme".into()), Lang::Tr).unwrap();
        assert_eq!((c.title.as_str(), c.meta.as_deref()), ("Kopyalandı", Some("2 satır, 13 karakter")));
    }

    #[test]
    fn longer_text_stays_longer_up_to_a_limit() {
        assert_eq!(hold_ms(&TextOutcome::Copied("abc".into())), 2436);
        assert_eq!(hold_ms(&TextOutcome::Copied("x".repeat(1000))), 4000);
        assert_eq!(hold_ms(&TextOutcome::Busy), 1800);
        assert_eq!(hold_ms(&TextOutcome::NoText), 3600);
    }

    #[test]
    fn the_card_sits_in_the_corner_of_the_work_area() {
        let work = Rect::new(0, 0, 2560, 1392);
        assert_eq!(place(work, 400, 100, 16), (2144, 1276));
        assert_eq!(place(Rect::new(-1920, 0, 0, 1040), 400, 100, 16), (-416, 924));
    }

    #[test]
    fn line_breaks_stay_and_long_lines_wrap() {
        assert_eq!(wrap_lines("ab cd\r\nef", 3, 100, &width), vec!["ab cd", "ef"]);
        assert_eq!(wrap_lines("aaaa bbbb cccc", 3, 90, &width), vec!["aaaa bbbb", "cccc"]);
        assert_eq!(wrap_lines("\r\n\r\n", 3, 90, &width), Vec::<String>::new());
    }

    #[test]
    fn text_that_does_not_fit_ends_in_an_ellipsis_on_a_whole_word() {
        let v = wrap_lines("one\ntwo\nthree\nfour", 3, 100, &width);
        assert_eq!(v, vec!["one", "two", "three\u{2026}"]);
        let v = wrap_lines("aaaa bbbb cccc dddd", 1, 100, &width);
        assert_eq!(v, vec!["aaaa bbbb\u{2026}"]);
        assert!(v.iter().all(|l| width(l) <= 100));
    }

    #[test]
    fn a_word_wider_than_the_line_is_cut() {
        let v = wrap_lines(&"x".repeat(35), 3, 100, &width);
        assert_eq!(v.len(), 3);
        assert!(v.iter().all(|l| width(l) <= 100), "{v:?}");
    }

    #[test]
    fn the_card_renders_with_see_through_corners_and_the_yellow_mark() {
        let t = Theme::new(Look::Graphite, Mode::Dark);
        let c = content(&TextOutcome::Copied("satır bir\r\nsatır iki".into()), Lang::Tr).unwrap();
        let cv = render(&c, &t, 1.0, &BoxFont);
        assert_eq!(cv.w, 400);
        assert_eq!(cv.get(0, 0).a, 0);
        let (px, py) = (16 + 10, 16 + 2);
        assert_eq!(cv.get(px, py), theme::MARKER, "the yellow disc");
        for mode in [Mode::Light, Mode::Dark] {
            let t = Theme::new(Look::Graphite, mode);
            for o in [TextOutcome::NoText, TextOutcome::NoEngine, TextOutcome::Error, TextOutcome::Busy, TextOutcome::ClipboardUnchanged] {
                let c = content(&o, Lang::Tr).unwrap();
                assert!(render(&c, &t, 1.5, &BoxFont).h > 40);
            }
        }
    }
}
