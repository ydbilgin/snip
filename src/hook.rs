//! The WH_KEYBOARD_LL hook on its own thread.
//!
//! Microsoft: the low-level hook "is not injected into another process"; it is called in this
//! process's context, and a callback slower than the timeout gets the hook "silently removed". So
//! the callback only reads two atomics and the modifier state, decides through `HookState`, posts a
//! message, and returns. Keys other than S, Esc, Enter, the arrows and the text capture key return
//! after a few compares.
//! The thread re-installs the hook when asked (unlock, resume, display change, every N minutes).

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicIsize, AtomicU32, AtomicU64, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_HIGHEST};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::hookstate::{HookState, KeyAction, KeyEvent, Mods, OVERLAY_KEYS, VK_S, prewarm_hint};
use crate::log::qpc;
use crate::textkey::{self, TextAction, TextKeyState};

/// dwExtraInfo of the keys snip injects itself ('SNIP').
pub const INJECT_TAG: usize = 0x534E_4950;
/// The Win-key mask: VK 0xE8 is unassigned (AutoHotkey's default menu mask key).
pub const MASK_VK: u16 = 0xE8;

pub const WM_APP_TRIGGER: u32 = WM_APP + 1; // wParam 0, lParam = QPC at key-down
pub const WM_APP_OVERLAY_KEY: u32 = WM_APP + 2; // wParam vk, lParam 1 when Shift is held
pub const WM_APP_HOOK_STATUS: u32 = WM_APP + 3; // wParam 1 armed / 0 failed, lParam = error code
pub const WM_APP_PREWARM: u32 = WM_APP + 4; // Win and Shift are down; lParam = QPC
/// The text capture key was pressed: lParam = QPC at the key-down.
pub const WM_APP_TEXT_TRIGGER: u32 = WM_APP + 5;
/// The settings recorder got a chord: wParam vk, lParam modifier bits (`HkMods::bits`).
pub const WM_APP_RECORDED_CHORD: u32 = WM_APP + 6;
const WM_APP_REARM: u32 = WM_APP + 10;
static PREWARM_ENABLED: AtomicBool = AtomicBool::new(false);
static LAST_PREWARM: AtomicI64 = AtomicI64::new(0);
const WM_APP_MASK: u32 = WM_APP + 11;

static UI_HWND: AtomicIsize = AtomicIsize::new(0);
static THREAD_ID: AtomicU32 = AtomicU32::new(0);
static TRIGGER_ENABLED: AtomicBool = AtomicBool::new(true);
static MASK_ENABLED: AtomicBool = AtomicBool::new(true);
/// The text capture key, `textkey::pack`ed; 0: none.
static TEXT_KEY: AtomicU64 = AtomicU64::new(0);
/// The settings window is listening for a new key.
static RECORDING: AtomicBool = AtomicBool::new(false);
/// QPC time the overlay opened; 0 when no overlay is up.
static OVERLAY_SINCE: AtomicI64 = AtomicI64::new(0);
pub static EVENTS: AtomicU64 = AtomicU64::new(0);
pub static TICKS_TOTAL: AtomicU64 = AtomicU64::new(0);
pub static TICKS_MAX: AtomicU64 = AtomicU64::new(0);
pub static TRIGGERS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static STATE: Cell<HookState> = Cell::new(HookState::new());
    static TEXT_STATE: Cell<TextKeyState> = const { Cell::new(TextKeyState::new()) };
    static HOOK: Cell<HHOOK> = Cell::new(HHOOK::default());
}

/// The hook keeps routing overlay keys for at most this long after the overlay opened, even if
/// the UI thread never says it closed (a safety net against eating Esc and Enter forever).
const OVERLAY_MAX_TICKS_SECONDS: i64 = 3600;

/// Turn the Win+Shift+S trigger on or off without reinstalling the hook (settings reload).
/// Off means `HookState::on_key` gets `trigger_enabled = false`: Win+Shift+S passes to Windows.
pub fn set_trigger_enabled(on: bool) {
    TRIGGER_ENABLED.store(on, Ordering::SeqCst);
}

/// The text capture key, or None: then nothing is ever eaten for it.
pub fn set_text_key(key: Option<crate::hotkey::Hotkey>) {
    TEXT_KEY.store(textkey::pack(key), Ordering::SeqCst);
}

pub fn set_recording(on: bool) {
    RECORDING.store(on, Ordering::SeqCst);
}

pub fn set_overlay_active(active: bool) {
    OVERLAY_SINCE.store(if active { qpc().max(1) } else { 0 }, Ordering::SeqCst);
}

fn overlay_active(now: i64) -> bool {
    let since = OVERLAY_SINCE.load(Ordering::Relaxed);
    since != 0 && now - since < OVERLAY_MAX_TICKS_SECONDS * crate::log::freq()
}

fn down(vk: VIRTUAL_KEY) -> bool {
    unsafe { (GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000) != 0 }
}

unsafe extern "system" fn proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    let t0 = qpc();
    let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    let msg = wparam.0 as u32;
    let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
    let vk = kb.vkCode;
    let mut action = KeyAction::Pass;
    let mut text_action = TextAction::Pass;
    if (is_down || is_up) && (vk == VK_S || OVERLAY_KEYS.contains(&vk)) {
        let mods = Mods {
            win: down(VK_LWIN) || down(VK_RWIN),
            shift: down(VK_SHIFT),
            ctrl: down(VK_CONTROL),
            alt: down(VK_MENU),
        };
        let ev = KeyEvent { vk, down: is_down, injected_by_us: kb.dwExtraInfo == INJECT_TAG, mods };
        let mut st = STATE.with(|s| s.get());
        action = st.on_key(ev, TRIGGER_ENABLED.load(Ordering::Relaxed), overlay_active(t0));
        STATE.with(|s| s.set(st));
        let ui = HWND(UI_HWND.load(Ordering::Relaxed) as *mut _);
        match action {
            KeyAction::Trigger => unsafe {
                TRIGGERS.fetch_add(1, Ordering::Relaxed);
                let _ = PostMessageW(Some(ui), WM_APP_TRIGGER, WPARAM(0), LPARAM(t0 as isize));
                if MASK_ENABLED.load(Ordering::Relaxed) {
                    let _ = PostThreadMessageW(GetCurrentThreadId(), WM_APP_MASK, WPARAM(0), LPARAM(0));
                }
            },
            KeyAction::OverlayKey(k) => unsafe {
                let _ = PostMessageW(Some(ui), WM_APP_OVERLAY_KEY, WPARAM(k as usize), LPARAM(mods.shift as isize));
            },
            _ => {}
        }
    }
    // Snip's own keys answer first. Win+Shift+R and Win+Shift+S never reach the rules below in a
    // way that eats them: the key checks refuse those as a binding.
    if action == KeyAction::Pass && (is_down || is_up) {
        let binding = textkey::unpack(TEXT_KEY.load(Ordering::Relaxed));
        let recording = RECORDING.load(Ordering::Relaxed);
        let mut st = TEXT_STATE.with(|s| s.get());
        if recording || st.routes(vk, binding) {
            let mods = Mods { win: down(VK_LWIN) || down(VK_RWIN), shift: down(VK_SHIFT), ctrl: down(VK_CONTROL), alt: down(VK_MENU) };
            let injected = kb.dwExtraInfo == INJECT_TAG;
            text_action = if recording { st.record(vk, is_down, injected, mods) } else { st.evaluate(binding, vk, is_down, injected, mods) };
            TEXT_STATE.with(|s| s.set(st));
            if text_action == TextAction::Trigger {
                let ui = HWND(UI_HWND.load(Ordering::Relaxed) as *mut _);
                unsafe {
                    if recording {
                        let _ = PostMessageW(Some(ui), WM_APP_RECORDED_CHORD, WPARAM(vk as usize), LPARAM(textkey::hk_mods(mods).bits() as isize));
                    } else {
                        TRIGGERS.fetch_add(1, Ordering::Relaxed);
                        let _ = PostMessageW(Some(ui), WM_APP_TEXT_TRIGGER, WPARAM(0), LPARAM(t0 as isize));
                    }
                    // Releasing Win after this must not open Start.
                    if mods.win && MASK_ENABLED.load(Ordering::Relaxed) {
                        let _ = PostThreadMessageW(GetCurrentThreadId(), WM_APP_MASK, WPARAM(0), LPARAM(0));
                    }
                }
            }
        }
    }
    if action == KeyAction::Pass && text_action == TextAction::Pass && is_down && PREWARM_ENABLED.load(Ordering::Relaxed) && matches!(vk, 0x10 | 0xA0 | 0xA1 | 0x5B | 0x5C) {
        let mods = Mods { win: down(VK_LWIN) || down(VK_RWIN), shift: down(VK_SHIFT), ctrl: down(VK_CONTROL), alt: down(VK_MENU) };
        // At most one hint per second (a held Shift auto-repeats).
        if prewarm_hint(vk, true, kb.dwExtraInfo == INJECT_TAG, mods) && t0 - LAST_PREWARM.load(Ordering::Relaxed) > crate::log::freq() {
            LAST_PREWARM.store(t0, Ordering::Relaxed);
            let ui = HWND(UI_HWND.load(Ordering::Relaxed) as *mut _);
            unsafe {
                let _ = PostMessageW(Some(ui), WM_APP_PREWARM, WPARAM(0), LPARAM(t0 as isize));
            }
        }
    }
    let dt = (qpc() - t0).max(0) as u64;
    EVENTS.fetch_add(1, Ordering::Relaxed);
    TICKS_TOTAL.fetch_add(dt, Ordering::Relaxed);
    TICKS_MAX.fetch_max(dt, Ordering::Relaxed);
    if action.swallows() || text_action != TextAction::Pass { LRESULT(1) } else { unsafe { CallNextHookEx(None, code, wparam, lparam) } }
}

fn install() -> windows::core::Result<HHOOK> {
    unsafe {
        let hmod = GetModuleHandleW(None)?;
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(proc), Some(hmod.into()), 0)
    }
}

fn send_mask() {
    let mk = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(MASK_VK), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: INJECT_TAG } },
    };
    let inputs = [mk(KEYBD_EVENT_FLAGS(0)), mk(KEYEVENTF_KEYUP)];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn report(ok: bool, err: i32) {
    let ui = HWND(UI_HWND.load(Ordering::Relaxed) as *mut _);
    unsafe {
        let _ = PostMessageW(Some(ui), WM_APP_HOOK_STATUS, WPARAM(ok as usize), LPARAM(err as isize));
    }
}

/// Start the hook thread. Status arrives at `ui` as WM_APP_HOOK_STATUS.
pub fn start(ui: HWND, trigger_enabled: bool, mask: bool, prewarm: bool) -> std::thread::JoinHandle<()> {
    UI_HWND.store(ui.0 as isize, Ordering::SeqCst);
    PREWARM_ENABLED.store(prewarm, Ordering::SeqCst);
    TRIGGER_ENABLED.store(trigger_enabled, Ordering::SeqCst);
    MASK_ENABLED.store(mask, Ordering::SeqCst);
    let (tx, rx) = std::sync::mpsc::channel();
    let h = std::thread::Builder::new()
        .name("snip-hook".into())
        .spawn(move || unsafe {
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE); // create the queue
            THREAD_ID.store(GetCurrentThreadId(), Ordering::SeqCst);
            let _ = tx.send(());
            match install() {
                Ok(h) => {
                    HOOK.with(|c| c.set(h));
                    report(true, 0);
                }
                Err(e) => report(false, e.code().0),
            }
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                match msg.message {
                    WM_APP_MASK => send_mask(),
                    WM_APP_REARM => {
                        // New hook first, then drop the old one: no gap. No key event can run
                        // in between, because the callback only runs while this thread pumps.
                        match install() {
                            Ok(h) => {
                                let old = HOOK.with(|c| c.replace(h));
                                if !old.is_invalid() {
                                    let _ = UnhookWindowsHookEx(old);
                                }
                                STATE.with(|s| s.set(HookState::new()));
                                TEXT_STATE.with(|s| s.set(TextKeyState::new()));
                                report(true, 0);
                            }
                            Err(e) => report(false, e.code().0),
                        }
                    }
                    _ => {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            }
            let old = HOOK.with(|c| c.replace(HHOOK::default()));
            if !old.is_invalid() {
                let _ = UnhookWindowsHookEx(old);
            }
        })
        .expect("spawn hook thread");
    let _ = rx.recv();
    h
}

pub fn rearm() {
    let t = THREAD_ID.load(Ordering::SeqCst);
    if t != 0 {
        unsafe {
            let _ = PostThreadMessageW(t, WM_APP_REARM, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn stop(h: std::thread::JoinHandle<()>) {
    let t = THREAD_ID.load(Ordering::SeqCst);
    if t != 0 {
        unsafe {
            let _ = PostThreadMessageW(t, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        let _ = h.join();
    }
}

/// (events, average microseconds, max microseconds, triggers) of the callback so far.
pub fn stats() -> (u64, f64, f64, u64) {
    let n = EVENTS.load(Ordering::Relaxed);
    let tot = TICKS_TOTAL.load(Ordering::Relaxed);
    let max = TICKS_MAX.load(Ordering::Relaxed);
    let us = |t: u64| t as f64 * 1_000_000.0 / crate::log::freq() as f64;
    (n, if n > 0 { us(tot) / n as f64 } else { 0.0 }, us(max), TRIGGERS.load(Ordering::Relaxed))
}
