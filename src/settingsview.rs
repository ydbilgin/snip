//! The settings window: three tabs (screen snip, copy text, general), rows, layout, hit testing,
//! applying a click to the config, the hotkey recorder, and drawing. Every change is written to
//! config.toml at once and the tray process reloads it; there is no Apply button.

use crate::config::Config;
use crate::hotkey::{self, HkMods, Hotkey};
use crate::i18n::{Lang, LangChoice, S};
use crate::icons::{self, Icon};
use crate::raster::{Canvas, Glyphs, IBox, Mask, Pt, Rgba};
use crate::theme::{Look, Theme, ThemeChoice};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    SnipEnabled,
    WinShiftS,
    Hotkey,
    Hotkey2,
    AllHotkey,
    AllHotkey2,
    SaveFiles,
    Folder,
    Clipboard,
    History,
    TextEnabled,
    TextHotkey,
    OcrLanguage,
    KeepTexts,
    Autostart,
    Theme,
    Language,
    Look,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyField {
    Editor,
    Editor2,
    All,
    All2,
    Text,
}

pub const TABS: usize = 3;
pub const TAB_SNIP: usize = 0;
pub const TAB_TEXT: usize = 1;
pub const TAB_GENERAL: usize = 2;

const SNIP_KEYS: [Row; 6] = [Row::SnipEnabled, Row::WinShiftS, Row::AllHotkey, Row::AllHotkey2, Row::Hotkey, Row::Hotkey2];
const SNIP_SAVE: [Row; 4] = [Row::SaveFiles, Row::Folder, Row::Clipboard, Row::History];
const TEXT_KEYS: [Row; 2] = [Row::TextEnabled, Row::TextHotkey];
const TEXT_READ: [Row; 2] = [Row::OcrLanguage, Row::KeepTexts];
const GEN_PROGRAM: [Row; 1] = [Row::Autostart];
const GEN_LOOKS: [Row; 3] = [Row::Theme, Row::Language, Row::Look];

/// Each tab's groups: a heading and its rows.
pub fn sections(tab: usize) -> [(S, &'static [Row]); 2] {
    match tab {
        TAB_SNIP => [(S::SectionKeysAndUse, &SNIP_KEYS), (S::SectionSaving, &SNIP_SAVE)],
        TAB_TEXT => [(S::SectionKeysAndUse, &TEXT_KEYS), (S::SectionReading, &TEXT_READ)],
        _ => [(S::SectionProgram, &GEN_PROGRAM), (S::SectionLooks, &GEN_LOOKS)],
    }
}

pub fn tab_title(tab: usize) -> S {
    match tab {
        TAB_SNIP => S::FnSnip,
        TAB_TEXT => S::FnText,
        _ => S::TabGeneral,
    }
}

pub const WIDTH: f32 = crate::theme::SETTINGS_WIDTH;
pub const HEADER: f32 = crate::theme::SETTINGS_HEADER;
pub const TAB_H: f32 = crate::theme::SETTINGS_TABS;
pub const ROW_H: f32 = crate::theme::SETTINGS_ROW_HEIGHT;
pub const FOOTER: f32 = crate::theme::SETTINGS_FOOTER;
pub const MARGIN: f32 = crate::theme::SETTINGS_MARGIN;
pub const CARD_PAD: f32 = crate::theme::SETTINGS_CARD_PAD;

/// The tallest tab decides the window: it does not change size between tabs.
pub fn height_dip() -> f32 {
    let rows = (0..TABS).map(|t| sections(t).iter().map(|(_, r)| r.len()).sum::<usize>()).max().unwrap_or(0);
    HEADER + TAB_H + rows as f32 * ROW_H + 2.0 * crate::theme::SETTINGS_SECTION_LABEL + crate::theme::SETTINGS_SECTION_GAP + FOOTER
}

impl Row {
    pub fn title(self) -> S {
        match self {
            Row::SnipEnabled => S::SnipEnabled,
            Row::WinShiftS => S::WinShiftS,
            Row::Hotkey => S::EditorHotkey,
            Row::Hotkey2 => S::Editor2Hotkey,
            Row::AllHotkey => S::AllHotkey,
            Row::AllHotkey2 => S::AllHotkey2,
            Row::SaveFiles => S::SaveFiles,
            Row::Folder => S::Folder,
            Row::Clipboard => S::CopyToClipboard,
            Row::History => S::KeepHistory,
            Row::TextEnabled => S::TextEnabled,
            Row::TextHotkey => S::TextHotkey,
            Row::OcrLanguage => S::OcrLanguage,
            Row::KeepTexts => S::KeepTexts,
            Row::Autostart => S::StartWithWindows,
            Row::Theme => S::Theme,
            Row::Language => S::Language,
            Row::Look => S::Look,
        }
    }

    pub fn hint(self) -> Option<S> {
        match self {
            Row::SnipEnabled => Some(S::SnipEnabledHint),
            Row::WinShiftS => Some(S::WinShiftSHint),
            Row::Hotkey => Some(S::EditorHotkeyHint),
            Row::Hotkey2 => Some(S::Editor2HotkeyHint),
            Row::AllHotkey => Some(S::AllHotkeyHint),
            Row::AllHotkey2 => Some(S::AllHotkey2Hint),
            Row::TextEnabled => Some(S::TextEnabledHint),
            Row::TextHotkey => Some(S::TextHotkeyHint),
            Row::OcrLanguage => Some(S::OcrLanguageHint),
            Row::KeepTexts => Some(S::KeepTextsHint),
            Row::Autostart => Some(S::StartWithWindowsHint),
            Row::Look => Some(S::LookHint),
            _ => None,
        }
    }

    /// A master switch turned off leaves the rest of its tab inactive.
    pub fn enabled(self, c: &Config) -> bool {
        match self {
            Row::WinShiftS | Row::Hotkey | Row::Hotkey2 | Row::AllHotkey | Row::AllHotkey2 | Row::SaveFiles | Row::Folder | Row::Clipboard | Row::History => c.snip_enabled,
            Row::TextHotkey | Row::OcrLanguage | Row::KeepTexts => c.text_enabled,
            _ => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Tab(usize),
    Toggle(Row),
    Segment(Row, usize),
    HotkeyField(KeyField),
    FolderChange,
    LanguageBox,
}

impl Part {
    fn row(self) -> Option<Row> {
        match self {
            Part::Tab(_) => None,
            Part::Toggle(r) | Part::Segment(r, _) => Some(r),
            Part::HotkeyField(KeyField::Editor) => Some(Row::Hotkey),
            Part::HotkeyField(KeyField::Editor2) => Some(Row::Hotkey2),
            Part::HotkeyField(KeyField::All) => Some(Row::AllHotkey),
            Part::HotkeyField(KeyField::All2) => Some(Row::AllHotkey2),
            Part::HotkeyField(KeyField::Text) => Some(Row::TextHotkey),
            Part::FolderChange => Some(Row::Folder),
            Part::LanguageBox => Some(Row::OcrLanguage),
        }
    }

    /// Does this part answer to the mouse with this config?
    pub fn active(self, c: &Config) -> bool {
        self.row().is_none_or(|r| r.enabled(c))
    }
}

/// What the window must do after a click or key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    None,
    /// The config changed: write it and tell the tray process.
    Config,
    StartRecording(KeyField),
    PickFolder,
    PickLanguage,
    ToggleAutostart,
    ShowTab(usize),
    /// A refused hotkey; show this.
    Message(S),
}

fn segments(row: Row, lang: Lang) -> Vec<String> {
    let s = |x: S| x.get(lang).to_string();
    match row {
        Row::Theme => vec![s(S::ThemeSystem), s(S::ThemeLight), s(S::ThemeDark)],
        Row::Language => vec![s(S::LangSystem), "English".into(), "Türkçe".into()],
        Row::Look => vec![s(S::LookGraphite), s(S::LookPaper)],
        _ => vec![],
    }
}

fn selected_segment(row: Row, c: &Config) -> usize {
    match row {
        Row::Theme => [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark].iter().position(|x| *x == c.theme).unwrap_or(0),
        Row::Language => [LangChoice::System, LangChoice::En, LangChoice::Tr].iter().position(|x| *x == c.language).unwrap_or(0),
        Row::Look => [Look::Graphite, Look::Paper].iter().position(|x| *x == c.look).unwrap_or(0),
        _ => 0,
    }
}

fn toggle_value(row: Row, c: &Config, autostart: bool) -> Option<bool> {
    match row {
        Row::SnipEnabled => Some(c.snip_enabled),
        Row::WinShiftS => Some(c.win_shift_s),
        Row::SaveFiles => Some(c.save_file),
        Row::Clipboard => Some(c.copy_to_clipboard),
        Row::History => Some(c.history),
        Row::TextEnabled => Some(c.text_enabled),
        Row::KeepTexts => Some(c.text_history),
        Row::Autostart => Some(autostart),
        _ => None,
    }
}

/// Apply a click on `part` to `c`.
pub fn apply(c: &mut Config, part: Part) -> Change {
    match part {
        Part::Tab(i) => Change::ShowTab(i),
        Part::Toggle(row) => {
            match row {
                Row::SnipEnabled => c.snip_enabled = !c.snip_enabled,
                Row::WinShiftS => c.win_shift_s = !c.win_shift_s,
                Row::SaveFiles => c.save_file = !c.save_file,
                Row::Clipboard => c.copy_to_clipboard = !c.copy_to_clipboard,
                Row::History => c.history = !c.history,
                Row::TextEnabled => c.text_enabled = !c.text_enabled,
                Row::KeepTexts => c.text_history = !c.text_history,
                Row::Autostart => return Change::ToggleAutostart,
                _ => return Change::None,
            }
            Change::Config
        }
        Part::Segment(row, i) => {
            if selected_segment(row, c) == i {
                return Change::None;
            }
            match (row, i) {
                (Row::Theme, 0) => c.theme = ThemeChoice::System,
                (Row::Theme, 1) => c.theme = ThemeChoice::Light,
                (Row::Theme, 2) => c.theme = ThemeChoice::Dark,
                (Row::Language, 0) => c.language = LangChoice::System,
                (Row::Language, 1) => c.language = LangChoice::En,
                (Row::Language, 2) => c.language = LangChoice::Tr,
                (Row::Look, 0) => c.look = Look::Graphite,
                (Row::Look, 1) => c.look = Look::Paper,
                _ => return Change::None,
            }
            Change::Config
        }
        Part::HotkeyField(f) => Change::StartRecording(f),
        Part::FolderChange => Change::PickFolder,
        Part::LanguageBox => Change::PickLanguage,
    }
}

pub fn field_value(c: &Config, f: KeyField) -> &str {
    match f {
        KeyField::Editor => &c.editor_hotkey,
        KeyField::Editor2 => &c.editor_hotkey_2,
        KeyField::All => &c.all_hotkey,
        KeyField::All2 => &c.all_hotkey_2,
        KeyField::Text => &c.text_hotkey,
    }
}

fn set_field(c: &mut Config, f: KeyField, v: String) {
    match f {
        KeyField::Editor => c.editor_hotkey = v,
        KeyField::Editor2 => c.editor_hotkey_2 = v,
        KeyField::All => c.all_hotkey = v,
        KeyField::All2 => c.all_hotkey_2 = v,
        KeyField::Text => c.text_hotkey = v,
    }
}

/// A key-down while recording. Esc cancels (None), Backspace or Delete without modifiers clears
/// the hotkey, anything else is checked by `hotkey.rs`. One key does one job: a key another
/// field holds is refused, so the recorder cannot make a key do two things.
pub fn record_key(c: &mut Config, field: KeyField, vk: u32, mods: HkMods) -> Option<Change> {
    let plain = mods == HkMods::default();
    match vk {
        0x1B if plain => Some(Change::None),
        0x08 | 0x2E if plain => {
            set_field(c, field, String::new());
            Some(Change::Config)
        }
        _ => {
            let hk = hotkey::from_keydown(vk, mods)?;
            if hk.check().is_err() {
                return Some(Change::Message(S::HotkeyRefused));
            }
            for other in [KeyField::Editor, KeyField::Editor2, KeyField::All, KeyField::All2, KeyField::Text] {
                if other != field && Hotkey::parse(field_value(c, other)).is_ok_and(|o| o == hk) {
                    return Some(Change::Message(S::HotkeyTaken));
                }
            }
            set_field(c, field, if field == KeyField::Text { hk.display() } else { hk.text() });
            Some(Change::Config)
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub w: i32,
    pub h: i32,
    pub scale: f32,
    pub tab: usize,
    pub parts: Vec<(Part, IBox)>,
}

fn s(scale: f32, d: f32) -> i32 {
    (d * scale).round() as i32
}

fn rows_top(scale: f32) -> i32 {
    s(scale, HEADER) + s(scale, TAB_H)
}

/// The top of each row of `tab`, and the bottom of the last.
fn row_tops(scale: f32, tab: usize) -> (Vec<(Row, i32)>, i32) {
    let mut y = rows_top(scale);
    let mut v = Vec::new();
    for (k, (_, rows)) in sections(tab).iter().enumerate() {
        if k > 0 {
            y += s(scale, crate::theme::SETTINGS_SECTION_GAP);
        }
        y += s(scale, crate::theme::SETTINGS_SECTION_LABEL);
        for r in *rows {
            v.push((*r, y));
            y += s(scale, ROW_H);
        }
    }
    (v, y)
}

pub fn layout(w: i32, h: i32, scale: f32, lang: Lang, g: &dyn Glyphs, t: &Theme, tab: usize) -> Layout {
    use crate::theme::*;
    let mut parts = Vec::new();
    let right = w - s(scale, MARGIN) - s(scale, CARD_PAD);
    let font = t.font * scale;
    // The tabs: a row of words under the title, the current one underlined.
    let mut x = s(scale, MARGIN);
    let tab_top = s(scale, HEADER);
    for i in 0..TABS {
        let tw = g.text(tab_title(i).get(lang), font, true).w as i32 + s(scale, 24.0);
        parts.push((Part::Tab(i), IBox::new(x, tab_top, x + tw, tab_top + s(scale, TAB_H) - s(scale, 8.0))));
        x += tw + s(scale, 4.0);
    }
    let half = s(scale, SETTINGS_CONTROL_HALF_HEIGHT);
    for (row, top) in row_tops(scale, tab).0 {
        let mid = top + s(scale, ROW_H) / 2;
        match row {
            Row::Hotkey | Row::Hotkey2 | Row::AllHotkey | Row::AllHotkey2 | Row::TextHotkey => {
                let field = match row {
                    Row::Hotkey => KeyField::Editor,
                    Row::Hotkey2 => KeyField::Editor2,
                    Row::AllHotkey => KeyField::All,
                    Row::AllHotkey2 => KeyField::All2,
                    _ => KeyField::Text,
                };
                let fw = s(scale, SETTINGS_HOTKEY_WIDTH);
                parts.push((Part::HotkeyField(field), IBox::new(right - fw, mid - half, right, mid + half)));
            }
            Row::Folder => {
                let tw = g.text(S::Change.get(lang), font, false).w as i32 + s(scale, SETTINGS_CONTROL_TEXT_PAD);
                parts.push((Part::FolderChange, IBox::new(right - tw, mid - half, right, mid + half)));
            }
            Row::OcrLanguage => {
                let fw = s(scale, SETTINGS_LANGUAGE_WIDTH);
                parts.push((Part::LanguageBox, IBox::new(right - fw, mid - half, right, mid + half)));
            }
            Row::Theme | Row::Language | Row::Look => {
                let labels = segments(row, lang);
                let widths: Vec<i32> = labels.iter().map(|l| g.text(l, font, false).w as i32 + s(scale, SETTINGS_CONTROL_TEXT_PAD)).collect();
                let mut x = right - widths.iter().sum::<i32>();
                for (k, sw) in widths.iter().enumerate() {
                    parts.push((Part::Segment(row, k), IBox::new(x, mid - half, x + sw, mid + half)));
                    x += sw;
                }
            }
            _ => {
                let th = s(scale, SETTINGS_TOGGLE_HALF_HEIGHT);
                parts.push((Part::Toggle(row), IBox::new(right - s(scale, SETTINGS_TOGGLE_WIDTH), mid - th, right, mid + th)));
            }
        }
    }
    Layout { w, h, scale, tab, parts }
}

impl Layout {
    pub fn hit(&self, x: i32, y: i32) -> Option<Part> {
        for (p, b) in &self.parts {
            if b.contains(x, y) {
                return Some(*p);
            }
        }
        // Toggles react to a click anywhere on their row.
        for (row, top) in row_tops(self.scale, self.tab).0 {
            if y >= top && y < top + s(self.scale, ROW_H) && self.part_box(Part::Toggle(row)).is_some() {
                return Some(Part::Toggle(row));
            }
        }
        None
    }

    pub fn part_box(&self, p: Part) -> Option<IBox> {
        self.parts.iter().find(|(q, _)| *q == p).map(|(_, b)| *b)
    }
}

pub struct ViewState<'a> {
    pub recording: Option<KeyField>,
    pub hover: Option<Part>,
    pub message: Option<(&'a str, bool)>,
    pub folder_text: &'a str,
    /// The Windows start entry exists.
    pub autostart: bool,
    /// What the language box shows.
    pub language_text: &'a str,
}

pub fn render(l: &Layout, t: &Theme, lang: Lang, c: &Config, st: &ViewState, g: &dyn Glyphs) -> Canvas {
    use crate::theme::*;
    let sc = l.scale;
    let mut cv = Canvas::new(l.w as u32, l.h as u32, t.window);
    let font = t.font * sc;
    let small = t.font_small * sc;
    let one = s(sc, BORDER_WIDTH).max(1);
    let card_x0 = s(sc, MARGIN);
    let card_x1 = l.w - card_x0;
    let x0 = card_x0 + s(sc, CARD_PAD);
    let rh = s(sc, ROW_H);
    let title = g.text(S::Settings.get(lang), TITLE_FONT * sc, true);
    cv.text_mask(&title, card_x0, (s(sc, HEADER) - title.h as i32) / 2, t.text);
    // A hairline under the tabs, broken by the current tab's thick line.
    let tab_line = s(sc, HEADER) + s(sc, TAB_H) - s(sc, 8.0);
    cv.fill_box(IBox::new(card_x0, tab_line - one, card_x1, tab_line), t.border);

    let (tops, end) = row_tops(sc, l.tab);
    let top_of = |row: Row| tops.iter().find(|(r, _)| *r == row).map(|(_, y)| *y).unwrap_or(0);
    for (name, rows) in sections(l.tab).iter() {
        let top = top_of(rows[0]);
        let bottom = top + rows.len() as i32 * rh;
        let label = g.text(name.get(lang), font, true);
        let label_h = s(sc, SETTINGS_SECTION_LABEL);
        cv.text_mask(&label, card_x0, top - label_h + (label_h - label.h as i32) / 2, t.text);
        fill_ring(&mut cv, IBox::new(card_x0, top, card_x1, bottom), sc, t.radius, t.card, t.border);
        for (i, row) in rows.iter().enumerate() {
            let rt = top_of(*row);
            if i > 0 {
                cv.fill_box(IBox::new(x0, rt, card_x1 - s(sc, CARD_PAD), rt + one), t.border);
            }
            // A toggle row answers anywhere on it, so all of it lights up.
            if st.hover == Some(Part::Toggle(*row)) && row.enabled(c) {
                let inset = s(sc, SETTINGS_ROW_HOVER_INSET);
                let b = IBox::new(card_x0 + inset, rt + inset, card_x1 - inset, rt + rh - inset);
                let mut m = Mask::new(b);
                m.rounded_box(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32, t.control_radius() * sc);
                cv.blend_mask(&m, t.hover);
            }
            let color = if row.enabled(c) { t.text } else { t.muted };
            let title_px = g.text(row.title().get(lang), font, false);
            let hint: Option<String> = if *row == Row::Folder { Some(st.folder_text.to_string()) } else { row.hint().map(|h| h.get(lang).to_string()) };
            match hint {
                Some(h) => {
                    let hint_h = g.line_height(small) as i32;
                    let y = rt + (rh - title_px.h as i32 - hint_h) / 2;
                    cv.text_mask(&title_px, x0, y, color);
                    let mut text = h;
                    if *row == Row::Folder {
                        // The folder path, shortened from the left so its end stays visible.
                        let change_x = l.part_box(Part::FolderChange).map(|b| b.x0).unwrap_or(l.w);
                        let avail = change_x - x0 - s(sc, SETTINGS_FOLDER_TEXT_GAP);
                        while !text.is_empty() && g.text(&text, small, false).w as i32 > avail {
                            let cut: String = text.chars().skip(4).collect();
                            text = format!("\u{2026}{cut}");
                            if text.chars().count() <= 2 {
                                break;
                            }
                        }
                    }
                    cv.text(g, &text, small, false, x0, y + title_px.h as i32, t.muted);
                }
                None => cv.text_mask(&title_px, x0, rt + (rh - title_px.h as i32) / 2, color),
            }
        }
    }
    for (p, b) in &l.parts {
        let hover = st.hover == Some(*p) && p.active(c);
        match p {
            Part::Tab(i) => {
                let sel = *i == l.tab;
                if hover && !sel {
                    let mut m = Mask::new(*b);
                    m.rounded_box(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32 - one as f32, t.control_radius() * sc);
                    cv.blend_mask(&m, t.hover);
                }
                let tm = g.text(tab_title(*i).get(lang), font, sel);
                cv.text_mask(&tm, b.x0 + (b.w() - tm.w as i32) / 2, b.y0 + (b.h() - tm.h as i32) / 2, if sel { t.text } else { t.muted });
                if sel {
                    let mark = if *i == TAB_TEXT { MARKER } else if *i == TAB_GENERAL { t.text } else { t.accent };
                    cv.fill_box(IBox::new(b.x0 + s(sc, 8.0), tab_line - s(sc, 2.0), b.x1 - s(sc, 8.0), tab_line), mark);
                }
            }
            Part::Toggle(row) => {
                // A row of a function turned off does not claim to be on: its value comes back with it.
                let usable = row.enabled(c);
                let on = usable && toggle_value(*row, c, st.autostart).unwrap_or(false);
                let yellow = l.tab == TAB_TEXT;
                let r = b.h() as f32 / 2.0;
                let mut m = Mask::new(*b);
                m.rounded_box(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32, r);
                if on {
                    cv.blend_mask(&m, if yellow { MARKER } else { t.accent });
                    if yellow && t.mode == Mode::Light {
                        let mut o = Mask::new(*b);
                        o.rounded_box_outline(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32, r, (BORDER_WIDTH * sc).max(1.0));
                        cv.blend_mask(&o, MARKER_LINE);
                    }
                } else {
                    let mut o = Mask::new(*b);
                    o.rounded_box_outline(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32, r, (BORDER_WIDTH * sc).max(1.0));
                    cv.blend_mask(&o, t.muted);
                }
                let knob_r = (if on { SETTINGS_TOGGLE_KNOB_ON } else { SETTINGS_TOGGLE_KNOB_OFF }) * sc;
                let cx = if on { b.x1 as f32 - r } else { b.x0 as f32 + r };
                let mut k = Mask::new(*b);
                k.disc(Pt::new(cx, b.y0 as f32 + r), knob_r);
                cv.blend_mask(&k, if on && yellow { MARKER_INK } else if on { t.on_accent } else { t.muted });
                if usable {
                    let label = g.text(if on { S::On } else { S::Off }.get(lang), small, false);
                    cv.text_mask(&label, b.x0 - label.w as i32 - s(sc, SETTINGS_TOGGLE_LABEL_GAP), b.y0 + (b.h() - label.h as i32) / 2, t.muted);
                }
            }
            Part::Segment(row, k) => {
                let sel = selected_segment(*row, c) == *k;
                if *k == 0 {
                    // One group: a recessed well around all segments of the row.
                    let last = l.parts.iter().filter(|(p, _)| matches!(p, Part::Segment(r, _) if r == row)).map(|(_, b)| *b).next_back().unwrap_or(*b);
                    fill_ring(&mut cv, IBox::new(b.x0, b.y0, last.x1, last.y1), sc, t.control_radius(), t.window, t.border);
                }
                let inset = (BORDER_WIDTH * sc).max(1.0).round() as i32 + s(sc, 1.0);
                let ib = IBox::new(b.x0 + inset, b.y0 + inset, b.x1 - inset, b.y1 - inset);
                let inner_r = (t.control_radius() - SETTINGS_FIELD_RADIUS_INSET).max(0.0);
                if sel {
                    fill_ring(&mut cv, ib, sc, inner_r, t.raised, t.border);
                } else if hover {
                    let mut m = Mask::new(ib);
                    m.rounded_box(ib.x0 as f32, ib.y0 as f32, ib.x1 as f32, ib.y1 as f32, inner_r * sc);
                    cv.blend_mask(&m, t.hover);
                }
                let label = &segments(*row, lang)[*k];
                let tm = g.text(label, font, false);
                cv.text_mask(&tm, b.x0 + (b.w() - tm.w as i32) / 2, b.y0 + (b.h() - tm.h as i32) / 2, if sel { t.on_raised } else { t.text });
            }
            Part::HotkeyField(field) => {
                if st.recording == Some(*field) {
                    let ring = if *field != KeyField::Text { t.accent } else if t.mode == Mode::Light { MARKER_LINE } else { MARKER };
                    fill_ring(&mut cv, *b, sc, t.control_radius(), t.window, ring);
                    let tm = g.text(S::PressKeys.get(lang), small, false);
                    cv.text_mask(&tm, b.x0 + (b.w() - tm.w as i32) / 2, b.y0 + (b.h() - tm.h as i32) / 2, t.muted);
                    continue;
                }
                if hover {
                    let mut m = Mask::new(*b);
                    m.rounded_box(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32, t.control_radius() * sc);
                    cv.blend_mask(&m, t.hover);
                }
                // The shortcut as key caps, with a pencil: it reads as a value that can be changed.
                let isz = SETTINGS_EDIT_ICON * sc;
                let pad = s(sc, SETTINGS_KEY_GAP);
                let icon_x = b.x1 as f32 - pad as f32 - isz;
                let mid = b.y0 + b.h() / 2;
                icons::draw(&mut cv, Icon::Pen, icon_x, mid as f32 - isz / 2.0, isz, t.muted);
                let value = field_value(c, *field);
                let text = if value.trim().is_empty() {
                    S::HotkeyNone.get(lang).to_string()
                } else {
                    Hotkey::parse(value).map(|h| h.display()).unwrap_or_else(|_| value.to_string())
                };
                let caps = key_caps(&text);
                let mut x = icon_x as i32 - s(sc, SETTINGS_EDIT_ICON_GAP);
                let half = s(sc, SETTINGS_KEY_HALF_HEIGHT);
                for cap in caps.iter().rev() {
                    let tm = g.text(cap, small, true);
                    let cw = tm.w as i32 + 2 * s(sc, SETTINGS_KEY_PAD);
                    let key = IBox::new(x - cw, mid - half, x, mid + half);
                    fill_ring(&mut cv, key, sc, t.control_radius(), t.raised, t.border);
                    cv.text_mask(&tm, key.x0 + (cw - tm.w as i32) / 2, mid - tm.h as i32 / 2, t.on_raised);
                    x -= cw + pad;
                }
            }
            Part::FolderChange => {
                fill_ring(&mut cv, *b, sc, t.control_radius(), if hover { t.hover } else { t.raised }, t.border);
                let tm = g.text(S::Change.get(lang), font, false);
                cv.text_mask(&tm, b.x0 + (b.w() - tm.w as i32) / 2, b.y0 + (b.h() - tm.h as i32) / 2, if hover { t.text } else { t.on_raised });
            }
            Part::LanguageBox => {
                fill_ring(&mut cv, *b, sc, t.control_radius(), if hover { t.hover } else { t.raised }, t.border);
                let pad = s(sc, 10.0);
                let chevron_w = s(sc, 16.0);
                let avail = b.w() - 2 * pad - chevron_w;
                let lines = crate::cardview::wrap_lines(st.language_text, 1, avail, &|x| g.text(x, font, false).w as i32);
                if let Some(line) = lines.first() {
                    let tm = g.text(line, font, false);
                    cv.text_mask(&tm, b.x0 + pad, b.y0 + (b.h() - tm.h as i32) / 2, t.on_raised);
                }
                let (cx, cy) = ((b.x1 - pad - chevron_w / 2) as f32, (b.y0 + b.y1) as f32 / 2.0);
                let k = 4.0 * sc;
                let mut m = Mask::new(*b);
                m.polyline(&[Pt::new(cx - k, cy - k * 0.5), Pt::new(cx, cy + k * 0.5), Pt::new(cx + k, cy - k * 0.5)], 1.5 * sc);
                cv.blend_mask(&m, t.muted);
            }
        }
    }
    // Rows of a function turned off: still there, plainly not in use.
    for (row, top) in &tops {
        if !row.enabled(c) {
            let b = IBox::new(card_x0 + s(sc, 2.0), *top + one, card_x1 - s(sc, 2.0), *top + rh - one);
            cv.fill_box(b, t.card.with_alpha(115));
        }
    }
    let fy = end;
    let version = format!("{} {}. {}", S::Version.get(lang), env!("CARGO_PKG_VERSION"), S::NoNetwork.get(lang));
    let (msg, warn) = match st.message {
        Some(m) => m,
        None if l.tab == TAB_GENERAL => (version.as_str(), false),
        None if l.tab == TAB_TEXT => (S::TextStaysHere.get(lang), false),
        None => ("", false),
    };
    if !msg.is_empty() {
        cv.text(g, msg, small, false, card_x0, fy + (s(sc, FOOTER) - g.line_height(small) as i32) / 2, if warn { t.danger } else { t.muted });
    }
    cv
}

/// "Ctrl+Shift+S" as separate keys; a plus sign that is itself the key stays one.
pub fn key_caps(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text.split('+').filter(|p| !p.is_empty()).map(str::to_string).collect();
    if text.ends_with('+') {
        v.push("+".into());
    }
    v
}

fn fill_ring(cv: &mut Canvas, b: IBox, sc: f32, radius: f32, fill: Rgba, ring: Rgba) {
    let mut m = Mask::new(b);
    m.rounded_box(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32, radius * sc);
    cv.blend_mask(&m, fill);
    let mut o = Mask::new(b);
    o.rounded_box_outline(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32, radius * sc, (crate::theme::BORDER_WIDTH * sc).max(1.0));
    cv.blend_mask(&o, ring);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::BoxFont;
    use crate::theme::Mode;

    fn lay(tab: usize) -> Layout {
        let t = Theme::new(Look::Graphite, Mode::Light);
        layout(WIDTH as i32, height_dip() as i32, 1.0, Lang::En, &BoxFont, &t, tab)
    }

    fn view<'a>() -> ViewState<'a> {
        ViewState { recording: None, hover: None, message: None, folder_text: "C:\\shots", autostart: false, language_text: "Otomatik" }
    }

    #[test]
    fn every_row_of_every_tab_has_a_control_inside_the_window() {
        for tab in 0..TABS {
            let l = lay(tab);
            for (_, rows) in sections(tab) {
                for row in rows {
                    let has = l.parts.iter().any(|(p, _)| p.row() == Some(*row));
                    assert!(has, "{row:?}");
                }
            }
            for (p, b) in &l.parts {
                assert!(b.x0 >= 0 && b.x1 <= l.w && b.y1 <= l.h, "{p:?} {b:?}");
                assert_eq!(l.hit(b.x0 + 1, b.y0 + 1), Some(*p));
            }
        }
    }

    #[test]
    fn the_window_keeps_its_size_and_nothing_is_cut() {
        for tab in 0..TABS {
            let (_, end) = row_tops(1.0, tab);
            assert!(end as f32 + FOOTER <= height_dip(), "tab {tab}");
        }
    }

    #[test]
    fn each_function_can_be_turned_off_alone() {
        let mut c = Config::default();
        assert_eq!(apply(&mut c, Part::Toggle(Row::TextEnabled)), Change::Config);
        assert!(!c.text_enabled && c.snip_enabled);
        assert!(!Part::HotkeyField(KeyField::Text).active(&c), "its rows go quiet");
        assert!(Part::HotkeyField(KeyField::Editor).active(&c), "the other function does not notice");
        assert!(Part::Toggle(Row::TextEnabled).active(&c), "and it can be turned back on");
        apply(&mut c, Part::Toggle(Row::TextEnabled));
        apply(&mut c, Part::Toggle(Row::SnipEnabled));
        assert!(!c.snip_enabled && c.text_enabled);
        assert!(!Part::FolderChange.active(&c));
        assert!(Part::LanguageBox.active(&c));
        assert_eq!(Config::parse(&c.to_file_text()).unwrap(), c, "written and read back");
    }

    #[test]
    fn clicks_change_the_config() {
        let mut c = Config::default();
        assert_eq!(apply(&mut c, Part::Toggle(Row::History)), Change::Config);
        assert!(!c.history);
        assert_eq!(apply(&mut c, Part::Segment(Row::Theme, 2)), Change::Config);
        assert_eq!(c.theme, ThemeChoice::Dark);
        assert_eq!(apply(&mut c, Part::Segment(Row::Theme, 2)), Change::None, "already selected");
        apply(&mut c, Part::Segment(Row::Language, 2));
        assert_eq!(c.language, LangChoice::Tr);
        apply(&mut c, Part::Segment(Row::Look, 1));
        assert_eq!(c.look, Look::Paper);
        assert_eq!(apply(&mut c, Part::HotkeyField(KeyField::Text)), Change::StartRecording(KeyField::Text));
        assert_eq!(apply(&mut c, Part::FolderChange), Change::PickFolder);
        assert_eq!(apply(&mut c, Part::LanguageBox), Change::PickLanguage);
        assert_eq!(apply(&mut c, Part::Toggle(Row::Autostart)), Change::ToggleAutostart, "the Run value decides, not the config");
        assert_eq!(apply(&mut c, Part::Tab(1)), Change::ShowTab(1));
        // What the window writes loads back the same.
        assert_eq!(Config::parse(&c.to_file_text()).unwrap(), c);
    }

    #[test]
    fn a_click_anywhere_on_a_toggle_row_toggles() {
        let l = lay(TAB_SNIP);
        let tops = row_tops(1.0, TAB_SNIP).0;
        let y = tops.iter().find(|(r, _)| *r == Row::SaveFiles).unwrap().1 + 10;
        assert_eq!(l.hit(30, y), Some(Part::Toggle(Row::SaveFiles)));
        assert_eq!(l.hit(30, 10), None, "header");
    }

    #[test]
    fn hotkeys_split_into_key_caps() {
        assert_eq!(key_caps("PrtScn"), vec!["PrtScn"]);
        assert_eq!(key_caps("Ctrl+Shift+S"), vec!["Ctrl", "Shift", "S"]);
        assert_eq!(key_caps("Ctrl++"), vec!["Ctrl", "+"]);
    }

    #[test]
    fn recorder_sets_clears_and_refuses() {
        let mut c = Config::default();
        let ctrl_shift = HkMods { ctrl: true, shift: true, ..Default::default() };
        let f = KeyField::Editor;
        assert_eq!(record_key(&mut c, f, 0x10, HkMods { shift: true, ..Default::default() }), None, "modifier only: keep listening");
        assert_eq!(record_key(&mut c, f, 0x41, ctrl_shift), Some(Change::Config));
        assert_eq!(c.editor_hotkey, "Ctrl+Shift+A");
        let win_shift = HkMods { win: true, shift: true, ..Default::default() };
        assert_eq!(record_key(&mut c, f, 0x53, win_shift), Some(Change::Message(S::HotkeyRefused)));
        assert_eq!(record_key(&mut c, f, 0x41, HkMods::default()), Some(Change::Message(S::HotkeyRefused)), "plain A");
        assert_eq!(c.editor_hotkey, "Ctrl+Shift+A", "refusals keep the old one");
        assert_eq!(record_key(&mut c, f, 0x1B, HkMods::default()), Some(Change::None));
        assert_eq!(record_key(&mut c, f, 0x08, HkMods::default()), Some(Change::Config));
        assert_eq!(c.editor_hotkey, "");
        assert_eq!(record_key(&mut c, f, 0x2C, HkMods::default()), Some(Change::Message(S::HotkeyTaken)), "PrtScn is all screens");
        assert_eq!(record_key(&mut c, f, 0x78, HkMods::default()), Some(Change::Config));
        assert_eq!(c.editor_hotkey, "F9");
    }

    #[test]
    fn one_key_does_one_job() {
        let mut c = Config::default();
        let win_shift = HkMods { win: true, shift: true, ..Default::default() };
        assert_eq!(record_key(&mut c, KeyField::Editor, 0x54, win_shift), Some(Change::Message(S::HotkeyTaken)), "Win+Shift+T is the text key");
        assert_eq!(record_key(&mut c, KeyField::Text, 0x2C, HkMods::default()), Some(Change::Message(S::HotkeyTaken)), "PrtScn is the all-screens key");
        assert_eq!(record_key(&mut c, KeyField::Text, 0x2C, win_shift), Some(Change::Message(S::HotkeyTaken)), "Win+Shift+PrtScn is the second all-screens key");
        assert_eq!(record_key(&mut c, KeyField::Text, 0x13, HkMods::default()), Some(Change::Message(S::HotkeyTaken)), "Pause is the editor key");
        assert_eq!(record_key(&mut c, KeyField::Text, 0x53, win_shift), Some(Change::Message(S::HotkeyRefused)));
        let ctrl_alt = HkMods { ctrl: true, alt: true, ..Default::default() };
        assert_eq!(record_key(&mut c, KeyField::Text, 0x54, ctrl_alt), Some(Change::Config));
        assert_eq!(c.text_hotkey, "Ctrl+Alt+T");
        assert_eq!(record_key(&mut c, KeyField::Text, 0x54, win_shift), Some(Change::Config));
        assert_eq!(c.text_hotkey, crate::config::TEXT_HOTKEY_DEFAULT, "the default reads as the default, so it stays unwritten");
    }

    #[test]
    fn renders_every_tab_in_both_languages_and_scales() {
        let mut c = Config::default();
        for (lang, sc) in [(Lang::En, 1.0), (Lang::Tr, 1.5)] {
            let t = Theme::new(Look::Paper, Mode::Dark);
            let w = (WIDTH * sc) as i32;
            let h = (height_dip() * sc) as i32;
            for tab in 0..TABS {
                let l = layout(w, h, sc, lang, &BoxFont, &t, tab);
                let cv = render(&l, &t, lang, &c, &view(), &BoxFont);
                assert_eq!((cv.w as i32, cv.h as i32), (w, h));
            }
            c.text_enabled = false;
            c.snip_enabled = false;
        }
    }
}
