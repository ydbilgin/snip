//! The editor (the second hotkey): select on the frozen screen, then annotate, then copy, save or
//! pin. All of its behaviour is here, as a state machine over pointer and key events in desktop
//! coordinates; the Windows overlay feeds it events and draws what `view` returns.
//!
//! Flow: Selecting (drag a region, click a window, Enter for the monitor) -> Editing. While editing
//! Ctrl+left drag moves the selection with any tool. Without a tool, handles resize it and
//! an outside drag redraws it. With a tool, every plain left drag draws. The toolbar sits next to the selection on the monitor that
//! holds the selection's bottom-right corner, at that monitor's scale.

use serde::{Deserialize, Serialize};

use crate::annotate::{self, Doc, ObscureMode, Shape};
use crate::dpi::{self, Monitors};
use crate::geometry::{Point, Rect, is_click, window_at};
use crate::hookstate::{VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_UP};
use crate::i18n::Lang;
use crate::raster::{Canvas, Glyphs, IBox, Pt, Rgba};
use crate::theme::{MARKER_DEFAULT, PALETTE, Theme};
use crate::toolbar::{self, Bar, BarState, Item, Palette, SizeMenu, Tool};

pub const VK_BACK: u32 = 0x08;

/// Size presets in DIPs; the first three remain the existing 1/2/3 shortcuts.
pub const STROKE: [f32; 6] = crate::theme::ANNOTATION_STROKE;
pub const MARKER: [f32; 6] = crate::theme::ANNOTATION_MARKER;
pub const TEXT: [f32; 6] = crate::theme::ANNOTATION_TEXT;
pub const STEP_R: [f32; 6] = crate::theme::ANNOTATION_STEP_RADIUS;
pub const PIXEL_BLOCK: [f32; 6] = crate::theme::ANNOTATION_PIXEL_BLOCK;
pub const BLUR_R: [f32; 6] = crate::theme::ANNOTATION_BLUR_RADIUS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Something on screen changed; draw again.
    Redraw,
    /// The selection moved or changed size: crop the base image again, then draw.
    SelectionChanged,
    /// Move the mouse cursor by this much (arrow keys while selecting).
    Nudge(i32, i32),
    Copy,
    /// Read the selection as text (the frozen pixels, not what was drawn on them).
    CopyText,
    Save,
    SaveAs,
    Pin,
    /// Open the system colour dialog; answer with `set_color`.
    CustomColor,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Selecting,
    Editing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grab {
    Move,
    Handle(usize),
}

#[derive(Clone, Debug, PartialEq)]
enum Drag {
    Select { start: Point },
    Adjust { grab: Grab, start: Point, orig: Rect },
    Draw(Shape),
    Bar(Item),
}

/// What a mouse drag in progress is doing, for the cursor fence (`cursorfence.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragClass {
    /// Choosing a new selection: fenced to the monitor it began on, like a plain snip.
    Select(Point),
    /// Drawing a shape: fenced to the monitors the selection touches (the rest is invisible).
    Draw,
}

/// The mouse pointer that says what a press would do here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorKind {
    Cross,
    Arrow,
    IBeam,
    Move,
    SizeNwse,
    SizeNesw,
    SizeNs,
    SizeWe,
}

fn handle_cursor(h: usize) -> CursorKind {
    match h {
        0 | 2 => CursorKind::SizeNwse,
        1 | 3 => CursorKind::SizeNesw,
        4 | 6 => CursorKind::SizeNs,
        _ => CursorKind::SizeWe,
    }
}

/// The annotated selection, kept between frames so a drag repaints only what the drawn shape
/// touches (a full copy of a 3440x1440 selection on every mouse move is what made drags heavy).
pub struct Frame {
    pub sel: Rect,
    pub img: Canvas,
    rev: u64,
    /// What the preview touched so far, in image coordinates.
    drawn: Vec<IBox>,
    /// Points of the stroke already in `img`.
    stroke_done: usize,
    /// The frame contains a text preview (its bounds are only an estimate): redo it in full.
    has_text: bool,
}

/// What `update_frame` changed in `Frame::img`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameChange {
    None,
    All,
    Parts(Vec<IBox>),
}

fn hull(boxes: &[IBox]) -> Option<IBox> {
    boxes.iter().copied().reduce(|a, b| IBox::new(a.x0.min(b.x0), a.y0.min(b.y0), a.x1.max(b.x1), a.y1.max(b.y1)))
}

/// One upload instead of several when that sends no more pixels: a growing box covers its old self.
fn merged(parts: Vec<IBox>) -> Vec<IBox> {
    let area = |b: &IBox| (b.x1 - b.x0) as i64 * (b.y1 - b.y0) as i64;
    match hull(&parts) {
        Some(h) if parts.len() > 1 && area(&h) <= parts.iter().map(area).sum::<i64>() => vec![h],
        _ => parts,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextEdit {
    pub id: u64,
    pub at: Pt,
    pub text: String,
    pub color: Rgba,
    pub size: f32,
    pub original: Option<usize>,
}

/// What the editor remembers between snips (`editor.toml`, schema 1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Prefs {
    pub schema: u32,
    pub color: String,
    pub size: usize,
    pub blur: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { schema: 1, color: PALETTE[0].to_hex(), size: 1, blur: false }
    }
}

impl Prefs {
    pub fn parse(text: &str) -> Prefs {
        toml::from_str::<Prefs>(text).unwrap_or_default()
    }
    pub fn to_text(&self) -> String {
        toml::to_string(self).unwrap_or_default()
    }
}

pub struct Editor {
    pub mons: Monitors,
    pub windows: Vec<Rect>,
    pub theme: Theme,
    pub phase: Phase,
    pub sel: Option<Rect>,
    pub doc: Doc,
    pub tool: Option<Tool>,
    pub color: Rgba,
    /// The highlighter's colour, for this snip only.
    pub marker_color: Rgba,
    pub size: usize,
    pub obscure: ObscureMode,
    pub text: Option<TextEdit>,
    pub palette_open: bool,
    pub size_open: bool,
    pub cursor: Point,
    /// Language of the tooltips.
    pub lang: Lang,
    /// Text capture is on: the bar and Ctrl+Shift+C offer "copy text".
    pub text_action: bool,
    hover_item: Option<Item>,
    hover_cell: Option<Option<usize>>,
    drag: Option<Drag>,
    /// Bumped on every change to the document (for the composite cache).
    pub rev: u64,
    /// The `rev` at which one new shape went on top of the others (see `update_doc`).
    appended_rev: Option<u64>,
}

/// The selection's frozen pixels with the document's shapes, kept between frames.
pub struct DocImage {
    pub img: Canvas,
    rev: u64,
    sel: Rect,
    shapes: usize,
    /// The shape left out because it is being edited as text.
    skip: Option<usize>,
}

/// Everything the overlay needs to draw one frame, in desktop coordinates.
pub struct View {
    pub sel: Option<Rect>,
    pub handles: Vec<Rect>,
    pub bar: Option<(Point, Canvas)>,
    pub palette: Option<(Point, Canvas)>,
    pub size_menu: Option<(Point, Canvas)>,
    pub label: Option<(Point, Canvas)>,
    pub tooltip: Option<(Point, Canvas)>,
}

impl Editor {
    pub fn new(mons: Monitors, windows: Vec<Rect>, theme: Theme, prefs: &Prefs, cursor: Point) -> Editor {
        Editor {
            mons,
            windows,
            theme,
            phase: Phase::Selecting,
            sel: None,
            doc: Doc::default(),
            tool: Some(Tool::Arrow),
            color: Rgba::from_hex(&prefs.color).unwrap_or(PALETTE[0]),
            marker_color: MARKER_DEFAULT,
            size: prefs.size.min(TEXT.len() - 1),
            obscure: if prefs.blur { ObscureMode::Blur } else { ObscureMode::Pixelate },
            text: None,
            palette_open: false,
            size_open: false,
            cursor,
            lang: Lang::En,
            text_action: false,
            hover_item: None,
            hover_cell: None,
            appended_rev: None,
            drag: None,
            rev: 0,
        }
    }

    pub fn prefs(&self) -> Prefs {
        Prefs { schema: 1, color: self.color.to_hex(), size: self.size, blur: self.obscure == ObscureMode::Blur }
    }

    /// Scale of the monitor that holds the toolbar (the selection's bottom-right corner).
    pub fn scale(&self) -> f32 {
        let p = match self.sel {
            Some(s) => Point::new(s.right - 1, s.bottom - 1),
            None => self.cursor,
        };
        self.mons.at(p).map(|m| m.scale()).unwrap_or(1.0)
    }

    fn dip(&self, v: f32) -> f32 {
        v * self.scale()
    }

    fn bar(&self) -> Option<(Point, Bar)> {
        let sel = self.sel?;
        if self.phase != Phase::Editing {
            return None;
        }
        let bar = Bar::layout(&toolbar::editor_slots_with(self.tool, self.text_action), &self.theme, self.scale());
        let m = self.mons.at(Point::new(sel.right - 1, sel.bottom - 1))?;
        let gap = m.px(crate::theme::EDITOR_BAR_GAP);
        Some((dpi::place_beside(&sel, bar.w, bar.h, m, gap), bar))
    }

    /// Where a toolbar button is, in desktop coordinates.
    pub fn item_center(&self, it: Item) -> Option<(i32, i32)> {
        let (at, b) = self.bar()?;
        let bx = b.button(it)?;
        Some((at.x + bx.x0 + bx.w() / 2, at.y + bx.y0 + bx.h() / 2))
    }

    fn palette(&self) -> Option<(Point, Palette)> {
        if !self.palette_open {
            return None;
        }
        let (at, bar) = self.bar()?;
        let b = bar.button(Item::Color)?;
        let p = Palette::layout(&self.theme, self.scale());
        let m = self.mons.at(at)?;
        let gap = m.px(crate::theme::PALETTE_BAR_GAP);
        let x = (at.x + b.x0).clamp(m.rect.left, (m.rect.right - p.w).max(m.rect.left));
        // Above the bar when there is room, else below it.
        let y = if at.y - gap - p.h >= m.rect.top { at.y - gap - p.h } else { at.y + bar.h + gap };
        Some((Point::new(x, y), p))
    }

    fn handle_at(&self, p: Point) -> Option<usize> {
        let sel = self.sel?;
        let slop = self.dip(crate::theme::HANDLE_HIT_SLOP).round() as i32;
        toolbar::handles(&sel, self.scale()).into_iter().find(|(_, r)| Rect::new(r.left - slop, r.top - slop, r.right + slop, r.bottom + slop).contains(p)).map(|(i, _)| i)
    }

    fn width(&self, tool: Tool) -> f32 {
        let s = self.size.min(STROKE.len() - 1);
        self.dip(match tool {
            Tool::Marker => MARKER[s],
            _ => STROKE[s],
        })
    }

    fn ink(&self, tool: Tool) -> Rgba {
        if tool == Tool::Marker { self.marker_color } else { self.color }
    }

    /// The colour the toolbar shows: the highlighter keeps its own, so a red arrow does not make
    /// the next highlight red over the text.
    fn shown_color(&self) -> Rgba {
        self.ink(self.tool.unwrap_or(Tool::Arrow))
    }

    fn start_shape(&self, tool: Tool, p: Pt) -> Option<Shape> {
        let (color, width) = (self.ink(tool), self.width(tool));
        Some(match tool {
            Tool::Arrow => Shape::Arrow { a: p, b: p, color, width },
            Tool::Rect => Shape::Rect { a: p, b: p, color, width },
            Tool::Line => Shape::Line { a: p, b: p, color, width },
            Tool::Pen => Shape::Pen { pts: vec![p], color, width },
            Tool::Marker => Shape::Marker { pts: vec![p], color, width },
            Tool::Obscure => Shape::Obscure {
                a: p,
                b: p,
                mode: self.obscure,
                strength: self.dip(if self.obscure == ObscureMode::Pixelate { PIXEL_BLOCK[self.size] } else { BLUR_R[self.size] }),
            },
            Tool::Text | Tool::Steps => return None,
        })
    }

    fn changed(&mut self) {
        self.rev += 1;
    }

    /// Commit a single add/replace/remove undo operation. Starting an edit never mutates Doc.
    pub fn commit_text(&mut self) -> bool {
        let Some(t) = self.text.take() else {
            return false;
        };
        let empty = t.text.trim().is_empty();
        let shape = Shape::Text {
            at: t.at,
            text: t.text,
            color: t.color,
            size: t.size,
        };
        let mut added = false;
        match t.original {
            Some(index) if empty => {
                self.doc.remove(index);
            }
            Some(index) => {
                self.doc.replace(index, shape);
            }
            None if !empty => {
                added = self.doc.add(shape);
            }
            None => {}
        }
        self.changed();
        if added {
            self.appended_rev = Some(self.rev);
        }
        true
    }

    pub fn cancel_text(&mut self) -> bool {
        if self.text.take().is_none() {
            return false;
        }
        self.changed(); // reveal the unmodified original in the document cache
        true
    }

    /// Native edit controls send their whole Unicode buffer, not individual UTF-16 halves.
    pub fn set_text(&mut self, value: &str) {
        if let Some(t) = &mut self.text {
            t.text = value
                .replace("\r\n", "\n")
                .replace('\r', "\n")
                .chars()
                .take(2000)
                .collect();
        }
    }

    pub fn set_size(&mut self, index: usize) -> Effect {
        self.size = index.min(TEXT.len() - 1);
        let px = self.dip(TEXT[self.size]);
        if let Some(t) = &mut self.text {
            t.size = px;
        }
        Effect::Redraw
    }

    pub fn size_values(&self) -> &'static [f32] {
        match self.tool {
            Some(Tool::Text) => &TEXT,
            Some(Tool::Marker) => &MARKER,
            Some(Tool::Steps) => &STEP_R,
            Some(Tool::Obscure) if self.obscure == ObscureMode::Blur => &BLUR_R,
            Some(Tool::Obscure) => &PIXEL_BLOCK,
            _ => &STROKE,
        }
    }

    pub fn size_menu(&self) -> Option<(Point, SizeMenu)> {
        if !self.size_open {
            return None;
        }
        let (at, bar) = self.bar()?;
        let button = bar.button(Item::Size)?;
        let menu = SizeMenu::layout(&self.theme, self.scale(), self.size_values().len());
        let monitor = self.mons.at(at)?;
        let gap = monitor.px(crate::theme::PALETTE_BAR_GAP);
        let x = (at.x + button.x0).clamp(
            monitor.rect.left,
            (monitor.rect.right - menu.w).max(monitor.rect.left),
        );
        let y = (at.y - gap - menu.h).clamp(
            monitor.rect.top,
            (monitor.rect.bottom - menu.h).max(monitor.rect.top),
        );
        Some((Point::new(x, y), menu))
    }

    pub fn set_color(&mut self, c: Rgba) -> Effect {
        if self.tool == Some(Tool::Marker) {
            // White or black multiply to nothing useful: such a pick falls back to yellow.
            let c = c.with_alpha(255);
            self.marker_color = if c == crate::theme::BADGE_LIGHT_TEXT || c.luminance() < 0.02 { MARKER_DEFAULT } else { c };
            self.palette_open = false;
            return Effect::Redraw;
        }
        self.color = c.with_alpha(255);
        if let Some(t) = &mut self.text {
            t.color = self.color;
        }
        self.palette_open = false;
        Effect::Redraw
    }

    /// Finish any text and return to selection adjustment after a right-click.
    pub fn drop_tool(&mut self) -> Effect {
        self.commit_text();
        self.tool = None;
        self.drag = None;
        self.palette_open = false;
        self.size_open = false;
        Effect::Redraw
    }

    fn press_item(&mut self, it: Item) -> Effect {
        match it {
            Item::Tool(t) => {
                self.commit_text();
                self.palette_open = false;
                self.size_open = false;
                self.tool = if self.tool == Some(t) { None } else { Some(t) };
                Effect::Redraw
            }
            Item::Color => {
                self.palette_open = !self.palette_open;
                self.size_open = false;
                Effect::Redraw
            }
            Item::ObscureMode => {
                self.obscure = if self.obscure == ObscureMode::Pixelate { ObscureMode::Blur } else { ObscureMode::Pixelate };
                Effect::Redraw
            }
            Item::Size => {
                self.size_open = !self.size_open;
                self.palette_open = false;
                Effect::Redraw
            }
            Item::Undo => self.undo(),
            Item::Redo => self.redo(),
            Item::Pin => self.finish(Effect::Pin),
            Item::Save => self.finish(Effect::Save),
            Item::Copy => self.finish(Effect::Copy),
            Item::CopyText => self.finish(Effect::CopyText),
            Item::Close => Effect::Close,
            Item::ModeRegion | Item::ModeWindow | Item::ModeScreen | Item::ModeText => Effect::None,
        }
    }

    fn finish(&mut self, e: Effect) -> Effect {
        self.commit_text();
        if self.sel.is_some() { e } else { Effect::None }
    }

    fn undo(&mut self) -> Effect {
        if self.cancel_text() {
            return Effect::Redraw;
        }
        if self.doc.undo() {
            self.changed();
            Effect::Redraw
        } else { Effect::None }
    }

    fn redo(&mut self) -> Effect {
        if self.doc.redo() {
            self.changed();
            Effect::Redraw
        } else {
            Effect::None
        }
    }

    /// The fenced kind of the drag in progress. Moving or resizing the selection, and the toolbar,
    /// are not fenced: moving a selection onto another monitor is a real use.
    pub fn drag_class(&self) -> Option<DragClass> {
        match &self.drag {
            Some(Drag::Select { start }) => Some(DragClass::Select(*start)),
            Some(Drag::Draw(_)) => Some(DragClass::Draw),
            _ => None,
        }
    }

    pub fn on_down(&mut self, p: Point, shift: bool) -> Effect {
        self.on_down_mods(p, shift, false, &crate::raster::BoxFont)
    }

    /// `p` is on a toolbar button other than `popup_button`.
    fn on_other_button(&self, p: Point, popup_button: Item) -> bool {
        self.bar().and_then(|(at, b)| b.hit(p.x - at.x, p.y - at.y)).is_some_and(|it| it != popup_button)
    }

    pub fn on_down_mods(
        &mut self,
        p: Point,
        _shift: bool,
        ctrl: bool,
        glyphs: &dyn Glyphs,
    ) -> Effect {
        self.cursor = p;
        if self.phase == Phase::Selecting {
            self.drag = Some(Drag::Select { start: p });
            return Effect::Redraw;
        }
        if let Some((at, menu)) = self.size_menu() {
            self.size_open = false;
            if let Some(index) = menu.hit(p.x - at.x, p.y - at.y) {
                return self.set_size(index);
            }
            // On the canvas the click that closes the popup does nothing else. On another toolbar
            // button it also presses that button; on the Size button itself it only closes.
            if !self.on_other_button(p, Item::Size) {
                return Effect::Redraw;
            }
        }
        if let Some((at, pal)) = self.palette() {
            match pal.hit(p.x - at.x, p.y - at.y) {
                Some(Some(i)) => return self.set_color(PALETTE[i]),
                Some(None) => {
                    self.palette_open = false;
                    return Effect::CustomColor;
                }
                None => {
                    self.palette_open = false;
                    if !self.on_other_button(p, Item::Color) {
                        return Effect::Redraw;
                    }
                }
            }
        }
        if let Some((at, bar)) = self.bar()
            && let Some(it) = bar.hit(p.x - at.x, p.y - at.y)
        {
            self.drag = Some(Drag::Bar(it));
            return Effect::Redraw;
        }
        let sel = self.sel.unwrap_or_default();
        // Ctrl has precedence even on an edge/corner and while a drawing tool is active.
        if ctrl && (sel.contains(p) || self.handle_at(p).is_some()) {
            self.commit_text();
            self.drag = Some(Drag::Adjust { grab: Grab::Move, start: p, orig: sel });
            return Effect::Redraw;
        }
        self.commit_text();
        let pf = Pt::new(p.x as f32 + 0.5, p.y as f32 + 0.5);
        match self.tool {
            None => {
                if let Some(h) = self.handle_at(p) {
                    self.drag = Some(Drag::Adjust { grab: Grab::Handle(h), start: p, orig: sel });
                } else if !sel.contains(p) {
                    self.drag = Some(Drag::Select { start: p });
                }
                Effect::Redraw
            }
            Some(Tool::Text) => {
                let hit = self
                    .doc
                    .shapes()
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(i, shape)| {
                        annotate::text_bounds(shape, glyphs)
                            .filter(|r| r.contains(p.x, p.y))
                            .map(|_| i)
                    });
                if let Some(index) = hit {
                    if let Shape::Text {
                        at,
                        text,
                        color,
                        size,
                    } = self.doc.shapes()[index].clone()
                    {
                        self.color = color;
                        self.size = TEXT
                            .iter()
                            .enumerate()
                            .min_by(|(_, a), (_, b)| {
                                (self.dip(**a) - size)
                                    .abs()
                                    .total_cmp(&(self.dip(**b) - size).abs())
                            })
                            .map(|(i, _)| i)
                            .unwrap_or(1);
                        self.text = Some(TextEdit {
                            id: self.rev + 1,
                            at,
                            text,
                            color,
                            size,
                            original: Some(index),
                        });
                    }
                } else if sel.contains(p) {
                    self.text = Some(TextEdit {
                        id: self.rev + 1,
                        at: Pt::new(p.x as f32, p.y as f32),
                        text: String::new(),
                        color: self.color,
                        size: self.dip(TEXT[self.size]),
                        original: None,
                    });
                }
                self.changed();
                Effect::Redraw
            }
            Some(Tool::Steps) => {
                // A badge outside the selection would be invisible yet take a number.
                if !sel.contains(p) {
                    return Effect::Redraw;
                }
                let r = self.dip(STEP_R[self.size]);
                let added = self.doc.add(Shape::Step { at: pf, n: self.doc.next_step(), color: self.color, r });
                self.changed();
                if added {
                    self.appended_rev = Some(self.rev);
                }
                Effect::Redraw
            }
            Some(t) => {
                self.drag = self.start_shape(t, pf).map(Drag::Draw);
                Effect::Redraw
            }
        }
    }

    pub fn on_move(&mut self, p: Point, shift: bool) -> Effect {
        self.cursor = p;
        let bounds = self.mons.layout().bounds();
        match self.drag.clone() {
            Some(Drag::Select { .. }) => Effect::Redraw,
            Some(Drag::Adjust { grab, start, orig }) => {
                let (dx, dy) = (p.x - start.x, p.y - start.y);
                let r = match grab {
                    Grab::Move => {
                        let dx = dx.clamp(bounds.left - orig.left, bounds.right - orig.right);
                        let dy = dy.clamp(bounds.top - orig.top, bounds.bottom - orig.bottom);
                        orig.offset(dx, dy)
                    }
                    Grab::Handle(h) => resize(orig, h, dx, dy),
                };
                let r = self.mons.layout().clip(&r).unwrap_or(orig);
                if Some(r) != self.sel && !r.is_empty() {
                    self.sel = Some(r);
                    Effect::SelectionChanged
                } else {
                    Effect::None
                }
            }
            Some(Drag::Draw(mut s)) => {
                let pf = Pt::new(p.x as f32 + 0.5, p.y as f32 + 0.5);
                match &mut s {
                    Shape::Arrow { a, b, .. } | Shape::Line { a, b, .. } => *b = if shift { annotate::snap45(*a, pf) } else { pf },
                    Shape::Rect { a, b, .. } => *b = if shift { annotate::square(*a, pf) } else { pf },
                    Shape::Obscure { b, .. } => *b = pf,
                    Shape::Pen { pts, .. } | Shape::Marker { pts, .. } if pts.last().is_none_or(|l| l.dist(pf) >= 1.5) => pts.push(pf),
                    _ => {}
                }
                self.drag = Some(Drag::Draw(s));
                Effect::Redraw
            }
            Some(Drag::Bar(_)) => Effect::None,
            None => self.update_hover(p),
        }
    }

    fn update_hover(&mut self, p: Point) -> Effect {
        let item = self.bar().and_then(|(at, b)| b.hit(p.x - at.x, p.y - at.y));
        let cell = self.palette().and_then(|(at, pal)| pal.hit(p.x - at.x, p.y - at.y));
        let selecting_label = self.phase == Phase::Selecting;
        let hover_window = if selecting_label { window_at(&self.windows, p) } else { None };
        let changed = item != self.hover_item || cell != self.hover_cell;
        self.hover_item = item;
        self.hover_cell = cell;
        if changed || hover_window.is_some() || selecting_label { Effect::Redraw } else { Effect::None }
    }

    pub fn on_up(&mut self, p: Point, shift: bool) -> Effect {
        let e = self.on_move(p, shift);
        match self.drag.take() {
            Some(Drag::Select { start }) => {
                if !is_click(start, p) {
                    let r = crate::geometry::Rect::from_drag(start, p);
                    let r = Rect::new(r.left, r.top, r.right + 1, r.bottom + 1);
                    self.sel = self.mons.layout().clip(&r);
                } else if self.phase == Phase::Selecting {
                    // A click picks the window under the cursor.
                    self.sel = window_at(&self.windows, p).and_then(|i| self.mons.layout().clip(&self.windows[i]));
                }
                if self.sel.is_some() && self.phase == Phase::Selecting {
                    self.phase = Phase::Editing;
                }
                if self.sel.is_some() { Effect::SelectionChanged } else { Effect::Redraw }
            }
            Some(Drag::Adjust { .. }) => Effect::SelectionChanged,
            Some(Drag::Draw(s)) => {
                // Started and ended outside: nothing would show, but undo would need an extra press.
                let inside = self.sel.is_some_and(|r| !s.bounds().intersect(&IBox::new(r.left, r.top, r.right, r.bottom)).is_empty());
                if inside && self.doc.add(s) {
                    self.changed();
                    self.appended_rev = Some(self.rev);
                }
                Effect::Redraw
            }
            Some(Drag::Bar(it)) => {
                let at_up = self.bar().and_then(|(at, b)| b.hit(p.x - at.x, p.y - at.y));
                if at_up == Some(it) { self.press_item(it) } else { Effect::Redraw }
            }
            None => e,
        }
    }

    /// Mouse wheel over the overlay: change the size (positive delta = larger).
    pub fn on_wheel(&mut self, delta: i32) -> Effect {
        if delta == 0 {
            return Effect::None;
        }
        let before = self.size;
        let next = if delta > 0 {
            (self.size + 1).min(TEXT.len() - 1)
        } else { self.size.saturating_sub(1) };
        self.set_size(next);
        if self.size != before { Effect::Redraw } else { Effect::None }
    }

    /// A key-down. Letters come here as their virtual-key code (A = 0x41); typed text comes
    /// through `on_char`.
    pub fn on_key(&mut self, vk: u32, ctrl: bool, shift: bool) -> Effect {
        if vk == VK_ESCAPE && (self.size_open || self.palette_open) {
            self.size_open = false;
            self.palette_open = false;
            return Effect::Redraw;
        }
        if self.text.is_some() {
            return match vk {
                VK_ESCAPE => {
                    self.cancel_text();
                    Effect::Redraw
                }
                VK_RETURN if !shift => {
                    self.commit_text();
                    Effect::Redraw
                }
                VK_RETURN => self.type_char('\n'),
                VK_BACK => {
                    if let Some(t) = &mut self.text {
                        t.text.pop();
                    }
                    Effect::Redraw
                }
                0x5A if ctrl => self.undo(),
                _ => Effect::None,
            };
        }
        if self.phase == Phase::Selecting {
            return match vk {
                VK_ESCAPE => Effect::Close,
                VK_RETURN => {
                    let r = match self.drag.take() {
                        Some(Drag::Select { start }) if !is_click(start, self.cursor) => {
                            let r = Rect::from_drag(start, self.cursor);
                            self.mons.layout().clip(&Rect::new(r.left, r.top, r.right + 1, r.bottom + 1))
                        }
                        _ => self.mons.at(self.cursor).map(|m| m.rect),
                    };
                    self.sel = r;
                    if r.is_some() {
                        self.phase = Phase::Editing;
                        Effect::SelectionChanged
                    } else {
                        Effect::None
                    }
                }
                VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN => {
                    let (dx, dy) = arrow(vk, shift);
                    Effect::Nudge(dx, dy)
                }
                _ => Effect::None,
            };
        }
        match vk {
            VK_ESCAPE => {
                if self.size_open {
                    self.size_open = false;
                    Effect::Redraw
                } else if self.palette_open {
                    self.palette_open = false;
                    Effect::Redraw
                } else if self.drag.take().is_some() {
                    Effect::Redraw
                } else {
                    Effect::Close
                }
            }
            VK_RETURN => self.finish(Effect::Copy),
            0x5A if ctrl && shift => self.redo(),
            0x5A if ctrl => self.undo(),
            0x59 if ctrl => self.redo(),
            0x43 if ctrl && shift && self.text_action => self.finish(Effect::CopyText),
            0x43 if ctrl => self.finish(Effect::Copy),
            0x53 if ctrl && shift => self.finish(Effect::SaveAs),
            0x53 if ctrl => self.finish(Effect::Save),
            0x50 if ctrl => self.finish(Effect::Pin),
            0x31..=0x33 if !ctrl => self.set_size((vk - 0x31) as usize),
            0x41..=0x5A if !ctrl => match Tool::from_key(char::from_u32(vk).unwrap_or(' ')) {
                Some(t) => self.press_item(Item::Tool(t)),
                None => Effect::None,
            },
            _ => Effect::None,
        }
    }

    pub fn on_char(&mut self, c: char) -> Effect {
        if self.text.is_none() || c.is_control() {
            return Effect::None;
        }
        self.type_char(c)
    }

    fn type_char(&mut self, c: char) -> Effect {
        if let Some(t) = &mut self.text
            && t.text.chars().count() < 2000
        {
            t.text.push(c);
        }
        Effect::Redraw
    }

    /// The selection image with every annotation: `base` is the frozen screen under the selection.
    /// `preview` adds the shape being dragged, the text being typed and its caret.
    pub fn compose(&self, base: &Canvas, g: &dyn Glyphs, preview: bool) -> Canvas {
        let mut img = self.compose_doc(base, g);
        if preview {
            self.add_preview(&mut img, g);
        }
        img
    }

    /// `base` with the document's shapes (no preview parts). Cache this per `rev`.
    pub fn compose_doc(&self, base: &Canvas, g: &dyn Glyphs) -> Canvas {
        self.compose_doc_on(base.clone(), g)
    }

    /// `compose_doc` that draws on the given pixels instead of copying them.
    fn compose_doc_on(&self, mut img: Canvas, g: &dyn Glyphs) -> Canvas {
        if let Some(sel) = self.sel {
            for (index, shape) in self.doc.shapes().iter().enumerate() {
                if self.text.as_ref().and_then(|t| t.original) != Some(index) {
                    annotate::render(
                        &mut img,
                        (sel.left, sel.top),
                        std::slice::from_ref(shape),
                        g,
                    );
                }
            }
        }
        img
    }

    /// Bring `cache` up to date with the document. One new shape on top is drawn on the image it
    /// has; anything else (undo, a text edit, a moved selection) starts over from the frozen pixels,
    /// which `base` crops for the given selection. False: no selection, or `base` failed.
    pub fn update_doc(&self, cache: &mut Option<DocImage>, g: &dyn Glyphs, base: &mut dyn FnMut(Rect) -> Option<Canvas>) -> bool {
        let Some(sel) = self.sel else {
            *cache = None;
            return false;
        };
        let skip = self.text.as_ref().and_then(|t| t.original);
        let shapes = self.doc.shapes();
        if let Some(c) = cache
            && c.sel == sel
            && c.skip == skip
        {
            if c.rev == self.rev {
                return true;
            }
            if skip.is_none() && self.appended_rev == Some(self.rev) && c.rev + 1 == self.rev && c.shapes + 1 == shapes.len() {
                annotate::render(&mut c.img, (sel.left, sel.top), &shapes[c.shapes..], g);
                c.rev = self.rev;
                c.shapes = shapes.len();
                return true;
            }
        }
        *cache = None; // freed before the new copy exists
        let Some(frozen) = base(sel) else { return false };
        *cache = Some(DocImage { img: self.compose_doc_on(frozen, g), rev: self.rev, sel, shapes: shapes.len(), skip });
        true
    }

    /// Draw the parts that are not in the document yet onto a `compose_doc` image.
    pub fn add_preview(&self, img: &mut Canvas, g: &dyn Glyphs) {
        self.preview(img, g, true);
    }

    pub fn preview(&self, img: &mut Canvas, g: &dyn Glyphs, draw_text: bool) {
        let Some(sel) = self.sel else { return };
        let mut extra = Vec::new();
        if let Some(Drag::Draw(s)) = &self.drag {
            extra.push(s.clone());
        }
        if draw_text && let Some(t) = &self.text {
            extra.push(Shape::Text {
                at: t.at,
                text: t.text.clone(),
                color: t.color,
                size: t.size,
            });
        }
        annotate::render(img, (sel.left, sel.top), &extra, g);
        if draw_text && let Some(t) = &self.text {
            // Caret after the last line, 1 DIP wide.
            let size = t.size;
            let last = t.text.rsplit('\n').next().unwrap_or("");
            let lines = t.text.split('\n').count() as f32 - 1.0;
            let w = if last.is_empty() { 0 } else { g.text(last, size, false).w as i32 };
            let x = (t.at.x.round() as i32 - sel.left) + w + crate::theme::ANNOTATION_CARET_GAP;
            let y = (t.at.y + lines * g.line_height(size)).round() as i32 - sel.top;
            let cw = self.dip(crate::theme::BORDER_WIDTH).round().max(1.0) as i32;
            img.fill_box(IBox::xywh(x, y, cw, (size * crate::theme::ANNOTATION_CARET_HEIGHT).round() as i32), self.color);
        }
    }

    /// True when the picture under the selection differs from the frozen frame. While false the
    /// overlay shows the frozen texture itself: no readback, no copies.
    pub fn needs_comp(&self) -> bool {
        !self.doc.is_empty() || self.text.is_some() || matches!(self.drag, Some(Drag::Draw(_)))
    }

    /// The selection is being moved or resized: its pixels are cropped again on every move.
    pub fn is_adjusting(&self) -> bool {
        matches!(self.drag, Some(Drag::Adjust { .. }))
    }

    /// A drawing drag is running: the toolbar and the labels cannot change.
    pub fn is_drawing(&self) -> bool {
        matches!(self.drag, Some(Drag::Draw(_)))
    }

    /// Boxes the shape being dragged touches (from point `from` for a stroke), in image coordinates.
    fn preview_cover(&self, sel: Rect, img: &Canvas, from: usize) -> Vec<IBox> {
        let Some(Drag::Draw(s)) = &self.drag else { return Vec::new() };
        s.cover(from)
            .into_iter()
            .map(|b| IBox::new(b.x0 - sel.left, b.y0 - sel.top, b.x1 - sel.left, b.y1 - sel.top).intersect(&img.bounds()))
            .filter(|b| !b.is_empty())
            .collect()
    }

    /// Points of the stroke being dragged (None: not a stroke).
    fn stroke_len(&self) -> Option<usize> {
        match &self.drag {
            Some(Drag::Draw(Shape::Pen { pts, .. } | Shape::Marker { pts, .. })) => Some(pts.len()),
            _ => None,
        }
    }

    /// Bring `frame` up to date with the document and the preview, redoing only what changed.
    /// `doc` is the current `compose_doc` image of the selection.
    pub fn update_frame(&self, frame: &mut Option<Frame>, doc: &Canvas, g: &dyn Glyphs, draw_text: bool) -> FrameChange {
        let Some(sel) = self.sel else {
            *frame = None;
            return FrameChange::None;
        };
        let with_text = draw_text && self.text.is_some();
        if let Some(f) = frame
            && f.sel == sel
            && f.rev == self.rev
            && !f.has_text
            && !with_text
            && f.img.w == doc.w
            && f.img.h == doc.h
        {
            let stroke = self.stroke_len();
            let new = self.preview_cover(sel, &f.img, if stroke.is_some() { f.stroke_done } else { 0 });
            let mut parts = Vec::with_capacity(new.len() + f.drawn.len());
            // A stroke only grows; everything else is taken back and drawn at its new place.
            if stroke.is_none() {
                for b in f.drawn.drain(..) {
                    f.img.copy_box_from(doc, b);
                    parts.push(b);
                }
                f.stroke_done = 0;
            }
            if let Some(Drag::Draw(s)) = &self.drag {
                // Each box is rebuilt from the document, so overlapping boxes cannot blend twice.
                for b in &new {
                    f.img.copy_box_from(doc, *b);
                    annotate::render_in(&mut f.img, (sel.left, sel.top), s, *b, g);
                    parts.push(*b);
                }
            }
            match stroke {
                Some(n) => {
                    f.stroke_done = n;
                    // One box is enough to take a cancelled stroke back.
                    f.drawn = hull(&[f.drawn.as_slice(), new.as_slice()].concat()).into_iter().collect();
                }
                None => f.drawn = new,
            }
            return if parts.is_empty() { FrameChange::None } else { FrameChange::Parts(merged(parts)) };
        }
        // One new shape went on top: only its pixels and the old preview's differ from the document.
        if let Some(f) = frame
            && f.sel == sel
            && f.rev + 1 == self.rev
            && self.appended_rev == Some(self.rev)
            && self.drag.is_none()
            && !f.has_text
            && !with_text
            && f.img.w == doc.w
            && f.img.h == doc.h
            && let Some(shape) = self.doc.shapes().last()
            && !matches!(shape, Shape::Text { .. }) // its bounds are only an estimate
        {
            let bounds = f.img.bounds();
            let mut parts = std::mem::take(&mut f.drawn);
            parts.extend(shape.cover(0).into_iter().map(|b| IBox::new(b.x0 - sel.left, b.y0 - sel.top, b.x1 - sel.left, b.y1 - sel.top).intersect(&bounds)).filter(|b| !b.is_empty()));
            for b in &parts {
                f.img.copy_box_from(doc, *b);
            }
            f.rev = self.rev;
            f.stroke_done = 0;
            return if parts.is_empty() { FrameChange::None } else { FrameChange::Parts(merged(parts)) };
        }
        // The old frame's memory holds the new one: a big selection has no third copy, even briefly.
        let reuse = frame.take().filter(|f| f.img.w == doc.w && f.img.h == doc.h);
        let mut img = match reuse {
            Some(f) => {
                let mut img = f.img;
                img.px.copy_from_slice(&doc.px);
                img
            }
            None => doc.clone(),
        };
        self.preview(&mut img, g, draw_text);
        let stroke = self.stroke_len();
        let cover = self.preview_cover(sel, &img, 0);
        let drawn = if stroke.is_some() { hull(&cover).into_iter().collect() } else { cover };
        *frame = Some(Frame { sel, img, rev: self.rev, drawn, stroke_done: stroke.unwrap_or(0), has_text: with_text });
        FrameChange::All
    }

    pub fn cursor_kind(&self, p: Point, ctrl: bool) -> CursorKind {
        if self.phase == Phase::Selecting {
            return CursorKind::Cross;
        }
        match &self.drag {
            Some(Drag::Adjust { grab: Grab::Move, .. }) => return CursorKind::Move,
            Some(Drag::Adjust { grab: Grab::Handle(h), .. }) => return handle_cursor(*h),
            Some(Drag::Draw(_) | Drag::Select { .. }) => return CursorKind::Cross,
            _ => {}
        }
        let on_ui = self.size_menu().is_some_and(|(at, m)| m.hit(p.x - at.x, p.y - at.y).is_some())
            || self.palette().is_some_and(|(at, pal)| pal.hit(p.x - at.x, p.y - at.y).is_some())
            || self.bar().is_some_and(|(at, b)| b.hit(p.x - at.x, p.y - at.y).is_some());
        if on_ui {
            return CursorKind::Arrow;
        }
        let sel = self.sel.unwrap_or_default();
        if ctrl && (sel.contains(p) || self.handle_at(p).is_some()) {
            return CursorKind::Move;
        }
        match self.tool {
            None => match self.handle_at(p) {
                Some(h) => handle_cursor(h),
                None if sel.contains(p) => CursorKind::Arrow,
                None => CursorKind::Cross,
            },
            Some(Tool::Text) if sel.contains(p) => CursorKind::IBeam,
            Some(Tool::Text | Tool::Steps) if !sel.contains(p) => CursorKind::Arrow,
            Some(_) => CursorKind::Cross,
        }
    }

    /// The rectangle the overlay shows undimmed right now (a drag in progress, the window under
    /// the cursor while selecting, or the selection).
    pub fn shown_sel(&self) -> Option<Rect> {
        match (&self.drag, self.phase) {
            (Some(Drag::Select { start }), _) if !is_click(*start, self.cursor) => {
                let r = Rect::from_drag(*start, self.cursor);
                Some(Rect::new(r.left, r.top, r.right + 1, r.bottom + 1))
            }
            (_, Phase::Selecting) => window_at(&self.windows, self.cursor).and_then(|i| self.mons.layout().clip(&self.windows[i])),
            _ => self.sel,
        }
    }

    pub fn handle_rects(&self) -> Vec<Rect> {
        if self.phase != Phase::Editing || self.tool.is_some() || matches!(self.drag, Some(Drag::Select { .. })) {
            return Vec::new();
        }
        self.sel.map(|s| toolbar::handles(&s, self.scale()).into_iter().map(|(_, r)| r).collect()).unwrap_or_default()
    }

    pub fn view(&self, g: &dyn Glyphs) -> View {
        let current = self.shown_sel();
        let sc = self.scale();
        let handles = self.handle_rects();
        let bar = self.bar().map(|(at, b)| {
            let mut active = Vec::new();
            if let Some(t) = self.tool {
                active.push(Item::Tool(t));
            }
            if self.palette_open {
                active.push(Item::Color);
            }
            if self.size_open {
                active.push(Item::Size);
            }
            let mut disabled = Vec::new();
            if !self.doc.can_undo() && self.text.is_none() {
                disabled.push(Item::Undo);
            }
            if !self.doc.can_redo() {
                disabled.push(Item::Redo);
            }
            let pressed = if let Some(Drag::Bar(it)) = self.drag { Some(it) } else { None };
            let st = BarState { active, disabled, hover: self.hover_item, pressed, color: self.shown_color(), size: self.size, obscure: Some(self.obscure) };
            (at, toolbar::render_bar(&b, &self.theme, &st))
        });
        let palette = self.palette().map(|(at, p)| {
            (at, toolbar::render_palette(&p, &self.theme, self.shown_color(), self.hover_cell))
        });
        let size_menu = self.size_menu().map(|(at, menu)| {
            (
                at,
                toolbar::render_sizes(
                    &menu,
                    &self.theme,
                    self.lang,
                    self.size_values(),
                    self.size,
                    self.tool == Some(Tool::Text),
                    g,
                ),
            )
        });
        let label = current.map(|r| {
            let cv = toolbar::render_label(&format!("{} \u{00D7} {}", r.width(), r.height()), &self.theme, sc, g);
            let gap = (crate::theme::SELECTION_LABEL_GAP * sc).round() as i32;
            let m = self.mons.at(Point::new(r.left, r.top)).map(|m| m.rect).unwrap_or(r);
            let y = if r.top - gap - cv.h as i32 >= m.top { r.top - gap - cv.h as i32 } else { r.top + gap };
            let x = r.left.max(m.left).min(m.right - cv.w as i32);
            (Point::new(x, y), cv)
        });
        let tooltip = self
            .bar()
            .zip(self.hover_item)
            .filter(|_| self.drag.is_none() && !self.palette_open && !self.size_open)
            .and_then(|((at, b), it)| {
            let bx = b.button(it)?;
            let cv = toolbar::render_tooltip(&it.tooltip(self.lang, self.obscure), &self.theme, sc, g);
            let m = self.mons.at(at)?.rect;
            let gap = (crate::theme::SELECTION_LABEL_GAP * sc).round() as i32;
            let x = (at.x + bx.x0 + bx.w() / 2 - cv.w as i32 / 2).clamp(m.left, (m.right - cv.w as i32).max(m.left));
            let above = at.y - gap - cv.h as i32;
            let y = if above >= m.top { above } else { at.y + b.h + gap };
            Some((Point::new(x, y), cv))
        });
        View {
            sel: current,
            handles,
            bar,
            palette,
            size_menu,
            label,
            tooltip,
        }
    }

    pub fn tooltip_item(&self) -> Option<Item> {
        self.hover_item
    }

    pub fn is_typing(&self) -> bool {
        self.text.is_some()
    }
}

fn arrow(vk: u32, shift: bool) -> (i32, i32) {
    let s = if shift { 10 } else { 1 };
    match vk {
        VK_LEFT => (-s, 0),
        VK_RIGHT => (s, 0),
        VK_UP => (0, -s),
        _ => (0, s),
    }
}

/// Move the edges that handle `h` owns; a drag past the opposite edge flips the box.
pub fn resize(r: Rect, h: usize, dx: i32, dy: i32) -> Rect {
    let (mut l, mut t, mut rr, mut b) = (r.left, r.top, r.right, r.bottom);
    match h {
        0 => {
            l += dx;
            t += dy;
        }
        1 => {
            rr += dx;
            t += dy;
        }
        2 => {
            rr += dx;
            b += dy;
        }
        3 => {
            l += dx;
            b += dy;
        }
        4 => t += dy,
        5 => rr += dx,
        6 => b += dy,
        _ => l += dx,
    }
    Rect::new(l.min(rr), t.min(b), l.max(rr).max(l.min(rr) + 1), t.max(b).max(t.min(b) + 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dpi::{Monitor, mixed_example};
    use crate::raster::BoxFont;
    use crate::theme::{Look, Mode};

    fn ed() -> Editor {
        Editor::new(mixed_example(), vec![Rect::new(100, 100, 500, 400)], Theme::new(Look::Graphite, Mode::Light), &Prefs::default(), Point::new(10, 10))
    }

    fn drag(e: &mut Editor, a: (i32, i32), b: (i32, i32)) -> Effect {
        e.on_down(Point::new(a.0, a.1), false);
        e.on_move(Point::new((a.0 + b.0) / 2, (a.1 + b.1) / 2), false);
        e.on_up(Point::new(b.0, b.1), false)
    }

    fn selected(a: (i32, i32), b: (i32, i32)) -> Editor {
        let mut e = ed();
        assert_eq!(drag(&mut e, a, b), Effect::SelectionChanged);
        e.tool = None; // most fixtures exercise the explicit no-tool resize mode
        e
    }

    fn click_item(e: &mut Editor, it: Item) -> Effect {
        let (at, bar) = e.bar().expect("toolbar");
        let b = bar.button(it).unwrap_or_else(|| panic!("{it:?} not on the bar"));
        let p = Point::new(at.x + b.x0 + 2, at.y + b.y0 + 2);
        e.on_down(p, false);
        e.on_up(p, false)
    }

    #[test]
    fn drag_selects_inclusively_then_edits() {
        let e = selected((200, 200), (299, 249));
        assert_eq!(e.phase, Phase::Editing);
        assert_eq!(e.sel, Some(Rect::new(200, 200, 300, 250)));
    }

    #[test]
    fn click_takes_the_window_and_enter_the_monitor() {
        let mut e = ed();
        e.on_down(Point::new(150, 150), false);
        assert_eq!(e.on_up(Point::new(151, 150), false), Effect::SelectionChanged);
        assert_eq!(e.sel, Some(Rect::new(100, 100, 500, 400)));
        let mut f = ed();
        f.cursor = Point::new(-10, 10);
        assert_eq!(f.on_key(VK_RETURN, false, false), Effect::SelectionChanged);
        assert_eq!(f.sel, Some(Rect::from_xywh(-3840, -400, 3840, 2160)), "the 150 % monitor at negative x");
        let mut g = ed();
        assert_eq!(g.on_key(VK_ESCAPE, false, false), Effect::Close);
        assert_eq!(g.on_key(VK_LEFT, false, true), Effect::Nudge(-10, 0));
    }

    #[test]
    fn selection_across_monitors_with_different_scales() {
        // From the 150 % monitor (x < 0) into the 100 % primary.
        let e = selected((-300, 100), (199, 299));
        assert_eq!(e.sel, Some(Rect::new(-300, 100, 200, 300)));
        // The toolbar follows the bottom-right corner: on the primary, 100 %.
        assert_eq!(e.scale(), 1.0);
        let f = selected((-600, 100), (-101, 299));
        assert_eq!(f.scale(), 1.5, "fully on the 150 % monitor");
        let (_, bar_a) = e.bar().unwrap();
        let (_, bar_b) = f.bar().unwrap();
        assert_eq!(bar_a.h, 40);
        assert_eq!(bar_b.h, 60);
    }

    #[test]
    fn toolbar_stays_on_the_monitor() {
        // Bottom-right corner of the primary: no room below or right.
        let e = selected((2000, 1000), (2559, 1439));
        let (at, bar) = e.bar().unwrap();
        assert!(at.x >= 0 && at.x + bar.w <= 2560, "{at:?}");
        assert!(at.y >= 0 && at.y + bar.h <= 1440, "{at:?}");
        assert!(at.y + bar.h <= 1000, "above the selection");
    }

    #[test]
    fn drawing_with_each_drag_tool_adds_one_shape_and_undo_removes_it() {
        for t in [Tool::Arrow, Tool::Rect, Tool::Line, Tool::Pen, Tool::Marker, Tool::Obscure] {
            let mut e = selected((100, 100), (900, 700));
            assert_eq!(click_item(&mut e, Item::Tool(t)), Effect::Redraw);
            assert_eq!(e.tool, Some(t));
            drag(&mut e, (200, 200), (400, 300));
            assert_eq!(e.doc.shapes().len(), 1, "{t:?}");
            assert_eq!(e.on_key(0x5A, true, false), Effect::Redraw);
            assert!(e.doc.is_empty());
            assert_eq!(e.on_key(0x59, true, false), Effect::Redraw);
            assert_eq!(e.doc.shapes().len(), 1);
        }
    }

    #[test]
    fn steps_count_up_and_text_is_typed() {
        let mut e = selected((100, 100), (900, 700));
        e.on_key(0x4E, false, false); // N
        assert_eq!(e.tool, Some(Tool::Steps));
        for x in [200, 300, 400] {
            e.on_down(Point::new(x, 300), false);
            e.on_up(Point::new(x, 300), false);
        }
        let ns: Vec<u32> = e.doc.shapes().iter().filter_map(|s| if let Shape::Step { n, .. } = s { Some(*n) } else { None }).collect();
        assert_eq!(ns, vec![1, 2, 3]);

        e.on_key(0x54, false, false); // T
        e.on_down(Point::new(300, 500), false);
        e.on_up(Point::new(300, 500), false);
        assert!(e.is_typing());
        // While typing, letter keys do not switch tools and Ctrl+Z cancels the typing.
        assert_eq!(e.on_key(0x41, false, false), Effect::None);
        for c in "Hi".chars() {
            e.on_char(c);
        }
        e.on_key(VK_RETURN, false, true); // Shift+Enter: new line
        e.on_char('x');
        e.on_key(VK_BACK, false, false);
        e.on_char('y');
        assert_eq!(e.text.as_ref().unwrap().text, "Hi\ny");
        e.on_key(VK_RETURN, false, false);
        assert!(!e.is_typing());
        assert!(matches!(e.doc.shapes().last(), Some(Shape::Text { text, .. }) if text == "Hi\ny"));
    }

    #[test]
    fn empty_text_is_dropped_and_escape_while_typing_does_not_close() {
        let mut e = selected((100, 100), (900, 700));
        e.on_key(0x54, false, false);
        e.on_down(Point::new(300, 500), false);
        assert_eq!(e.on_key(VK_ESCAPE, false, false), Effect::Redraw);
        assert!(e.doc.is_empty());
        assert_eq!(e.on_key(VK_ESCAPE, false, false), Effect::Close);
    }

    #[test]
    fn dropping_text_tool_commits_text_and_restores_handles() {
        let mut e = selected((100, 100), (900, 700));
        e.on_key(0x54, false, false);
        e.on_down(Point::new(300, 500), false);
        e.on_char('ş');
        e.palette_open = true;
        assert_eq!(e.drop_tool(), Effect::Redraw);
        assert!(!e.is_typing() && e.tool.is_none() && !e.palette_open);
        assert!(matches!(e.doc.shapes().last(), Some(Shape::Text { text, .. }) if text == "ş"));
        assert_eq!(e.handle_rects().len(), 8);
    }

    #[test]
    fn only_ctrl_moves_but_no_tool_handles_still_resize() {
        let mut e = selected((100, 100), (299, 199));
        drag(&mut e, (150, 150), (160, 170));
        assert_eq!(
            e.sel,
            Some(Rect::new(100, 100, 300, 200)),
            "plain inside never moves"
        );
        e.on_down_mods(Point::new(150, 150), false, true, &BoxFont);
        assert_eq!(
            e.on_move(Point::new(160, 170), false),
            Effect::SelectionChanged
        );
        e.on_up(Point::new(160, 170), false);
        assert_eq!(e.sel, Some(Rect::new(110, 120, 310, 220)));
        drag(&mut e, (309, 219), (409, 319));
        assert_eq!(e.sel, Some(Rect::new(110, 120, 410, 320)));
        assert_eq!(e.on_key(VK_RIGHT, false, true), Effect::None);
        assert_eq!(
            e.sel.unwrap().left,
            110,
            "keyboard does not move the editor selection"
        );
        e.on_down_mods(Point::new(200, 200), false, true, &BoxFont);
        e.on_move(Point::new(-9000, 200), false);
        e.on_up(Point::new(-9000, 200), false);
        assert_eq!(e.sel.unwrap().left, -3840);
    }

    #[test]
    fn the_cursor_fence_knows_which_drags_it_holds() {
        let mut e = ed();
        assert_eq!(e.drag_class(), None);
        e.on_down(Point::new(10, 10), false);
        assert_eq!(e.drag_class(), Some(DragClass::Select(Point::new(10, 10))), "choosing the first selection");
        e.on_up(Point::new(300, 200), false);
        assert_eq!(e.drag_class(), None, "released with the drag");
        e.tool = Some(Tool::Rect);
        e.on_down(Point::new(50, 50), false);
        assert_eq!(e.drag_class(), Some(DragClass::Draw), "a shape inside the selection");
        e.on_up(Point::new(80, 80), false);
        assert_eq!(e.drag_class(), None);
        // Ctrl moves the selection, a handle resizes it: neither is fenced.
        e.on_down_mods(Point::new(100, 100), false, true, &BoxFont);
        assert_eq!(e.drag_class(), None, "moving the selection may go to another monitor");
        e.on_up(Point::new(110, 100), false);
        e.tool = None;
        let corner = e.sel.map(|r| Point::new(r.left, r.top)).unwrap();
        e.on_down(corner, false);
        assert_eq!(e.drag_class(), None, "a handle resize");
        e.on_up(corner, false);
        e.on_down(Point::new(-500, -500), false);
        assert!(matches!(e.drag_class(), Some(DragClass::Select(_))), "outside the selection starts a new one");
    }

    #[test]
    fn resize_flips_past_the_opposite_edge() {
        let r = Rect::new(0, 0, 100, 100);
        assert_eq!(resize(r, 2, 50, 50), Rect::new(0, 0, 150, 150));
        assert_eq!(resize(r, 7, 150, 0), Rect::new(100, 0, 150, 100));
        assert_eq!(resize(r, 4, 0, 100), Rect::new(0, 100, 100, 101), "never empty");
    }

    #[test]
    fn resizing_cannot_leave_the_frozen_desktop() {
        let mut e = selected((100, 100), (299, 199));
        drag(&mut e, (299, 199), (9000, 9000));
        let sel = e.sel.unwrap();
        assert_eq!(e.mons.layout().clip(&sel), Some(sel));
        assert!(sel.right <= e.mons.layout().bounds().right);
        assert!(sel.bottom <= e.mons.layout().bounds().bottom);
    }

    #[test]
    fn actions_finish_pending_text_first() {
        let mut e = selected((100, 100), (900, 700));
        e.on_key(0x54, false, false);
        e.on_down(Point::new(300, 500), false);
        e.on_char('k');
        assert_eq!(click_item(&mut e, Item::Copy), Effect::Copy);
        assert_eq!(e.doc.shapes().len(), 1);
        assert_eq!(e.on_key(0x53, true, false), Effect::Save);
        assert_eq!(e.on_key(0x53, true, true), Effect::SaveAs);
        assert_eq!(e.on_key(0x50, true, false), Effect::Pin);
        assert_eq!(e.on_key(VK_RETURN, false, false), Effect::Copy);
    }

    #[test]
    fn the_highlighter_keeps_its_own_colour() {
        let mut e = selected((100, 100), (900, 700));
        e.set_color(PALETTE[0]);
        e.tool = Some(Tool::Marker);
        assert_eq!(e.shown_color(), MARKER_DEFAULT, "a red pen does not make a red highlight");
        e.set_color(PALETTE[3]);
        assert_eq!(e.shown_color(), PALETTE[3]);
        e.set_color(PALETTE[7]);
        assert_eq!(e.shown_color(), MARKER_DEFAULT, "white multiplies to nothing");
        e.tool = Some(Tool::Arrow);
        assert_eq!(e.shown_color(), PALETTE[0], "the other tools kept theirs");
    }

    #[test]
    fn palette_picks_a_colour_and_custom_asks_the_system() {
        let mut e = selected((100, 100), (900, 700));
        click_item(&mut e, Item::Color);
        assert!(e.palette_open);
        let (at, p) = e.palette().unwrap();
        let (_, b) = p.cells[4];
        e.on_down(Point::new(at.x + b.x0 + 3, at.y + b.y0 + 3), false);
        assert_eq!(e.color, PALETTE[4]);
        assert!(!e.palette_open);
        click_item(&mut e, Item::Color);
        let (at, p) = e.palette().unwrap();
        let (_, b) = *p.cells.last().unwrap();
        assert_eq!(e.on_down(Point::new(at.x + b.x0 + 3, at.y + b.y0 + 3), false), Effect::CustomColor);
        e.set_color(Rgba::rgb(1, 2, 3));
        assert_eq!(e.prefs().color, "#010203");
    }

    #[test]
    fn size_keys_wheel_and_prefs() {
        let mut e = selected((100, 100), (900, 700));
        e.on_key(0x33, false, false);
        assert_eq!(e.size, 2);
        for expected in 3..=5 {
            assert_eq!(e.on_wheel(1), Effect::Redraw);
            assert_eq!(e.size, expected);
        }
        assert_eq!(e.on_wheel(1), Effect::None, "largest of six");
        e.on_key(0x32, false, false);
        assert_eq!(e.size, 1);
        click_item(&mut e, Item::Tool(Tool::Obscure));
        click_item(&mut e, Item::ObscureMode);
        let p = e.prefs();
        assert!(p.blur);
        assert_eq!(Prefs::parse(&p.to_text()), p);
        assert_eq!(Prefs::parse("nonsense = 1"), Prefs::default(), "a broken file falls back");
    }

    #[test]
    fn strokes_scale_with_the_monitor() {
        let mut e = selected((-600, 100), (-101, 299)); // 150 %
        e.on_key(0x52, false, false); // R
        drag(&mut e, (-500, 150), (-300, 250));
        match &e.doc.shapes()[0] {
            Shape::Rect { width, .. } => assert_eq!(*width, STROKE[1] * 1.5),
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn compose_crops_to_the_selection_and_preview_adds_the_caret() {
        let mut e = selected((-50, -50), (49, 49));
        e.on_key(0x4C, false, false); // L
        drag(&mut e, (-40, 0), (40, 0));
        let base = Canvas::new(100, 100, Rgba::rgb(255, 255, 255));
        let out = e.compose(&base, &BoxFont, false);
        assert_eq!(out.get(50, 50), e.color, "line through the middle");
        assert_eq!(out.get(50, 10), Rgba::rgb(255, 255, 255));
        e.on_key(0x54, false, false);
        e.on_down(Point::new(-20, 30), false);
        let pv = e.compose(&base, &BoxFont, true);
        let fin = e.compose(&base, &BoxFont, false);
        assert_ne!(pv, fin, "the caret shows only in the preview");
    }

    #[test]
    fn view_shows_handles_only_without_a_tool() {
        let mut e = selected((100, 100), (900, 700));
        assert_eq!(e.view(&BoxFont).handles.len(), 8);
        e.on_key(0x41, false, false);
        let v = e.view(&BoxFont);
        assert!(v.handles.is_empty());
        assert!(v.bar.is_some() && v.label.is_some());
    }

    #[test]
    fn single_monitor_at_200_percent() {
        let mons = Monitors::new(vec![Monitor::new(Rect::from_xywh(0, 0, 3840, 2160), 192)]);
        let mut e = Editor::new(mons, vec![], Theme::new(Look::Paper, Mode::Dark), &Prefs::default(), Point::new(0, 0));
        drag(&mut e, (100, 100), (900, 900));
        let (_, bar) = e.bar().unwrap();
        assert_eq!(bar.h, (36.0 * 2.0 + 6.0 * 4.0) as i32);
    }

    #[test]
    fn new_editor_draws_immediately_without_accidental_move() {
        let mut e = ed();
        drag(&mut e, (100, 100), (900, 700));
        assert_eq!(e.tool, Some(Tool::Arrow));
        let sel = e.sel;
        drag(&mut e, (200, 200), (400, 300));
        assert_eq!(e.sel, sel);
        assert!(matches!(e.doc.shapes(), [Shape::Arrow { .. }]));
    }

    #[test]
    fn plain_corners_and_edges_use_every_active_tool() {
        for tool in toolbar::TOOLS {
            for point in [
                (100, 100),
                (899, 699),
                (500, 100),
                (100, 400),
                (899, 400),
                (500, 699),
            ] {
                let mut e = selected((100, 100), (899, 699));
                e.tool = Some(tool);
                let sel = e.sel;
                drag(&mut e, point, (point.0 + 30, point.1 + 30));
                assert_eq!(e.sel, sel, "{tool:?} at {point:?}");
                assert!(
                    e.text.is_some() || !e.doc.is_empty(),
                    "tool did not act: {tool:?} at {point:?}"
                );
            }
        }
    }

    #[test]
    fn ctrl_moves_inside_and_all_handles_with_any_tool() {
        let mut tools: Vec<_> = toolbar::TOOLS.into_iter().map(Some).collect();
        tools.push(None);
        for tool in tools {
            for point in [
                (400, 400),
                (100, 100),
                (899, 100),
                (899, 699),
                (100, 699),
                (500, 100),
                (899, 400),
                (500, 699),
                (100, 400),
            ] {
                let mut e = selected((100, 100), (899, 699));
                e.tool = tool;
                let sel = e.sel.unwrap();
                e.on_down_mods(Point::new(point.0, point.1), false, true, &BoxFont);
                e.on_move(Point::new(point.0 + 25, point.1 + 10), false);
                e.on_up(Point::new(point.0 + 25, point.1 + 10), false);
                assert_eq!(e.sel, Some(sel.offset(25, 10)), "{tool:?} at {point:?}");
                assert!(e.doc.is_empty() && e.text.is_none());
            }
        }
    }

    #[test]
    fn size_popup_changes_live_text_without_a_key_and_fits_monitor() {
        let mut e = selected((100, 100), (899, 699));
        e.tool = Some(Tool::Text);
        e.on_down(Point::new(250, 250), false);
        e.set_text("Türkçe ığüşöç İı 😀");
        let id = e.text.as_ref().unwrap().id;
        click_item(&mut e, Item::Size);
        let (at, menu) = e.size_menu().unwrap();
        let m = e.mons.at(at).unwrap();
        assert!(
            m.rect.contains(at) && at.x + menu.w <= m.rect.right && at.y + menu.h <= m.rect.bottom
        );
        let r = menu.rows[5];
        e.on_down(Point::new(at.x + r.x0 + 3, at.y + r.y0 + 3), false);
        assert!(!e.size_open);
        let t = e.text.as_ref().unwrap();
        assert_eq!((e.size, t.size, t.id), (5, 72.0, id));
        assert_eq!(t.text, "Türkçe ığüşöç İı 😀");
        click_item(&mut e, Item::Size);
        click_item(&mut e, Item::Size);
        assert!(!e.size_open, "the same button closes the popup");
        e.on_key(VK_RETURN, false, false);
        assert!(matches!(e.doc.shapes(), [Shape::Text { size: 72.0, .. }]));
    }

    #[test]
    fn text_anchor_reedit_cancel_replace_and_delete_are_reversible() {
        let mut e = selected((100, 100), (899, 699));
        e.tool = Some(Tool::Text);
        let at = Point::new(250, 250);
        e.on_down(at, false);
        assert_eq!(
            e.text.as_ref().unwrap().at,
            Pt::new(250.0, 250.0),
            "no upward jump"
        );
        e.set_text("İstanbul\r\nığüşöç 😀");
        e.on_key(VK_RETURN, false, false);
        let original = e.doc.shapes()[0].clone();
        e.set_size(5);
        e.set_color(Rgba::rgb(12, 34, 56));
        e.on_down(at, false);
        let t = e.text.as_ref().unwrap();
        assert_eq!(t.original, Some(0));
        assert_eq!(
            (t.size, t.color),
            (22.0, PALETTE[0]),
            "editing preserves the original style"
        );
        e.set_text("vazgeç");
        e.on_key(VK_ESCAPE, false, false);
        assert_eq!(e.doc.shapes(), std::slice::from_ref(&original));
        e.on_down(at, false);
        e.set_text("Düzenlendi");
        e.commit_text();
        assert_eq!(e.doc.shapes().len(), 1);
        e.on_key(0x5A, true, false);
        assert_eq!(e.doc.shapes(), std::slice::from_ref(&original));
        e.on_key(0x59, true, false);
        assert!(matches!(e.doc.shapes(), [Shape::Text { text, .. }] if text == "Düzenlendi"));
        e.on_down(at, false);
        e.set_text("");
        e.commit_text();
        assert!(e.doc.is_empty());
        e.on_key(0x5A, true, false);
        assert!(matches!(e.doc.shapes(), [Shape::Text { text, .. }] if text == "Düzenlendi"));
        e.on_key(0x59, true, false);
        assert!(e.doc.is_empty());
    }

    #[test]
    fn closing_the_size_menu_with_a_click_draws_nothing() {
        let mut e = selected((100, 100), (899, 699));
        e.tool = Some(Tool::Rect);
        click_item(&mut e, Item::Size);
        assert!(e.size_open);
        drag(&mut e, (200, 200), (400, 400));
        assert!(!e.size_open);
        assert!(e.doc.is_empty(), "the click that closes the popup must not draw");
    }

    #[test]
    fn marks_outside_the_selection_are_not_kept() {
        let mut e = selected((100, 100), (499, 399));
        e.tool = Some(Tool::Steps);
        e.on_down(Point::new(700, 700), false);
        e.on_up(Point::new(700, 700), false);
        assert!(e.doc.is_empty(), "invisible badge must not take a number");
        e.on_down(Point::new(200, 200), false);
        e.on_up(Point::new(200, 200), false);
        assert!(matches!(e.doc.shapes(), [Shape::Step { n: 1, .. }]));
        e.tool = Some(Tool::Arrow);
        drag(&mut e, (700, 700), (800, 800));
        assert_eq!(e.doc.shapes().len(), 1, "a shape fully outside adds no undo step");
        drag(&mut e, (700, 200), (300, 200));
        assert_eq!(e.doc.shapes().len(), 2, "a shape that crosses the selection is kept");
    }

    #[test]
    fn cursor_says_what_a_press_does() {
        let mut e = ed();
        assert_eq!(e.cursor_kind(Point::new(150, 150), false), CursorKind::Cross);
        drag(&mut e, (100, 100), (899, 699));
        assert_eq!(e.tool, Some(Tool::Arrow));
        assert_eq!(e.cursor_kind(Point::new(500, 400), false), CursorKind::Cross);
        assert_eq!(e.cursor_kind(Point::new(500, 400), true), CursorKind::Move);
        assert_eq!(e.cursor_kind(Point::new(100, 100), true), CursorKind::Move, "Ctrl wins on a corner");
        assert_eq!(e.cursor_kind(Point::new(100, 100), false), CursorKind::Cross, "a tool draws on corners");
        let (at, bar) = e.bar().unwrap();
        assert_eq!(e.cursor_kind(Point::new(at.x + bar.w / 2, at.y + bar.h / 2), false), CursorKind::Arrow);
        e.tool = None;
        for (p, want) in [
            ((100, 100), CursorKind::SizeNwse),
            ((899, 100), CursorKind::SizeNesw),
            ((499, 100), CursorKind::SizeNs),
            ((899, 399), CursorKind::SizeWe),
        ] {
            assert_eq!(e.cursor_kind(Point::new(p.0, p.1), false), want, "{p:?}");
        }
        assert_eq!(e.cursor_kind(Point::new(500, 400), false), CursorKind::Arrow);
        e.tool = Some(Tool::Text);
        assert_eq!(e.cursor_kind(Point::new(500, 400), false), CursorKind::IBeam);
    }

    /// The incremental frame must be pixel-identical to a full compose at every step.
    #[test]
    fn partial_frames_match_a_full_compose() {
        let base = {
            let mut px = Vec::with_capacity(400 * 300 * 4);
            for y in 0..300u32 {
                for x in 0..400u32 {
                    px.extend_from_slice(&[(x % 251) as u8, (y % 241) as u8, ((x ^ y) % 239) as u8, 255]);
                }
            }
            Canvas::from_bgra(400, 300, px)
        };
        let check = |e: &Editor, frame: &mut Option<Frame>, want: &[FrameChange]| {
            let doc = e.compose_doc(&base, &BoxFont);
            let change = e.update_frame(frame, &doc, &BoxFont, false);
            assert!(want.iter().any(|w| match (w, &change) {
                (FrameChange::Parts(_), FrameChange::Parts(_)) => true,
                (a, b) => a == b,
            }), "{change:?}");
            let mut full = doc.clone();
            e.preview(&mut full, &BoxFont, false);
            let got = &frame.as_ref().unwrap().img;
            if *got != full {
                let bad = (0..got.h as i32).flat_map(|y| (0..got.w as i32).map(move |x| (x, y))).find(|&(x, y)| got.get(x, y) != full.get(x, y));
                panic!("frame differs from a full compose after {change:?}: first at {bad:?}");
            }
            change
        };
        const PART: FrameChange = FrameChange::Parts(Vec::new());
        // Cancelling a stroke mid-drag takes everything it drew back.
        {
            let mut e = selected((100, 100), (499, 399));
            let mut frame = None;
            e.tool = Some(Tool::Pen);
            check(&e, &mut frame, &[FrameChange::All]);
            e.on_down(Point::new(150, 150), false);
            for p in [(300, 250), (450, 380)] {
                e.on_move(Point::new(p.0, p.1), false);
                check(&e, &mut frame, &[PART]);
            }
            assert_eq!(e.on_key(VK_ESCAPE, false, false), Effect::Redraw);
            check(&e, &mut frame, &[PART]);
            assert!(e.doc.is_empty());
        }
        for tool in [Tool::Arrow, Tool::Rect, Tool::Line, Tool::Pen, Tool::Marker, Tool::Obscure] {
            let mut e = selected((100, 100), (499, 399));
            let mut frame = None;
            e.tool = Some(tool);
            check(&e, &mut frame, &[FrameChange::All]);
            e.on_down(Point::new(150, 150), false);
            check(&e, &mut frame, &[PART, FrameChange::None]);
            for p in [(300, 250), (450, 380), (200, 120), (160, 160)] {
                e.on_move(Point::new(p.0, p.1), false);
                check(&e, &mut frame, &[PART]);
            }
            e.on_up(Point::new(160, 160), false);
            check(&e, &mut frame, &[PART, FrameChange::None]);
            assert_eq!(check(&e, &mut frame, &[FrameChange::None]), FrameChange::None, "no change, no work");
            // A second shape over the first, then undo, which starts over.
            e.on_down(Point::new(420, 130), false);
            e.on_move(Point::new(130, 370), false);
            check(&e, &mut frame, &[PART]);
            e.on_up(Point::new(130, 370), false);
            check(&e, &mut frame, &[PART]);
            assert_eq!(e.on_key(0x5A, true, false), Effect::Redraw);
            check(&e, &mut frame, &[FrameChange::All]);
        }
        // A step badge goes on the document without a drag.
        {
            let mut e = selected((100, 100), (499, 399));
            let mut frame = None;
            e.tool = Some(Tool::Steps);
            check(&e, &mut frame, &[FrameChange::All]);
            e.on_down(Point::new(250, 250), false);
            e.on_up(Point::new(250, 250), false);
            check(&e, &mut frame, &[PART]);
        }
    }

    /// The kept document image must equal a full compose after any sequence of edits, and a new
    /// shape on top must not crop the frozen pixels again.
    #[test]
    fn kept_document_image_matches_a_full_compose() {
        let base = {
            let mut px = Vec::with_capacity(400 * 300 * 4);
            for y in 0..300u32 {
                for x in 0..400u32 {
                    px.extend_from_slice(&[(x % 251) as u8, (y % 241) as u8, ((x ^ y) % 239) as u8, 255]);
                }
            }
            Canvas::from_bgra(400, 300, px)
        };
        let mut crops = 0;
        let mut cache = None;
        let mut sync = |e: &Editor, want_crop: bool| {
            let mut asked = false;
            assert!(e.update_doc(&mut cache, &BoxFont, &mut |_| {
                asked = true;
                Some(base.clone())
            }));
            assert_eq!(asked, want_crop, "crop asked={asked} at rev {}", e.rev);
            if asked {
                crops += 1;
            }
            assert!(cache.as_ref().unwrap().img == e.compose_doc(&base, &BoxFont), "differs at rev {}", e.rev);
        };
        let mut e = selected((100, 100), (499, 399));
        sync(&e, true);
        sync(&e, false);
        // Shapes one on top of another, the pixelate over the others last.
        for (tool, a, b) in [(Tool::Arrow, (150, 150), (450, 380)), (Tool::Rect, (120, 120), (300, 300)), (Tool::Pen, (130, 300), (480, 130)), (Tool::Obscure, (140, 140), (400, 350))] {
            e.tool = Some(tool);
            e.on_down(Point::new(a.0, a.1), false);
            e.on_move(Point::new((a.0 + b.0) / 2, (a.1 + b.1) / 2), false);
            e.on_move(Point::new(b.0, b.1), false);
            e.on_up(Point::new(b.0, b.1), false);
            sync(&e, false);
        }
        // Undo and redo put shapes back in their place: they start over.
        assert_eq!(e.on_key(0x5A, true, false), Effect::Redraw);
        sync(&e, true);
        assert_eq!(e.on_key(0x59, true, false), Effect::Redraw);
        sync(&e, true);
        // Steps are added without a drag.
        e.tool = Some(Tool::Steps);
        e.on_down(Point::new(200, 200), false);
        e.on_up(Point::new(200, 200), false);
        sync(&e, false);
        // A new text shape on commit; an edit of it replaces and starts over.
        e.tool = Some(Tool::Text);
        e.on_down(Point::new(320, 320), false);
        e.on_up(Point::new(320, 320), false);
        sync(&e, true); // opening a text box counts as a change
        e.set_text("abc");
        e.commit_text();
        sync(&e, false);
        e.on_down(Point::new(321, 322), false);
        e.on_up(Point::new(321, 322), false);
        assert!(e.text.as_ref().is_some_and(|t| t.original.is_some()), "the click must open the text again");
        sync(&e, true);
        e.set_text("abcd");
        e.commit_text();
        sync(&e, true);
    }

    /// The popups: a click on another toolbar button also presses it; a click on their own button
    /// or on the canvas only closes them and draws nothing.
    #[test]
    fn closing_a_popup_on_another_button_presses_it() {
        let mut e = selected((100, 100), (899, 699));
        e.tool = Some(Tool::Rect);
        let (at, bar) = e.bar().unwrap();
        let centre = |it: Item| {
            let b = bar.button(it).unwrap();
            Point::new(at.x + (b.x0 + b.x1) / 2, at.y + (b.y0 + b.y1) / 2)
        };
        let (arrow, size, color) = (centre(Item::Tool(Tool::Arrow)), centre(Item::Size), centre(Item::Color));
        let click = |e: &mut Editor, p: Point| {
            e.on_down_mods(p, false, false, &BoxFont);
            e.on_up(p, false);
        };
        click(&mut e, size);
        assert!(e.size_open);
        click(&mut e, arrow);
        assert!(!e.size_open);
        assert_eq!(e.tool, Some(Tool::Arrow));
        click(&mut e, size);
        click(&mut e, size);
        assert!(!e.size_open, "its own button closes it");
        click(&mut e, color);
        assert!(e.palette_open);
        click(&mut e, color);
        assert!(!e.palette_open, "its own button closes it");
        click(&mut e, color);
        click(&mut e, arrow);
        assert!(!e.palette_open);
        assert_eq!(e.tool, None, "pressed the active tool: it dropped");
        e.tool = Some(Tool::Arrow);
        click(&mut e, size);
        click(&mut e, Point::new(300, 300));
        assert!(!e.size_open);
        assert!(e.doc.is_empty(), "the closing click draws nothing");
    }

    /// A box dragged wider goes to the GPU as one rectangle, not its old and new self.
    #[test]
    fn nested_parts_are_uploaded_once() {
        let a = IBox::new(10, 10, 100, 100);
        let b = IBox::new(10, 10, 120, 110);
        assert_eq!(merged(vec![a, b]), vec![b]);
        // Far apart: two small boxes stay two.
        let c = IBox::new(500, 500, 510, 510);
        let d = IBox::new(10, 10, 20, 20);
        assert_eq!(merged(vec![c, d]), vec![c, d]);
    }

    /// A long diagonal must not repaint its whole bounding box on every move.
    #[test]
    fn a_long_arrow_touches_a_small_part_of_its_bounding_box() {
        let mut e = selected((0, 0), (3000, 1400));
        e.tool = Some(Tool::Arrow);
        e.on_down(Point::new(10, 10), false);
        e.on_move(Point::new(2990, 1390), false);
        let Some(Drag::Draw(s)) = e.drag.clone() else { panic!("no drag") };
        let area = |b: &IBox| b.w() as i64 * b.h() as i64;
        let cover: i64 = s.cover(0).iter().map(area).sum();
        let bounds = area(&s.bounds());
        assert!(cover * 5 < bounds, "cover {cover} vs bounds {bounds}");
    }

    #[test]
    fn untouched_selection_needs_no_copy() {
        let mut e = selected((100, 100), (499, 399));
        assert!(!e.needs_comp());
        e.tool = Some(Tool::Arrow);
        e.on_down(Point::new(150, 150), false);
        assert!(e.needs_comp(), "a drawing drag needs the composite");
        e.on_move(Point::new(300, 300), false);
        e.on_up(Point::new(300, 300), false);
        assert!(e.needs_comp());
        e.on_key(0x5A, true, false);
        assert!(!e.needs_comp(), "undo back to nothing returns to the frozen frame");
    }

    #[test]
    fn outside_click_commits_but_escape_discards_new_text() {
        let mut e = selected((100, 100), (899, 699));
        e.tool = Some(Tool::Text);
        e.on_down(Point::new(250, 250), false);
        e.set_text("kalacak");
        e.on_down(Point::new(600, 500), false);
        assert!(matches!(e.doc.shapes(), [Shape::Text { text, .. }] if text == "kalacak"));
        e.set_text("iptal");
        e.on_key(VK_ESCAPE, false, false);
        assert_eq!(e.doc.shapes().len(), 1);
        assert!(!e.is_typing());
    }
}
