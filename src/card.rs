//! The window around the result card: layered, topmost, never active. It slides in, stays, fades
//! out. The mouse over it holds it, a click closes it. A new card closes the one before, also
//! across processes, so two captures do not stack. WM_APP_CARD_CLOSED goes to `notify` when it ends.

use std::cell::RefCell;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::cardview::{self, CardContent, FADE_OUT_MS, LEAVE_MS, SLIDE_IN_MS};
use crate::raster::Canvas;
use crate::theme::{self, Theme};

pub const CLASS: PCWSTR = w!("Laureth.snip.card");
pub const WM_APP_CARD_CLOSED: u32 = WM_APP + 80;
const TIMER_HOLD: usize = 1;
const TIMER_ANIM: usize = 2;
const STEP_MS: u32 = 15;
const WM_MOUSELEAVE: u32 = 0x02A3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    In,
    Hold,
    Out,
}

struct State {
    hwnd: HWND,
    notify: HWND,
    canvas: Canvas,
    x: i32,
    y: i32,
    slide: i32,
    stage: Stage,
    started: u64,
    hold_ms: u32,
    animate: bool,
    hovering: bool,
}

thread_local! {
    static CARD: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn animations_on() -> bool {
    let mut on = windows::core::BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    on.as_bool()
}

fn fade(k: f32) -> u8 {
    (255.0 * k.clamp(0.0, 1.0)).round() as u8
}

fn step(hwnd: HWND) {
    CARD.with(|c| {
        let mut g = c.borrow_mut();
        let Some(s) = g.as_mut() else { return };
        let dt = crate::sys::tick_ms().saturating_sub(s.started) as f32;
        match s.stage {
            Stage::In => {
                let k = dt / SLIDE_IN_MS as f32;
                let ease = 1.0 - (1.0 - k.min(1.0)).powi(3);
                let y = s.y + ((1.0 - ease) * s.slide as f32) as i32;
                crate::gdi::update_layered(hwnd, &s.canvas, s.x, y, fade(ease));
                if k >= 1.0 {
                    s.stage = Stage::Hold;
                    unsafe {
                        let _ = KillTimer(Some(hwnd), TIMER_ANIM);
                        SetTimer(Some(hwnd), TIMER_HOLD, s.hold_ms, None);
                    }
                }
            }
            Stage::Out => {
                let k = dt / FADE_OUT_MS as f32;
                crate::gdi::update_layered(hwnd, &s.canvas, s.x, s.y, fade(1.0 - k));
                if k >= 1.0 {
                    unsafe {
                        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                }
            }
            Stage::Hold => {}
        }
    });
}

fn begin_out(hwnd: HWND) {
    let close_now = CARD.with(|c| {
        let mut g = c.borrow_mut();
        let Some(s) = g.as_mut() else { return true };
        if !s.animate {
            return true;
        }
        s.stage = Stage::Out;
        s.started = crate::sys::tick_ms();
        unsafe {
            SetTimer(Some(hwnd), TIMER_ANIM, STEP_MS, None);
        }
        false
    });
    if close_now {
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_TIMER if wp.0 == TIMER_ANIM => {
            step(hwnd);
            LRESULT(0)
        }
        WM_TIMER if wp.0 == TIMER_HOLD => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_HOLD);
            }
            if !CARD.with(|c| c.borrow().as_ref().is_some_and(|s| s.hovering)) {
                begin_out(hwnd);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let newly = CARD.with(|c| {
                let mut g = c.borrow_mut();
                let Some(s) = g.as_mut() else { return false };
                let newly = !s.hovering;
                s.hovering = true;
                if s.stage == Stage::Out {
                    // Coming back during the fade: show it fully again.
                    s.stage = Stage::Hold;
                    unsafe {
                        let _ = KillTimer(Some(hwnd), TIMER_ANIM);
                    }
                    crate::gdi::update_layered(hwnd, &s.canvas, s.x, s.y, 255);
                }
                newly
            });
            if newly {
                let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                unsafe {
                    let _ = TrackMouseEvent(&mut t);
                    let _ = KillTimer(Some(hwnd), TIMER_HOLD);
                }
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            CARD.with(|c| {
                if let Some(s) = c.borrow_mut().as_mut() {
                    s.hovering = false;
                }
            });
            unsafe {
                SetTimer(Some(hwnd), TIMER_HOLD, LEAVE_MS, None);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let notify = CARD.with(|c| c.borrow_mut().take().map(|s| s.notify));
            if let Some(n) = notify {
                unsafe {
                    let _ = PostMessageW(Some(n), WM_APP_CARD_CLOSED, WPARAM(0), LPARAM(0));
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Is a card open on this thread?
pub fn is_open() -> bool {
    CARD.with(|c| c.borrow().is_some())
}

/// Close the card now, on this thread.
pub fn close() {
    if let Some(h) = CARD.with(|c| c.borrow().as_ref().map(|s| s.hwnd)) {
        unsafe {
            let _ = DestroyWindow(h);
        }
    }
}

/// Show `content` near the cursor's monitor. False: no window could be made, nothing will notify.
pub fn show(notify: HWND, content: &CardContent, hold_ms: u32, t: &Theme) -> bool {
    close();
    unsafe {
        // The card of an earlier capture, in its own process: it goes first.
        if let Ok(other) = FindWindowW(CLASS, PCWSTR::null()) {
            let _ = PostMessageW(Some(other), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        let Ok(module) = GetModuleHandleW(None) else { return false };
        let hinst = module.into();
        let wc = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(proc), hInstance: hinst, hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), lpszClassName: CLASS, ..Default::default() };
        RegisterClassExW(&wc); // a second time in one process fails harmlessly
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mons = crate::sys::monitors();
        let p = crate::geometry::Point::new(pt.x, pt.y);
        let Some(m) = mons.at(p).or(mons.list.first()).copied() else { return false };
        let g = crate::gdi::GdiGlyphs::new();
        let canvas = cardview::render(content, t, m.scale(), &g);
        let (x, y) = cardview::place(m.work, canvas.w as i32, canvas.h as i32, m.px(theme::CARD_MARGIN));
        let Ok(hwnd) = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE, CLASS, w!("Snip"), WS_POPUP, x, y, canvas.w as i32, canvas.h as i32, None, None, Some(hinst), None) else {
            return false;
        };
        let animate = animations_on();
        let slide = m.px(cardview::SLIDE_DIP);
        crate::gdi::update_layered(hwnd, &canvas, x, if animate { y + slide } else { y }, if animate { 0 } else { 255 });
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let started = crate::sys::tick_ms();
        if animate {
            SetTimer(Some(hwnd), TIMER_ANIM, STEP_MS, None);
        } else {
            SetTimer(Some(hwnd), TIMER_HOLD, hold_ms, None);
        }
        CARD.with(|c| *c.borrow_mut() = Some(State { hwnd, notify, canvas, x, y, slide, stage: if animate { Stage::In } else { Stage::Hold }, started, hold_ms, animate, hovering: false }));
        true
    }
}
