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
    /// Pick a drawn shape, move it, delete it (V).
    Select,
    Arrow,
    Rect,
    Ellipse,
    Line,
    Pen,
    Marker,
    Text,
    Steps,
    Spotlight,
    Obscure,
    /// Take the colour of a screen pixel (I); then the previous tool comes back.
    Picker,
}

pub const TOOLS: [Tool; 12] = [Tool::Select, Tool::Arrow, Tool::Rect, Tool::Ellipse, Tool::Line, Tool::Pen, Tool::Marker, Tool::Text, Tool::Steps, Tool::Spotlight, Tool::Obscure, Tool::Picker];

/// The drawing tools in toolbar order; the picker sits with the colour, the select tool first.
pub const DRAW_TOOLS: [Tool; 10] = [Tool::Arrow, Tool::Rect, Tool::Ellipse, Tool::Line, Tool::Pen, Tool::Marker, Tool::Text, Tool::Steps, Tool::Spotlight, Tool::Obscure];

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
            Tool::Select => Icon::Select,
            Tool::Ellipse => Icon::Ellipse,
            Tool::Spotlight => Icon::Spotlight,
            Tool::Picker => Icon::Picker,
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
            Tool::Select => 'V',
            Tool::Ellipse => 'E',
            Tool::Spotlight => 'F',
            Tool::Picker => 'I',
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
            Tool::Select => S::ToolSelect,
            Tool::Ellipse => S::ToolEllipse,
            Tool::Spotlight => S::ToolSpotlight,
            Tool::Picker => S::ToolPicker,
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
    /// Put the finished picture on a framed ground (a toggle).
    Frame,
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
            Item::Frame => s(S::FrameTip),
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
    // Groups: pick | draw | colour and size | history | out.
    let mut v: Vec<Slot> = vec![Slot::Btn(Item::Tool(Tool::Select)), Slot::Sep];
    v.extend(DRAW_TOOLS.iter().map(|&t| Slot::Btn(Item::Tool(t))));
    v.push(Slot::Sep);
    v.push(Slot::Btn(Item::Tool(Tool::Picker)));
    v.push(Slot::Btn(if tool == Some(Tool::Obscure) { Item::ObscureMode } else { Item::Color }));
    v.push(Slot::Btn(Item::Size));
    v.push(Slot::Sep);
    v.extend([Slot::Btn(Item::Undo), Slot::Btn(Item::Redo), Slot::Sep, Slot::Btn(Item::Frame), Slot::Btn(Item::Pin), Slot::Btn(Item::Save)]);
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

    /// Like `layout`, but no wider than `max_w`: a bar that does not fit (a small monitor at a high
    /// scale) breaks into rows between its groups, as few rows as fit, the widest row as narrow
    /// as it can be. Only a single group wider than `max_w` is broken inside.
    pub fn layout_fit(slots: &[Slot], t: &Theme, scale: f32, max_w: i32) -> Bar {
        let one = Bar::layout(slots, t, scale);
        if one.w <= max_w {
            return one;
        }
        let groups: Vec<Vec<Slot>> = slots.split(|s| *s == Slot::Sep).filter(|g| !g.is_empty()).map(|g| g.to_vec()).collect();
        let width = |gs: &[Vec<Slot>]| Bar::layout(&join_groups(gs), t, scale).w;
        // Few groups (six on the editor bar): try every way to cut them into 2, 3, ... rows.
        for rows in 2..=groups.len() {
            let mut best: Option<(i32, Vec<usize>)> = None;
            for cuts in cut_points(groups.len(), rows) {
                let widest = row_ranges(&cuts, groups.len()).map(|(a, b)| width(&groups[a..b])).max().unwrap_or(0);
                if widest <= max_w && best.as_ref().is_none_or(|(w, _)| widest < *w) {
                    best = Some((widest, cuts));
                }
            }
            if let Some((_, cuts)) = best {
                let rows: Vec<Vec<Slot>> = row_ranges(&cuts, groups.len()).map(|(a, b)| join_groups(&groups[a..b])).collect();
                return Bar::stack(&rows, t, scale);
            }
        }
        // Even one group per row is too wide: fill rows button by button.
        let mut rows: Vec<Vec<Slot>> = Vec::new();
        for g in &groups {
            let mut row: Vec<Slot> = Vec::new();
            for s in g {
                row.push(*s);
                if row.len() > 1 && Bar::layout(&row, t, scale).w > max_w {
                    let last = row.pop().unwrap_or(*s);
                    rows.push(std::mem::replace(&mut row, vec![last]));
                }
            }
            rows.push(row);
        }
        Bar::stack(&rows, t, scale)
    }

    /// Rows laid out one under the other, each centred in the widest.
    fn stack(rows: &[Vec<Slot>], t: &Theme, scale: f32) -> Bar {
        let laid: Vec<Bar> = rows.iter().map(|r| Bar::layout(r, t, scale)).collect();
        let pad = (t.pad * scale).round() as i32;
        let gap = (t.gap * scale).round() as i32;
        let w = laid.iter().map(|b| b.w).max().unwrap_or(2 * pad);
        let row_h = laid.first().map(|b| b.h - 2 * pad).unwrap_or(0);
        let mut bar = Bar { w, h: 2 * pad, scale, buttons: Vec::new(), seps: Vec::new() };
        for (i, r) in laid.iter().enumerate() {
            let (dx, dy) = ((w - r.w) / 2, i as i32 * (row_h + gap));
            bar.buttons.extend(r.buttons.iter().map(|(it, b)| (*it, b.translate(dx, dy))));
            bar.seps.extend(r.seps.iter().map(|b| b.translate(dx, dy)));
        }
        bar.h += laid.len() as i32 * row_h + (laid.len() as i32 - 1).max(0) * gap;
        bar
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<Item> {
        self.buttons.iter().find(|(_, b)| b.contains(x, y)).map(|(i, _)| *i)
    }

    pub fn button(&self, item: Item) -> Option<IBox> {
        self.buttons.iter().find(|(i, _)| *i == item).map(|(_, b)| *b)
    }
}

fn join_groups(gs: &[Vec<Slot>]) -> Vec<Slot> {
    let mut v = Vec::new();
    for (i, g) in gs.iter().enumerate() {
        if i > 0 {
            v.push(Slot::Sep);
        }
        v.extend_from_slice(g);
    }
    v
}

/// Every way to cut `n` groups into `rows` non-empty rows, as the sorted start indices of rows
/// 2..: `[2, 4]` means rows `0..2`, `2..4` and `4..n`.
fn cut_points(n: usize, rows: usize) -> Vec<Vec<usize>> {
    fn go(from: usize, n: usize, left: usize, cur: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if left == 0 {
            out.push(cur.clone());
            return;
        }
        for c in from..=n - left {
            cur.push(c);
            go(c + 1, n, left - 1, cur, out);
            cur.pop();
        }
    }
    let mut out = Vec::new();
    if rows >= 1 && rows <= n {
        go(1, n, rows - 1, &mut Vec::new(), &mut out);
    }
    out
}

fn row_ranges(cuts: &[usize], n: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
    let starts = std::iter::once(0).chain(cuts.iter().copied());
    let ends = cuts.iter().copied().chain(std::iter::once(n));
    starts.zip(ends)
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
        Item::Frame => Icon::Frame,
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
    render_hint(what, keys, t, scale, g, i32::MAX)
}

/// The plain snip's hint under the mode bar: the sentence, then its keys as caps.
pub fn render_plain_hint(lang: Lang, t: &Theme, scale: f32, g: &dyn Glyphs, max_w: i32) -> Canvas {
    let keys = [
        ("Enter", S::PlainKeyFull.get(lang)),
        ("Shift+Enter", S::PlainKeyWork.get(lang)),
        (S::PlainKeyShiftClick.get(lang), S::PlainKeyAdd.get(lang)),
        ("Backspace", S::PlainKeyRemove.get(lang)),
        ("Alt", S::PlainKeyParts.get(lang)),
        ("Esc", S::TextHintCancel.get(lang)),
    ];
    // A compact panel under the mode bar, not a strip across the screen.
    let max_w = max_w.min((theme::HINT_MAX_W * scale).round() as i32);
    render_hint(S::PlainHint.get(lang), &keys, t, scale, g, max_w)
}

/// The hint while picking the area to record.
pub fn render_record_hint(lang: Lang, t: &Theme, scale: f32, g: &dyn Glyphs) -> Canvas {
    render_text_hint(S::RecordHint.get(lang), &[("Enter", S::PlainKeyFull.get(lang)), ("Esc", S::TextHintCancel.get(lang))], t, scale, g)
}

/// A hint panel: what to do, then each key as a small cap with its meaning after it
/// ("Alt  başka ekrana geçer   Esc  iptal"). One row when it fits in `max_w` pixels; otherwise
/// the sentence on its own row and the keys wrapped under it, left-aligned.
pub fn render_hint(what: &str, keys: &[(&str, &str)], t: &Theme, scale: f32, g: &dyn Glyphs, max_w: i32) -> Canvas {
    let s = |d: f32| (d * scale).round() as i32;
    let font = t.font * scale;
    let a = g.text(what, font, true);
    let parts: Vec<_> = keys.iter().map(|(k, label)| (g.text(k, t.font_small * scale, true), g.text(label, font, false))).collect();
    let (pad_x, pad_y, gap, cap_pad, after) = (s(16.0), s(10.0), s(14.0), s(7.0), s(6.0));
    let cap_h = parts.iter().map(|(k, _)| k.h as i32).max().unwrap_or(0) + s(4.0);
    let row_h = (a.h as i32).max(cap_h);
    let part_w = |k: &crate::raster::TextMask, l: &crate::raster::TextMask| k.w as i32 + 2 * cap_pad + after + l.w as i32;
    let one_row = pad_x * 2 + a.w as i32 + parts.iter().map(|(k, l)| gap + part_w(k, l)).sum::<i32>();
    // Rows of (x, part index) after the sentence; the first row may share the sentence's line.
    let mut rows: Vec<Vec<(i32, usize)>> = Vec::new();
    let w = if one_row <= max_w || parts.is_empty() {
        let mut x = pad_x + a.w as i32;
        rows.push(parts.iter().enumerate().map(|(i, (k, l))| { let at = x + gap; x = at + part_w(k, l); (at, i) }).collect());
        one_row
    } else {
        // Greedy rows at the widest allowed, then the narrowest width that still needs no more
        // rows, so the rows come out even instead of one long row and a lone key.
        let widths: Vec<i32> = parts.iter().map(|(k, l)| part_w(k, l)).collect();
        let pack = |limit: i32| -> Vec<Vec<usize>> {
            let mut out: Vec<Vec<usize>> = Vec::new();
            let mut x = 0;
            for (i, &pw) in widths.iter().enumerate() {
                if out.is_empty() || (x > 0 && x + gap + pw > limit) {
                    out.push(Vec::new());
                    x = 0;
                }
                x = if x == 0 { pw } else { x + gap + pw };
                out.last_mut().unwrap().push(i);
            }
            out
        };
        let inner = (max_w - 2 * pad_x).max(1);
        let n = pack(inner).len();
        let mut limit = widths.iter().copied().max().unwrap_or(0).max(widths.iter().sum::<i32>() / n as i32);
        while pack(limit).len() > n && limit < inner {
            limit += s(4.0).max(1);
        }
        rows.push(Vec::new());
        let mut widest = a.w as i32;
        for row in pack(limit.min(inner)) {
            let mut x = 0;
            let mut placed = Vec::new();
            for i in row {
                let at = if x == 0 { 0 } else { x + gap };
                placed.push((pad_x + at, i));
                x = at + widths[i];
            }
            widest = widest.max(x);
            rows.push(placed);
        }
        widest + 2 * pad_x
    };
    let row_gap = s(6.0);
    let h = rows.len() as i32 * row_h + (rows.len() as i32 - 1) * row_gap + 2 * pad_y;
    let mut cv = Canvas::new(w as u32, h as u32, Rgba::rgb(0, 0, 0).with_alpha(0));
    panel(&mut cv, t, scale);
    let mid0 = pad_y + row_h / 2;
    cv.text_mask(&a, pad_x, mid0 - a.h as i32 / 2, t.text);
    for (r, row) in rows.iter().enumerate() {
        let mid = mid0 + r as i32 * (row_h + row_gap);
        for &(x, i) in row {
            let (k, label) = &parts[i];
            let cap = IBox::new(x, mid - cap_h / 2, x + k.w as i32 + 2 * cap_pad, mid + cap_h - cap_h / 2);
            let mut m = Mask::new(cap);
            m.rounded_box(cap.x0 as f32, cap.y0 as f32, cap.x1 as f32, cap.y1 as f32, t.control_radius() * scale);
            cv.blend_mask(&m, t.raised);
            let mut o = Mask::new(cap);
            o.rounded_box_outline(cap.x0 as f32, cap.y0 as f32, cap.x1 as f32, cap.y1 as f32, t.control_radius() * scale, (theme::BORDER_WIDTH * scale).max(1.0));
            cv.blend_mask(&o, t.border);
            cv.text_mask(k, cap.x0 + cap_pad, cap.y0 + (cap.h() - k.h as i32) / 2, t.on_raised);
            cv.text_mask(label, cap.x1 + after, mid - label.h as i32 / 2, t.muted);
        }
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
        assert_eq!(b.buttons.len(), 21);
        for t in TOOLS {
            assert_eq!(b.buttons.iter().filter(|(i, _)| *i == Item::Tool(t)).count(), 1, "{t:?}");
        }
        for it in [Item::Color, Item::Size, Item::Frame, Item::Undo, Item::Redo, Item::Pin, Item::Save, Item::Copy, Item::Close] {
            assert_eq!(b.buttons.iter().filter(|(i, _)| *i == it).count(), 1, "{it:?}");
        }
        assert_eq!(b.seps.len(), 5);
        // 21 buttons of 32, 5 separators of 9, 25 gaps of 2, 4 padding each side.
        assert_eq!((b.w, b.h), (21 * 32 + 5 * 9 + 25 * 2 + 8, 40));
        // The select tool leads; the picker sits right before the colour.
        assert_eq!(b.buttons[0].0, Item::Tool(Tool::Select));
        let at = |it: Item| b.buttons.iter().position(|(i, _)| *i == it).unwrap();
        assert_eq!(at(Item::Tool(Tool::Picker)) + 1, at(Item::Color));
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
    fn a_bar_too_wide_breaks_between_groups_and_keeps_every_button() {
        let t = theme();
        let slots = editor_slots_with(None, true);
        let one = Bar::layout(&slots, &t, 2.5);
        assert_eq!(Bar::layout_fit(&slots, &t, 2.5, one.w), one, "fits: unchanged");
        for max_w in [one.w - 1, 1900, 1200, 700, 300] {
            let b = Bar::layout_fit(&slots, &t, 2.5, max_w);
            let widest_btn = (t.button * 2.5).round() as i32 + 2 * (t.pad * 2.5).round() as i32;
            assert!(b.w <= max_w.max(widest_btn), "{max_w}: {}", b.w);
            assert_eq!(b.buttons.len(), one.buttons.len());
            for (it, _) in &one.buttons {
                assert_eq!(b.buttons.iter().filter(|(i, _)| i == it).count(), 1, "{it:?}");
            }
            for (i, (_, a)) in b.buttons.iter().enumerate() {
                assert!(a.x0 >= 0 && a.y0 >= 0 && a.x1 <= b.w && a.y1 <= b.h);
                assert_eq!(b.hit(a.x0 + 1, a.y0 + 1), Some(b.buttons[i].0));
                for (_, c) in &b.buttons[i + 1..] {
                    assert!(a.intersect(c).is_empty());
                }
            }
        }
        // 1920 px at 250 %: two rows, cut where a group ends.
        let b = Bar::layout_fit(&slots, &t, 2.5, 1920 - 2 * 20);
        let rows: std::collections::BTreeSet<i32> = b.buttons.iter().map(|(_, x)| x.y0).collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(cut_points(4, 2), vec![vec![1], vec![2], vec![3]]);
        assert_eq!(cut_points(4, 3), vec![vec![1, 2], vec![1, 3], vec![2, 3]]);
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
    fn plain_hint_wraps_its_keys_into_even_rows_under_the_sentence() {
        let t = theme();
        let keys = [("Enter", "full screen"), ("Esc", "cancel")];
        let one = render_hint("Drag", &keys, &t, 1.0, &BoxFont, i32::MAX);
        assert_eq!(one.h, render_text_hint("Drag", &keys, &t, 1.0, &BoxFont).h, "one row when there is room");
        for lang in [Lang::Tr, Lang::En] {
            for (scale, screen) in [(1.0, 1920), (1.0, 700), (1.5, 1560)] {
                let cv = render_plain_hint(lang, &t, scale, &BoxFont, screen - 32);
                assert!(cv.w as i32 <= ((theme::HINT_MAX_W * scale) as i32).min(screen - 32), "{lang:?} {scale} {screen}: {}", cv.w);
                assert!(cv.h > one.h * 2, "the sentence, then the keys");
                assert!(cv.px.iter().any(|b| *b != 0));
            }
        }
        // Even rows: no row of the keys is much shorter than the others because of one lone key.
        let what = "Drag an area or click a window";
        let six = [("Enter", "full screen"), ("Shift+Enter", "without the taskbar"), ("Shift+click", "add one more"), ("Backspace", "remove the last"), ("Alt", "inner parts"), ("Esc", "cancel")];
        let wide = render_hint(what, &six, &t, 1.0, &BoxFont, 760);
        let tight = render_hint(what, &six, &t, 1.0, &BoxFont, 1060);
        assert!(tight.w < 1060 && wide.w <= 760);
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
        assert_eq!(Item::Tool(Tool::Ellipse).tooltip(Lang::En, ObscureMode::Pixelate), "Ellipse (E)");
        assert_eq!(Item::Tool(Tool::Spotlight).tooltip(Lang::Tr, ObscureMode::Pixelate), "Spot ışığı (F)");
        assert_eq!(Item::Tool(Tool::Select).tooltip(Lang::En, ObscureMode::Pixelate), "Select and move (V)");
        assert_eq!(Item::Tool(Tool::Picker).tooltip(Lang::En, ObscureMode::Pixelate), "Pick a colour (I)");
    }
}
