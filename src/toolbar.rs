//! The bars snip draws on the frozen screen: the editor toolbar, its colour popover, the plain
//! snip's mode bar and the size label. Layout, hit testing and drawing are here, in physical
//! pixels at a given scale; the Windows side only blits the resulting canvas.

use crate::annotate::ObscureMode;
use crate::geometry::Rect;
use crate::i18n::{Lang, S};
use crate::icons::{self, Icon};
use crate::raster::{Canvas, Glyphs, IBox, Mask, Pt, Rgba};
use crate::theme::{self, PALETTE, Theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Arrow,
    Rect,
    Line,
    Pen,
    Marker,
    Text,
    Steps,
    Obscure,
}

pub const TOOLS: [Tool; 8] = [Tool::Arrow, Tool::Rect, Tool::Line, Tool::Pen, Tool::Marker, Tool::Text, Tool::Steps, Tool::Obscure];

impl Tool {
    pub fn icon(self) -> Icon {
        match self {
            Tool::Arrow => Icon::Arrow,
            Tool::Rect => Icon::Rect,
            Tool::Line => Icon::Line,
            Tool::Pen => Icon::Pen,
            Tool::Marker => Icon::Marker,
            Tool::Text => Icon::Text,
            Tool::Steps => Icon::Steps,
            Tool::Obscure => Icon::Obscure,
        }
    }

    /// The single-letter shortcut (no modifier, not while typing text).
    pub fn key(self) -> char {
        match self {
            Tool::Arrow => 'A',
            Tool::Rect => 'R',
            Tool::Line => 'L',
            Tool::Pen => 'P',
            Tool::Marker => 'H',
            Tool::Text => 'T',
            Tool::Steps => 'N',
            Tool::Obscure => 'B',
        }
    }

    pub fn label(self) -> S {
        match self {
            Tool::Arrow => S::ToolArrow,
            Tool::Rect => S::ToolRect,
            Tool::Line => S::ToolLine,
            Tool::Pen => S::ToolPen,
            Tool::Marker => S::ToolMarker,
            Tool::Text => S::ToolText,
            Tool::Steps => S::ToolSteps,
            Tool::Obscure => S::ToolObscure,
        }
    }

    pub fn from_key(c: char) -> Option<Tool> {
        TOOLS.iter().copied().find(|t| t.key() == c.to_ascii_uppercase())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    Tool(Tool),
    Color,
    /// Replaces Color while the pixelate tool is active: pixelate or blur.
    ObscureMode,
    Size,
    Undo,
    Redo,
    Pin,
    Save,
    Copy,
    Close,
    // Plain snip mode bar.
    ModeRegion,
    ModeWindow,
    ModeScreen,
    /// Plain mode bar: read the area as text instead of taking a picture of it.
    ModeText,
    /// Editor: read the selection as text.
    CopyText,
}

impl Item {
    pub fn tooltip(self, lang: Lang, obscure: ObscureMode) -> String {
        let s = |x: S| x.get(lang).to_string();
        match self {
            Item::Tool(t) => format!("{} ({})", t.label().get(lang), t.key()),
            Item::Color => s(S::Color),
            Item::ObscureMode => s(if obscure == ObscureMode::Pixelate { S::ToolObscure } else { S::ToolBlur }),
            Item::Size => format!("{} (1, 2, 3)", S::Size.get(lang)),
            Item::Undo => format!("{} (Ctrl+Z)", S::Undo.get(lang)),
            Item::Redo => format!("{} (Ctrl+Y)", S::Redo.get(lang)),
            Item::Pin => format!("{} (Ctrl+P)", S::Pin.get(lang)),
            Item::Save => format!("{} (Ctrl+S)", S::Save.get(lang)),
            Item::Copy => format!("{} (Ctrl+C)", S::Copy.get(lang)),
            Item::CopyText => s(S::CopyTextTip),
            Item::ModeText => s(S::ModeText),
            Item::Close => format!("{} (Esc)", S::Close.get(lang)),
            Item::ModeRegion => s(S::ModeRegion),
            Item::ModeWindow => s(S::ModeWindow),
            Item::ModeScreen => s(S::ModeScreen),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Btn(Item),
    Sep,
}

pub fn editor_slots(tool: Option<Tool>) -> Vec<Slot> {
    editor_slots_with(tool, false)
}

/// `text_action`: text capture is on, so the bar offers "copy text" beside "copy".
pub fn editor_slots_with(tool: Option<Tool>, text_action: bool) -> Vec<Slot> {
    let mut v: Vec<Slot> = TOOLS.iter().map(|&t| Slot::Btn(Item::Tool(t))).collect();
    v.push(Slot::Sep);
    v.push(Slot::Btn(if tool == Some(Tool::Obscure) { Item::ObscureMode } else { Item::Color }));
    v.push(Slot::Btn(Item::Size));
    v.push(Slot::Sep);
    v.extend([Slot::Btn(Item::Undo), Slot::Btn(Item::Redo), Slot::Sep, Slot::Btn(Item::Pin), Slot::Btn(Item::Save)]);
    if text_action {
        v.push(Slot::Btn(Item::CopyText));
    }
    v.extend([Slot::Btn(Item::Copy), Slot::Sep, Slot::Btn(Item::Close)]);
    v
}

pub fn mode_slots() -> Vec<Slot> {
    mode_slots_with(false)
}

/// `text_action`: text capture is on, so one more mode reads the dragged area as text.
pub fn mode_slots_with(text_action: bool) -> Vec<Slot> {
    let mut v = vec![Slot::Btn(Item::ModeRegion), Slot::Btn(Item::ModeWindow), Slot::Btn(Item::ModeScreen)];
    if text_action {
        v.push(Slot::Btn(Item::ModeText));
    }
    v.extend([Slot::Sep, Slot::Btn(Item::Close)]);
    v
}

/// A laid-out bar: size and button boxes relative to its top-left, in physical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub w: i32,
    pub h: i32,
    pub scale: f32,
    pub buttons: Vec<(Item, IBox)>,
    pub seps: Vec<IBox>,
}

impl Bar {
    pub fn layout(slots: &[Slot], t: &Theme, scale: f32) -> Bar {
        let s = |d: f32| (d * scale).round() as i32;
        let (pad, btn, gap, sep_w) = (s(t.pad), s(t.button), s(t.gap), s(theme::SEPARATOR_WIDTH));
        let mut x = pad;
        let mut buttons = Vec::new();
        let mut seps = Vec::new();
        for (i, slot) in slots.iter().enumerate() {
            if i > 0 {
                x += gap;
            }
            match slot {
                Slot::Btn(it) => {
                    buttons.push((*it, IBox::xywh(x, pad, btn, btn)));
                    x += btn;
                }
                Slot::Sep => {
                    seps.push(IBox::xywh(x + sep_w / 2, pad + s(theme::SEPARATOR_INSET), s(theme::BORDER_WIDTH).max(1), btn - 2 * s(theme::SEPARATOR_INSET)));
                    x += sep_w;
                }
            }
        }
        Bar { w: x + pad, h: btn + 2 * pad, scale, buttons, seps }
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<Item> {
        self.buttons.iter().find(|(_, b)| b.contains(x, y)).map(|(i, _)| *i)
    }

    pub fn button(&self, item: Item) -> Option<IBox> {
        self.buttons.iter().find(|(i, _)| *i == item).map(|(_, b)| *b)
    }
}

/// What the bar shows besides its layout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BarState {
    pub active: Vec<Item>,
    pub disabled: Vec<Item>,
    pub hover: Option<Item>,
    pub pressed: Option<Item>,
    pub color: Rgba,
    /// 0, 1 or 2.
    pub size: usize,
    pub obscure: Option<ObscureMode>,
}

fn panel(cv: &mut Canvas, t: &Theme, scale: f32) {
    let (w, h) = (cv.w as f32, cv.h as f32);
    let r = t.radius * scale;
    let mut m = Mask::new(cv.bounds());
    m.rounded_box(0.0, 0.0, w, h, r);
    cv.blend_mask(&m, t.border);
    let bw = (theme::BORDER_WIDTH * scale).round().max(1.0);
    let mut inner = Mask::new(cv.bounds());
    inner.rounded_box(bw, bw, w - bw, h - bw, (r - bw).max(0.0));
    cv.blend_mask(&inner, t.bg);
}

fn button_bg(cv: &mut Canvas, b: IBox, c: Rgba, t: &Theme, scale: f32) {
    let mut m = Mask::new(b);
    m.rounded_box(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32, t.control_radius() * scale);
    cv.blend_mask(&m, c);
}

fn mix(a: Rgba, b: Rgba, k: f32) -> Rgba {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
    Rgba::rgb(f(a.r, b.r), f(a.g, b.g), f(a.b, b.b))
}

/// The soft shadow under a floating panel: a canvas larger than the panel by the returned margin
/// on the left and top. It is black with varying alpha, drawn before the panel.
pub fn render_shadow(w: i32, h: i32, t: &Theme, scale: f32) -> (Canvas, i32, i32) {
    let blur = (theme::SHADOW_BLUR * scale).max(1.0);
    let dy = (theme::SHADOW_Y * scale).round() as i32;
    let m = blur.ceil() as i32;
    let (cw, ch) = (w + 2 * m, h + 2 * m + dy);
    let mut cv = Canvas::new(cw as u32, ch as u32, Rgba::default());
    let half = Pt::new(w as f32 / 2.0, h as f32 / 2.0);
    let r = t.radius * scale;
    let centre = Pt::new(m as f32 + half.x, (m + dy) as f32 + half.y);
    let inner = Pt::new(half.x - r, half.y - r);
    let peak = t.shadow_alpha() as f32;
    for y in 0..ch {
        for x in 0..cw {
            let p = Pt::new(x as f32 + 0.5 - centre.x, y as f32 + 0.5 - centre.y);
            let d = crate::raster::sd_box(p, inner) - r;
            let k = (1.0 - (d + blur * 0.25) / blur).clamp(0.0, 1.0);
            let a = peak * k * k * (3.0 - 2.0 * k);
            if a >= 1.0 {
                cv.set(x, y, theme::DIM_COLOR.with_alpha(a as u8));
            }
        }
    }
    (cv, m, m)
}

pub fn render_bar(bar: &Bar, t: &Theme, st: &BarState) -> Canvas {
    let mut cv = Canvas::new(bar.w as u32, bar.h as u32, Rgba::default());
    let sc = bar.scale;
    panel(&mut cv, t, sc);
    for s in &bar.seps {
        cv.fill_box(*s, t.border);
    }
    let isz = t.icon * sc;
    for (item, b) in &bar.buttons {
        let active = st.active.contains(item);
        let disabled = st.disabled.contains(item);
        // One primary action: the finished picture goes to the clipboard.
        let primary = *item == Item::Copy;
        // Copying text is the other function: its colour, and it cannot pass for the T tool.
        let text_item = matches!(item, Item::ModeText | Item::CopyText);
        if text_item {
            let lit = st.pressed == Some(*item) || st.hover == Some(*item);
            button_bg(&mut cv, *b, if lit { theme::MARKER_HOVER } else { theme::MARKER }, t, sc);
        } else if primary {
            let c = if st.pressed == Some(*item) {
                mix(t.accent, t.on_accent, 0.25)
            } else if st.hover == Some(*item) {
                mix(t.accent, t.on_accent, 0.14)
            } else {
                t.accent
            };
            button_bg(&mut cv, *b, c, t, sc);
        } else if active {
            button_bg(&mut cv, *b, t.selected, t, sc);
        } else if !disabled && (st.pressed == Some(*item) || st.hover == Some(*item)) {
            button_bg(&mut cv, *b, if st.pressed == Some(*item) { pressed(t) } else { t.hover }, t, sc);
        }
        let fg = if text_item {
            theme::MARKER_INK
        } else if primary {
            t.on_accent
        } else if active {
            t.on_selected()
        } else if disabled {
            t.muted.with_alpha(theme::DISABLED_ALPHA)
        } else {
            t.text
        };
        let ix = b.x0 as f32 + (b.w() as f32 - isz) / 2.0;
        let iy = b.y0 as f32 + (b.h() as f32 - isz) / 2.0;
        let (cx, cy) = (b.x0 as f32 + b.w() as f32 / 2.0, b.y0 as f32 + b.h() as f32 / 2.0);
        match item {
            Item::Color => {
                let mut m = Mask::new(*b);
                m.disc(Pt::new(cx, cy), theme::COLOR_BUTTON_RADIUS * sc);
                cv.blend_mask(&m, st.color);
                // A ring so white and near-background colours stay visible.
                let mut r = Mask::new(*b);
                r.ring(Pt::new(cx, cy), theme::COLOR_BUTTON_RADIUS * sc, (theme::BORDER_WIDTH * sc).max(1.0));
                cv.blend_mask(&r, t.border);
            }
            Item::Size => {
                let r = theme::SIZE_DOT_RADII[st.size.min(theme::SIZE_DOT_RADII.len() - 1)] * sc;
                let mut m = Mask::new(*b);
                m.disc(Pt::new(cx, cy), r);
                cv.blend_mask(&m, fg);
            }
            Item::ObscureMode => {
                let icon = if st.obscure == Some(ObscureMode::Blur) { Icon::Blur } else { Icon::Obscure };
                icons::draw(&mut cv, icon, ix, iy, isz, fg);
            }
            _ => icons::draw(&mut cv, item_icon(*item), ix, iy, isz, fg),
        }
    }
    cv
}

/// Pressed fill: the border colour. Further from the panel than hover in every look and mode.
pub fn pressed(t: &Theme) -> Rgba {
    t.border
}

pub fn item_icon(item: Item) -> Icon {
    match item {
        Item::Tool(t) => t.icon(),
        Item::Color => Icon::Color,
        Item::ObscureMode => Icon::Obscure,
        Item::Size => Icon::Size,
        Item::Undo => Icon::Undo,
        Item::Redo => Icon::Redo,
        Item::Pin => Icon::Pin,
        Item::Save => Icon::Save,
        Item::Copy => Icon::Copy,
        Item::Close => Icon::Close,
        Item::ModeRegion => Icon::Region,
        Item::ModeWindow => Icon::Window,
        Item::ModeScreen => Icon::Screen,
        Item::ModeText | Item::CopyText => Icon::TextCopy,
    }
}

/// The colour popover: the palette in one row, then a "custom" cell.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub w: i32,
    pub h: i32,
    pub scale: f32,
    /// Index into PALETTE, or None for the custom cell.
    pub cells: Vec<(Option<usize>, IBox)>,
}

impl Palette {
    pub fn layout(t: &Theme, scale: f32) -> Palette {
        let s = |d: f32| (d * scale).round() as i32;
        let (pad, cell, gap) = (s(t.pad), s(theme::PALETTE_CELL), s(theme::PALETTE_GAP));
        let mut cells = Vec::new();
        let mut x = pad;
        for i in 0..=PALETTE.len() {
            cells.push((if i < PALETTE.len() { Some(i) } else { None }, IBox::xywh(x, pad, cell, cell)));
            x += cell + gap;
        }
        Palette { w: x - gap + pad, h: cell + 2 * pad, scale, cells }
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<Option<usize>> {
        self.cells.iter().find(|(_, b)| b.contains(x, y)).map(|(i, _)| *i)
    }
}

pub fn render_palette(p: &Palette, t: &Theme, current: Rgba, hover: Option<Option<usize>>) -> Canvas {
    let mut cv = Canvas::new(p.w as u32, p.h as u32, Rgba::default());
    let sc = p.scale;
    panel(&mut cv, t, sc);
    let custom = !PALETTE.contains(&current);
    for (idx, b) in &p.cells {
        if hover == Some(*idx) {
            button_bg(&mut cv, *b, t.hover, t, sc);
        }
        let c = Pt::new(b.x0 as f32 + b.w() as f32 / 2.0, b.y0 as f32 + b.h() as f32 / 2.0);
        match idx {
            Some(i) => {
                let col = PALETTE[*i];
                let mut m = Mask::new(*b);
                m.disc(c, theme::PALETTE_SWATCH_RADIUS * sc);
                cv.blend_mask(&m, col);
                let mut r = Mask::new(*b);
                r.ring(c, theme::PALETTE_SWATCH_RADIUS * sc, (theme::BORDER_WIDTH * sc).max(1.0));
                cv.blend_mask(&r, t.border);
                if col == current {
                    let mut sel = Mask::new(*b);
                    sel.ring(c, theme::PALETTE_SELECTION_RADIUS * sc, theme::PALETTE_SELECTION_WIDTH * sc);
                    cv.blend_mask(&sel, t.text);
                }
            }
            None => {
                if custom {
                    // A colour from the system dialog: show it in the custom cell, ringed.
                    let mut m = Mask::new(*b);
                    m.disc(c, theme::PALETTE_SWATCH_RADIUS * sc);
                    cv.blend_mask(&m, current);
                    let mut sel = Mask::new(*b);
                    sel.ring(c, theme::PALETTE_SELECTION_RADIUS * sc, theme::PALETTE_SELECTION_WIDTH * sc);
                    cv.blend_mask(&sel, t.text);
                    continue;
                }
                for dx in [-theme::PALETTE_CUSTOM_DOT_SPACING, 0.0, theme::PALETTE_CUSTOM_DOT_SPACING] {
                    let mut m = Mask::new(*b);
                    m.disc(Pt::new(c.x + dx * sc, c.y), theme::PALETTE_CUSTOM_DOT_RADIUS * sc);
                    cv.blend_mask(&m, t.text);
                }
            }
        }
    }
    cv
}

/// "1280 × 720" in a small panel; returns the canvas.
#[derive(Clone, Debug)]
pub struct SizeMenu {
    pub w: i32,
    pub h: i32,
    pub scale: f32,
    pub rows: Vec<IBox>,
}

impl SizeMenu {
    pub fn layout(t: &Theme, scale: f32, count: usize) -> Self {
        let s = |n: f32| (n * scale).round() as i32;
        let pad = s(t.pad);
        let w = s(theme::SIZE_MENU_WIDTH);
        let top = pad + s(theme::SIZE_MENU_HEADER);
        let row_h = s(theme::SIZE_MENU_ROW);
        Self {
            w,
            h: top + row_h * count as i32 + pad,
            scale,
            rows: (0..count)
                .map(|i| IBox::xywh(pad, top + row_h * i as i32, w - 2 * pad, row_h))
                .collect(),
        }
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<usize> {
        self.rows.iter().position(|r| r.contains(x, y))
    }
}

pub fn render_sizes(
    menu: &SizeMenu,
    t: &Theme,
    lang: Lang,
    values: &[f32],
    selected: usize,
    text: bool,
    g: &dyn Glyphs,
) -> Canvas {
    let mut cv = Canvas::new(menu.w as u32, menu.h as u32, Rgba::default());
    let sc = menu.scale;
    panel(&mut cv, t, sc);
    let x = (theme::SIZE_MENU_TEXT_PAD * sc).round() as i32;
    cv.text(
        g,
        (if text { S::FontSize } else { S::StrokeSize }).get(lang),
        t.font_small * sc,
        false,
        x,
        (t.pad * sc).round() as i32,
        t.muted,
    );
    for (index, row) in menu.rows.iter().enumerate() {
        let on = index == selected;
        if on {
            button_bg(&mut cv, *row, t.selected, t, sc);
        }
        let fg = if on { t.on_selected() } else { t.text };
        let mid = row.y0 + row.h() / 2;
        // What this size looks like, then its number on the right.
        if text {
            let preview = g.text("Aa", values[index].min(theme::SIZE_MENU_PREVIEW_MAX) * sc, false);
            cv.text_mask(&preview, x, mid - preview.h as i32 / 2, fg);
        } else {
            let width = (values[index].min(theme::SIZE_MENU_PREVIEW_MAX) * sc).max(1.0);
            let x1 = x as f32 + theme::SIZE_MENU_PREVIEW_W * sc;
            let mut m = Mask::new(IBox::new(x - 2, row.y0, x1.ceil() as i32 + 2, row.y1));
            m.capsule(Pt::new(x as f32 + width / 2.0, mid as f32), Pt::new(x1 - width / 2.0, mid as f32), width);
            cv.blend_mask(&m, fg);
        }
        let label = g.text(&format!("{}", values[index]), t.font_small * sc, on);
        cv.text_mask(&label, menu.w - x - label.w as i32, mid - label.h as i32 / 2, if on { fg } else { t.muted });
    }
    cv
}

/// A tooltip: the name, then its key in the quieter colour. `"Rectangle (R)"` is one string in
/// `Item::tooltip`, so its tests keep reading whole sentences.
pub fn render_tooltip(text: &str, t: &Theme, scale: f32, g: &dyn Glyphs) -> Canvas {
    let px = t.font_small * scale;
    let (name, key) = match text.rsplit_once(" (") {
        Some((n, k)) => (n, k.trim_end_matches(')')),
        None => (text, ""),
    };
    let a = g.text(name, px, false);
    let padx = (theme::LABEL_PAD_X * scale).round() as u32;
    let pady = (theme::LABEL_PAD_Y * scale).round() as u32;
    if key.is_empty() {
        return render_label(text, t, scale, g);
    }
    let b = g.text(key, px, false);
    let gap = (theme::LABEL_PAD_X * scale).round() as u32;
    let mut cv = Canvas::new(a.w + gap + b.w + 2 * padx, a.h + 2 * pady, Rgba::default());
    panel(&mut cv, t, scale);
    cv.text_mask(&a, padx as i32, pady as i32, t.text);
    cv.text_mask(&b, (padx + a.w + gap) as i32, pady as i32, t.muted);
    cv
}

/// The hint of the text overlay: what to do, then Esc as a key cap and what it does.
pub fn render_text_hint(what: &str, keys: &[(&str, &str)], t: &Theme, scale: f32, g: &dyn Glyphs) -> Canvas {
    let s = |d: f32| (d * scale).round() as i32;
    let font = t.font * scale;
    let a = g.text(what, font, true);
    // Each key is a small cap with its meaning after it: "Alt  başka ekrana geçer   Esc  iptal".
    let parts: Vec<_> = keys.iter().map(|(k, label)| (g.text(k, t.font_small * scale, true), g.text(label, font, false))).collect();
    let (pad_x, pad_y, gap, cap_pad) = (s(16.0), s(10.0), s(14.0), s(7.0));
    let cap_h = parts.iter().map(|(k, _)| k.h as i32).max().unwrap_or(0) + s(4.0);
    let h = (a.h as i32).max(cap_h) + 2 * pad_y;
    let w = pad_x * 2 + a.w as i32 + parts.iter().map(|(k, l)| gap + k.w as i32 + 2 * cap_pad + s(6.0) + l.w as i32).sum::<i32>();
    let mut cv = Canvas::new(w as u32, h as u32, Rgba::rgb(0, 0, 0).with_alpha(0));
    panel(&mut cv, t, scale);
    let mid = h / 2;
    cv.text_mask(&a, pad_x, mid - a.h as i32 / 2, t.text);
    let mut x = pad_x + a.w as i32;
    for (k, label) in &parts {
        let cap = IBox::new(x + gap, mid - cap_h / 2, x + gap + k.w as i32 + 2 * cap_pad, mid + cap_h - cap_h / 2);
        let mut m = Mask::new(cap);
        m.rounded_box(cap.x0 as f32, cap.y0 as f32, cap.x1 as f32, cap.y1 as f32, t.control_radius() * scale);
        cv.blend_mask(&m, t.raised);
        let mut o = Mask::new(cap);
        o.rounded_box_outline(cap.x0 as f32, cap.y0 as f32, cap.x1 as f32, cap.y1 as f32, t.control_radius() * scale, (theme::BORDER_WIDTH * scale).max(1.0));
        cv.blend_mask(&o, t.border);
        cv.text_mask(k, cap.x0 + cap_pad, cap.y0 + (cap.h() - k.h as i32) / 2, t.on_raised);
        cv.text_mask(label, cap.x1 + s(6.0), mid - label.h as i32 / 2, t.muted);
        x = cap.x1 + s(6.0) + label.w as i32;
    }
    cv
}

/// The size label of the text selection: the marker colour with dark ink.
pub fn render_text_label(text: &str, scale: f32, g: &dyn Glyphs) -> Canvas {
    let s = |d: f32| (d * scale).round() as i32;
    let tm = g.text(text, 13.0 * scale, true);
    let (w, h) = (tm.w as i32 + 2 * s(theme::LABEL_PAD_X), tm.h as i32 + 2 * s(theme::LABEL_PAD_Y));
    let mut cv = Canvas::new(w as u32, h as u32, theme::MARKER.with_alpha(0));
    let b = IBox::new(0, 0, w, h);
    let mut m = Mask::new(b);
    m.rounded_box(0.0, 0.0, w as f32, h as f32, 4.0 * scale);
    cv.blend_mask(&m, theme::MARKER);
    cv.text_mask(&tm, s(theme::LABEL_PAD_X), s(theme::LABEL_PAD_Y), theme::MARKER_INK);
    cv
}

pub fn render_label(text: &str, t: &Theme, scale: f32, g: &dyn Glyphs) -> Canvas {
    let px = t.font_small * scale;
    let m = g.text(text, px, false);
    let padx = (theme::LABEL_PAD_X * scale).round() as u32;
    let pady = (theme::LABEL_PAD_Y * scale).round() as u32;
    let mut cv = Canvas::new(m.w + 2 * padx, m.h + 2 * pady, Rgba::default());
    panel(&mut cv, t, scale);
    cv.text_mask(&m, padx as i32, pady as i32, t.text);
    cv
}

/// The eight resize handles of a selection, as (id 0..8, box). Order: corners clockwise from
/// top-left (0..4), then edges top, right, bottom, left (4..8).
pub fn handles(sel: &Rect, scale: f32) -> Vec<(usize, Rect)> {
    let h = (theme::HANDLE_SIZE * scale).round() as i32 | 1;
    let r = h / 2;
    let (cx, cy) = ((sel.left + sel.right) / 2, (sel.top + sel.bottom) / 2);
    let pts = [
        (sel.left, sel.top),
        (sel.right - 1, sel.top),
        (sel.right - 1, sel.bottom - 1),
        (sel.left, sel.bottom - 1),
        (cx, sel.top),
        (sel.right - 1, cy),
        (cx, sel.bottom - 1),
        (sel.left, cy),
    ];
    pts.iter().enumerate().map(|(i, &(x, y))| (i, Rect::new(x - r, y - r, x - r + h, y - r + h))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::BoxFont;
    use crate::theme::{Look, Mode};

    fn theme() -> Theme {
        Theme::new(Look::Graphite, Mode::Light)
    }

    #[test]
    fn editor_bar_has_every_tool_and_action_once() {
        let b = Bar::layout(&editor_slots(None), &theme(), 1.0);
        assert_eq!(b.buttons.len(), 16);
        for t in TOOLS {
            assert!(b.button(Item::Tool(t)).is_some(), "{t:?}");
        }
        for it in [Item::Color, Item::Size, Item::Undo, Item::Redo, Item::Pin, Item::Save, Item::Copy, Item::Close] {
            assert_eq!(b.buttons.iter().filter(|(i, _)| *i == it).count(), 1, "{it:?}");
        }
        assert_eq!(b.seps.len(), 4);
        // 16 buttons of 32, 4 separators of 9, 19 gaps of 2, 4 padding each side.
        assert_eq!((b.w, b.h), (16 * 32 + 4 * 9 + 19 * 2 + 8, 40));
    }

    #[test]
    fn pixelate_tool_swaps_the_colour_button() {
        let b = Bar::layout(&editor_slots(Some(Tool::Obscure)), &theme(), 1.0);
        assert!(b.button(Item::Color).is_none());
        assert!(b.button(Item::ObscureMode).is_some());
    }

    #[test]
    fn layout_scales_with_dpi_and_hits_match() {
        let t = theme();
        let a = Bar::layout(&editor_slots(None), &t, 1.0);
        let b = Bar::layout(&editor_slots(None), &t, 1.5);
        assert!((b.w as f32 - a.w as f32 * 1.5).abs() <= 12.0, "{} vs {}", a.w, b.w);
        assert_eq!(b.h, 60);
        for (item, bx) in &b.buttons {
            let (cx, cy) = (bx.x0 + bx.w() / 2, bx.y0 + bx.h() / 2);
            assert_eq!(b.hit(cx, cy), Some(*item));
        }
        assert_eq!(b.hit(0, 0), None, "padding");
        for s in &b.seps {
            assert_eq!(b.hit(s.x0, s.y0), None, "separator");
        }
    }

    #[test]
    fn buttons_do_not_overlap() {
        let b = Bar::layout(&editor_slots(None), &Theme::new(Look::Paper, Mode::Dark), 1.25);
        for (i, (_, x)) in b.buttons.iter().enumerate() {
            for (_, y) in &b.buttons[i + 1..] {
                assert!(x.intersect(y).is_empty());
            }
            assert!(x.x1 <= b.w && x.y1 <= b.h);
        }
    }

    #[test]
    fn bar_renders_active_tool_differently() {
        let t = theme();
        let bar = Bar::layout(&editor_slots(None), &t, 1.0);
        let plain = render_bar(&bar, &t, &BarState { color: PALETTE[0], ..Default::default() });
        let active = render_bar(&bar, &t, &BarState { active: vec![Item::Tool(Tool::Rect)], color: PALETTE[0], ..Default::default() });
        let b = bar.button(Item::Tool(Tool::Rect)).unwrap();
        assert_ne!(plain.get(b.x0 + 3, b.y0 + 3), active.get(b.x0 + 3, b.y0 + 3));
        assert_eq!(plain.get(0, 0).a, 0, "rounded corner is transparent");
        assert_eq!(plain.get(bar.w / 2, 1), t.bg, "panel fill");
    }

    #[test]
    fn copy_is_the_one_primary_button() {
        let t = theme();
        let bar = Bar::layout(&editor_slots(None), &t, 1.0);
        let cv = render_bar(&bar, &t, &BarState { color: PALETTE[0], ..Default::default() });
        let at = |it: Item| bar.button(it).unwrap();
        assert_eq!(cv.get(at(Item::Copy).x0 + 3, at(Item::Copy).y0 + 3), t.accent);
        assert_eq!(cv.get(at(Item::Save).x0 + 3, at(Item::Save).y0 + 3), t.bg);
    }

    #[test]
    fn the_shadow_surrounds_the_panel_and_fades_out() {
        let t = theme();
        let (cv, mx, my) = render_shadow(100, 40, &t, 1.0);
        assert_eq!(cv.w as i32, 100 + 2 * mx);
        assert!(my > 0 && cv.h as i32 > 40 + 2 * my - 1);
        assert!(cv.get(mx + 50, my + 20).a > 0, "under the panel");
        assert_eq!(cv.get(0, 0).a, 0, "gone at the far corner");
    }

    #[test]
    fn tooltip_shows_name_and_key() {
        let plain = render_tooltip("Pencere", &theme(), 1.0, &BoxFont);
        let keyed = render_tooltip("Pencere (P)", &theme(), 1.0, &BoxFont);
        assert!(keyed.w > plain.w);
    }

    #[test]
    fn palette_hits_every_cell() {
        let p = Palette::layout(&theme(), 2.0);
        assert_eq!(p.cells.len(), PALETTE.len() + 1);
        for (idx, b) in &p.cells {
            assert_eq!(p.hit(b.x0 + 1, b.y0 + 1), Some(*idx));
        }
        let cv = render_palette(&p, &theme(), PALETTE[2], None);
        assert_eq!((cv.w as i32, cv.h as i32), (p.w, p.h));
    }

    #[test]
    fn pressed_is_darker_than_hover_in_every_theme() {
        for look in [Look::Graphite, Look::Paper] {
            for mode in [Mode::Light, Mode::Dark] {
                let t = Theme::new(look, mode);
                let mut c = Canvas::new(2, 1, t.bg);
                c.set(0, 0, t.hover);
                c.blend(1, 0, pressed(&t), 1.0);
                let (h, p) = (c.get(0, 0), c.get(1, 0));
                assert!(h.contrast(p) > 1.08, "{look:?}/{mode:?}: hover {h:?} vs pressed {p:?}");
                let d = |x: Rgba| (x.luminance() - t.bg.luminance()).abs();
                assert!(d(p) > d(h), "{look:?}/{mode:?}: pressed must stand out more than hover");
            }
        }
    }

    #[test]
    fn text_action_is_offered_only_when_text_capture_is_on() {
        let t = theme();
        let on = Bar::layout(&editor_slots_with(None, true), &t, 1.0);
        let off = Bar::layout(&editor_slots_with(None, false), &t, 1.0);
        assert!(on.button(Item::CopyText).is_some() && off.button(Item::CopyText).is_none());
        assert!(Bar::layout(&mode_slots_with(true), &t, 1.0).button(Item::ModeText).is_some());
        assert!(Bar::layout(&mode_slots(), &t, 1.0).button(Item::ModeText).is_none());
        // Copy stays the one primary, and the text action sits beside it.
        let (a, b) = (on.button(Item::CopyText).unwrap(), on.button(Item::Copy).unwrap());
        assert!(a.x1 <= b.x0);
    }

    #[test]
    fn plain_hint_is_one_line_in_both_languages() {
        let t = theme();
        for lang in [Lang::Tr, Lang::En] {
            let text = S::PlainHint.get(lang);
            assert!(!text.contains('\n'));
            let cv = render_text_hint(text, &[], &t, 1.0, &BoxFont);
            let short = render_text_hint("Enter", &[], &t, 1.0, &BoxFont);
            assert_eq!(cv.h, short.h);
            assert!(cv.w > short.w && cv.px.iter().any(|b| *b != 0));
            for key in ["Enter", "Shift+Enter", "Backspace", "Esc"] { assert!(text.contains(key)); }
        }
    }

    #[test]
    fn the_text_hint_and_label_fit_their_text() {
        let t = theme();
        let h = render_text_hint("Kopyalanacak metnin üzerinden sürükleyin", &[("Alt", "başka ekrana geçer"), ("Esc", "iptal")], &t, 1.0, &BoxFont);
        assert!(h.w > 200 && h.h > 20);
        let l = render_text_label("514 \u{00D7} 78", 1.0, &BoxFont);
        assert_eq!(l.get(l.w as i32 / 2, 1), theme::MARKER);
    }

    #[test]
    fn tool_keys_are_unique() {
        for t in TOOLS {
            assert_eq!(Tool::from_key(t.key().to_ascii_lowercase()), Some(t));
        }
        assert_eq!(Tool::from_key('z'), None);
    }

    #[test]
    fn handles_sit_on_the_selection_edges() {
        let sel = Rect::new(-100, -50, 100, 50);
        let hs = handles(&sel, 1.0);
        assert_eq!(hs.len(), 8);
        assert!(hs[0].1.contains(crate::geometry::Point::new(-100, -50)));
        assert!(hs[2].1.contains(crate::geometry::Point::new(99, 49)));
        assert!(hs[5].1.contains(crate::geometry::Point::new(99, 0)));
        assert_eq!(hs[0].1.width(), 7);
        assert_eq!(handles(&sel, 1.5)[0].1.width(), 11);
    }

    #[test]
    fn label_fits_its_text() {
        let c = render_label("1280 × 720", &theme(), 1.0, &BoxFont);
        assert!(c.w > 16 && c.h > 6);
    }

    #[test]
    fn tooltips_name_their_shortcut() {
        assert_eq!(Item::Tool(Tool::Arrow).tooltip(Lang::En, ObscureMode::Pixelate), "Arrow (A)");
        assert_eq!(Item::Copy.tooltip(Lang::Tr, ObscureMode::Pixelate), "Kopyala (Ctrl+C)");
    }
}
