//! A stock Unicode EDIT owns typing, caret, selection, surrogate pairs and Windows IME.
//! The core owns the document. Only a completed edit becomes one undo operation.
//!
//! The EDIT lives in its own small top-level window owned by the overlay. The overlay window has
//! no GDI surface (WS_EX_NOREDIRECTIONBITMAP) and shows a flip-model swap chain, so a GDI child of
//! it has nothing to draw into; the host has its own surface and the compositor keeps it above.

use crate::editor::TextEdit;
use crate::gdi::GdiGlyphs;
use crate::geometry::{Point, Rect};
use crate::raster::Rgba;
use crate::theme::Theme;
use std::cell::Cell;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::{EM_CHARFROMPOS, EM_GETSEL, EM_SETLIMITTEXT, EM_SETRECTNP, EM_SETSEL};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus, GetKeyState, SetFocus};
use windows::Win32::UI::Shell::{
    DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result, w};

pub const CHANGED: u32 = WM_APP + 70;
pub const COMMIT: u32 = WM_APP + 71;
pub const CANCEL: u32 = WM_APP + 72;
pub const SIZE: u32 = WM_APP + 73;
/// Finish the text, then run this editor shortcut (lParam: vk, bit 8 = Shift).
pub const KEY: u32 = WM_APP + 74;
const SUBCLASS: usize = 0x534E4950;
const HOST_CLASS: PCWSTR = w!("Laureth.snip.textbox");

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // Ctrl+drag over the text box moves the selection: let it fall through to the overlay.
        WM_NCHITTEST if unsafe { GetKeyState(0x11) } < 0 => LRESULT(HTTRANSPARENT as isize),
        WM_COMMAND if lp.0 != 0 && notification(HWND(lp.0 as *mut _), ((wp.0 >> 16) & 0xFFFF) as u32) => LRESULT(0),
        WM_CTLCOLOREDIT => match control_color(HWND(lp.0 as *mut _), HDC(wp.0 as *mut _)) {
            Some(brush) => LRESULT(brush.0 as isize),
            None => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        },
        WM_ERASEBKGND => LRESULT(1),
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// A second registration in the same process (self-test) is fine to fail.
pub fn register(hinst: windows::Win32::Foundation::HINSTANCE) {
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(host_proc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_IBEAM).unwrap_or_default(),
            lpszClassName: HOST_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

struct State {
    main: HWND,
    owner: HWND,
    anchor: Cell<Point>,
    bounds: Cell<Rect>,
    scale: Cell<f32>,
    px: Cell<f32>,
    ink: Cell<Rgba>,
    bg: HBRUSH,
    bg_color: Rgba,
    border: HBRUSH,
    composing: Cell<bool>,
    sizing: Cell<bool>,
    glyphs: GdiGlyphs,
}

impl Drop for State {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.bg.into());
            let _ = DeleteObject(self.border.into());
        }
    }
}

pub struct TextInput {
    pub hwnd: HWND,
    pub host: HWND,
    pub id: u64,
    state: Box<State>,
}

fn color(c: Rgba) -> COLORREF {
    COLORREF(c.r as u32 | (c.g as u32) << 8 | (c.b as u32) << 16)
}

pub fn read(hwnd: HWND) -> String {
    unsafe {
        let count = GetWindowTextLengthW(hwnd).clamp(0, 4000) as usize;
        let mut text = vec![0u16; count + 1];
        let n = GetWindowTextW(hwnd, &mut text).max(0) as usize;
        String::from_utf16_lossy(&text[..n]).replace("\r\n", "\n")
    }
}

fn post(s: &State, hwnd: HWND, message: u32, value: isize) {
    unsafe {
        let _ = PostMessageW(
            Some(s.main),
            message,
            WPARAM(hwnd.0 as usize),
            LPARAM(value),
        );
    }
}

unsafe fn state_of(hwnd: HWND) -> Option<&'static State> {
    let mut data = 0;
    if unsafe { GetWindowSubclass(hwnd, Some(edit_proc), SUBCLASS, Some(&mut data)) }.as_bool()
        && data != 0
    {
        // The owner removes the subclass and destroys EDIT before dropping this allocation.
        Some(unsafe { &*(data as *const State) })
    } else {
        None
    }
}

/// EN_UPDATE is sent before the native text paints. Grow the same HWND, at the same anchor,
/// without resetting its buffer, selection or IME composition.
fn autosize(hwnd: HWND, s: &State) {
    if s.sizing.replace(true) {
        return;
    }
    let text = read(hwnd);
    let px = s.px.get();
    let pad = (crate::theme::TEXT_INPUT_PAD * s.scale.get()).round() as i32;
    let ink_pad = crate::gdi::text_overhang(px);
    let line_h = s.glyphs.measure("", px).1;
    let width = text
        .split('\n')
        .map(|line| s.glyphs.measure(line, px).0)
        .max()
        .unwrap_or(0);
    let lines = text.split('\n').count().max(1) as i32;
    let at = s.anchor.get();
    let bounds = s.bounds.get();
    let x = at.x - pad;
    let y = at.y - pad;
    let wanted_w = (width
        + 2 * (pad + ink_pad)
        + (crate::theme::TEXT_INPUT_CARET_SPACE * s.scale.get()).ceil() as i32)
        .max((crate::theme::TEXT_INPUT_MIN_WIDTH * s.scale.get()).round() as i32);
    let w = wanted_w.min(bounds.right - x).max(2 * pad + 1);
    let h = (lines * line_h + 2 * pad)
        .min(bounds.bottom - y)
        .max(2 * pad + 1);
    unsafe {
        // The host is top-level: desktop coordinates. The EDIT fills it.
        let host = GetParent(hwnd).unwrap_or_default();
        let mut old = RECT::default();
        let _ = GetWindowRect(hwnd, &mut old);
        if (
            old.left,
            old.top,
            old.right - old.left,
            old.bottom - old.top,
        ) != (x, y, w, h)
        {
            let flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS;
            let _ = SetWindowPos(host, None, x, y, w, h, flags);
            let _ = SetWindowPos(hwnd, None, 0, 0, w, h, flags);
        }
        let rect = RECT {
            left: pad + ink_pad,
            top: pad,
            right: (w - pad - ink_pad).max(pad + ink_pad + 1),
            bottom: (h - pad).max(pad + 1),
        };
        SendMessageW(
            hwnd,
            EM_SETRECTNP,
            None,
            Some(LPARAM(&rect as *const _ as isize)),
        );
    }
    s.sizing.set(false);
}

pub fn notification(hwnd: HWND, code: u32) -> bool {
    let Some(s) = (unsafe { state_of(hwnd) }) else {
        return false;
    };
    if code == EN_UPDATE {
        autosize(hwnd, s);
    }
    if code == EN_CHANGE {
        post(s, hwnd, CHANGED, 0);
    }
    true
}

pub fn control_color(hwnd: HWND, dc: HDC) -> Option<HBRUSH> {
    let s = unsafe { state_of(hwnd) }?;
    unsafe {
        SetTextColor(dc, color(s.ink.get()));
        SetBkColor(dc, color(s.bg_color));
    }
    Some(s.bg)
}

unsafe extern "system" fn edit_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    let s = unsafe { &*(data as *const State) };
    match msg {
        // Let the overlay receive Ctrl+drag even over the native text box.
        WM_NCHITTEST if unsafe { GetKeyState(0x11) } < 0 => return LRESULT(HTTRANSPARENT as isize),
        WM_RBUTTONDOWN | WM_CONTEXTMENU => return LRESULT(0),
        WM_RBUTTONUP => {
            // Preserve Snip's right-click-to-finish/drop-tool inside the native child too.
            // Post: the parent may destroy this child after reading its final Unicode buffer.
            unsafe {
                let mut child_rect = RECT::default();
                let mut owner_rect = RECT::default();
                let _ = GetWindowRect(hwnd, &mut child_rect);
                let _ = GetWindowRect(s.owner, &mut owner_rect);
                let x = (lp.0 & 0xFFFF) as i16 as i32 + child_rect.left - owner_rect.left;
                let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32 + child_rect.top - owner_rect.top;
                let xy = ((y as u32 & 0xFFFF) << 16) | (x as u32 & 0xFFFF);
                let _ = PostMessageW(Some(s.owner), WM_RBUTTONUP, wp, LPARAM(xy as isize));
            }
            return LRESULT(0);
        }
        WM_IME_STARTCOMPOSITION => s.composing.set(true),
        WM_IME_ENDCOMPOSITION => s.composing.set(false),
        WM_KEYDOWN if !s.composing.get() && wp.0 == 0x1B => {
            post(s, hwnd, CANCEL, 0);
            return LRESULT(0);
        }
        WM_KEYDOWN if !s.composing.get() && wp.0 == 0x0D && unsafe { GetKeyState(0x10) } >= 0 => {
            post(s, hwnd, COMMIT, 0);
            return LRESULT(0);
        }
        WM_KEYDOWN if !s.composing.get() && crate::hotkey::is_shortcut_ctrl(unsafe { GetKeyState(0x11) } < 0, unsafe { GetKeyState(0x12) } < 0) => {
            let shift = unsafe { GetKeyState(0x10) } < 0;
            match wp.0 as u32 {
                // Ctrl+A: every EDIT should do this, the multiline one does not everywhere.
                0x41 => {
                    unsafe { SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1))) };
                    return LRESULT(0);
                }
                // Ctrl+C with nothing selected, Ctrl+S, Ctrl+Shift+S, Ctrl+P: the editor's shortcuts.
                0x43 => {
                    let (mut start, mut end) = (0u32, 0u32);
                    unsafe {
                        SendMessageW(hwnd, EM_GETSEL, Some(WPARAM(&mut start as *mut _ as usize)), Some(LPARAM(&mut end as *mut _ as isize)));
                    }
                    if start == end {
                        post(s, hwnd, KEY, 0x43);
                        return LRESULT(0);
                    }
                }
                vk @ (0x53 | 0x50) => {
                    post(s, hwnd, KEY, vk as isize | if shift { 0x100 } else { 0 });
                    return LRESULT(0);
                }
                _ => {}
            }
        }
        // The control characters of the shortcuts above would only beep.
        WM_CHAR if matches!(wp.0, 0x01 | 0x10 | 0x13) => return LRESULT(0),
        WM_CHAR if wp.0 == 0x1B || (wp.0 == 0x0D && unsafe { GetKeyState(0x10) } >= 0) => {
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xFFFF) as i16;
            post(s, hwnd, SIZE, delta.signum() as isize);
            return LRESULT(0);
        }
        WM_PAINT => {
            let result = unsafe { DefSubclassProc(hwnd, msg, wp, lp) };
            unsafe {
                let dc = GetDC(Some(hwnd));
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                let width = (crate::theme::BORDER_WIDTH * s.scale.get()).round().max(1.0) as i32;
                for edge in [
                    RECT { bottom: rect.top + width, ..rect },
                    RECT { top: rect.bottom - width, ..rect },
                    RECT { right: rect.left + width, ..rect },
                    RECT { left: rect.right - width, ..rect },
                ] { FillRect(dc, &edge, s.border); }
                ReleaseDC(Some(hwnd), dc);
            }
            return result;
        }
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

impl TextInput {
    pub fn new(
        parent: HWND,
        main: HWND,
        text: &TextEdit,
        theme: &Theme,
        bounds: Rect,
        scale: f32,
        click: Point,
    ) -> Result<Self> {
        let state = Box::new(State {
            main,
            owner: parent,
            anchor: Cell::new(Point::new(
                text.at.x.round() as i32,
                text.at.y.round() as i32,
            )),
            bounds: Cell::new(bounds),
            scale: Cell::new(scale),
            px: Cell::new(text.size),
            ink: Cell::new(text.color),
            bg: unsafe { CreateSolidBrush(color(theme.bg)) },
            bg_color: theme.bg,
            border: unsafe { CreateSolidBrush(color(theme.accent)) },
            composing: Cell::new(false),
            sizing: Cell::new(false),
            glyphs: GdiGlyphs::new(),
        });
        let initial = crate::sys::wide(&text.text.replace('\n', "\r\n"));
        let instance = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None)? };
        let host = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                HOST_CLASS,
                w!("snip text"),
                WS_POPUP | WS_CLIPCHILDREN,
                0,
                0,
                1,
                1,
                Some(parent),
                None,
                Some(instance.into()),
                None,
            )?
        };
        let edit = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("EDIT"),
                PCWSTR(initial.as_ptr()),
                WS_CHILD
                    | WS_CLIPSIBLINGS
                    | WINDOW_STYLE(
                        (ES_MULTILINE | ES_WANTRETURN | ES_AUTOHSCROLL | ES_AUTOVSCROLL) as u32,
                    ),
                0,
                0,
                1,
                1,
                Some(host),
                None,
                Some(instance.into()),
                None,
            )
        };
        let hwnd = match edit {
            Ok(h) => h,
            Err(e) => {
                let _ = unsafe { DestroyWindow(host) };
                return Err(e);
            }
        };
        let mut input = Self {
            hwnd,
            host,
            id: text.id,
            state,
        };
        if !unsafe {
            SetWindowSubclass(
                hwnd,
                Some(edit_proc),
                SUBCLASS,
                &*input.state as *const _ as usize,
            )
        }
        .as_bool()
        {
            return Err(windows::core::Error::from_thread());
        }
        unsafe {
            SendMessageW(hwnd, EM_SETLIMITTEXT, Some(WPARAM(2000)), None);
        }
        input.update(text, bounds, scale);
        unsafe {
            let mut r = RECT::default();
            let _ = GetWindowRect(hwnd, &mut r);
            let x = (click.x - r.left).max(0) as u32;
            let y = (click.y - r.top).max(0) as u32;
            let position = SendMessageW(
                hwnd,
                EM_CHARFROMPOS,
                None,
                Some(LPARAM(((y & 0xFFFF) << 16 | (x & 0xFFFF)) as isize)),
            )
            .0 as u32
                & 0xFFFF;
            SendMessageW(
                hwnd,
                EM_SETSEL,
                Some(WPARAM(position as usize)),
                Some(LPARAM(position as isize)),
            );
        }
        input.show(true);
        Ok(input)
    }

    pub fn update(&mut self, text: &TextEdit, bounds: Rect, scale: f32) {
        let s = &self.state;
        let font_changed = s.px.get() != text.size;
        s.px.set(text.size);
        s.ink.set(text.color);
        s.bounds.set(bounds);
        s.scale.set(scale);
        unsafe {
            // Always set the initial font; subsequent updates reuse the same cached HFONT.
            let font = s.glyphs.font(text.size, false);
            if font_changed || SendMessageW(self.hwnd, WM_GETFONT, None, None).0 == 0 {
                SendMessageW(
                    self.hwnd,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(0)),
                );
            }
        }
        autosize(self.hwnd, s);
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    pub fn show(&self, visible: bool) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            let _ = ShowWindow(self.host, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
            if visible && GetFocus() != self.hwnd {
                let _ = SetFocus(Some(self.hwnd));
            }
            if !visible && GetFocus() == self.hwnd {
                let _ = SetFocus(Some(self.state.owner));
            }
        }
    }

    /// The overlay window this box floats over.
    pub fn owner(&self) -> HWND {
        self.state.owner
    }

    /// Off while a system dialog is open over the editor.
    pub fn set_enabled(&self, on: bool) {
        unsafe {
            let _ = EnableWindow(self.host, on);
        }
    }

    pub fn visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }

    pub fn repaint(&self) {
        if self.visible() {
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
                let _ = UpdateWindow(self.hwnd);
            }
        }
    }
}

impl Drop for TextInput {
    fn drop(&mut self) {
        unsafe {
            let _ = RemoveWindowSubclass(self.hwnd, Some(edit_proc), SUBCLASS);
            let _ = DestroyWindow(self.host);
        }
    }
}
