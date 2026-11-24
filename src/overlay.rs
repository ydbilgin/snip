//! The overlay on the frozen screen: one borderless topmost window per monitor, each with a
//! 2-buffer flip-model swap chain drawn by Direct2D. The frozen frame is the background (a GPU
//! texture is wrapped, not copied), dimmed, with the selection shown undimmed.
//!
//! Three modes:
//! - Text (the text capture key, or the mode bar): drag over text; yellow frame, thin guide lines
//!   under the cursor, no window picking. Only the rectangle comes back; `app.rs` reads it.
//! - Plain (Win+Shift+S): drag a region, click a window, Enter for the monitor; a small mode bar
//!   at the top switches what a click takes (region/window: the window, full screen: the monitor).
//! - Editor (the editor hotkey): `editor::Editor` runs the interaction; this file crops the frozen
//!   pixels under the selection, lets the core draw annotations, toolbar and labels into canvases,
//!   and blits them.
//!
//! Every UI element (bars, labels, the annotated selection) is drawn by the core renderer, so the
//! overlay only places bitmaps and two 1-DIP frame lines. Drawing happens only on input.

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::{HINSTANCE, HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetAsyncKeyState, GetFocus, ReleaseCapture, SetCapture, SetFocus, VK_CONTROL, VK_LBUTTON, VK_MENU, VK_RBUTTON, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Interface, PCWSTR, Result, w};

use crate::capture::{Pixels, Snapshot};
use crate::dpi::{Monitor, Monitors};
use crate::editor::{CursorKind, DocImage, DragClass, Editor, Effect, Frame, FrameChange, Prefs};
use crate::gdi::GdiGlyphs;
use crate::cursorfence::{ClipSink, Driver, crossing_held, draw_area};
use crate::geometry::{Point, Rect, is_click, window_at};
use crate::hookstate::{VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_UP};
use crate::i18n::Lang;
use crate::raster::Canvas;
use crate::theme::{Look, Mode, Theme};
use crate::toolbar::{self, Bar, BarState, Item};

pub const CLASS: PCWSTR = w!("Laureth.snip.overlay");
/// Drawing faster than this during a drag only heats the GPU (the game may still be using it).
const FRAME_MS: f64 = 7.0;
/// A frame that sent this much to the GPU makes the next one wait longer: at full speed a pixelate
/// of the whole selection only grows the graphics driver's upload memory.
const HEAVY_UPLOAD_BYTES: i64 = 8 * 1024 * 1024;
const HEAVY_FRAME_MS: f64 = 16.0;
/// WM_TIMER id on the first overlay window: draw the frame a fast drag skipped.
pub const FRAME_TIMER: usize = 0x5346;

struct Win {
    hwnd: HWND,
    swap: IDXGISwapChain1,
    target: ID2D1Bitmap1,
    frozen: ID2D1Bitmap1,
    w: u32,
    h: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Rectangle,
    Window,
    Monitor,
    Frames,
    Part,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Rectangle => "rect",
            Kind::Window => "window",
            Kind::Monitor => "monitor",
            Kind::Frames => "frames",
            Kind::Part => "part",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    Commit(Rect, Kind),
    Cancel,
    /// Editor: copy, save, save as or pin (the image comes from `editor_result`).
    Edited(Effect),
    /// Text mode, or "copy text" in the editor: read this rectangle. `work`: the editor has drawn
    /// shapes and stays open.
    TextArea(Rect, bool),
    /// Editor: open the system colour dialog, then call `set_color`.
    CustomColor,
}

/// How to open the overlay.
#[derive(Clone, Debug)]
pub struct Opts {
    pub dim: f32,
    pub theme: Theme,
    pub lang: Lang,
    /// Some: editor mode with these remembered settings.
    pub editor: Option<Prefs>,
    /// Open in text mode: the text capture key.
    pub text: bool,
    /// Text capture is on: the mode bar and the editor offer it.
    pub text_action: bool,
}

impl Opts {
    pub fn plain(dim: f32) -> Opts {
        Opts { dim, theme: Theme::new(Look::Graphite, Mode::Light), lang: Lang::En, editor: None, text: false, text_action: false }
    }
}

/// The shadow of one panel size: panel width, height and scale; bitmap; margin; bitmap size.
struct Shadow {
    key: (i32, i32, u32),
    bmp: ID2D1Bitmap1,
    margin: (i32, i32),
    size: (i32, i32),
}

/// A core-rendered bitmap placed in desktop coordinates.
#[derive(Clone)]
struct Placed {
    at: Point,
    w: i32,
    h: i32,
    bmp: ID2D1Bitmap1,
}

impl Placed {
    fn rect(&self) -> Rect {
        Rect::from_xywh(self.at.x, self.at.y, self.w, self.h)
    }
}

pub struct Overlay {
    pub snap: Snapshot,
    ctx: ID2D1DeviceContext,
    wins: Vec<Win>,
    dim: ID2D1SolidColorBrush,
    inner: ID2D1SolidColorBrush,
    outer: ID2D1SolidColorBrush,
    handle_fill: ID2D1SolidColorBrush,
    handle_ring: ID2D1SolidColorBrush,
    marker: ID2D1SolidColorBrush,
    marker_ink: ID2D1SolidColorBrush,
    guide: ID2D1SolidColorBrush,
    /// Shadows are built once per panel size.
    shadows: Vec<Shadow>,
    theme: Theme,
    lang: Lang,
    glyphs: GdiGlyphs,
    mons: Monitors,
    // Plain mode.
    drag: Option<(Point, Point)>,
    drag_add: bool,
    frames: crate::frameset::FrameSet,
    pub(crate) parts: crate::partsoverlay::Session,
    parts_visual: Option<(String,Option<Rect>)>,
    hover: Option<usize>,
    workarea_commit: bool,
    cursor: Point,
    mode: Item,
    mode_bar: Option<(Point, Bar)>,
    /// Static hint built alongside the mode bar, reused unchanged on hover/drag redraws.
    plain_hint: Option<Placed>,
    bar_hover: Option<Item>,
    // Editor mode.
    pub editor: Option<Editor>,
    text_input: Option<crate::textinput::TextInput>,
    doc_cache: Option<DocImage>,
    comp: Option<(Rect, ID2D1Bitmap1)>,
    frame: Option<Frame>,
    /// CPU copies of the monitors under the selection, kept only while it is being moved or resized.
    cpu_frames: Vec<Option<Vec<u8>>>,
    /// Whole monitors in memory for the magnifier while it shows (dropped when it goes).
    loupe_frames: Vec<Option<Vec<u8>>>,
    ui: Vec<Placed>,
    /// Rectangles drawn last frame (for partial redraws).
    shown: Vec<Rect>,
    /// The editor got the keyboard. False: Windows refused the foreground; the first click takes it.
    pub focused: bool,
    /// Milliseconds of the first frame: draw and present, show, activate.
    pub show_ms: (f64, f64, f64),
    moving: bool,
    pending: bool,
    last_frame: i64,
    heavy_frame: bool,
    /// Self-test: measure every frame, skip none.
    pub no_throttle: bool,
    /// Keeps the mouse on the monitor where a drag began (`cursorfence.rs`).
    fence: Driver,
    /// Self-test: the mouse buttons and Alt come from the test, not from the keyboard and mouse.
    pub synthetic_input: bool,
    pub synthetic_crossing: bool,
    record_mode: bool,
}

/// A desktop of the user's station that is only a test: nothing may clip the user's real mouse.
struct NoClip;

impl ClipSink for NoClip {
    fn clip(&mut self, _: Rect) {}
    fn release(&mut self) {}
}

fn windows_numerics_point(x: f32, y: f32) -> windows_numerics::Vector2 {
    windows_numerics::Vector2 { X: x, Y: y }
}

fn rf(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.left as f32, top: r.top as f32, right: r.right as f32, bottom: r.bottom as f32 }
}

fn color(c: crate::raster::Rgba, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.r as f32 / 255.0, g: c.g as f32 / 255.0, b: c.b as f32 / 255.0, a }
}

fn props(options: D2D1_BITMAP_OPTIONS, alpha: D2D1_ALPHA_MODE) -> D2D1_BITMAP_PROPERTIES1 {
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: alpha },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: options,
        colorContext: ManuallyDrop::new(None),
    }
}

/// Inclusive drag: both the press pixel and the release pixel are inside the selection.
pub fn drag_rect(a: Point, b: Point) -> Rect {
    let r = Rect::from_drag(a, b);
    Rect::new(r.left, r.top, r.right + 1, r.bottom + 1)
}

fn ctrl_held() -> bool {
    unsafe { (GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0 }
}

/// Ctrl for a keyboard shortcut. AltGr reaches Windows as Ctrl+Alt: it types characters, it is not one.
fn shortcut_ctrl_held() -> bool {
    let alt = unsafe { (GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0 };
    crate::hotkey::is_shortcut_ctrl(ctrl_held(), alt)
}

impl Overlay {
    /// Build the windows (hidden) and their swap chains. `hinst` owns the registered CLASS.
    pub fn new(snap: Snapshot, hinst: HINSTANCE, o: &Opts) -> Result<Overlay> {
        let gpu = snap
            .gpu
            .as_ref()
            .ok_or_else(|| windows::core::Error::new(DXGI_ERROR_UNSUPPORTED, "no D3D device"))?;
        unsafe {
            let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&D2D1_FACTORY_OPTIONS::default()))?;
            let dxgi_dev: IDXGIDevice = gpu.device.cast()?;
            let dev = factory.CreateDevice(&dxgi_dev)?;
            let ctx = dev.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            ctx.SetDpi(96.0, 96.0);
            let t = o.theme;
            let dim_a = if o.editor.is_some() { t.dim_alpha as f32 / 255.0 } else { o.dim };
            let dim = ctx.CreateSolidColorBrush(&color(crate::theme::DIM_COLOR, dim_a), None)?;
            let inner = ctx.CreateSolidColorBrush(&color(t.sel_inner, 1.0), None)?;
            let outer = ctx.CreateSolidColorBrush(&color(t.sel_outer, crate::theme::SEL_OUTER_ALPHA), None)?;
            let handle_fill = ctx.CreateSolidColorBrush(&color(crate::theme::HANDLE_FILL, 1.0), None)?;
            let handle_ring = ctx.CreateSolidColorBrush(&color(t.handle_ring(), 1.0), None)?;
            let marker = ctx.CreateSolidColorBrush(&color(crate::theme::MARKER, 1.0), None)?;
            let marker_ink = ctx.CreateSolidColorBrush(&color(crate::theme::MARKER_INK, 0.6), None)?;
            let guide = ctx.CreateSolidColorBrush(&color(crate::raster::Rgba::rgb(255, 255, 255), 72.0 / 255.0), None)?;
            let mons = Monitors::new(snap.monitors.iter().map(|m| Monitor::new(m.rect, m.dpi)).collect());
            let mut pt = windows::Win32::Foundation::POINT::default();
            let _ = GetCursorPos(&mut pt);
            let cursor = Point::new(pt.x, pt.y);
            let editor = o.editor.as_ref().map(|p| {
                let mut e = Editor::new(mons.clone(), snap.windows.iter().map(|w| w.rect).collect(), t, p, cursor);
                e.lang = o.lang;
                e.text_action = o.text_action;
                e
            });
            // Own each completed window immediately so every later error drops it too.
            let mut ov = Overlay {
                ctx,
                wins: Vec::with_capacity(snap.monitors.len()),
                dim,
                inner,
                outer,
                handle_fill,
                handle_ring,
                marker,
                marker_ink,
                guide,
                shadows: Vec::new(),
                theme: t,
                lang: o.lang,
                glyphs: GdiGlyphs::new(),
                mons,
                drag: None,
                drag_add: false,
                frames: Default::default(),
                parts: Default::default(),
                parts_visual: None,
                hover: None,
                workarea_commit: false,
                cursor,
                mode: if o.text { Item::ModeText } else { Item::ModeRegion },
                mode_bar: None,
                plain_hint: None,
                bar_hover: None,
                editor,
                text_input: None,
                doc_cache: None,
                comp: None,
                frame: None,
                cpu_frames: Vec::new(),
                loupe_frames: Vec::new(),
                ui: Vec::new(),
                shown: Vec::new(),
                focused: false,
                show_ms: (0.0, 0.0, 0.0),
                moving: false,
                pending: false,
                last_frame: 0,
                heavy_frame: false,
                no_throttle: false,
                fence: if crate::clipboard::on_a_hidden_desktop_of_his_station() { Driver::new(Box::new(NoClip)) } else { Driver::real() },
                synthetic_input: false,
                synthetic_crossing: false,
                record_mode: false,
                snap,
            };
            for (i, m) in ov.snap.monitors.iter().enumerate() {
                let r = m.rect;
                // The editor's GDI EDIT has its own child HWND, separate from the flip swapchain.
                let style = if o.editor.is_some() { WS_POPUP | WS_CLIPCHILDREN } else { WS_POPUP };
                let hwnd = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP, CLASS, w!("snip"), style, r.left, r.top, r.width(), r.height(), None, None, Some(hinst), None)?;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, i as isize);
                let made = (|| -> Result<Win> {
                    let gpu = ov.snap.gpu.as_ref().unwrap();
                    let desc = DXGI_SWAP_CHAIN_DESC1 {
                        Width: r.width() as u32,
                        Height: r.height() as u32,
                        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                        BufferCount: 2,
                        Scaling: DXGI_SCALING_STRETCH,
                        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                        AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                        ..Default::default()
                    };
                    let swap = gpu.factory.CreateSwapChainForHwnd(&gpu.device, hwnd, &desc, None, None)?;
                    let _ = gpu.factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER | DXGI_MWA_NO_WINDOW_CHANGES);
                    let surface: IDXGISurface = swap.GetBuffer(0)?;
                    let target = ov.ctx.CreateBitmapFromDxgiSurface(&surface, Some(&props(D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_ALPHA_MODE_IGNORE)))?;
                    let f = &ov.snap.frames[i];
                    let frozen = match &f.pixels {
                        Pixels::Gpu(tex) => {
                            let s: IDXGISurface = tex.cast()?;
                            ov.ctx.CreateBitmapFromDxgiSurface(&s, Some(&props(D2D1_BITMAP_OPTIONS_NONE, D2D1_ALPHA_MODE_IGNORE)))?
                        }
                        Pixels::Cpu(v) => ov.ctx.CreateBitmap(D2D_SIZE_U { width: f.width, height: f.height }, Some(v.as_ptr() as *const _), f.width * 4, &props(D2D1_BITMAP_OPTIONS_NONE, D2D1_ALPHA_MODE_IGNORE))?,
                    };
                    Ok(Win { hwnd, swap, target, frozen, w: r.width() as u32, h: r.height() as u32 })
                })();
                match made {
                    Ok(wi) => ov.wins.push(wi),
                    Err(e) => {
                        let _ = DestroyWindow(hwnd);
                        return Err(e);
                    }
                }
            }
            ov.hover = window_at(&ov.window_rects(), ov.cursor);
            if ov.editor.is_none() && !o.text {
                // The mode bar: top centre of the monitor under the cursor, at its scale.
                if let Some(m) = ov.mons.at(ov.cursor).or(ov.mons.list.first()).copied() {
                    let bar = Bar::layout(&toolbar::mode_slots_with(o.text_action), &ov.theme, m.scale());
                    let at = Point::new(m.rect.left + (m.rect.width() - bar.w) / 2, m.rect.top + m.px(crate::theme::MODE_BAR_TOP));
                    let hint = toolbar::render_plain_hint(ov.lang, &ov.theme, m.scale(), &ov.glyphs, m.rect.width() - 2 * m.px(crate::theme::MODE_BAR_TOP));
                    let hint_at = Point::new(m.rect.left + (m.rect.width() - hint.w as i32) / 2, at.y + bar.h + m.px(crate::theme::SELECTION_LABEL_GAP));
                    ov.plain_hint = ov.placed(hint_at, &hint);
                    ov.mode_bar = Some((at, bar));
                }
            }
            ov.rebuild_ui();
            Ok(ov)
        }
    }

    fn window_rects(&self) -> Vec<Rect> {
        self.snap.windows.iter().map(|w| w.rect).collect()
    }

    /// Draw every monitor, show the windows, and bring the one under the cursor to the front.
    pub fn show(&mut self) -> Result<()> {
        self.open(true)
    }

    /// Show without asking for the foreground (the real-screen test: never take the user's focus).
    pub fn show_passive(&mut self) -> Result<()> {
        self.open(false)
    }

    fn open(&mut self, take_focus: bool) -> Result<()> {
        let t = crate::log::qpc();
        for i in 0..self.wins.len() {
            self.draw(i)?;
        }
        let draw_ms = crate::log::ms_since(t);
        let t = crate::log::qpc();
        unsafe {
            for w in &self.wins {
                let _ = ShowWindow(w.hwnd, SW_SHOWNOACTIVATE);
            }
        }
        let show_ms = crate::log::ms_since(t);
        let t = crate::log::qpc();
        let i = self.snap.layout.nearest_monitor(self.cursor).unwrap_or(0).min(self.wins.len().saturating_sub(1));
        if take_focus && let Some(w) = self.wins.get(i) {
            ask_foreground(w.hwnd);
        }
        self.show_ms = (draw_ms, show_ms, crate::log::ms_since(t));
        Ok(())
    }

    /// The text box's EDIT and its host window, while one is open.
    pub fn text_windows(&self) -> Option<(HWND, HWND)> {
        self.text_input.as_ref().map(|t| (t.hwnd, t.host))
    }

    /// Unsaved work that a timeout must not throw away.
    pub fn has_work(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| !e.doc.is_empty() || e.text.is_some())
    }

    /// While a system dialog is open only the dialog takes input; its owner alone is not enough
    /// because the other monitors have overlay windows too.
    pub fn set_modal(&self, on: bool) {
        unsafe {
            for w in &self.wins {
                let _ = EnableWindow(w.hwnd, !on);
            }
        }
        if let Some(t) = &self.text_input {
            t.set_enabled(!on);
        }
    }

    /// The pointer for where the mouse is now.
    pub fn cursor_id(&self) -> PCWSTR {
        let kind = match &self.editor {
            Some(e) => e.cursor_kind(self.cursor, ctrl_held()),
            None if self.bar_hover.is_some() => CursorKind::Arrow,
            None => CursorKind::Cross,
        };
        match kind {
            CursorKind::Cross => IDC_CROSS,
            CursorKind::Arrow => IDC_ARROW,
            CursorKind::IBeam => IDC_IBEAM,
            CursorKind::Move => IDC_SIZEALL,
            CursorKind::SizeNwse => IDC_SIZENWSE,
            CursorKind::SizeNesw => IDC_SIZENESW,
            CursorKind::SizeNs => IDC_SIZENS,
            CursorKind::SizeWe => IDC_SIZEWE,
        }
    }

    pub fn hwnds(&self) -> Vec<HWND> {
        self.wins.iter().map(|w| w.hwnd).collect()
    }

    /// The window that owns dialogs (the one under the cursor).
    pub fn owner(&self) -> HWND {
        let i = self.snap.layout.nearest_monitor(self.cursor).unwrap_or(0);
        self.wins.get(i).or(self.wins.first()).map(|w| w.hwnd).unwrap_or_default()
    }

    /// Text mode: the yellow frame, no window picking.
    pub fn is_text(&self) -> bool {
        self.editor.is_none() && self.mode == Item::ModeText
    }

    /// Where a mode bar button is, in the first monitor's coordinates (the look test).
    pub fn mode_item_center(&self, it: Item) -> Option<(i32, i32)> {
        let (at, bar) = self.mode_bar.as_ref()?;
        let b = bar.button(it)?;
        Some((at.x + b.x0 + b.w() / 2, at.y + b.y0 + b.h() / 2))
    }

    /// The monitor under the cursor, whose guide lines must follow the mouse.
    fn cursor_monitor(&self) -> Option<Rect> {
        self.mons.at(self.cursor).map(|m| m.rect)
    }

    /// Plain mode: the rectangle the overlay currently shows.
    pub fn set_record_mode(&mut self) {
        self.record_mode = true;
        self.mode_bar = None;
        self.rebuild_ui();
    }

    pub fn current(&self) -> Option<(Rect, Kind)> {
        let (r, k) = self.raw_current()?;
        if !self.record_mode { return Some((r, k)); }
        let anchor = self.drag.map_or(self.cursor, |(a, _)|a);
        let monitors: Vec<Rect> = self.mons.list.iter().map(|m|m.rect).collect();
        crate::record::selection(r, anchor, &monitors).map(|r|(r,k))
    }

    fn raw_current(&self) -> Option<(Rect, Kind)> {
        if let Some(e) = &self.editor {
            return e.sel.map(|r| (r, Kind::Rectangle));
        }
        if self.is_text() {
            return self.drag.filter(|(a, b)| !is_click(*a, *b)).map(|(a, b)| (drag_rect(a, b), Kind::Rectangle));
        }
        if (self.mode == Item::ModeRegion || self.drag_add || !self.frames.is_empty())
            && let Some((a, b)) = self.drag
            && !is_click(a, b)
        {
            return Some((drag_rect(a, b), Kind::Rectangle));
        }
        if self.parts.mode.active() && let Some(r) = self.parts.current() { return Some((r, Kind::Part)); }
        if self.mode == Item::ModeScreen {
            return self.mons.at(self.cursor).map(|m| (m.rect, Kind::Monitor));
        }
        self.hover.and_then(|k| self.snap.layout.clip(&self.snap.layout.snap_window(&self.snap.windows[k].rect))).map(|r| (r, Kind::Window))
    }

    fn sel_shown(&self) -> Option<Rect> {
        match &self.editor {
            Some(e) => e.shown_sel(),
            None => self.current().map(|c| c.0),
        }
    }

    fn placed(&self, at: Point, cv: &Canvas) -> Option<Placed> {
        if cv.w == 0 || cv.h == 0 {
            return None;
        }
        let pm = cv.premultiplied();
        let bmp = unsafe { self.ctx.CreateBitmap(D2D_SIZE_U { width: cv.w, height: cv.h }, Some(pm.as_ptr() as *const _), cv.w * 4, &props(D2D1_BITMAP_OPTIONS_NONE, D2D1_ALPHA_MODE_PREMULTIPLIED)).ok()? };
        Some(Placed { at, w: cv.w as i32, h: cv.h as i32, bmp })
    }

    /// The soft shadow of a floating panel, from a small per-size cache.
    fn shadow(&mut self, at: Point, w: i32, h: i32) -> Option<Placed> {
        let scale = self.mons.at(at).map(|m| m.scale()).unwrap_or(1.0);
        let key = (w, h, (scale * 100.0) as u32);
        if !self.shadows.iter().any(|s| s.key == key) {
            let (cv, mx, my) = toolbar::render_shadow(w, h, &self.theme, scale);
            let pm = cv.premultiplied();
            let bmp = unsafe { self.ctx.CreateBitmap(D2D_SIZE_U { width: cv.w, height: cv.h }, Some(pm.as_ptr() as *const _), cv.w * 4, &props(D2D1_BITMAP_OPTIONS_NONE, D2D1_ALPHA_MODE_PREMULTIPLIED)).ok()? };
            self.shadows.push(Shadow { key, bmp, margin: (mx, my), size: (cv.w as i32, cv.h as i32) });
        }
        let s = self.shadows.iter().find(|s| s.key == key)?;
        Some(Placed { at: Point::new(at.x - s.margin.0, at.y - s.margin.1), w: s.size.0, h: s.size.1, bmp: s.bmp.clone() })
    }

    /// The magnifier next to the cursor, when the editor wants it (`loupe.rs`).
    pub(crate) fn loupe_canvas(&mut self) -> Option<(Point, Canvas)> {
        if !self.editor.as_ref().is_some_and(|e| e.wants_loupe()) {
            self.loupe_frames.clear();
            return None;
        }
        let p = self.cursor;
        let m = self.mons.at(p).copied()?;
        let got = self.snap.layout.clip(&crate::loupe::sample_rect(p))?;
        let (w, h, px) = self.snap.crop_cached(&got, &mut self.loupe_frames).ok()?;
        let square = crate::loupe::square(p, got, &Canvas::from_bgra(w, h, px));
        let cv = crate::loupe::render(&square, p, &self.theme, m.scale(), &self.glyphs);
        let at = crate::loupe::place(p, cv.w as i32, cv.h as i32, m.rect, m.px(crate::theme::LOUPE_OFFSET));
        Some((at, cv))
    }

    /// One pixel of the frozen screen (the colour picker reads the screen, not the drawing).
    pub fn frozen_pixel(&self, p: Point) -> Option<crate::raster::Rgba> {
        let (_, _, px) = self.snap.crop(&Rect::from_xywh(p.x, p.y, 1, 1)).ok()?;
        (px.len() >= 4).then(|| crate::raster::Rgba::rgb(px[2], px[1], px[0]))
    }

    /// Re-render the bars and labels (core canvases) as bitmaps.
    fn rebuild_ui(&mut self) {
        let mut ui = Vec::new();
        let loupe = self.loupe_canvas();
        if let Some(e) = &self.editor {
            let v = e.view(&self.glyphs);
            for (floating, item) in [(true, v.bar), (true, v.palette), (true, v.size_menu), (false, v.label), (false, v.tooltip), (true, loupe)] {
                let Some((at, cv)) = item else { continue };
                if floating && let Some(s) = self.shadow(at, cv.w as i32, cv.h as i32) {
                    ui.push(s);
                }
                if let Some(p) = self.placed(at, &cv) {
                    ui.push(p);
                }
            }
        } else {
            if self.parts_eligible() && self.parts.mode.active() && self.drag.is_none()
                && let Some(m) = self.mons.at(self.cursor).copied() {
                    let cv = toolbar::render_tooltip(&self.parts.label(self.lang), &self.theme, m.scale(), &self.glyphs);
                    let r = self.current().map(|(r,_)| r).unwrap_or(Rect::from_xywh(self.cursor.x,self.cursor.y,1,1));
                    let x = r.left.clamp(m.rect.left,(m.rect.right-cv.w as i32).max(m.rect.left));
                    let y = (r.top-cv.h as i32-6).clamp(m.rect.top,(m.rect.bottom-cv.h as i32).max(m.rect.top));
                    if let Some(p) = self.placed(Point::new(x,y),&cv) { ui.push(p); }
            }
            // The hint steps aside while a drag picks the area and while a mode button shows its name.
            if !self.is_text() && !self.record_mode && self.drag.is_none() && self.bar_hover.is_none()
                && let Some(hint) = self.plain_hint.clone() {
                let r = hint.rect();
                if let Some(s) = self.shadow(Point::new(r.left, r.top), r.width(), r.height()) {
                    ui.push(s);
                }
                ui.push(hint);
            }
            if let Some((at, bar)) = self.mode_bar.clone() {
                let st = BarState { active: vec![self.mode], hover: self.bar_hover, ..Default::default() };
                let cv = toolbar::render_bar(&bar, &self.theme, &st);
                if let Some(s) = self.shadow(at, cv.w as i32, cv.h as i32) {
                    ui.push(s);
                }
                if let Some(p) = self.placed(at, &cv) {
                    ui.push(p);
                }
                // The icons need their names until they are learned.
                if let Some(it) = self.bar_hover
                    && let Some(b) = bar.button(it)
                {
                    let tip = toolbar::render_tooltip(&it.tooltip(self.lang, crate::annotate::ObscureMode::Pixelate), &self.theme, bar.scale, &self.glyphs);
                    let gap = (crate::theme::SELECTION_LABEL_GAP * bar.scale).round() as i32;
                    let x = at.x + b.x0 + b.w() / 2 - tip.w as i32 / 2;
                    if let Some(p) = self.placed(Point::new(x, at.y + bar.h + gap), &tip) {
                        ui.push(p);
                    }
                }
            }
            if self.record_mode
                && let Some(m) = self.mons.at(self.cursor).or(self.mons.list.first()).copied() {
                    // Same panel as the text hint: the sentence, then the keys as caps.
                    let cv = toolbar::render_record_hint(self.lang, &self.theme, m.scale(), &self.glyphs);
                    let at = Point::new(m.rect.left + (m.rect.width() - cv.w as i32)/2, m.rect.top + m.px(crate::theme::MODE_BAR_TOP));
                    if let Some(s) = self.shadow(at, cv.w as i32, cv.h as i32) { ui.push(s); }
                    if let Some(p) = self.placed(at, &cv) { ui.push(p); }
            }
            if self.is_text() && self.drag.is_none() {
                // What to do, in words, on the monitor under the cursor.
                if let Some(m) = self.mons.at(self.cursor).or(self.mons.list.first()).copied() {
                    use crate::i18n::S;
                    let cv = toolbar::render_text_hint(S::TextHintDrag.get(self.lang), &[(crate::cursorfence::CROSSING_KEY_NAME, S::TextHintCross.get(self.lang)), ("Esc", S::TextHintCancel.get(self.lang))], &self.theme, m.scale(), &self.glyphs);
                    let bar_bottom = self.mode_bar.as_ref().filter(|(at, _)| m.rect.contains(*at)).map(|(at, b)| at.y + b.h + m.px(crate::theme::SELECTION_LABEL_GAP));
                    let at = Point::new(m.rect.left + (m.rect.width() - cv.w as i32) / 2, bar_bottom.unwrap_or(m.rect.top + m.px(crate::theme::MODE_BAR_TOP)));
                    if let Some(s) = self.shadow(at, cv.w as i32, cv.h as i32) {
                        ui.push(s);
                    }
                    if let Some(p) = self.placed(at, &cv) {
                        ui.push(p);
                    }
                }
            }
            if let Some((r, Kind::Rectangle)) = self.current() {
                let sc = self.mons.at(Point::new(r.left, r.top)).map(|m| m.scale()).unwrap_or(1.0);
                let size_text = format!("{} \u{00D7} {}", r.width(), r.height());
                let cv = if self.is_text() {
                    let text = if crate::textcore::selection_usable(r.width(), r.height()) { size_text } else { crate::i18n::S::TooSmall.get(self.lang).to_string() };
                    toolbar::render_text_label(&text, sc, &self.glyphs)
                } else {
                    toolbar::render_label(&size_text, &self.theme, sc, &self.glyphs)
                };
                let m = self.mons.at(Point::new(r.left, r.top)).map(|m| m.rect).unwrap_or(r);
                let gap = (crate::theme::SELECTION_LABEL_GAP * sc).round() as i32;
                let y = if r.top - gap - cv.h as i32 >= m.top { r.top - gap - cv.h as i32 } else { r.top + gap };
                let y = y.min(m.bottom - cv.h as i32).max(m.top);
                if let Some(p) = self.placed(Point::new(r.left.max(m.left).min(m.right - cv.w as i32), y), &cv) {
                    ui.push(p);
                }
            }
        }
        self.ui = ui;
    }

    /// Editor: bring the document image up to date. The frozen pixels are read again only when it
    /// must start over; while the selection moves, whole monitors stay in memory to keep that cheap.
    fn update_doc(&mut self) -> bool {
        let Some(e) = &self.editor else {
            self.doc_cache = None;
            return false;
        };
        let adjusting = e.is_adjusting();
        if !adjusting {
            self.cpu_frames.clear();
        }
        let (snap, cpu) = (&self.snap, &mut self.cpu_frames);
        e.update_doc(&mut self.doc_cache, &self.glyphs, &mut |sel| {
            let crop = if adjusting { snap.crop_cached(&sel, cpu) } else { snap.crop(&sel) };
            match crop {
                Ok((w, h, px)) => Some(Canvas::from_bgra(w, h, px)),
                Err(e) => {
                    crate::logf!("editor crop FAILED {:#010x} {} sel={:?}", e.code().0, e.message(), sel);
                    None
                }
            }
        })
    }

    /// Editor: the annotated selection as a bitmap.
    fn rebuild_comp(&mut self) {
        if !self.update_doc() {
            self.comp = None;
            self.frame = None;
            return;
        }
        let (Some(e), Some(c)) = (&self.editor, &self.doc_cache) else { return };
        let Some(sel) = &e.sel else { return };
        let doc = &c.img;
        let draw_text = !self.text_input.as_ref().is_some_and(|t| t.visible());
        let change = e.update_frame(&mut self.frame, doc, &self.glyphs, draw_text);
        let Some(f) = &self.frame else {
            self.comp = None;
            return;
        };
        let same_sel = self.comp.as_ref().is_some_and(|(r, _)| r == sel);
        match &change {
            FrameChange::None if same_sel => return,
            FrameChange::Parts(parts) if same_sel => {
                // Only the boxes the shape touched go to the GPU.
                let bytes: i64 = parts.iter().map(|b| (b.x1 - b.x0) as i64 * (b.y1 - b.y0) as i64 * 4).sum();
                self.heavy_frame = bytes >= HEAVY_UPLOAD_BYTES;
                let ok = self.comp.as_ref().is_some_and(|(_, bmp)| {
                    parts.iter().all(|b| {
                        let rect = D2D_RECT_U { left: b.x0 as u32, top: b.y0 as u32, right: b.x1 as u32, bottom: b.y1 as u32 };
                        let at = (b.y0 as usize * f.img.w as usize + b.x0 as usize) * 4;
                        unsafe { bmp.CopyFromMemory(Some(&rect), f.img.px[at..].as_ptr() as *const _, f.img.w * 4).is_ok() }
                    })
                });
                if !ok {
                    self.comp = None;
                }
                return;
            }
            _ => {}
        }
        let size = D2D_SIZE_U { width: f.img.w, height: f.img.h };
        let reuse = self
            .comp
            .as_ref()
            .filter(|(r, _)| r.width() == sel.width() && r.height() == sel.height())
            .map(|(_, b)| b.clone());
        let bmp = match reuse {
            Some(b) => unsafe { b.CopyFromMemory(None, f.img.px.as_ptr() as *const _, f.img.w * 4).ok().map(|_| b) },
            None => unsafe { self.ctx.CreateBitmap(size, Some(f.img.px.as_ptr() as *const _), f.img.w * 4, &props(D2D1_BITMAP_OPTIONS_NONE, D2D1_ALPHA_MODE_IGNORE)).ok() },
        };
        self.comp = bmp.map(|b| (*sel, b));
    }

    /// The final image of the editor (no caret, no preview parts) and its desktop rectangle.
    pub fn editor_result(&mut self) -> Option<(Rect, Canvas, Prefs)> {
        self.pull_text();
        if let Some(e) = &mut self.editor {
            e.commit_text();
        }
        self.text_input = None;
        if !self.update_doc() {
            return None;
        }
        let e = self.editor.as_ref()?;
        // The overlay closes after this; the document image is the result, no further copy.
        let img = self.doc_cache.take()?.img;
        let sel = e.sel?;
        if e.frame {
            // The framed picture is larger: a pin keeps the screenshot where it was.
            let pad = crate::beautify::FrameLayout::for_size(img.w, img.h, e.scale()).pad;
            let framed = crate::beautify::apply(&img, e.scale());
            return Some((Rect::new(sel.left - pad, sel.top - pad, sel.right + pad, sel.bottom + pad), framed, e.prefs()));
        }
        Some((sel, img, e.prefs()))
    }

    pub fn set_color(&mut self, c: crate::raster::Rgba) {
        self.pull_text();
        if let Some(e) = &mut self.editor {
            e.set_color(c);
        }
        self.refresh(true);
    }

    pub fn editor_color(&self) -> Option<crate::raster::Rgba> {
        self.editor.as_ref().map(|e| e.color)
    }

    /// A system colour dialog restores the top-level owner first; then restore the caret HWND.
    pub fn restore_editor_focus(&mut self) -> bool {
        let owner = self.text_input.as_ref().map(|input| input.owner()).unwrap_or(self.owner());
        if !activate(owner) { return false; }
        self.sync_text_input();
        self.text_input.as_ref().filter(|input| input.visible())
            .is_none_or(|input| unsafe { GetFocus() == input.hwnd })
    }

    fn draw(&self, i: usize) -> Result<()> {
        self.paint(i, &self.wins[i].target)?;
        unsafe { self.wins[i].swap.Present(0, DXGI_PRESENT(0)).ok() }
    }

    /// The look test: the same drawing as the screen, read back instead of shown.
    pub fn read_back(&self, i: usize) -> Result<Canvas> {
        let w = &self.wins[i];
        let size = D2D_SIZE_U { width: w.w, height: w.h };
        unsafe {
            let target = self.ctx.CreateBitmap(size, None, 0, &props(D2D1_BITMAP_OPTIONS_TARGET, D2D1_ALPHA_MODE_IGNORE))?;
            self.paint(i, &target)?;
            let cpu = self.ctx.CreateBitmap(size, None, 0, &props(D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_ALPHA_MODE_IGNORE))?;
            cpu.CopyFromBitmap(None, &target, None)?;
            let map = cpu.Map(D2D1_MAP_OPTIONS_READ)?;
            let mut px = vec![255u8; (w.w * w.h * 4) as usize];
            for y in 0..w.h as usize {
                let row = std::slice::from_raw_parts(map.bits.add(y * map.pitch as usize), w.w as usize * 4);
                px[y * w.w as usize * 4..(y + 1) * w.w as usize * 4].copy_from_slice(row);
            }
            let _ = cpu.Unmap();
            for p in px.as_chunks_mut::<4>().0 {
                p[3] = 255;
            }
            Ok(Canvas::from_bgra(w.w, w.h, px))
        }
    }

    fn paint(&self, i: usize, target: &ID2D1Bitmap1) -> Result<()> {
        let w = &self.wins[i];
        let m = self.snap.monitors[i].rect;
        let one = crate::dpi::px(crate::theme::BORDER_WIDTH, self.snap.monitors[i].dpi).max(1) as f32;
        unsafe {
            self.ctx.SetTarget(target);
            self.ctx.BeginDraw();
            let full = D2D_RECT_F { left: 0.0, top: 0.0, right: w.w as f32, bottom: w.h as f32 };
            self.ctx.DrawBitmap(&w.frozen, Some(&full), 1.0, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None, None);
            self.ctx.FillRectangle(&full, &self.dim);
            // Text mode: two thin lines through the cursor help line up with a row of text.
            if self.is_text() && self.drag.is_none() && self.cursor_monitor() == Some(m) {
                let (x, y) = ((self.cursor.x - m.left) as f32 + 0.5, (self.cursor.y - m.top) as f32 + 0.5);
                let d2 = |a, b| windows_numerics_point(a, b);
                self.ctx.DrawLine(d2(x, 0.0), d2(x, w.h as f32), &self.guide, one, None);
                self.ctx.DrawLine(d2(0.0, y), d2(w.w as f32, y), &self.guide, one, None);
            }
            if self.editor.is_none() && !self.is_text() {
                for sel in self.frames.rects() {
                    if let Some(local) = self.snap.layout.local_on(i, sel) {
                        let r = rf(local);
                        self.ctx.DrawBitmap(&w.frozen, Some(&r), 1.0, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, Some(&r), None);
                    }
                }
                for sel in self.frames.rects() {
                    for bar in crate::frame::selection_frame(m, *sel, one as i32) {
                        self.ctx.FillRectangle(&rf(bar.rect.offset(-m.left, -m.top)), if bar.layer == 0 { &self.inner } else { &self.outer });
                    }
                }
            }
            if let Some(sel) = self.sel_shown() {
                if let Some(local) = self.snap.layout.local_on(i, &sel) {
                    let r = rf(local);
                    match &self.comp {
                        Some((cr, bmp)) if *cr == sel && self.editor.is_some() => {
                            let dst = rf(sel.offset(-m.left, -m.top));
                            self.ctx.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED);
                            self.ctx.DrawBitmap(bmp, Some(&dst), 1.0, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None, None);
                            self.ctx.PopAxisAlignedClip();
                        }
                        _ => self.ctx.DrawBitmap(&w.frozen, Some(&r), 1.0, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, Some(&r), None),
                    }
                }
                // The frame is bars inside this monitor (frame.rs): none on a monitor the selection
                // does not overlap, none past the monitor's edge.
                let text = self.is_text();
                let ipx = one as i32;
                let local = |r: Rect| rf(r.offset(-m.left, -m.top));
                for bar in crate::frame::selection_frame(m, sel, ipx) {
                    let brush = match (bar.layer, text) {
                        (0, true) => &self.marker,
                        (0, false) => &self.inner,
                        (_, true) => &self.marker_ink,
                        (_, false) => &self.outer,
                    };
                    self.ctx.FillRectangle(&local(bar.rect), brush);
                }
                if text {
                    // L corners: the frame stays visible on a yellow or a white page.
                    let len = crate::dpi::px(14.0, self.snap.monitors[i].dpi);
                    let th = crate::dpi::px(3.0, self.snap.monitors[i].dpi).max(2);
                    for bar in crate::frame::corner_bars(m, sel, th, len) {
                        self.ctx.FillRectangle(&local(bar), &self.marker);
                    }
                }
                if let Some(e) = &self.editor {
                    let radius = crate::theme::HANDLE_RADIUS * e.scale();
                    for h in e.handle_rects() {
                        // A handle on a seam belongs to the monitor that holds its centre.
                        if !crate::frame::handle_belongs(m, (h.left + h.right) / 2, (h.top + h.bottom) / 2) {
                            continue;
                        }
                        let hr = rf(h.offset(-m.left, -m.top));
                        let mut disc = D2D1_ELLIPSE { radiusX: radius, radiusY: radius, ..Default::default() };
                        disc.point.X = (hr.left + hr.right) / 2.0;
                        disc.point.Y = (hr.top + hr.bottom) / 2.0;
                        self.ctx.FillEllipse(&disc, &self.handle_fill);
                        self.ctx.DrawEllipse(&disc, &self.handle_ring, one, None);
                    }
                }
            }
            for p in &self.ui {
                if p.rect().intersect(&m).is_some() {
                    let d = rf(p.rect().offset(-m.left, -m.top));
                    self.ctx.DrawBitmap(&p.bmp, Some(&d), 1.0, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None, None);
                }
            }
            let r = self.ctx.EndDraw(None, None);
            self.ctx.SetTarget(None);
            r
        }
    }

    fn visible_rects(&self) -> Vec<Rect> {
        let mut v: Vec<Rect> = self.ui.iter().map(|p| p.rect()).collect();
        if self.is_text() && let Some(m) = self.cursor_monitor() {
            v.push(m);
        }
        if self.editor.is_none() && !self.is_text() {
            let g = crate::theme::CROSSHAIR_SIZE.ceil() as i32;
            v.extend(self.frames.rects().iter().map(|s| Rect::new(s.left - g, s.top - g, s.right + g, s.bottom + g)));
        }
        if let Some(s) = self.sel_shown() {
            let g = crate::theme::CROSSHAIR_SIZE.ceil() as i32;
            v.push(Rect::new(s.left - g, s.top - g, s.right + g, s.bottom + g));
        }
        v
    }

    /// Rebuild what changed and redraw the monitors it touches. `content`: the selection's
    /// pixels changed even if no rectangle moved.
    fn refresh(&mut self, content: bool) {
        let frame_ms = if self.heavy_frame { HEAVY_FRAME_MS } else { FRAME_MS };
        if self.moving && !self.no_throttle && self.last_frame != 0 && crate::log::ms_since(self.last_frame) < frame_ms {
            // A later move or the timer draws the newest state; nothing is lost.
            self.pending = true;
            if let Some(w) = self.wins.first() {
                unsafe { SetTimer(Some(w.hwnd), FRAME_TIMER, frame_ms.ceil() as u32, None) };
            }
            return;
        }
        self.pending = false;
        self.heavy_frame = false;
        self.sync_text_input();
        if self.editor.as_ref().is_some_and(|e| e.needs_comp()) {
            self.rebuild_comp();
        } else {
            // Nothing drawn yet: the frozen texture is the picture; no readback and no copies.
            self.comp = None;
            self.frame = None;
        }
        // During a drawing drag the bars and labels are as they were.
        if !(self.moving && self.editor.as_ref().is_some_and(|e| e.is_drawing())) {
            self.rebuild_ui();
        }
        let now = self.visible_rects();
        if !content && now == self.shown && self.editor.is_none() && self.drag.is_none() && !self.is_text() {
            return; // hovering inside the same window: nothing changed on screen
        }
        for i in 0..self.wins.len() {
            let m = &self.snap.monitors[i].rect;
            if now.iter().chain(self.shown.iter()).any(|r| r.intersect(m).is_some()) {
                let _ = self.draw(i);
            }
        }
        self.shown = now;
        self.last_frame = crate::log::qpc();
        if let Some(t) = &self.text_input {
            t.repaint();
        }
    }

    /// The frame timer fired: draw what a fast drag skipped.
    pub fn flush_frame(&mut self) {
        if let Some(w) = self.wins.first() {
            unsafe {
                let _ = KillTimer(Some(w.hwnd), FRAME_TIMER);
            }
        }
        if self.pending {
            self.moving = false;
            self.refresh(true);
        }
    }

    /// Read synchronously before a click/export: an EN_CHANGE notification may still be queued.
    fn pull_text(&mut self) {
        if let (Some(input), Some(e)) = (&self.text_input, &mut self.editor)
            && e.text.as_ref().is_some_and(|t| t.id == input.id)
        {
            e.set_text(&crate::textinput::read(input.hwnd));
        }
    }

    fn sync_text_input(&mut self) {
        let Some(e) = &self.editor else {
            self.text_input = None;
            return;
        };
        let Some(text) = &e.text else {
            self.text_input = None;
            return;
        };
        let Some(bounds) = e.sel else {
            self.text_input = None;
            return;
        };
        let scale = e.scale();
        let visible = !e.palette_open && !e.size_open;
        if !self
            .text_input
            .as_ref()
            .is_some_and(|input| input.id == text.id)
        {
            self.text_input = None;
            let point = Point::new(text.at.x.round() as i32, text.at.y.round() as i32);
            let monitor = self.snap.layout.nearest_monitor(point).unwrap_or(0);
            let Some(win) = self.wins.get(monitor) else {
            return;
        };
            let main = crate::app::main_hwnd();
            match crate::textinput::TextInput::new(
                win.hwnd,
                main,
                text,
                &self.theme,
                bounds,
                scale,
                self.cursor,
            ) {
                Ok(input) => self.text_input = Some(input),
                Err(err) => {
                    crate::logf!(
                        "text input creation FAILED {:#010x} {}",
                        err.code().0,
                        err.message()
                    );
                    if let Some(e) = &mut self.editor {
                        e.cancel_text();
                    }
                    return;
                }
            }
        }
        if let (Some(input), Some(e)) = (&mut self.text_input, &self.editor)
            && let Some(text) = &e.text
        {
            input.update(text, bounds, scale);
            input.show(visible);
        }
    }

    pub fn on_text_input(&mut self, hwnd: HWND, message: u32, value: isize) -> Outcome {
        if !self.text_input.as_ref().is_some_and(|t| t.hwnd == hwnd) {
            return Outcome::Nothing;
        }
        self.pull_text();
        let Some(e) = &mut self.editor else {
            return Outcome::Nothing;
        };
        match message {
            crate::textinput::CHANGED => {
                // EDIT grew before painting and owns caret/IME. No screen-sized bitmap per key.
                Outcome::Nothing
            }
            crate::textinput::COMMIT => {
                e.commit_text();
                self.refresh(true);
                Outcome::Nothing
            }
            crate::textinput::CANCEL => {
                e.cancel_text();
                self.refresh(true);
                Outcome::Nothing
            }
            crate::textinput::SIZE => {
                let effect = e.on_wheel(value as i32);
                self.effect(effect)
            }
            crate::textinput::KEY => {
                e.commit_text();
                let effect = e.on_key((value & 0xFF) as u32, true, value & 0x100 != 0);
                self.effect(effect)
            }
            _ => Outcome::Nothing,
        }
    }

    fn effect(&mut self, e: Effect) -> Outcome {
        match e {
            Effect::None => Outcome::Nothing,
            Effect::Redraw => {
                self.refresh(true);
                Outcome::Nothing
            }
            Effect::SelectionChanged => {
                self.refresh(true);
                Outcome::Nothing
            }
            Effect::Nudge(dx, dy) => {
                let mut pt = windows::Win32::Foundation::POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut pt);
                }
                let p = self.snap.layout.nudge(Point::new(pt.x, pt.y), dx, dy);
                unsafe {
                    let _ = SetCursorPos(p.x, p.y); // the window gets WM_MOUSEMOVE and redraws
                }
                Outcome::Nothing
            }
            Effect::Copy | Effect::Save | Effect::SaveAs | Effect::Pin => Outcome::Edited(e),
            Effect::CopyText => match self.editor.as_ref().and_then(|ed| ed.sel) {
                Some(sel) => {
                    let work = self.has_work();
                    self.refresh(true);
                    Outcome::TextArea(sel, work)
                }
                None => Outcome::Nothing,
            },
            Effect::CustomColor => {
                self.refresh(true);
                Outcome::CustomColor
            }
            Effect::PickColor { at, copy } => {
                let Some(c) = self.frozen_pixel(at) else { return Outcome::Nothing };
                if copy {
                    // Shift+click: the hex goes to the clipboard as text, through the one text route.
                    let outcome = crate::textclip::publish(self.owner(), &c.to_hex());
                    crate::logf!("picker copied: {}", crate::textcore::TextOutcome::name_of_code(outcome.done_code()));
                }
                let eff = self.editor.as_mut().map(|e| e.picked_color(c)).unwrap_or(Effect::None);
                self.effect(eff)
            }
            Effect::Close => Outcome::Cancel,
        }
    }

    /// Mouse input from window `i`. `x`, `y` are client coordinates of that window.
    pub fn on_mouse(&mut self, i: usize, msg: u32, x: i32, y: i32) -> Outcome {
        self.on_mouse_mods(i, msg, x, y, Self::shift_held(), ctrl_held())
    }

    // Explicit modifiers keep the native selftest on owned windows, without SendInput.
    pub(crate) fn on_mouse_mods(&mut self, i: usize, msg: u32, x: i32, y: i32, shift: bool, ctrl: bool) -> Outcome {
        if matches!(msg, WM_LBUTTONDOWN|WM_RBUTTONDOWN) { self.parts.mode.interrupt(); }
        if msg != WM_MOUSEMOVE { self.parts.mode.interrupt(); }
        self.moving = msg == WM_MOUSEMOVE;
        let out = self.mouse(i, msg, x, y, shift, ctrl);
        if out == Outcome::Cancel && self.editor.is_none() { self.frames.clear(); }
        self.moving = false;
        self.sync_fence(&out);
        out
    }

    /// The crossing key (Alt, without Ctrl: that is AltGr) lets a drag go to another monitor.
    fn crossing_key(&self) -> bool {
        if self.synthetic_input {
            return self.synthetic_crossing;
        }
        let down = |vk: u16| unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 };
        crossing_held(down(VK_MENU.0), down(VK_CONTROL.0))
    }

    /// A mouse button is physically down (either, for swapped buttons). A lost button-up must not
    /// leave the mouse clipped.
    fn button_down(&self) -> bool {
        if self.synthetic_input {
            return true;
        }
        let down = |vk: u16| unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 };
        down(VK_LBUTTON.0) || down(VK_RBUTTON.0)
    }

    /// Where the drag in progress may go: its start monitor, or for editor drawing the monitors
    /// the selection touches. None: no fenced drag.
    fn drag_area(&self) -> Option<Rect> {
        let monitor_of = |p: Point| self.mons.at(p).map(|m| m.rect);
        match &self.editor {
            Some(e) => match e.drag_class()? {
                DragClass::Select(start) => monitor_of(start),
                DragClass::Draw => {
                    let rects: Vec<Rect> = self.mons.list.iter().map(|m| m.rect).collect();
                    e.sel.and_then(|s| draw_area(&s, &rects)).or_else(|| monitor_of(self.cursor))
                }
            },
            None => self.drag.and_then(|(a, _)| monitor_of(a)),
        }
    }

    /// Bring the clip in line with the drag state; a finished outcome ends it at once.
    fn sync_fence(&mut self, out: &Outcome) {
        if *out != Outcome::Nothing {
            self.fence.end();
            return;
        }
        match self.drag_area() {
            None => self.fence.end(),
            Some(_) if !self.button_down() => self.fence.end(),
            Some(area) => {
                let free = self.crossing_key();
                if self.fence.area() == Some(area) {
                    self.fence.update(self.cursor, free);
                } else {
                    self.fence.begin(area, free);
                }
            }
        }
    }

    /// The crossing key went down or up.
    pub fn crossing_key_changed(&mut self) {
        self.sync_fence(&Outcome::Nothing);
    }

    /// Focus or mouse capture was lost: the mouse is free until this drag is over.
    pub fn release_fence(&mut self) {
        self.fence.suspend();
    }

    /// Self-test: the clip as the overlay asked for it.
    pub fn fence_clipped(&self) -> bool {
        self.fence.clipped()
    }

    /// Self-test: replace the clip with a recording sink.
    pub fn set_fence_sink(&mut self, sink: Box<dyn ClipSink>) {
        self.fence = Driver::new(sink);
    }

    fn mouse(&mut self, i: usize, msg: u32, x: i32, y: i32, shift: bool, ctrl: bool) -> Outcome {
        let Some(m) = self.snap.monitors.get(i) else {
            return Outcome::Nothing;
        };
        let p = self
            .snap
            .layout
            .clamp_point(Point::new(x + m.rect.left, y + m.rect.top));
        self.cursor = p;
        if self.editor.is_some() {
            if msg != WM_MOUSEMOVE {
                self.pull_text();
            }
            match msg {
                WM_LBUTTONDOWN => unsafe {
                    SetCapture(self.wins[i].hwnd);
                },
                WM_LBUTTONUP => unsafe {
                    let _ = ReleaseCapture();
                },
                _ => {}
            }
            let e = self.editor.as_mut().unwrap();
            let eff = match msg {
                WM_MOUSEMOVE => e.on_move(p, shift),
                WM_LBUTTONDOWN => e.on_down_mods(p, shift, ctrl, &self.glyphs),
                WM_LBUTTONUP => e.on_up(p, shift),
                WM_RBUTTONUP => {
                    if e.phase == crate::editor::Phase::Selecting {
                        Effect::Close
                    } else if e.tool.is_some() {
                        // Right-click drops the tool and reveals resize handles; Ctrl still owns moving.
                        e.drop_tool()
                    } else {
                        Effect::None
                    }
                }
                _ => Effect::None,
            };
            return self.effect(eff);
        }
        let bar_item = self
            .mode_bar
            .as_ref()
            .and_then(|(at, b)| b.hit(p.x - at.x, p.y - at.y));
        let out = match msg {
            WM_MOUSEMOVE => {
                if let Some((a, _)) = self.drag {
                    self.drag = Some((a, p));
                } else if !self.is_text() {
                    self.hover = window_at(&self.window_rects(), p);
                    self.sync_parts();
                }
                if bar_item != self.bar_hover {
                    self.bar_hover = bar_item;
                    self.refresh(true);
                }
                Outcome::Nothing
            }
            WM_LBUTTONDOWN => {
                if let Some(it) = bar_item {
                    return match it {
                        Item::Close => Outcome::Cancel,
                        _ => {
                            self.mode = it;
                            if self.is_text() { self.frames.clear(); self.parts.stop(); self.sync_parts(); }
                            self.refresh(true);
                            Outcome::Nothing
                        }
                    };
                }
                unsafe {
                    SetCapture(self.wins[i].hwnd);
                }
                self.drag_add = shift && !self.is_text() && !self.record_mode;
                self.drag = Some((p, p));
                Outcome::Nothing
            }
            WM_LBUTTONUP => {
                unsafe {
                    let _ = ReleaseCapture();
                }
                match self.drag.take() {
                    // Too small to read is a cancel, without a word (WinTextCapture's rule).
                    Some((a, _)) if self.is_text() => {
                        let r = drag_rect(a, p);
                        if crate::textcore::selection_usable(r.width(), r.height()) { Outcome::TextArea(r, false) } else { Outcome::Cancel }
                    }
                    Some((a, _)) if !self.record_mode && (shift || self.drag_add || !self.frames.is_empty()) => {
                        let adding = shift || self.drag_add;
                        self.drag_add = false;
                        self.hover = window_at(&self.window_rects(), p);
                        let rect = if !is_click(a, p) {
                            self.snap.layout.clip(&drag_rect(a, p))
                        } else {
                            self.parts.current().filter(|_| self.parts.mode.active()).or_else(|| window_at(&self.window_rects(), p).and_then(|k| self.snap.layout.clip(&self.snap.layout.snap_window(&self.snap.windows[k].rect))))
                        };
                        if adding && is_click(a, p) {
                            if self.parts.mode.active() && self.parts.current().is_some() {
                                if let Some(r)=rect { self.frames.toggle_rect(r); }
                            } else { self.frames.toggle(p, rect); }
                        }
                        else if let Some(r) = rect { self.frames.add(r); }
                        if adding { Outcome::Nothing }
                        else { self.frames.union().map(|r| Outcome::Commit(r, Kind::Frames)).unwrap_or(Outcome::Nothing) }
                    }
                    Some((a, _)) if self.mode == Item::ModeRegion && !is_click(a, p) => {
                        if self.record_mode {
                            let monitors: Vec<Rect> = self.mons.list.iter().map(|m|m.rect).collect();
                            crate::record::selection(drag_rect(a, p), a, &monitors).map_or(Outcome::Cancel, |r|Outcome::Commit(r, Kind::Rectangle))
                        } else { Outcome::Commit(drag_rect(a, p), Kind::Rectangle) }
                    }
                    Some(_) => {
                        self.hover = window_at(&self.window_rects(), p);
                        match self.current() {
                            Some((r, k)) if k != Kind::Rectangle => Outcome::Commit(r, k),
                            _ => Outcome::Nothing,
                        }
                    }
                    None => Outcome::Nothing,
                }
            }
            // On the release: closing on the press would hand the release to the program under
            // the overlay, and it would open its context menu.
            WM_RBUTTONUP => Outcome::Cancel,
            _ => Outcome::Nothing,
        };
        if out == Outcome::Nothing {
            self.refresh(false);
        }
        out
    }

    /// Self-test: press, move and release on the overlay's own windows; nothing reaches the desktop.
    pub fn selftest_drag(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) -> Outcome {
        let local = |s: &Self, x: i32, y: i32| s.snap.layout.nearest_monitor(Point::new(x, y)).map(|i| (i, x - s.snap.monitors[i].rect.left, y - s.snap.monitors[i].rect.top));
        let (Some((i, ax, ay)), Some((j, bx, by))) = (local(self, x0, y0), local(self, x1, y1)) else { return Outcome::Nothing };
        self.on_mouse_mods(i, WM_MOUSEMOVE, ax, ay, false, false);
        self.on_mouse_mods(i, WM_LBUTTONDOWN, ax, ay, false, false);
        let (ex, ey) = (bx + self.snap.monitors[j].rect.left - self.snap.monitors[i].rect.left, by + self.snap.monitors[j].rect.top - self.snap.monitors[i].rect.top);
        self.on_mouse_mods(i, WM_MOUSEMOVE, ex, ey, false, false);
        self.on_mouse_mods(i, WM_LBUTTONUP, ex, ey, false, false)
    }

    pub fn on_wheel(&mut self, delta: i32) -> Outcome {
        self.pull_text();
        match self.editor.as_mut() {
            Some(e) => {
                let eff = e.on_wheel(delta);
                self.effect(eff)
            }
            None => {
                if self.drag.is_some() || self.is_text() { self.parts.mode.interrupt(); return Outcome::Nothing; }
                if self.parts_eligible() && self.drag.is_none() {
                    self.sync_parts();
                    let len=self.parts.selection().1.len();
                    if !self.parts.mode.active() && delta<0 { self.parts.mode.window(self.hover.map(|i| self.snap.windows[i].hwnd)); }
                    self.parts.mode.wheel(delta,len);
                    self.sync_parts();
                    self.refresh(true);
                }
                Outcome::Nothing
            },
        }
    }

    pub fn on_char(&mut self, c: char) -> Outcome {
        match self.editor.as_mut() {
            Some(e) => {
                let eff = e.on_char(c);
                self.effect(eff)
            }
            None => Outcome::Nothing,
        }
    }

    /// Esc cancels; Enter takes the drag so far, or the whole monitor under the cursor; arrows move
    /// the cursor by 1 pixel (10 with Shift). In the editor every key goes to `Editor::on_key`.
    pub fn on_key(&mut self, vk: u32, shift: bool) -> Outcome {
        // AltGr must use physical Ctrl: shortcut_ctrl_held intentionally hides Ctrl+Alt.
        if vk == VK_MENU.0 as u32 { self.on_alt(true); return Outcome::Nothing; }
        self.on_key_mods(vk, shortcut_ctrl_held(), shift)
    }

    pub(crate) fn on_key_mods(&mut self, vk: u32, ctrl: bool, shift: bool) -> Outcome {
        if vk == VK_MENU.0 as u32 { self.on_alt_mods(true,ctrl); return Outcome::Nothing; }
        self.parts.mode.interrupt();
        let out = self.key_mods(vk, ctrl, shift);
        if out == Outcome::Cancel && self.editor.is_none() { self.frames.clear(); }
        // Esc always ends a drag and its clip, whatever the mode answers.
        self.sync_fence(&if vk == VK_ESCAPE && out == Outcome::Nothing { Outcome::Cancel } else { out });
        out
    }

    fn key_mods(&mut self, vk: u32, ctrl: bool, shift: bool) -> Outcome {
        self.pull_text();
        if let Some(e) = self.editor.as_mut() {
            let eff = e.on_key(vk, ctrl, shift);
            return self.effect(eff);
        }
        match vk {
            VK_ESCAPE => Outcome::Cancel,
            // Reading a whole monitor by accident helps nobody: text needs a drag.
            VK_RETURN if self.is_text() => Outcome::Nothing,
            VK_RETURN => {
                if let Some(r) = self.frames.union() { return Outcome::Commit(r, Kind::Frames); }
                self.workarea_commit = shift;
                if !shift && let Some((sel, Kind::Rectangle)) = self.current() {
                    return Outcome::Commit(sel, Kind::Rectangle);
                }
                let i = self.snap.layout.nearest_monitor(self.cursor).unwrap_or(0);
                match self.snap.monitors.get(i) {
                    Some(m) => Outcome::Commit(if shift { m.work } else { m.rect }, Kind::Monitor),
                    None => Outcome::Cancel,
                }
            }
            0x08 if !self.is_text() => {
                self.frames.remove_last();
                self.refresh(true);
                Outcome::Nothing
            }
            VK_LEFT | VK_UP | VK_RIGHT | VK_DOWN => {
                let step = if shift { 10 } else { 1 };
                let (dx, dy) = match vk {
                    VK_LEFT => (-step, 0),
                    VK_RIGHT => (step, 0),
                    VK_UP => (0, -step),
                    _ => (0, step),
                };
                self.effect(Effect::Nudge(dx, dy))
            }
            _ => Outcome::Nothing,
        }
    }

    // Recording picks one rectangle on one monitor: no inner parts or frame sets there.
    fn parts_eligible(&self) -> bool { self.editor.is_none() && !self.is_text() && !self.record_mode }

    fn sync_parts(&mut self) {
        if !self.parts_eligible() { return; }
        self.parts.sync(&self.snap,self.hover,self.cursor,self.wins.first().map(|w| w.hwnd.0 as isize).unwrap_or(0));
        unsafe {
            if let Some(w)=self.wins.first() {
                if self.parts.mode.active() { SetTimer(Some(w.hwnd),crate::partworker::TIMER,25,None); }
                else { let _=KillTimer(Some(w.hwnd),crate::partworker::TIMER); }
            }
        }
    }
    pub fn on_alt(&mut self, down: bool) { self.on_alt_mods(down,ctrl_held()); }
    pub(crate) fn on_alt_mods(&mut self, down: bool, ctrl: bool) {
        let before=self.parts.mode.active();
        self.parts.mode.alt(down,self.parts_eligible() && self.drag.is_none() && !ctrl);
        if before!=self.parts.mode.active() { crate::logf!("parts_on={}",self.parts.mode.active()); }
        self.crossing_key_changed();
        self.sync_parts(); self.refresh(true);
    }
    pub fn on_parts(&mut self) {
        if self.parts_eligible() && self.parts.mode.active() && self.drag.is_none() {
            self.parts.poll();
            self.parts.log_used();
            let visual=(self.parts.label(self.lang),self.parts.current());
            if self.parts_visual.as_ref()!=Some(&visual) {
                self.parts_visual=Some(visual); self.refresh(true);
            }
        }
    }
    pub fn parts_metadata(&self, kind: Kind) -> String {
        if kind==Kind::Part { self.parts.metadata() } else { String::new() }
    }

    pub fn frame_count(&self) -> usize { self.frames.len() }

    pub fn commit_name(&self, kind: Kind) -> &'static str {
        if kind == Kind::Monitor && self.workarea_commit { "workarea" } else { kind.name() }
    }

    pub fn shift_held() -> bool {
        unsafe { (GetAsyncKeyState(VK_SHIFT.0 as i32) as u16 & 0x8000) != 0 }
    }

    pub fn is_editor(&self) -> bool {
        self.editor.is_some()
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.parts.stop();
        self.parts = Default::default(); // stop worker notifications before HWND destruction
        self.fence.end(); // the mouse is free before any window goes
        self.text_input = None; // child owns its font/brushes until DestroyWindow completes
        unsafe {
            self.ctx.SetTarget(None);
            self.ui.clear();
            self.comp = None;
            self.frame = None;
            for w in std::mem::take(&mut self.wins) {
                let hwnd = w.hwnd;
                let _ = ShowWindow(hwnd, SW_HIDE);
                drop(w);
                let _ = DestroyWindow(hwnd);
            }
        }
    }
}

/// Ask for the foreground from a helper thread, so the UI thread never waits for it. On the hidden
/// test desktop the first activation call on the UI thread, BringWindowToTop or SetForegroundWindow,
/// often took about 2 s, while SetForegroundWindow on a helper thread took about 1 ms. Why is not
/// known, and the wait has not been seen on a real screen. WM_ACTIVATEAPP then reports the keyboard
/// on the UI thread. Windows may also refuse (a hook or a timer started this snip, not a key it
/// credits to us). The overlay stays open then: Esc, Enter and the arrows come through the hook and
/// the first click takes the keyboard.
fn ask_foreground(hwnd: HWND) {
    let raw = hwnd.0 as isize;
    let _ = std::thread::Builder::new().name("snip-foreground".into()).stack_size(64 * 1024).spawn(move || {
        let t = crate::log::qpc();
        let ok = unsafe { SetForegroundWindow(HWND(raw as *mut _)) }.as_bool();
        crate::logf!("foreground {} in {:.1} ms", if ok { "granted" } else { "refused" }, crate::log::ms_since(t));
    });
}

/// Give the foreground back to `hwnd` after the overlay closed, without letting a slow answer hold
/// up the clipboard write that follows. The short wait is for a process about to exit: it would end
/// a request that had not started yet.
pub fn give_foreground(hwnd: HWND) {
    let raw = hwnd.0 as isize;
    let (done, wait) = std::sync::mpsc::channel();
    let started = std::thread::Builder::new().name("snip-restore-focus".into()).stack_size(64 * 1024).spawn(move || {
        let _ = unsafe { SetForegroundWindow(HWND(raw as *mut _)) };
        let _ = done.send(());
    });
    if started.is_ok() {
        let _ = wait.recv_timeout(std::time::Duration::from_millis(50));
    }
}

/// Request activation without attaching our input queue to another application's thread.
/// A successful API return alone is insufficient: verify foreground and keyboard focus.
pub fn activate(hwnd: HWND) -> bool {
    unsafe {
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        GetForegroundWindow() == hwnd && GetFocus() == hwnd
    }
}

/// Client-area origin check used by the self-test: a window placed at a monitor's rect.
pub fn window_rect(hwnd: HWND) -> Rect {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(hwnd, &mut r);
    }
    Rect::new(r.left, r.top, r.right, r.bottom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;

    thread_local! {
        static CREATED: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
    }

    unsafe extern "system" fn reject_second_window(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_NCCREATE {
            let reject = CREATED.with(|g| {
                let mut windows = g.borrow_mut();
                windows.push(hwnd);
                windows.len() == 2
            });
            if reject {
                return LRESULT(0);
            }
        }
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    }

    #[test]
    fn audit_later_window_creation_failure_destroys_earlier_windows() {
        use crate::capture::{self, Frame, Method, MonitorInfo};
        use crate::geometry::Layout;
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
            let gpu = capture::create_gpu().unwrap();
            let hinst: HINSTANCE = GetModuleHandleW(None).unwrap().into();
            // These windows are never shown. Refusing the second WM_NCCREATE exercises the
            // error before its swap chain exists, after the first window was fully built.
            let wc = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(reject_second_window), hInstance: hinst, lpszClassName: CLASS, ..Default::default() };
            assert_ne!(RegisterClassExW(&wc), 0);
            let rects = vec![Rect::from_xywh(0, 0, 64, 64), Rect::from_xywh(64, 0, 64, 64)];
            let snap = Snapshot {
                monitors: rects.iter().map(|r| MonitorInfo { hmon: Default::default(), rect: *r, work: *r, name: "test".into(), dpi: 96, primary: r.left == 0 }).collect(),
                frames: rects.iter().map(|_| Frame { width: 64, height: 64, pixels: Pixels::Cpu(vec![255; 64 * 64 * 4]), method: Method::Gdi, ms: 0.0, zero_present: false, note: String::new() }).collect(),
                layout: Layout::new(rects),
                gpu: Some(gpu),
                windows: vec![],
                gpu_ms: 0.0,
            };
            let result = Overlay::new(snap, hinst, &Opts::plain(0.45));
            let failed = result.is_err();
            drop(result);
            let windows = CREATED.with(|g| std::mem::take(&mut *g.borrow_mut()));
            let leaked = windows.iter().any(|hwnd| IsWindow(Some(*hwnd)).as_bool());
            for hwnd in &windows {
                if IsWindow(Some(*hwnd)).as_bool() {
                    let _ = DestroyWindow(*hwnd);
                }
            }
            UnregisterClassW(CLASS, Some(hinst)).unwrap();
            CoUninitialize();
            assert!(failed, "the second window creation must fail");
            assert_eq!(windows.len(), 2, "both window creations must have been attempted");
            assert!(!leaked, "an earlier overlay window survived constructor failure");
        }
    }
}
