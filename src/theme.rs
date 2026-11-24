//! Colours and sizes for everything snip draws: two looks (Graphite, Paper), each light and dark.
//!
//! The owner picks a look from the mockups in docs/design/; both are built in, so `look = "paper"`
//! in config.toml shows the other one without a rebuild. Sizes are in DIPs (1 DIP = 1 pixel at
//! 100 % scaling) and are multiplied by the monitor's scale (`dpi.rs`).
//! Rules (docs/design/DESIGN.md): flat fills and 1-DIP borders only; no gradients, glow, blur
//! behind panels, shadows or glass.

use serde::Deserialize;

use crate::raster::Rgba;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Look {
    #[default]
    Graphite,
    Paper,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Light,
    Dark,
}

impl ThemeChoice {
    /// `system_dark`: what Windows reports (AppsUseLightTheme = 0).
    pub fn resolve(self, system_dark: bool) -> Mode {
        match self {
            ThemeChoice::Light => Mode::Light,
            ThemeChoice::Dark => Mode::Dark,
            ThemeChoice::System if system_dark => Mode::Dark,
            ThemeChoice::System => Mode::Light,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ThemeChoice::System => "system",
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
        }
    }
}

impl Look {
    pub fn name(self) -> &'static str {
        match self {
            Look::Graphite => "graphite",
            Look::Paper => "paper",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub look: Look,
    pub mode: Mode,
    /// Panels: toolbar, menus, history and settings windows.
    pub bg: Rgba,
    /// A raised area inside a panel (cards, input fields, the selected segment).
    pub surface: Rgba,
    /// The ground of the history and settings windows, and of the title bar above it.
    pub window: Rgba,
    /// A settings group on that ground.
    pub card: Rgba,
    /// The chosen segment and a key cap: lifted off the card, no colour.
    pub raised: Rgba,
    pub on_raised: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub accent: Rgba,
    /// Text and icons on `accent`.
    pub on_accent: Rgba,
    pub hover: Rgba,
    /// The active tool's background.
    pub selected: Rgba,
    /// The selection frame on the frozen screen: `sel_inner` 1 DIP, `sel_outer` 1 DIP around it, so
    /// the frame shows on light and dark screenshots alike.
    pub sel_inner: Rgba,
    pub sel_outer: Rgba,
    /// Darkening of the frozen screen outside the selection (alpha only).
    pub dim_alpha: u8,
    pub danger: Rgba,
    // Metrics in DIPs.
    pub radius: f32,
    pub button: f32,
    pub icon: f32,
    pub pad: f32,
    pub gap: f32,
    pub font: f32,
    pub font_small: f32,
}

const fn c(hex: u32) -> Rgba {
    Rgba::rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Theme {
    pub fn new(look: Look, mode: Mode) -> Theme {
        match (look, mode) {
            (Look::Graphite, Mode::Light) => Theme {
                look,
                mode,
                bg: c(0xFFFFFF),
                surface: c(0xF4F4F5),
                window: c(0xF4F4F5),
                card: c(0xFFFFFF),
                raised: c(0xFFFFFF),
                on_raised: c(0x18181B),
                border: c(0xDCDCE0),
                text: c(0x18181B),
                muted: c(0x63636B),
                accent: c(0x1F57D6),
                on_accent: c(0xFFFFFF),
                hover: c(0xF0F0F2),
                selected: c(0xE3EBFD),
                sel_inner: c(0x1F57D6),
                sel_outer: c(0xFFFFFF),
                dim_alpha: 115,
                danger: c(0xC62828),
                radius: 8.0,
                button: 32.0,
                icon: 20.0,
                pad: 4.0,
                gap: 2.0,
                font: 13.0,
                font_small: 12.0,
            },
            (Look::Graphite, Mode::Dark) => Theme {
                look,
                mode,
                bg: c(0x1C1C1F),
                surface: c(0x27272B),
                window: c(0x1C1C1F),
                card: c(0x27272B),
                raised: c(0x3A3A40),
                on_raised: c(0xF4F4F5),
                border: c(0x3A3A40),
                text: c(0xF4F4F5),
                muted: c(0xA1A1AA),
                accent: c(0x7AA2F7),
                on_accent: c(0x0B0B0C),
                hover: c(0x2C2C31),
                selected: c(0x2B3750),
                sel_inner: c(0x7AA2F7),
                sel_outer: c(0x000000),
                dim_alpha: 140,
                danger: c(0xF07070),
                radius: 8.0,
                button: 32.0,
                icon: 20.0,
                pad: 4.0,
                gap: 2.0,
                font: 13.0,
                font_small: 12.0,
            },
            (Look::Paper, Mode::Light) => Theme {
                look,
                mode,
                bg: c(0xFAFAF7),
                surface: c(0xEFEEE9),
                window: c(0xEFEEE9),
                card: c(0xFAFAF7),
                raised: c(0x1F1E1B),
                on_raised: c(0xFAFAF7),
                border: c(0xD6D4CB),
                text: c(0x1F1E1B),
                muted: c(0x5F5D55),
                accent: c(0x1F1E1B),
                on_accent: c(0xFAFAF7),
                hover: c(0xF0EFEA),
                selected: c(0x1F1E1B),
                sel_inner: c(0xFFFFFF),
                sel_outer: c(0x1F1E1B),
                dim_alpha: 100,
                danger: c(0xB3261E),
                radius: 2.0,
                button: 36.0,
                icon: 20.0,
                pad: 6.0,
                gap: 0.0,
                font: 13.0,
                font_small: 12.0,
            },
            (Look::Paper, Mode::Dark) => Theme {
                look,
                mode,
                bg: c(0x1E1D1B),
                surface: c(0x2A2926),
                window: c(0x1E1D1B),
                card: c(0x2A2926),
                raised: c(0xEDECE8),
                on_raised: c(0x1E1D1B),
                border: c(0x403E39),
                text: c(0xEDECE8),
                muted: c(0xA8A69E),
                accent: c(0xEDECE8),
                on_accent: c(0x1E1D1B),
                hover: c(0x292825),
                selected: c(0xEDECE8),
                sel_inner: c(0xFFFFFF),
                sel_outer: c(0x000000),
                dim_alpha: 130,
                danger: c(0xF2867E),
                radius: 2.0,
                button: 36.0,
                icon: 20.0,
                pad: 6.0,
                gap: 0.0,
                font: 13.0,
                font_small: 12.0,
            },
        }
    }

    /// The colour of an icon on the active tool's background.
    pub fn on_selected(&self) -> Rgba {
        if self.look == Look::Paper { self.on_accent } else { self.accent }
    }

    /// Buttons, fields and thumbnails; panels use `radius`.
    pub fn control_radius(&self) -> f32 {
        (self.radius - BUTTON_RADIUS_INSET).max(0.0)
    }

    /// The ring of a resize handle: the accent where it shows against the white handle.
    pub fn handle_ring(&self) -> Rgba {
        if self.accent.contrast(HANDLE_FILL) >= 2.0 { self.accent } else { DIM_COLOR }
    }

    /// Opacity of the soft shadow under a floating panel.
    pub fn shadow_alpha(&self) -> u8 {
        if self.mode == Mode::Dark { SHADOW_ALPHA_DARK } else { SHADOW_ALPHA_LIGHT }
    }
}

/// Shared visual metrics, in DIPs unless otherwise noted. They apply to both looks.
pub const FONT_FAMILY: &str = "Segoe UI";
/// Below 1 thickens antialiased edges a little, so small text stays readable on both grounds.
pub const TEXT_GAMMA: f32 = 0.85;
pub const FONT_WEIGHT_NORMAL: i32 = 400;
pub const FONT_WEIGHT_SEMIBOLD: i32 = 600;
pub const ICON_GRID: f32 = 20.0;
pub const ICON_STROKE: f32 = 1.5;
pub const BORDER_WIDTH: f32 = 1.0;
pub const BUTTON_RADIUS_INSET: f32 = 4.0;
pub const SEPARATOR_WIDTH: f32 = 9.0;
pub const SEPARATOR_INSET: f32 = 6.0;
pub const DISABLED_ALPHA: u8 = 110;
pub const COLOR_BUTTON_RADIUS: f32 = 7.0;
pub const SIZE_DOT_RADII: [f32; 6] = [2.0, 3.5, 5.5, 7.0, 9.0, 11.0];
pub const PALETTE_CELL: f32 = 28.0;
pub const PALETTE_GAP: f32 = 2.0;
pub const PALETTE_SWATCH_RADIUS: f32 = 9.0;
pub const PALETTE_SELECTION_RADIUS: f32 = 12.0;
pub const PALETTE_SELECTION_WIDTH: f32 = 1.5;
pub const PALETTE_CUSTOM_DOT_SPACING: f32 = 5.0;
pub const PALETTE_CUSTOM_DOT_RADIUS: f32 = 1.4;
pub const LABEL_PAD_X: f32 = 8.0;
pub const LABEL_PAD_Y: f32 = 3.0;
pub const HANDLE_SIZE: f32 = 7.0;
/// Handles are drawn as discs; `HANDLE_SIZE` is the box that answers to the mouse.
pub const HANDLE_RADIUS: f32 = 4.5;
pub const HANDLE_FILL: Rgba = c(0xFFFFFF);
/// The outer frame line is half see-through: it separates, it does not draw a heavy box.
pub const SEL_OUTER_ALPHA: f32 = 0.5;
/// Soft shadow under the bars and menus: how far it spreads and how far down it sits.
pub const SHADOW_BLUR: f32 = 12.0;
pub const SHADOW_Y: f32 = 4.0;
pub const SHADOW_ALPHA_LIGHT: u8 = 60;
pub const SHADOW_ALPHA_DARK: u8 = 120;
pub const HANDLE_HIT_SLOP: f32 = 4.0;
pub const DIM_COLOR: Rgba = c(0x000000);
pub const BADGE_LIGHT_TEXT: Rgba = c(0xFFFFFF);
pub const BADGE_DARK_TEXT: Rgba = c(0x111111);
pub const ANNOTATION_LINE_SCALE: f32 = 1.3;
pub const ANNOTATION_TEXT_Y_OFFSET: f32 = 0.6;
pub const ANNOTATION_CARET_HEIGHT: f32 = 1.2;
pub const ANNOTATION_CARET_GAP: i32 = 1;
pub const ANNOTATION_TEXT_WIDTH_ESTIMATE: f32 = 0.8;
pub const ANNOTATION_TEXT_BOUNDS_PAD: f32 = 2.0;
pub const STEP_FONT_SCALE: f32 = 1.15;
pub const ARROW_HEAD_WIDTH_SCALE: f32 = 3.0;
pub const ARROW_HEAD_EXTRA: f32 = 8.0;
pub const ARROW_HEAD_MIN: f32 = 10.0;
pub const ARROW_HEAD_HALF_SCALE: f32 = 0.55;
pub const ARROW_SHAFT_END_SCALE: f32 = 0.6;
pub const PLAIN_DIM_DEFAULT: f32 = 0.45;
/// The spotlight: how dark the rest of the picture goes (alpha of black) and the lit box's corners.
pub const SPOTLIGHT_DIM: u8 = 150;
pub const SPOTLIGHT_RADIUS: f32 = 6.0;
/// The magnifier: pixels across (odd, so one sits in the middle), their size, the readout below.
pub const LOUPE_PIXELS: u32 = 15;
pub const LOUPE_ZOOM: f32 = 10.0;
pub const LOUPE_READOUT: f32 = 24.0;
pub const LOUPE_OFFSET: f32 = 24.0;
/// The picture frame (the editor's Frame button): padding as a share of the shorter side, within
/// these limits, the picture's corner radius, and its shadow.
pub const FRAME_PAD_SHARE: f32 = 0.07;
pub const FRAME_PAD_MIN: f32 = 28.0;
pub const FRAME_PAD_MAX: f32 = 96.0;
pub const FRAME_RADIUS: f32 = 10.0;
pub const FRAME_SHADOW_BLUR: f32 = 22.0;
pub const FRAME_SHADOW_Y: f32 = 8.0;
pub const FRAME_SHADOW_ALPHA: u8 = 90;
/// The frame's ground: a quiet two-stop vertical wash, slate to a cooler slate. No hue jumps.
pub const FRAME_TOP: Rgba = c(0x3B4552);
pub const FRAME_BOTTOM: Rgba = c(0x262C35);
pub const WINDOW_WORK_MARGIN: f32 = 24.0;
pub const HISTORY_THUMB_PIXELS: (u32, u32) = (200, 125);
pub const PIN_OPACITY: [u8; 4] = [255, 217, 166, 115];
pub const PIN_OPACITY_PERCENT: [u32; 4] = [100, 85, 65, 45];

pub const MODE_BAR_TOP: f32 = 16.0;
/// The widest the plain snip's hint grows before its keys wrap into rows.
pub const HINT_MAX_W: f32 = 760.0;
pub const SELECTION_LABEL_GAP: f32 = 6.0;
pub const EDITOR_BAR_GAP: f32 = 8.0;
pub const PALETTE_BAR_GAP: f32 = 6.0;
pub const CROSSHAIR_SIZE: f32 = 16.0;
pub const COUNTDOWN_TOP: f32 = 48.0;
pub const TRAY_ICON_MIN_SIZE: i32 = 16;
pub const TRAY_ICON_MIN_STROKE: i32 = 2;
pub const TRAY_ARMED_RGB: (u8, u8, u8) = (64, 190, 255);
pub const TRAY_UNARMED_RGB: (u8, u8, u8) = (255, 140, 0);
/// Annotation sizes in DIPs, ordered small to large.
pub const ANNOTATION_STROKE: [f32; 6] = [2.0, 4.0, 7.0, 10.0, 14.0, 20.0];
pub const ANNOTATION_MARKER: [f32; 6] = [12.0, 18.0, 28.0, 36.0, 48.0, 64.0];
pub const ANNOTATION_TEXT: [f32; 6] = [16.0, 22.0, 32.0, 40.0, 56.0, 72.0];
pub const ANNOTATION_STEP_RADIUS: [f32; 6] = [11.0, 14.0, 19.0, 24.0, 30.0, 38.0];
pub const ANNOTATION_PIXEL_BLOCK: [f32; 6] = [8.0, 14.0, 22.0, 30.0, 40.0, 56.0];
pub const ANNOTATION_BLUR_RADIUS: [f32; 6] = [5.0, 10.0, 18.0, 24.0, 32.0, 44.0];
pub const SIZE_MENU_WIDTH: f32 = 176.0;
pub const SIZE_MENU_ROW: f32 = 32.0;
pub const SIZE_MENU_HEADER: f32 = 28.0;
pub const SIZE_MENU_TEXT_PAD: f32 = 12.0;
pub const SIZE_MENU_PREVIEW_W: f32 = 104.0;
pub const SIZE_MENU_PREVIEW_MAX: f32 = 24.0;
pub const TEXT_INPUT_PAD: f32 = 4.0;
pub const TEXT_INPUT_MIN_WIDTH: f32 = 120.0;
pub const TEXT_INPUT_CARET_SPACE: f32 = 2.0;
pub const TEXT_GLYPH_OVERHANG_DIVISOR: f32 = 8.0;

pub const HISTORY_THUMB_W: f32 = 160.0;
pub const HISTORY_THUMB_H: f32 = 100.0;
pub const HISTORY_CAPTION: f32 = 22.0;
pub const HISTORY_GAP: f32 = 12.0;
pub const HISTORY_MARGIN: f32 = 16.0;
pub const HISTORY_HEADER: f32 = 60.0;
pub const HISTORY_ACTION: f32 = 26.0;
pub const HISTORY_ACTION_PAD: f32 = 4.0;
pub const HISTORY_ACTION_GAP: f32 = 2.0;
pub const HISTORY_FOLDER_W: f32 = 128.0;
pub const HISTORY_FOLDER_H: f32 = 32.0;
pub const HISTORY_FOLDER_ICON: f32 = 16.0;
pub const HISTORY_EMPTY_ICON: f32 = 40.0;
pub const HISTORY_EMPTY_GAP: f32 = 12.0;
pub const HISTORY_FOCUS_WIDTH: f32 = 2.0;
pub const HISTORY_CAPTION_GAP: f32 = 4.0;
pub const HISTORY_ACTION_ICON: f32 = 16.0;
pub const HISTORY_COUNT_GAP: f32 = 12.0;
pub const HISTORY_SCROLL_MIN: f32 = 24.0;
pub const HISTORY_SCROLL_RIGHT: f32 = 6.0;
pub const HISTORY_SCROLL_WIDTH: f32 = 4.0;
pub const HISTORY_SCROLL_RADIUS: f32 = 2.0;
pub const SETTINGS_WIDTH: f32 = 640.0;
pub const SETTINGS_HEADER: f32 = 60.0;
pub const SETTINGS_ROW_HEIGHT: f32 = 52.0;
pub const SETTINGS_FOOTER: f32 = 44.0;
pub const SETTINGS_MARGIN: f32 = 20.0;
pub const SETTINGS_CARD_PAD: f32 = 16.0;
pub const SETTINGS_SECTION_LABEL: f32 = 32.0;
pub const SETTINGS_SECTION_GAP: f32 = 8.0;
pub const SETTINGS_ROW_HOVER_INSET: f32 = 3.0;
pub const SETTINGS_KEY_PAD: f32 = 9.0;
pub const SETTINGS_KEY_GAP: f32 = 4.0;
pub const SETTINGS_KEY_HALF_HEIGHT: f32 = 12.0;
pub const SETTINGS_EDIT_ICON: f32 = 16.0;
pub const SETTINGS_EDIT_ICON_GAP: f32 = 10.0;
pub const SETTINGS_HOTKEY_WIDTH: f32 = 180.0;
pub const SETTINGS_LANGUAGE_WIDTH: f32 = 240.0;
pub const SETTINGS_CONTROL_HALF_HEIGHT: f32 = 15.0;
pub const SETTINGS_CONTROL_TEXT_PAD: f32 = 24.0;
pub const SETTINGS_TOGGLE_WIDTH: f32 = 40.0;
pub const SETTINGS_TOGGLE_HALF_HEIGHT: f32 = 10.0;
pub const SETTINGS_HINT_GAP: f32 = 2.0;
pub const SETTINGS_FOLDER_GAP: f32 = 28.0;
pub const SETTINGS_FOLDER_TEXT_GAP: f32 = 14.0;
pub const SETTINGS_TOGGLE_KNOB_ON: f32 = 7.0;
pub const SETTINGS_TOGGLE_KNOB_OFF: f32 = 5.0;
pub const SETTINGS_TOGGLE_LABEL_GAP: f32 = 10.0;
pub const SETTINGS_FIELD_RADIUS_INSET: f32 = 1.5;
pub const COUNTDOWN_SIZE: f32 = 56.0;
pub const COUNTDOWN_FONT: f32 = 26.0;
/// Window titles ("Geçmiş", "Ayarlar").
pub const TITLE_FONT: f32 = 20.0;
pub const TEXT_LINE_SCALE: f32 = 1.25;
pub const HISTORY_SCROLL_ALPHA: u8 = 160;

/// Text capture's mark (decision 2026-10-23: the logo stays, the UI colour is yellow). A fill only,
/// always with `MARKER_INK` on it; as text on a light ground it would not be readable.
pub const MARKER: Rgba = c(0xFFD43B);
pub const MARKER_HOVER: Rgba = c(0xFFDD66);
pub const MARKER_INK: Rgba = c(0x1A1A1A);
/// Yellow's edge on a light ground, where the yellow alone does not stand out.
pub const MARKER_LINE: Rgba = c(0xB58900);
/// The result card (WinTextCapture's sizes).
pub const CARD_WIDTH: f32 = 400.0;
pub const CARD_PAD: f32 = 16.0;
pub const CARD_MARK: f32 = 20.0;
pub const CARD_MARK_GAP: f32 = 10.0;
pub const CARD_MARGIN: f32 = 16.0;
pub const CARD_BODY_LINES: usize = 3;
/// The main window.
pub const HOME_WIDTH: f32 = 760.0;
pub const HOME_HEIGHT: f32 = 690.0;
pub const SETTINGS_TABS: f32 = 44.0;

/// Annotation colours, in toolbar order. Red first: the default.
pub const PALETTE: [Rgba; 8] = [c(0xE5383B), c(0xF28C28), c(0xFFD60A), c(0x2DA44E), c(0x1F6FEB), c(0x8250DF), c(0x111111), c(0xFFFFFF)];

/// The highlighter's own colours (drawn with multiply, so text under them stays readable).
pub const MARKER_DEFAULT: Rgba = PALETTE[2];

/// Initial history client dimensions and status spacing, in DIPs.
pub const HISTORY_WINDOW_WIDTH: f32 = 900.0;
pub const HISTORY_WINDOW_HEIGHT: f32 = 600.0;
pub const HISTORY_NOTE_GAP: f32 = 12.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<Theme> {
        let mut v = Vec::new();
        for look in [Look::Graphite, Look::Paper] {
            for mode in [Mode::Light, Mode::Dark] {
                v.push(Theme::new(look, mode));
            }
        }
        v
    }

    #[test]
    fn text_meets_wcag_aa_on_every_panel() {
        let themes = all();
        assert_eq!(themes.len(), 4);
        for t in themes {
            let name = format!("{:?}/{:?}", t.look, t.mode);
            for (what, fg, bg, min) in [
                ("text/bg", t.text, t.bg, 7.0),
                ("text/surface", t.text, t.surface, 7.0),
                ("muted/bg", t.muted, t.bg, 4.5),
                ("muted/surface", t.muted, t.surface, 4.5),
                ("on_accent/accent", t.on_accent, t.accent, 4.5),
                ("selected icon", t.on_selected(), t.selected, 4.5),
                ("text/window", t.text, t.window, 7.0),
                ("muted/window", t.muted, t.window, 4.5),
                ("text/card", t.text, t.card, 7.0),
                ("muted/card", t.muted, t.card, 4.5),
                ("chosen segment", t.on_raised, t.raised, 7.0),
                ("danger/bg", t.danger, t.bg, 4.5),
                ("accent/bg (UI)", t.accent, t.bg, 3.0),
            ] {
                let r = fg.contrast(bg);
                assert!(r >= min, "{name} {what}: contrast {r:.2} < {min}");
            }
        }
    }

    #[test]
    fn the_marker_is_readable_with_its_ink() {
        assert!(MARKER_INK.contrast(MARKER) >= 7.0);
        assert!(MARKER_INK.contrast(MARKER_HOVER) >= 7.0);
    }

    #[test]
    fn selection_frame_has_a_light_and_a_dark_line() {
        for t in all() {
            let (a, b) = (t.sel_inner.luminance(), t.sel_outer.luminance());
            assert!((a - b).abs() > 0.15, "{:?}/{:?}: frame lines too alike", t.look, t.mode);
        }
    }

    #[test]
    fn system_choice_follows_windows() {
        assert_eq!(ThemeChoice::System.resolve(true), Mode::Dark);
        assert_eq!(ThemeChoice::System.resolve(false), Mode::Light);
        assert_eq!(ThemeChoice::Light.resolve(true), Mode::Light);
        assert_eq!(ThemeChoice::Dark.resolve(false), Mode::Dark);
    }

    #[test]
    fn palette_colours_are_distinct() {
        for (i, a) in PALETTE.iter().enumerate() {
            for b in &PALETTE[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }
}
