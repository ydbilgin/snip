//! The tray process and the snip session.
//!
//! The tray process holds only a hidden window, the hook thread, the PrtScn hotkey and the tray
//! icon (about 2 MB private). A snip runs in a short-lived child process (`snip --session`):
//! freeze -> overlay -> crop -> release GPU -> clipboard -> PNG, then the child exits, so the
//! graphics driver's memory (about 90-125 MB measured on 2026-10-08) never stays in the tray process.
//! When Win and Shift are both held the tray process starts the child early (prewarm), so the
//! D3D11 device (about 90-120 ms to create) is ready when S arrives.
//! With `separate_process = false` the same session code runs inside the tray process instead.
//!
//! v2: the editor hotkey (RegisterHotKey, `editor_hotkey`) opens the same session in editor mode;
//! the tray runs the 3/5 s countdown; the settings window asks for a config reload; every capture
//! that is copied, saved or pinned goes into the history (last 50).
//!
//! Text capture runs in the same session child, so the reader's memory never stays in the tray:
//! freeze -> text overlay -> crop -> release GPU -> read on a worker thread -> clipboard -> result
//! card; the child exits when the card has gone. The tray only hears how it ended (10 and up).

mod recording;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::Win32::System::RemoteDesktop::{NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{
    CreateProcessW, GetExitCodeProcess, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW, WaitForSingleObject,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_SNAPSHOT};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, PWSTR, w};

use crate::capture;
use crate::config::Config;
use crate::delay::{Countdown, Tick};
use crate::editor::{Effect, Prefs};
use crate::hook;
use crate::log::{Steps, qpc};
use crate::logf;
use crate::naming::{self, Stamp};
use crate::overlay::{self, Outcome, Overlay};
use crate::paths;
use crate::raster::Canvas;
use crate::tray;

pub const MAIN_CLASS: PCWSTR = w!("Laureth.snip.main");
pub const MAIN_CLASS_TEST: PCWSTR = w!("Laureth.snip.main.test");
pub const SESSION_CLASS: PCWSTR = w!("Laureth.snip.session");
const WM_APP_SAVED: u32 = WM_APP + 30;
const WM_APP_OUTCOME: u32 = WM_APP + 40;
/// child -> tray: the session window is ready (wParam = its HWND).
pub const WM_APP_SESSION_READY: u32 = WM_APP + 50;
/// child -> tray: the overlay is up (wParam 1) or gone (wParam 0).
pub const WM_APP_SESSION_OVERLAY: u32 = WM_APP + 51; // bit 0: open, bit 1: editor
/// child -> tray: the session ended (wParam 0 cancelled or failed, 1 done but a file or history
/// write failed, 2 done and every write succeeded).
pub const WM_APP_SESSION_DONE: u32 = WM_APP + 52;
/// tray -> child: start the snip now (wParam = source code, lParam = QPC at the key-down).
pub const WM_APP_GO: u32 = WM_APP + 53;
/// child -> tray: this session takes no new request (an all-screens capture has no overlay whose
/// closing would say so).
pub const WM_APP_SESSION_BUSY: u32 = WM_APP + 54;
const TIMER_REARM: usize = 1;
const TIMER_OVERLAY: usize = 2;
const TIMER_EXIT: usize = 3;
const TIMER_COOL: usize = 4;
const TIMER_CHILD: usize = 5;
const TIMER_DELAY: usize = 6;
const TIMER_RECORDING: usize = 7;
/// Another snip process -> tray: wParam 1 snip, 2 editor; lParam = delay seconds (0, 3, 5).
pub const WM_APP_COMMAND: u32 = WM_APP + 61;
const WM_APP_CUSTOM_COLOR: u32 = WM_APP + 62;
const WM_APP_SAVE_AS: u32 = WM_APP + 63;
const WM_APP_CHECK_FOCUS: u32 = WM_APP + 64;
const WM_APP_SESSION_KEYS: u32 = WM_APP + 66;
/// The reader's worker -> session: lParam = Box<Result<Reading, ReadError>>.
const WM_APP_TEXT_READ: u32 = WM_APP + 81;
/// Tray -> session with an overlay up: the text key was pressed again.
const WM_APP_TEXT_BUSY: u32 = WM_APP + 82;
/// Self-test: drag over the synthetic scene with the session's own overlay.
const WM_APP_SELFTEST_DRAG: u32 = WM_APP + 83;
/// Settings -> tray: temporarily release assignable keys while recording a replacement.
pub const WM_APP_RECORD_HOTKEY: u32 = WM_APP + 65;
pub const BADGE_CLASS: PCWSTR = w!("Laureth.snip.badge");
const HOTKEY_EDITOR: i32 = 2;
const HOTKEY_EDITOR2: i32 = 3;
const HOTKEY_ALL: i32 = 4;
const HOTKEY_ALL2: i32 = 5;
const COOL_MS: u32 = 10_000;
const HOTKEY_PRTSCN: i32 = 1;
const WTS_SESSION_UNLOCK: usize = 0x8;

pub fn source_code(s: &str) -> usize {
    match s {
        "win+shift+s" => 1,
        "prtscn" => 2,
        "tray" => 3,
        "editor" => 4,
        "delay" => 5,
        "text" => 6,
        "selftest-text" => 7,
        "all" => 8,
        "selftest" => 9,
        "selftest-all" => 10,
        "record" => 11,
        _ => 0,
    }
}

pub fn source_name(c: usize) -> &'static str {
    match c {
        1 => "win+shift+s",
        2 => "prtscn",
        3 => "tray",
        4 => "editor",
        5 => "delay",
        6 => "text",
        7 => "selftest-text",
        8 => "all",
        10 => "selftest-all",
        11 => "record",
        9 => "selftest",
        _ => "unknown",
    }
}

/// Arguments of a `snip --session` child.
#[derive(Clone, Debug, Default)]
pub struct SessionArgs {
    pub parent: isize,
    /// Start at once: (QPC at the key-down, source code).
    pub go: Option<(i64, usize)>,
    /// Self-test: no clipboard; save into this folder; press Enter by itself after the first frame.
    pub selftest_dir: Option<PathBuf>,
    /// Self-test: this picture instead of the screen (u32 width, u32 height, BGRA). Hidden desktop only.
    pub selftest_scene: Option<PathBuf>,
    /// Self-test: drag from here to there in desktop coordinates, once the overlay is up.
    pub selftest_drag: Option<(i32, i32, i32, i32)>,
}

#[derive(Clone, Debug, Default)]
pub struct RunOpts {
    pub no_hook: bool,
    pub no_tray: bool,
    /// Idle test: exit after this many seconds and print the memory and CPU used.
    pub exit_after: Option<u64>,
    /// Keep the D3D11 device warm regardless of the config (idle test of that variant).
    pub warm_device: bool,
    /// Idle test: hold the hook with no trigger and no key, to measure what it costs.
    pub measure_hook: bool,
    /// Run as a snip session child of the tray process.
    pub session: Option<SessionArgs>,
}

/// The tray process's view of a running session child.
struct Child {
    pid: u32,
    process: HANDLE,
    hwnd: Option<HWND>,
    pending: Option<(i64, &'static str)>,
    overlay: bool,
    editor: bool,
    spawned: Instant,
    /// Its overlay has closed: it is finishing up and takes no new snip.
    used: bool,
}

struct App {
    cfg: Config,
    opts: RunOpts,
    hwnd: HWND,
    hinst: HINSTANCE,
    overlay: Option<Overlay>,
    steps: Option<Steps>,
    source: &'static str,
    prev_fg: HWND,
    last_input: Instant,
    armed: Option<bool>,
    hook_err: i32,
    rearms: u64,
    prtscn: Option<bool>,
    tray_ok: bool,
    icon_ok: HICON,
    icon_bad: HICON,
    taskbar_created: u32,
    snips: u64,
    sessions: u64,
    started: String,
    hook_thread: Option<std::thread::JoinHandle<()>>,
    warm: Option<capture::Gpu>,
    child: Option<Child>,
    /// Finished children still writing their files; their handles close when they exit.
    retired: Vec<Child>,
    /// Session child: the outcome to report when the PNG is written (1 copied).
    done_pending: Option<usize>,
    /// The editor hotkey is registered (None: not wanted).
    editor_hk: Option<bool>,
    editor2_hk: Option<bool>,
    /// The all-screens keys are registered (None: not wanted).
    all_hk: Option<bool>,
    all2_hk: Option<bool>,
    /// A system dialog is open over the overlay.
    dialog_open: bool,
    /// The clipboard write of this capture failed (the user is told at the end).
    soft_fail: bool,
    /// The clipboard sequence number right after this snip's picture was written (for
    /// `clipboard_file`); taken by the next save job.
    clip_seq: Option<u32>,
    /// The running countdown and what it starts.
    countdown: Option<(Countdown, &'static str)>,
    badge: HWND,
    recording_window: HWND,
    /// The reader is working on a selection.
    text_pending: bool,
    /// A finish that waits for the reader or the card.
    deferred_finish: Option<usize>,
    texts: u64,
    text_started: i64,
    recorder: recording::Recorder,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    // Activation callbacks can arrive while APP is borrowed (including a nested dialog loop).
    static APP_HWND: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) };
}

static LAST_FILE: Mutex<String> = Mutex::new(String::new());
static SAVE_ERRORS: Mutex<u64> = Mutex::new(0);

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn post(h: HWND, msg: u32, wp: usize, lp: isize) {
    unsafe {
        let _ = PostMessageW(Some(h), msg, WPARAM(wp), LPARAM(lp));
    }
}

impl App {
    fn is_session(&self) -> bool {
        self.opts.session.is_some()
    }

    fn parent(&self) -> Option<HWND> {
        self.opts.session.as_ref().map(|s| HWND(s.parent as *mut _))
    }

    fn lang(&self) -> crate::i18n::Lang {
        crate::sys::lang(&self.cfg)
    }

    fn keep_device(&self) -> bool {
        self.cfg.keep_gpu_device || self.opts.warm_device || self.is_session()
    }

    fn print_screen_conflict(&self) -> bool {
        let p = self.cfg.plan();
        [p.editor, p.editor2, p.all, p.all2].iter().flatten().any(|h| h.is_plain_prtscn())
            && crate::sys::print_screen_snipping_enabled().unwrap_or(true)
    }

    fn status_text(&self) -> String {
        let base = self.main_status();
        for (state, key) in [(self.editor2_hk, &self.cfg.editor_hotkey_2), (self.all_hk, &self.cfg.all_hotkey), (self.all2_hk, &self.cfg.all_hotkey_2)] {
            if state == Some(false) {
                return format!("{base}; {key} in use by another program");
            }
        }
        base
    }

    fn main_status(&self) -> String {
        if self.cfg.snip_enabled && self.print_screen_conflict() {
            return crate::i18n::S::PrintScreenConflict.get(self.lang()).into();
        }
        if self.editor_hk == Some(false) {
            return format!("Snip: {} {}", self.cfg.editor_hotkey, crate::i18n::S::StatusKeyMissing.get(self.lang()));
        }
        match self.armed {
            _ if self.opts.no_hook => format!("Snip: {}", crate::i18n::S::StatusOff.get(self.lang())),
            _ if !self.hook_wanted() => crate::i18n::S::StatusReady.get(self.lang()).to_string(),
            _ if self.cfg.snip_enabled && !self.cfg.win_shift_s && self.armed == Some(true) => format!("Snip: {}", crate::i18n::S::StatusOff.get(self.lang())),
            Some(true) => crate::i18n::S::StatusReady.get(self.lang()).to_string(),
            Some(false) => format!("Snip: {} ({:#x})", crate::i18n::S::StatusFailed.get(self.lang()), self.hook_err),
            None => format!("Snip: {}", crate::i18n::S::StatusArming.get(self.lang())),
        }
    }

    fn healthy(&self) -> bool {
        !(self.hook_wanted() && !self.opts.no_hook && self.armed == Some(false))
            && self.editor_hk != Some(false) && !(self.cfg.snip_enabled && self.print_screen_conflict())
    }

    fn snip_trigger_wanted(&self) -> bool {
        self.cfg.snip_enabled && self.cfg.win_shift_s
    }

    /// The text capture key the hook should take.
    fn text_key(&self) -> Option<crate::hotkey::Hotkey> {
        let plan = self.cfg.plan();
        crate::hotkey::text_binding(self.cfg.text_enabled, &self.cfg.text_hotkey, &plan, self.cfg.snip_enabled).0
    }

    fn hook_wanted(&self) -> bool {
        self.snip_trigger_wanted() || self.text_key().is_some()
    }

    fn apply_text_key(&self) {
        let plan = self.cfg.plan();
        let (key, note) = crate::hotkey::text_binding(self.cfg.text_enabled, &self.cfg.text_hotkey, &plan, self.cfg.snip_enabled);
        if let Some(n) = note {
            logf!("{n}");
        }
        hook::set_text_key(key);
    }

    fn refresh_tray(&mut self) {
        if self.opts.no_tray {
            return;
        }
        let icon = if self.healthy() { self.icon_ok } else { self.icon_bad };
        let tip = self.status_text();
        // Modify the existing icon; add it when it is missing (first start, Explorer restart).
        if !self.tray_ok || !tray::update(self.hwnd, icon, &tip) {
            self.tray_ok = tray::add(self.hwnd, icon, &tip);
        }
    }

    fn write_state(&self) {
        if self.opts.exit_after.is_some() || self.is_session() {
            return;
        }
        let (ev, avg, max, trig) = hook::stats();
        let last = LAST_FILE.lock().map(|g| g.clone()).unwrap_or_default();
        let errs = SAVE_ERRORS.lock().map(|g| *g).unwrap_or(0);
        let j = format!(
            "{{\n  \"schema\": 6,\n  \"pid\": {},\n  \"version\": {},\n  \"started\": {},\n  \"updated\": {},\n  \"separate_process\": {},\n  \"hook_wanted\": {},\n  \"hook_armed\": {},\n  \"hook_error\": {},\n  \"hook_rearms\": {},\n  \"print_screen_registered\": {},\n  \"tray\": {},\n  \"sessions\": {},\n  \"snips\": {},\n  \"save_errors\": {},\n  \"last_file\": {},\n  \"hook_events\": {},\n  \"hook_callback_avg_us\": {:.3},\n  \"hook_callback_max_us\": {:.3},\n  \"hook_triggers\": {},\n  \"editor_hotkey\": {},\n  \"editor_hotkey_registered\": {},\n  \"editor_hotkey_2\": {},\n  \"editor_hotkey_2_registered\": {},\n  \"all_hotkey\": {},\n  \"all_hotkey_registered\": {},\n  \"all_hotkey_2\": {},\n  \"all_hotkey_2_registered\": {},\n  \"snip_enabled\": {},\n  \"text_enabled\": {},\n  \"text_hotkey\": {},\n  \"texts\": {}\n}}\n",
            std::process::id(),
            json_str(env!("CARGO_PKG_VERSION")),
            json_str(&self.started),
            json_str(&crate::log::now_text()),
            self.cfg.separate_process,
            self.hook_wanted() && !self.opts.no_hook,
            match self.armed { Some(b) => b.to_string(), None => "null".into() },
            self.hook_err,
            self.rearms,
            match self.prtscn { Some(b) => b.to_string(), None => "null".into() },
            self.tray_ok,
            self.sessions,
            self.snips,
            errs,
            json_str(&last),
            ev, avg, max, trig,
            json_str(&self.cfg.editor_hotkey),
            match self.editor_hk { Some(b) => b.to_string(), None => "null".into() },
            json_str(&self.cfg.editor_hotkey_2),
            match self.editor2_hk { Some(b) => b.to_string(), None => "null".into() },
            json_str(&self.cfg.all_hotkey),
            match self.all_hk { Some(b) => b.to_string(), None => "null".into() },
            json_str(&self.cfg.all_hotkey_2),
            match self.all2_hk { Some(b) => b.to_string(), None => "null".into() },
            self.cfg.snip_enabled,
            self.cfg.text_enabled,
            json_str(&self.cfg.text_hotkey),
            self.texts
        );
        let mut value: serde_json::Value = serde_json::from_str(&j).expect("state json");
        value["recorder"] = serde_json::to_value(&self.recorder.state).expect("recorder state");
        value["record_hotkey"] = serde_json::json!(self.cfg.record_hotkey);
        value["record_hotkey_registered"] = serde_json::json!(self.recorder.hotkey);
        if let Ok(bytes) = serde_json::to_vec_pretty(&value) { let _ = naming::write_small(&paths::state_file(), &bytes); }
    }

    // ---- the tray process side of a separate session ----

    fn spawn_child(&mut self, go: Option<(i64, &'static str)>) -> bool {
        let mut buf = [0u16; 1024];
        let n = unsafe { GetModuleFileNameW(None, &mut buf) } as usize;
        let exe = String::from_utf16_lossy(&buf[..n]);
        let cmd = format!("\"{exe}\" --session {}", self.hwnd.0 as isize);
        let mut wide: Vec<u16> = cmd.encode_utf16().chain(Some(0)).collect();
        let si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let mut pi = PROCESS_INFORMATION::default();
        let r = unsafe { CreateProcessW(PCWSTR::null(), Some(PWSTR(wide.as_mut_ptr())), None, None, false, PROCESS_CREATION_FLAGS(0), None, PCWSTR::null(), &si, &mut pi) };
        match r {
            Ok(()) => {
                unsafe {
                    let _ = CloseHandle(pi.hThread);
                    // Grant only our captured child PID permission to activate its editor.
                    let _ = AllowSetForegroundWindow(pi.dwProcessId);
                    SetTimer(Some(self.hwnd), TIMER_CHILD, 1000, None);
                }
                self.sessions += 1;
                self.child = Some(Child {
                    pid: pi.dwProcessId,
                    process: pi.hProcess,
                    hwnd: None,
                    pending: go,
                    overlay: false,
                    editor: go.is_some_and(|(_, src)| src == "editor"),
                    spawned: Instant::now(),
                    used: false,
                });
                true
            }
            Err(e) => {
                logf!("session start FAILED: {:#010x} {}", e.code().0, e.message());
                false
            }
        }
    }

    fn session_go(&mut self, source: &'static str, t0: i64) {
        self.check_child();
        let (exists, overlay, used) = self.child.as_ref().map_or((false, false, false), |c| (true, c.overlay, c.used));
        match crate::hotkey::session_step(exists, overlay, used) {
            crate::hotkey::SessionStep::Ignore => {
                logf!("trigger {source} ignored: overlay already open");
                if source == "text"
                    && let Some(h) = self.child.as_ref().and_then(|c| c.hwnd)
                {
                    post(h, WM_APP_TEXT_BUSY, 0, 0);
                }
            }
            crate::hotkey::SessionStep::Reuse => {
                let Some(c) = &mut self.child else { return };
                c.editor = source == "editor";
                match c.hwnd {
                    Some(h) => {
                        unsafe { let _ = AllowSetForegroundWindow(c.pid); }
                        post(h, WM_APP_GO, source_code(source), t0 as isize);
                    }
                    None => c.pending = Some((t0, source)),
                }
            }
            crate::hotkey::SessionStep::Spawn { retire } => {
                if retire {
                    self.retire_child();
                }
                if !self.spawn_child(Some((t0, source))) {
                    // Last resort: run the snip in this process.
                    self.start_snip_here(source, t0);
                }
            }
        }
    }

    /// Move a finished child aside so a new snip does not wait for it.
    fn retire_child(&mut self) {
        if let Some(c) = self.child.take() {
            self.retired.push(c);
        }
    }

    /// A prewarm or a countdown wants a ready child: a used one will not do.
    fn retire_used_child(&mut self) {
        self.check_child();
        if self.child.as_ref().is_some_and(|c| c.used && !c.overlay) {
            self.retire_child();
        }
    }

    fn check_child(&mut self) {
        self.retired.retain(|c| {
            let gone = unsafe { WaitForSingleObject(c.process, 0) } == WAIT_OBJECT_0;
            if gone {
                unsafe { let _ = CloseHandle(c.process); }
            }
            !gone
        });
        let Some(c) = &self.child else {
            if self.retired.is_empty() {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_CHILD);
                }
            }
            return;
        };
        if unsafe { WaitForSingleObject(c.process, 0) } == WAIT_OBJECT_0 {
            let mut code = 0u32;
            unsafe {
                let _ = GetExitCodeProcess(c.process, &mut code);
            }
            if c.overlay {
                hook::set_overlay_active(false);
                // A dead session cannot release its own clip on the mouse.
                crate::cursorfence::os::release_any();
                logf!("session pid={} ended with its overlay up (exit {code:#x}); key routing and mouse clip cleared", c.pid);
            } else if code != 0 {
                logf!("session pid={} exit {code:#x} after {:.1} s", c.pid, c.spawned.elapsed().as_secs_f64());
            }
            unsafe {
                let _ = CloseHandle(c.process);
                if self.retired.is_empty() {
                    let _ = KillTimer(Some(self.hwnd), TIMER_CHILD);
                }
            }
            self.child = None;
        }
    }

    // ---- the snip itself (session child, or the tray process when separate_process = false) ----

    fn start_snip(&mut self, source: &'static str, t0: i64) {
        if self.recorder.child.is_some() { logf!("capture deferred until recording ends"); return; }
        // Each function works only while it is on, however it was asked for.
        let on = if source == "record" { self.cfg.record_enabled } else if source == "text" { self.cfg.text_enabled } else { self.cfg.snip_enabled };
        if !on {
            logf!("trigger {source} ignored: this function is turned off");
            return;
        }
        if self.cfg.separate_process && !self.is_session() && self.opts.exit_after.is_none() {
            self.session_go(source, t0);
        } else {
            self.start_snip_here(source, t0);
        }
    }

    fn start_snip_here(&mut self, source: &'static str, t0: i64) {
        if self.overlay.is_some() {
            logf!("trigger {source} ignored: overlay already open");
            if source == "text" {
                self.show_card(&crate::textcore::TextOutcome::Busy);
            }
            return;
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_COOL);
        }
        if source == "record" { self.record_begin(t0); self.recorder.sequence = crate::clipboard::sequence(); }
        let mut steps = Steps::start_at(t0);
        steps.mark("dispatch");
        self.prev_fg = unsafe { GetForegroundWindow() };
        let scene = self.opts.session.as_ref().and_then(|s| s.selftest_scene.clone());
        let snap = match scene {
            Some(path) if crate::clipboard::on_a_hidden_desktop_of_his_station() => match crate::looktest::scene_snapshot(&path) {
                Ok(s) => s,
                Err(e) => {
                    logf!("selftest scene FAILED: {e}");
                    self.session_finish(0);
                    return;
                }
            },
            _ => capture::freeze(self.cfg.capture, self.warm.as_ref()),
        };
        steps.mark("freeze");
        let summary = snap.summary();
        if source == "all" || source == "selftest-all" {
            logf!("all screens: {summary}");
            self.finish_all(snap, steps);
            return;
        }
        let editor = source == "editor";
        let opts = overlay::Opts {
            text: source == "text" || source == "selftest-text",
            text_action: self.cfg.text_enabled,
            dim: self.cfg.dim,
            theme: crate::sys::theme(&self.cfg),
            lang: self.lang(),
            editor: editor.then(|| {
                Prefs::parse(&std::fs::read_to_string(paths::editor_prefs_file()).unwrap_or_default())
            }),
        };
        match Overlay::new(snap, self.hinst, &opts) {
            Ok(mut ov) => {
                if source == "record" { ov.set_record_mode(); }
                steps.mark("overlay_setup");
                // Editor and native EDIT receive their own keys after verified activation.
                // The hook must not consume caret arrows or IME Enter/Esc.
                hook::set_overlay_active(crate::hotkey::route_overlay_keys(true, editor, false));
                let shown = ov.show();
                steps.mark("first_frame");
                let (draw_ms, show_ms, _) = ov.show_ms;
                let (private, _, _) = self_usage();
                logf!("snip#{} open source={} {} | draw={draw_ms:.1}ms show={show_ms:.1}ms private={:.1}MB | {}", self.snips + 1, source, steps.text(), private as f64 / 1048576.0, summary);
                if let Err(e) = shown {
                    // Never leave an invisible overlay that routes Esc and Enter: close it now.
                    logf!("overlay draw FAILED, closing it: {:#010x} {}", e.code().0, e.message());
                    hook::set_overlay_active(false);
                    let ours = ov.hwnds().contains(&unsafe { GetForegroundWindow() });
                    drop(ov);
                    self.drop_warm_if_temporary();
                    if ours { self.restore_focus(); }
                    self.session_finish(0);
                    return;
                }
                self.overlay = Some(ov);
                self.steps = Some(steps);
                self.source = source;
                self.last_input = Instant::now();
                if let Some(p) = self.parent() {
                    // The keyboard arrives with WM_ACTIVATEAPP. Until then every overlay counts as
                    // plain: the hook routes its Esc, Enter and arrows.
                    post(p, WM_APP_SESSION_OVERLAY, 1, std::process::id() as isize);
                } else {
                    hook::set_overlay_active(crate::hotkey::route_overlay_keys(true, false, false));
                }
                unsafe {
                    SetTimer(Some(self.hwnd), TIMER_OVERLAY, 1000, None);
                }
                if source == "selftest" {
                    post(self.hwnd, hook::WM_APP_OVERLAY_KEY, crate::hookstate::VK_RETURN as usize, 0);
                }
                if source == "selftest-text" {
                    post(self.hwnd, WM_APP_SELFTEST_DRAG, 0, 0);
                }
            }
            Err(e) => {
                logf!("snip open FAILED source={} {:#010x} {} | {}", source, e.code().0, e.message(), summary);
                self.drop_warm_if_temporary();
                self.session_finish(0);
            }
        }
    }

    /// All screens: cut the whole desktop out of the frozen frames, clipboard, file, history, and a
    /// short card that takes no focus. No overlay is ever made.
    fn finish_all(&mut self, snap: capture::Snapshot, mut steps: Steps) {
        // This session takes no new request from now on.
        if let Some(p) = self.parent() {
            post(p, WM_APP_SESSION_BUSY, 0, std::process::id() as isize);
        }
        let shot = crate::allshot::take(&snap);
        drop(snap); // the frozen textures and the D3D11 device go here
        self.drop_warm_if_temporary();
        steps.mark("crop");
        let shot = match shot {
            Ok(s) => s,
            Err(e) => {
                logf!("all screens FAILED: {e}");
                self.all_card(None);
                self.session_finish(0);
                return;
            }
        };
        let selftest_dir = self.opts.session.as_ref().and_then(|s| s.selftest_dir.clone());
        if self.cfg.copy_to_clipboard && selftest_dir.is_none() {
            let mut sink = crate::allshot::RealClipboard(self.hwnd);
            if let Err(e) = crate::allshot::ImageSink::put(&mut sink, shot.w, shot.h, &shot.px) {
                logf!("clipboard FAILED {e}");
                self.soft_fail = true;
            } else {
                self.clip_seq = Some(crate::clipboard::sequence());
            }
            steps.mark("clipboard");
        }
        self.snips += 1;
        logf!("snip#{} all screens {}x{} at {},{} from {} monitors {}", self.snips, shot.w, shot.h, shot.bounds.left, shot.bounds.top, shot.screens, steps.text());
        self.all_card(Some(&shot));
        let job = if self.cfg.save_file || selftest_dir.is_some() {
            Some(Job::Auto(selftest_dir.unwrap_or_else(|| paths::output_dir(&self.cfg.folder))))
        } else if self.cfg.history {
            Some(Job::HistoryOnly)
        } else {
            None
        };
        let crate::allshot::AllShot { w, h, px, .. } = shot;
        self.run_job(job, w, h, px, "snip");
        self.write_state();
    }

    /// The card that says it worked (or did not). Never takes the foreground.
    fn all_card(&mut self, shot: Option<&crate::allshot::AllShot>) {
        use crate::cardview::{CardContent, Mark};
        use crate::i18n::S;
        let lang = self.lang();
        let (content, hold) = match shot {
            Some(s) => {
                let title = if self.cfg.copy_to_clipboard { S::ToastAll } else { S::ToastAllSaved };
                (CardContent { mark: Mark::Done, title: title.get(lang).to_string(), meta: None, body: crate::allshot::describe(s, lang), tip: None }, 2200)
            }
            None => (CardContent { mark: Mark::Failed, title: S::ToastAllFailed.get(lang).to_string(), meta: None, body: S::ToastAllFailedBody.get(lang).to_string(), tip: None }, 3600),
        };
        let theme = crate::sys::theme(&self.cfg);
        if !crate::card::show(self.hwnd, &content, hold, &theme) {
            logf!("result card could not be shown");
        }
    }

    fn drop_warm_if_temporary(&mut self) {
        if !self.keep_device() {
            self.warm = None; // a pre-warmed device lives for one snip only
        }
    }

    fn close_overlay(&mut self) -> Option<Overlay> {
        hook::set_overlay_active(false);
        self.drop_warm_if_temporary();
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_OVERLAY);
        }
        let ov = self.overlay.take();
        if let Some(e) = ov.as_ref().and_then(|o| o.editor.as_ref())
            && let Err(err) = naming::write_small(&paths::editor_prefs_file(), e.prefs().to_text().as_bytes()) {
                logf!("editor preferences save FAILED: {err}");
        }
        if ov.is_some()
            && let Some(p) = self.parent() {
                post(p, WM_APP_SESSION_OVERLAY, 0, std::process::id() as isize);
            }
        ov
    }

    /// Session child only: tell the tray process how it ended, then close. A reader still working
    /// or a card still showing keeps the process until they are done.
    fn session_finish(&mut self, result: usize) {
        if self.is_session() && (self.text_pending || crate::card::is_open()) {
            self.deferred_finish = Some(result);
            return;
        }
        if let Some(p) = self.parent() {
            post(p, WM_APP_SESSION_DONE, result, std::process::id() as isize);
            post(self.hwnd, WM_CLOSE, 0, 0);
        }
    }

    fn restore_focus(&self) {
        if !self.prev_fg.is_invalid() && unsafe { IsWindow(Some(self.prev_fg)) }.as_bool() {
            crate::overlay::give_foreground(self.prev_fg);
        }
    }

    fn handle(&mut self, out: Outcome) {
        self.last_input = Instant::now();
        match out {
            Outcome::Nothing => {}
            Outcome::Cancel => {
                let ov = self.close_overlay();
                drop(ov);
                self.restore_focus();
                if let Some(mut s) = self.steps.take() {
                    s.mark("cancel");
                    logf!("snip cancelled {}", s.text());
                }
                self.session_finish(0);
            }
            Outcome::TextArea(rect, stay_open) => self.read_area(rect, stay_open),
            Outcome::CustomColor => post(self.hwnd, WM_APP_CUSTOM_COLOR, 0, 0),
            Outcome::Edited(e) => self.finish_editor(e),
            Outcome::Commit(rect, _) if self.source == "record" => self.record_commit(rect),
            Outcome::Commit(rect, kind) => {
                let mut steps = self.steps.take().unwrap_or_else(Steps::start);
                steps.mark("select");
                let Some(ov) = self.close_overlay() else { return };
                let commit_name = ov.commit_name(kind);
                let frames = ov.frame_count();
                let parts = ov.parts_metadata(kind);
                let cropped = ov.snap.crop(&rect);
                steps.mark("crop");
                drop(ov); // windows, swap chains, frozen textures and the D3D device go here
                self.restore_focus();
                steps.mark("release");
                let (w, h, px) = match cropped {
                    Ok(c) => c,
                    Err(e) => {
                        logf!("snip crop FAILED {:#010x} {} rect={:?}", e.code().0, e.message(), rect);
                        self.session_finish(0);
                        return;
                    }
                };
                let selftest_dir = self.opts.session.as_ref().and_then(|s| s.selftest_dir.clone());
                if self.cfg.copy_to_clipboard && selftest_dir.is_none() {
                    if let Err(e) = crate::clipboard::set_image(self.hwnd, w, h, &px) {
                        logf!("clipboard FAILED {:#010x} {}", e.code().0, e.message());
                        self.soft_fail = true;
                    } else {
                        self.clip_seq = Some(crate::clipboard::sequence());
                    }
                    steps.mark("clipboard");
                }
                self.snips += 1;
                logf!("snip#{} commit {} {}x{} at {},{} {}={} frames={} {} {}", self.snips, commit_name, w, h, rect.left, rect.top, if kind==overlay::Kind::Part {"capture_source"} else {"source"}, self.source, frames, parts, steps.text());
                let job = if self.cfg.save_file || selftest_dir.is_some() {
                    Some(Job::Auto(selftest_dir.unwrap_or_else(|| paths::output_dir(&self.cfg.folder))))
                } else if self.cfg.history {
                    Some(Job::HistoryOnly)
                } else {
                    None
                };
                self.run_job(job, w, h, px, "snip");
                self.write_state();
            }
        }
    }

    /// Text: crop the frozen pixels (never what was drawn on them), let go of the overlay unless
    /// the editor has work to keep, and read on a worker thread.
    fn read_area(&mut self, rect: crate::geometry::Rect, stay_open: bool) {
        let mut steps = self.steps.take().unwrap_or_else(Steps::start);
        steps.mark("select");
        let cropped = if stay_open {
            let c = self.overlay.as_ref().map(|o| o.snap.crop(&rect));
            self.steps = Some(Steps::start());
            c
        } else {
            let ov = self.close_overlay();
            let c = ov.as_ref().map(|o| o.snap.crop(&rect));
            drop(ov);
            self.restore_focus();
            c
        };
        steps.mark("crop");
        let (w, h, px) = match cropped {
            Some(Ok(c)) => c,
            Some(Err(e)) => {
                logf!("text crop FAILED {:#010x} {} rect={:?}", e.code().0, e.message(), rect);
                self.text_done(crate::textcore::TextOutcome::Error);
                return;
            }
            None => return,
        };
        logf!("text read start {}x{} at {},{} source={} {}", w, h, rect.left, rect.top, self.source, steps.text());
        self.text_pending = true;
        self.text_started = qpc();
        let (hwnd, language) = (self.hwnd.0 as isize, self.cfg.ocr_language.clone());
        let started = std::thread::Builder::new().name("snip-ocr".into()).spawn(move || {
            let r = crate::ocr::read(w, h, &px, &language);
            post_boxed(HWND(hwnd as *mut _), WM_APP_TEXT_READ, r);
        });
        if let Err(e) = started {
            logf!("text worker FAILED: {e}");
            self.text_pending = false;
            self.text_done(crate::textcore::TextOutcome::Error);
        }
    }

    /// The reader answered: clipboard, list of recent texts, card.
    fn text_read(&mut self, r: Result<crate::ocr::Reading, crate::ocr::ReadError>) {
        use crate::textcore::TextOutcome;
        self.text_pending = false;
        let outcome = match r {
            Ok(reading) => {
                // Only counts and times go to the log: the text read stays private.
                let (lines, chars) = crate::textcore::measure(&reading.text);
                logf!("text read {} lines {} chars language={} prepared={}x{} engine={:.1}ms prepare={:.1}ms recognize={:.1}ms second={:.1}ms", lines, chars, reading.language, reading.prepared.0, reading.prepared.1, reading.engine_ms, reading.prepare_ms, reading.recognize_ms, reading.second_ms);
                if reading.text.is_empty() {
                    TextOutcome::NoText
                } else {
                    crate::textclip::publish(self.hwnd, &reading.text)
                }
            }
            Err(crate::ocr::ReadError::NoEngine) => TextOutcome::NoEngine,
            Err(crate::ocr::ReadError::Failed(e)) => {
                logf!("text read FAILED: {e}");
                TextOutcome::Error
            }
        };
        self.text_done(outcome);
    }

    fn text_done(&mut self, outcome: crate::textcore::TextOutcome) {
        use crate::textcore::TextOutcome;
        let selftest = self.opts.session.as_ref().and_then(|s| s.selftest_dir.clone());
        if let TextOutcome::Copied(text) = &outcome {
            self.texts += 1;
            if self.cfg.text_history {
                let dir = selftest.clone().unwrap_or_else(paths::history_dir);
                let t = local_stamp();
                let when = format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second);
                if let Err(e) = crate::texthistory::record(&dir, &when, text) {
                    logf!("recent texts save FAILED: {e}");
                }
            }
        }
        if let Some(dir) = &selftest {
            let body = match &outcome {
                TextOutcome::Copied(t) => format!("copied\n{t}"),
                other => format!("{other:?}\n"),
            };
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(dir.join("text-result.txt"), body);
        }
        logf!("text done {} after {:.1} ms of reading", TextOutcome::name_of_code(outcome.done_code()), crate::log::ms_since(self.text_started));
        self.show_card(&outcome);
        if !self.is_session() && TextOutcome::code_is_failure(outcome.done_code()) {
            self.warn_text_problem(outcome.done_code());
        }
        if self.overlay.is_none() {
            self.session_finish(outcome.done_code());
        }
        self.write_state();
    }

    fn show_card(&mut self, outcome: &crate::textcore::TextOutcome) {
        if let Some(c) = crate::cardview::content(outcome, self.lang()) {
            let theme = crate::sys::theme(&self.cfg);
            if !crate::card::show(self.hwnd, &c, crate::cardview::hold_ms(outcome), &theme) {
                logf!("result card could not be shown");
            }
        }
    }

    /// A failure also goes next to the tray icon, so it is not lost when the card is missed.
    fn warn_text_problem(&self, code: usize) {
        use crate::i18n::S;
        if self.opts.no_tray || !self.tray_ok {
            return;
        }
        let l = self.lang();
        let (title, text) = match code.wrapping_sub(crate::textcore::DONE_TEXT_BASE) {
            3 => (S::CardUnchanged, S::CardUnchangedBody),
            6 => (S::CardNoEngine, S::CardNoEngineTip),
            _ => (S::CardError, S::CardErrorBody),
        };
        let icon = if self.healthy() { self.icon_ok } else { self.icon_bad };
        tray::warn(self.hwnd, icon, &self.status_text(), title.get(l), text.get(l));
    }

    /// Copy, save, save as or pin the editor's image.
    fn finish_editor(&mut self, e: Effect) {
        if e == Effect::SaveAs {
            // The dialog comes first: cancelling it must leave the editor and its work open.
            post(self.hwnd, WM_APP_SAVE_AS, 0, 0);
            return;
        }
        let mut steps = self.steps.take().unwrap_or_else(Steps::start);
        steps.mark("edit");
        let res = self.overlay.as_mut().and_then(|o| o.editor_result());
        let ov = self.close_overlay();
        drop(ov);
        self.restore_focus();
        let Some((rect, img, _prefs)) = res else {
            logf!("editor: no image to {e:?}");
            self.session_finish(0);
            return;
        };
        let selftest = self.opts.session.as_ref().is_some_and(|s| s.selftest_dir.is_some());
        self.snips += 1;
        logf!("snip#{} editor {:?} {}x{} at {},{} {}", self.snips, e, img.w, img.h, rect.left, rect.top, steps.text());
        let job = match e {
            Effect::Copy => {
                if !selftest {
                    match crate::clipboard::set_image(self.hwnd, img.w, img.h, &img.px) {
                        Ok(()) => self.clip_seq = Some(crate::clipboard::sequence()),
                        Err(err) => {
                            logf!("clipboard FAILED {:#010x} {}", err.code().0, err.message());
                            self.soft_fail = true;
                        }
                    }
                }
                self.cfg.history.then_some(Job::HistoryOnly)
            }
            Effect::Save => Some(Job::Auto(paths::output_dir(&self.cfg.folder))),
            Effect::Pin => Some(Job::Pin(rect.left, rect.top)),
            _ => None,
        };
        let Canvas { w, h, px } = img;
        self.run_job(job, w, h, px, "editor");
        self.write_state();
    }

    /// The Save as dialog gave a path: finish the editor with it.
    fn finish_save_as(&mut self, path: PathBuf) {
        let res = self.overlay.as_mut().and_then(|o| o.editor_result());
        let ov = self.close_overlay();
        drop(ov);
        self.restore_focus();
        let Some((_, img, _)) = res else {
            self.session_finish(0);
            return;
        };
        self.snips += 1;
        logf!("snip#{} editor SaveAs {}x{}", self.snips, img.w, img.h);
        let Canvas { w, h, px } = img;
        self.run_job(Some(Job::To(path)), w, h, px, "editor");
        self.write_state();
    }

    /// Tell the user when a capture did not fully reach the clipboard or the disk.
    fn warn_save_problem(&self) {
        if self.opts.no_tray || !self.tray_ok {
            return;
        }
        use crate::i18n::S;
        let l = self.lang();
        let icon = if self.healthy() { self.icon_ok } else { self.icon_bad };
        tray::warn(self.hwnd, icon, &self.status_text(), S::SnipProblem.get(l), S::SnipProblemText.get(l));
    }

    /// Hand a finished image to the save worker (or finish at once when there is nothing to write).
    fn run_job(&mut self, job: Option<Job>, w: u32, h: u32, px: Vec<u8>, source: &'static str) {
        let real = self.opts.session.as_ref().is_none_or(|s| s.selftest_dir.is_none());
        let clip_seq = self.clip_seq.take().filter(|_| real && (self.cfg.clipboard_png || self.cfg.clipboard_file));
        let job = job.or_else(|| (clip_seq.is_some() && self.cfg.clipboard_png).then_some(Job::ClipboardOnly));
        let Some(job) = job else {
            let failed = std::mem::take(&mut self.soft_fail);
            self.session_finish(if failed { 1 } else { 2 });
            if !self.is_session() && failed {
                self.warn_save_problem();
            }
            return;
        };
        let task = SaveTask { n: self.snips, job, stamp: local_stamp(), w, h, px, hwnd: self.hwnd.0 as isize, real, history: self.cfg.history && real, source, jpeg_when_photo: self.cfg.image_format == "auto", clip_seq, clipboard_png: self.cfg.clipboard_png, clipboard_file: self.cfg.clipboard_file };
        self.done_pending = Some(1);
        std::thread::spawn(move || save_worker(task));
    }

    /// Tell the hook owner whether the editor holds the keyboard. Without it, the hook routes
    /// the editor's Esc, Enter and arrows like the plain overlay's.
    fn report_focus(&self) {
        let focused = self.overlay.as_ref().is_some_and(|o| o.is_editor() && o.focused);
        match self.parent() {
            Some(p) => post(p, WM_APP_SESSION_OVERLAY, if focused { 3 } else { 1 }, std::process::id() as isize),
            None => hook::set_overlay_active(crate::hotkey::route_overlay_keys(self.overlay.is_some(), focused, false)),
        }
    }

    /// Esc and Enter go to the overlay through the hook only while this is on (off while a
    /// system dialog is open over the overlay).
    fn overlay_keys(&self, on: bool) {
        let on = crate::hotkey::route_overlay_keys(
            on,
            self.overlay.as_ref().is_some_and(|o| o.is_editor() && o.focused),
            false,
        );
        match self.parent() {
            Some(p) => post(
                p,
                WM_APP_SESSION_KEYS,
                on as usize,
                std::process::id() as isize,
            ),
            None => hook::set_overlay_active(on),
        }
    }

    // ---- tray process: hotkeys, countdown, reload ----

    fn register_hotkeys(&mut self) {
        self.record_register();
        unsafe {
            let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_PRTSCN);
            let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_EDITOR);
            let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_EDITOR2);
            let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_ALL);
            let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_ALL2);
        }
        self.prtscn = None;
        self.editor_hk = None;
        self.editor2_hk = None;
        self.all_hk = None;
        self.all2_hk = None;
        if self.opts.no_hook || !self.recording_window.is_invalid() || !self.cfg.snip_enabled {
            return;
        }
        let plan = self.cfg.plan();
        if self.print_screen_conflict() {
            logf!("PrtScn conflict: Windows screen snipping is enabled or its setting could not be read; check Windows Accessibility > Keyboard or assign another editor hotkey in Settings (registration is not proof of delivery)");
        }
        for n in &plan.notes {
            logf!("{n}");
        }
        if plan.plain_prtscn {
            let r = unsafe { RegisterHotKey(Some(self.hwnd), HOTKEY_PRTSCN, MOD_NOREPEAT, VK_SNAPSHOT.0 as u32) };
            let ok = r.is_ok();
            logf!("PrtScn hotkey {}", if ok { "registered".to_string() } else { format!("NOT registered: {:?}", r.err()) });
            self.prtscn = Some(ok);
        }
        if let Some(hk) = plan.editor {
            let r = unsafe { RegisterHotKey(Some(self.hwnd), HOTKEY_EDITOR, MOD_NOREPEAT | HOT_KEY_MODIFIERS(hk.mods.bits()), hk.vk) };
            let ok = r.is_ok();
            logf!("editor hotkey {} {}", hk.text(), if ok { "registered".to_string() } else { format!("NOT registered (in use?): {:?}", r.err()) });
            self.editor_hk = Some(ok);
        }
        if let Some(hk) = plan.editor2 {
            self.editor2_hk = Some(self.register_second_key(hk));
        }
        if let Some(hk) = plan.all {
            self.all_hk = Some(self.register_all_key(HOTKEY_ALL, hk));
        }
        if let Some(hk) = plan.all2 {
            self.all2_hk = Some(self.register_all_key(HOTKEY_ALL2, hk));
        }
    }

    fn register_all_key(&self, id: i32, hk: crate::hotkey::Hotkey) -> bool {
        let r = unsafe { RegisterHotKey(Some(self.hwnd), id, MOD_NOREPEAT | HOT_KEY_MODIFIERS(hk.mods.bits()), hk.vk) };
        logf!("all-screens hotkey {} {}", hk.text(), if r.is_ok() { "registered".to_string() } else { format!("NOT registered (in use?): {:?}", r.as_ref().err()) });
        r.is_ok()
    }

    fn register_second_key(&self, hk: crate::hotkey::Hotkey) -> bool {
        let r = unsafe { RegisterHotKey(Some(self.hwnd), HOTKEY_EDITOR2, MOD_NOREPEAT | HOT_KEY_MODIFIERS(hk.mods.bits()), hk.vk) };
        logf!("second editor hotkey {} {}", hk.text(), if r.is_ok() { "registered" } else { "busy (another program holds it); retried at each re-arm" });
        r.is_ok()
    }

    /// A key another program held may be free now (he quit Lightshot). Keys we hold stay as they are.
    fn retry_hotkeys(&mut self) {
        if self.recorder.hotkey == Some(false) { self.record_register(); }
        if !self.recording_window.is_invalid() || self.opts.no_hook || !self.cfg.snip_enabled {
            return;
        }
        let plan = self.cfg.plan();
        let before = (self.editor_hk, self.editor2_hk, self.all_hk, self.all2_hk);
        if self.editor2_hk == Some(false)
            && let Some(hk) = plan.editor2 {
                self.editor2_hk = Some(self.register_second_key(hk));
        }
        if self.editor_hk == Some(false)
            && let Some(hk) = plan.editor {
                let r = unsafe { RegisterHotKey(Some(self.hwnd), HOTKEY_EDITOR, MOD_NOREPEAT | HOT_KEY_MODIFIERS(hk.mods.bits()), hk.vk) };
                self.editor_hk = Some(r.is_ok());
        }
        if self.all_hk == Some(false)
            && let Some(hk) = plan.all {
                self.all_hk = Some(self.register_all_key(HOTKEY_ALL, hk));
        }
        if self.all2_hk == Some(false)
            && let Some(hk) = plan.all2 {
                self.all2_hk = Some(self.register_all_key(HOTKEY_ALL2, hk));
        }
        if before != (self.editor_hk, self.editor2_hk, self.all_hk, self.all2_hk) {
            self.refresh_tray();
            self.write_state();
        }
    }

    fn reload(&mut self) {
        let (cfg, note) = Config::load_or_create(&paths::config_file());
        if let Some(n) = note {
            logf!("{n}");
        }
        let restart_needed = cfg.separate_process != self.cfg.separate_process || cfg.capture != self.cfg.capture || cfg.keep_gpu_device != self.cfg.keep_gpu_device;
        self.cfg = cfg;
        hook::set_trigger_enabled(self.snip_trigger_wanted() || !self.recording_window.is_invalid());
        self.apply_text_key();
        self.record_register(); // record_enabled / record_hotkey take effect without a restart
        if self.hook_wanted() && self.hook_thread.is_none() && !self.opts.no_hook {
            self.hook_thread = Some(hook::start(self.hwnd, true, self.cfg.mask_win_key, self.cfg.prewarm && !self.cfg.keep_gpu_device));
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_REARM, self.cfg.rearm_minutes.saturating_mul(60_000), None);
            }
        }
        self.register_hotkeys();
        logf!("config reloaded{}", if restart_needed { " (capture/process settings apply after a restart)" } else { "" });
        self.refresh_tray();
        self.write_state();
    }

    fn record_hotkey(&mut self, owner: HWND) {
        self.recording_window = owner;
        let recording = !owner.is_invalid();
        // Keep the existing exact Win+Shift+S hook rule, but route its result to the recorder.
        // Otherwise Windows or our own snip would open instead of showing "Not allowed".
        if recording && self.hook_thread.is_none() && !self.opts.no_hook {
            self.hook_thread = Some(hook::start(self.hwnd, true, self.cfg.mask_win_key, false));
            unsafe { SetTimer(Some(self.hwnd), TIMER_REARM, self.cfg.rearm_minutes.saturating_mul(60_000), None); }
        }
        hook::set_trigger_enabled(recording || self.snip_trigger_wanted());
        // While he presses a new key, a key Windows keeps must reach the recorder, not Windows.
        hook::set_recording(recording);
        self.register_hotkeys();
        unsafe {
            if recording {
                SetTimer(Some(self.hwnd), TIMER_RECORDING, 250, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), TIMER_RECORDING);
            }
        }
        self.refresh_tray();
        self.write_state();
    }

    fn start_countdown(&mut self, secs: u32, source: &'static str) {
        if self.countdown.is_some() {
            return;
        }
        if self.cfg.separate_process && self.opts.exit_after.is_none() {
            self.retire_used_child();
            if self.child.is_none() {
                self.spawn_child(None); // the device is ready when the countdown ends
            }
        }
        self.countdown = Some((Countdown::new(crate::sys::tick_ms(), secs), source));
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_DELAY, 50, None);
        }
        self.tick_countdown(None);
    }

    fn tick_countdown(&mut self, shown: Option<u32>) -> Option<u32> {
        let (cd, source) = self.countdown?;
        match cd.tick(crate::sys::tick_ms()) {
            Tick::Show(n) => {
                if shown != Some(n) {
                    self.show_badge(n);
                }
                Some(n)
            }
            Tick::Hide => {
                unsafe {
                    let _ = ShowWindow(self.badge, SW_HIDE);
                }
                None
            }
            Tick::Fire => {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_DELAY);
                    let _ = ShowWindow(self.badge, SW_HIDE);
                }
                self.countdown = None;
                self.start_snip(source, qpc());
                None
            }
        }
    }

    fn show_badge(&mut self, n: u32) {
        let mons = crate::sys::monitors();
        let Some(m) = mons.at(crate::sys::cursor()).copied() else { return };
        let theme = crate::sys::theme(&self.cfg);
        let cv = crate::delay::render_badge(n, &theme, m.scale(), &crate::gdi::GdiGlyphs::new());
        unsafe {
            if self.badge.is_invalid() {
                self.badge = CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
                    BADGE_CLASS,
                    w!("snip countdown"),
                    WS_POPUP,
                    0,
                    0,
                    1,
                    1,
                    None,
                    None,
                    Some(self.hinst),
                    None,
                )
                .unwrap_or_default();
            }
            let x = m.rect.left + (m.rect.width() - cv.w as i32) / 2;
            let y = m.rect.top + m.px(crate::theme::COUNTDOWN_TOP);
            crate::gdi::update_layered(self.badge, &cv, x, y, 255);
            let _ = ShowWindow(self.badge, SW_SHOWNOACTIVATE);
        }
    }
}

pub fn local_stamp() -> Stamp {
    let t = unsafe { GetLocalTime() };
    Stamp { year: t.wYear, month: t.wMonth as u8, day: t.wDay as u8, hour: t.wHour as u8, minute: t.wMinute as u8, second: t.wSecond as u8 }
}

/// Encode and write one PNG; never touches the UI thread except for one posted message.
pub fn save_png(dir: &std::path::Path, stamp: Stamp, w: u32, h: u32, px: &[u8]) -> Result<(PathBuf, usize, f64, f64), String> {
    let t = qpc();
    let bytes = crate::png::encode_png(w, h, px).map_err(|e| format!("encode {:#010x} {}", e.code().0, e.message()))?;
    let enc_ms = crate::log::ms_since(t);
    std::fs::create_dir_all(dir).map_err(|e| format!("folder {}: {e}", dir.display()))?;
    let t = qpc();
    let path = naming::write_capture(dir, &stamp, &bytes).map_err(|e| format!("write {}: {e}", dir.display()))?;
    Ok((path, bytes.len(), enc_ms, crate::log::ms_since(t)))
}

/// Pointers this process boxed for a posted message, with that message. Another process on the
/// desktop can post the same message number with any lParam; only a pointer listed here is unboxed.
static POSTED: std::sync::Mutex<Vec<(u32, isize)>> = std::sync::Mutex::new(Vec::new());

/// Post `value` boxed; when the post fails, take the box back so nothing leaks.
fn post_boxed<T>(h: HWND, msg: u32, value: T) {
    let p = boxed(msg, value);
    if unsafe { PostMessageW(Some(h), msg, WPARAM(0), LPARAM(p)) }.is_err() {
        // SAFETY: `p` was boxed just above as a `T` for `msg`.
        drop(unsafe { unboxed::<T>(msg, p) });
    }
}

fn boxed<T>(msg: u32, value: T) -> isize {
    let p = Box::into_raw(Box::new(value)) as isize;
    POSTED.lock().unwrap_or_else(|e| e.into_inner()).push((msg, p));
    p
}

/// # Safety
/// `T` must be the type `boxed` was called with for `msg`.
unsafe fn unboxed<T>(msg: u32, p: isize) -> Option<T> {
    let mut list = POSTED.lock().unwrap_or_else(|e| e.into_inner());
    let at = list.iter().position(|&e| e == (msg, p))?;
    list.swap_remove(at);
    drop(list);
    Some(unsafe { *Box::from_raw(p as *mut T) })
}

/// What the save worker does with a finished image.
#[derive(Clone, Debug)]
enum Job {
    /// Auto-named PNG in this folder (the screenshots folder, or the self-test folder).
    Auto(PathBuf),
    /// Exactly this file (Save as).
    To(PathBuf),
    /// No file in the screenshots folder; the history keeps its own copy.
    HistoryOnly,
    /// Only the asynchronous PNG clipboard addition; no disk or history.
    ClipboardOnly,
    /// Pin it at this desktop position.
    Pin(i32, i32),
}

struct SaveTask {
    n: u64,
    job: Job,
    stamp: Stamp,
    w: u32,
    h: u32,
    px: Vec<u8>,
    hwnd: isize,
    /// false in the self-test, which must not touch last.txt or the history.
    real: bool,
    history: bool,
    source: &'static str,
    /// `image_format = "auto"`: an automatic save of a photo-like capture is a JPEG.
    jpeg_when_photo: bool,
    /// `clipboard_file`: the sequence number of our picture on the clipboard.
    clip_seq: Option<u32>,
    clipboard_png: bool,
    clipboard_file: bool,
}

fn save_worker(t: SaveTask) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let n = t.n;
    let ok = (|| -> Result<(), String> {
        let t0 = qpc();
        let bytes = crate::png::encode_png(t.w, t.h, &t.px).map_err(|e| format!("encode {:#010x} {}", e.code().0, e.message()))?;
        let enc = crate::log::ms_since(t0);
        let saved_result = (|| -> Result<Option<PathBuf>, String> { Ok(match &t.job {
            Job::Auto(dir) => {
                std::fs::create_dir_all(dir).map_err(|e| format!("folder {}: {e}", dir.display()))?;
                // The PNG is the measure: a photo-like one is saved as a JPEG a third its size.
                let jpeg = if t.jpeg_when_photo && naming::photo_like(bytes.len(), t.w, t.h) {
                    let t1 = qpc();
                    match crate::png::encode_jpeg(t.w, t.h, &t.px, 0.95) {
                        Ok(j) => {
                            logf!("snip#{n} photo-like ({:.2} bit/px): JPEG {} bytes in {:.1}ms", bytes.len() as f64 * 8.0 / (t.w as f64 * t.h as f64), j.len(), crate::log::ms_since(t1));
                            Some(j)
                        }
                        Err(e) => {
                            logf!("snip#{n} JPEG FAILED {:#010x} {}, keeping PNG", e.code().0, e.message());
                            None
                        }
                    }
                } else {
                    None
                };
                let path = match &jpeg {
                    Some(j) => naming::write_capture_ext(dir, &t.stamp, "jpg", j),
                    None => naming::write_capture(dir, &t.stamp, &bytes),
                }
                .map_err(|e| format!("write {}: {e}", dir.display()))?;
                Some(path)
            }
            Job::To(path) => {
                naming::write_atomic(path, &bytes).map_err(|e| format!("write {}: {e}", path.display()))?;
                Some(path.clone())
            }
            Job::HistoryOnly | Job::ClipboardOnly | Job::Pin(..) => None,
        }) })();
        // Even a disk error must not prevent the lossless clipboard addition.
        let t_clip = qpc();
        let mut append_failed = false;
        let png_clip = if let Some(seq) = t.clip_seq {
            let file = saved_result.as_ref().ok().and_then(|p| p.as_deref()).filter(|_| t.clipboard_file);
            match crate::clipboard::add_capture(HWND(t.hwnd as *mut _), seq, t.clipboard_png.then_some(bytes.as_slice()), file) {
                Ok(true) if t.clipboard_png => format!("{:.2}", crate::log::ms_since(t_clip)),
                Ok(false) if t.clipboard_png => "stale".into(),
                Ok(_) => "off".into(),
                Err(e) => {
                    logf!("snip#{n} clipboard append FAILED {:#010x} {}", e.code().0, e.message());
                    append_failed = true;
                    if t.clipboard_png { format!("{:.2}", crate::log::ms_since(t_clip)) } else { "off".into() }
                }
            }
        } else { "off".into() };
        logf!("snip#{n} worker encode={enc:.2}ms png_clip={png_clip}");
        let saved = saved_result?;
        if let Some(p) = &saved {
            let ps = p.to_string_lossy().to_string();
            if t.real {
                let _ = naming::write_small(&paths::last_file(), ps.as_bytes());
            }
            logf!("snip#{n} saved {} bytes={} encode={:.2}ms", ps, bytes.len(), enc);
            if let Ok(mut g) = LAST_FILE.lock() {
                *g = ps;
            }
        }
        let history = if t.history { crate::actions::record_history(t.source, t.w, t.h, &t.px, saved.as_deref(), Some(&bytes)).map(Some) } else { Ok(None) };
        if let Job::Pin(x, y) = t.job {
            let (path, temp) = match history.as_ref().ok().and_then(|p| p.clone()) {
                Some(p) => (p, false),
                None => {
                    let p = paths::pins_dir().join(format!("pin-{}-{n}.png", std::process::id()));
                    naming::write_small(&p, &bytes).map_err(|e| format!("pin file {}: {e}", p.display()))?;
                    (p, true)
                }
            };
            let args = format!("--pin {} {x} {y}{}", crate::actions::quote(&path.to_string_lossy()), if temp { " --temp" } else { "" });
            if !crate::actions::spawn_self(&args) {
                return Err("pin process did not start".into());
            }
        }
        history?;
        if append_failed { Err("clipboard append failed (disk/history work completed)".into()) } else { Ok(()) }
    })();
    let ok = match ok {
        Ok(()) => true,
        Err(e) => {
            logf!("snip#{n} save FAILED {e}");
            if let Ok(mut g) = SAVE_ERRORS.lock() {
                *g += 1;
            }
            false
        }
    };
    unsafe {
        CoUninitialize();
    }
    post(HWND(t.hwnd as *mut _), WM_APP_SAVED, ok as usize, 0);
}

// History/pin windows have no App session to hold their save worker alive.
static COPY_JOBS: Mutex<Vec<std::sync::mpsc::Receiver<()>>> = Mutex::new(Vec::new());

/// DIB first, then the same PNG-only save worker used by captures with file/history off.
pub(crate) fn copy_picture(owner: HWND, image: &Canvas, clipboard_png: bool) -> windows::core::Result<()> {
    crate::clipboard::set_image(owner, image.w, image.h, &image.px)?;
    let seq = crate::clipboard::sequence();
    if !clipboard_png || crate::clipboard::on_a_hidden_desktop_of_his_station() { return Ok(()); }
    let task = SaveTask {
        n: 0, job: Job::ClipboardOnly, stamp: local_stamp(), w: image.w, h: image.h,
        px: image.px.clone(), hwnd: owner.0 as isize, real: false, history: false,
        source: "copy", jpeg_when_photo: false, clip_seq: Some(seq), clipboard_png: true, clipboard_file: false,
    };
    let (done, waiting) = std::sync::mpsc::channel();
    if let Ok(mut jobs) = COPY_JOBS.lock() {
        jobs.retain(|job| matches!(job.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));
        jobs.push(waiting);
    }
    std::thread::spawn(move || { save_worker(task); let _ = done.send(()); });
    Ok(())
}

/// Before destroying a copying window, keep its HWND alive until PNG is published (3 s total).
pub(crate) fn wait_copy_png() -> bool {
    let jobs = COPY_JOBS.lock().map(|mut jobs| std::mem::take(&mut *jobs)).unwrap_or_default();
    let deadline = Instant::now() + std::time::Duration::from_secs(3);
    let mut complete = true;
    for job in jobs {
        if matches!(job.recv_timeout(deadline.saturating_duration_since(Instant::now())), Err(std::sync::mpsc::RecvTimeoutError::Timeout)) {
            logf!("clipboard PNG wait reached 3 s");
            complete = false;
        }
    }
    complete
}

/// Private-station acceptance test: exercise the production worker, including PNG-only copies.
pub(crate) fn test_save_copy(owner: HWND, seq: u32, image: Canvas, file: Option<PathBuf>) {
    save_worker(SaveTask {
        n: 0, job: file.map(Job::To).unwrap_or(Job::ClipboardOnly), stamp: local_stamp(),
        w: image.w, h: image.h, px: image.px, hwnd: owner.0 as isize,
        real: false, history: false, source: "selftest", jpeg_when_photo: true,
        clip_seq: Some(seq), clipboard_png: true, clipboard_file: true,
    });
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

pub(crate) fn main_hwnd() -> HWND {
    APP_HWND.with(Cell::get)
}

fn lparam_xy(l: LPARAM) -> (i32, i32) {
    ((l.0 & 0xFFFF) as i16 as i32, ((l.0 >> 16) & 0xFFFF) as i16 as i32)
}

fn apply_cursor() {
    let id = with_app(|a| a.overlay.as_ref().map(|o| o.cursor_id())).flatten().unwrap_or(IDC_CROSS);
    unsafe {
        if let Ok(c) = LoadCursorW(None, id) {
            SetCursor(Some(c));
        }
    }
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        crate::partworker::READY => {
            with_app(|a| a.overlay.as_mut().map(|o| o.on_parts()));
            LRESULT(0)
        }
        WM_TIMER if wp.0 == crate::partworker::TIMER => {
            with_app(|a| a.overlay.as_mut().map(|o| o.on_parts()));
            LRESULT(0)
        }
        WM_SYSCOMMAND if wp.0 & 0xfff0 == SC_KEYMENU as usize => LRESULT(0),
        WM_TIMER if wp.0 == overlay::FRAME_TIMER => {
            with_app(|a| {
                if let Some(o) = a.overlay.as_mut() {
                    o.flush_frame();
                }
            });
            LRESULT(0)
        }
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP
        | WM_MBUTTONDOWN | WM_MBUTTONUP | WM_XBUTTONDOWN | WM_XBUTTONUP => {
            with_app(|a| a.last_input = Instant::now());
            let i = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as usize;
            let (x, y) = lparam_xy(lp);
            let out = with_app(|a| {
                a.overlay
                    .as_mut()
                    .map(|o| o.on_mouse(i, msg, x, y))
                    .unwrap_or(Outcome::Nothing)
            });
            if let Some(out) = out
                && out != Outcome::Nothing
            {
                // Act after the borrow is released: teardown destroys this very window.
                let main = with_app(|a| a.hwnd).unwrap_or(hwnd);
                post_boxed(main, WM_APP_OUTCOME, out);
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
            let out = with_app(|a| {
                a.last_input = Instant::now();
                a.overlay.as_mut().map(|o| o.on_wheel(delta)).unwrap_or(Outcome::Nothing)
            });
            if let Some(out) = out {
                with_app(|a| a.handle(out));
            }
            LRESULT(0)
        }
        WM_CHAR => {
            let out = with_app(|a| {
                a.last_input = Instant::now();
                char::from_u32(wp.0 as u32).and_then(|c| a.overlay.as_mut().map(|o| o.on_char(c))).unwrap_or(Outcome::Nothing)
            });
            if let Some(out) = out
                && out != Outcome::Nothing
            {
                let main = with_app(|a| a.hwnd).unwrap_or(hwnd);
                post_boxed(main, WM_APP_OUTCOME, out);
            }
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            // Esc, Enter and the arrows reach this only when the hook is not routing keys (hook
            // off, not armed, or the tray process is gone); the editor's other keys always do.
            let vk = wp.0 as u32;
            let out = with_app(|a| {
                a.last_input = Instant::now();
                a.overlay.as_mut().map(|o| o.on_key(vk, Overlay::shift_held())).unwrap_or(Outcome::Nothing)
            });
            if let Some(out) = out
                && out != Outcome::Nothing
            {
                // Act after the borrow is released: teardown destroys this very window.
                let main = with_app(|a| a.hwnd).unwrap_or(hwnd);
                post_boxed(main, WM_APP_OUTCOME, out);
            }
            if vk == 0x11 {
                apply_cursor();
            }
            LRESULT(0)
        }
        WM_KEYUP if wp.0 == 0x11 => {
            apply_cursor();
            with_app(|a| a.overlay.as_mut().map(|o| o.crossing_key_changed()));
            LRESULT(0)
        }
        // Alt let go: the drag is fenced to its monitor again. Handled here, not by DefWindowProc,
        // which would open a window menu for a lone Alt.
        WM_KEYUP | WM_SYSKEYUP if wp.0 == 0x12 => {
            with_app(|a| a.overlay.as_mut().map(|o| o.on_alt(false)));
            LRESULT(0)
        }
        // The mouse is never kept while the overlay is not in front or has lost the capture.
        WM_KILLFOCUS | WM_CANCELMODE | WM_CAPTURECHANGED => {
            with_app(|a| a.overlay.as_mut().map(|o| o.release_fence()));
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_ACTIVATEAPP => {
            if wp.0 == 0 {
                with_app(|a| a.overlay.as_mut().map(|o| o.release_fence()));
            }
            // Defer until activation has settled; never destroy a window inside this callback.
            let main = APP_HWND.with(Cell::get);
            if !main.is_invalid() {
                post(main, WM_APP_CHECK_FOCUS, wp.0, 0);
            }
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_ACTIVATE as isize),
        WM_SETCURSOR => {
            apply_cursor();
            LRESULT(1)
        }
        WM_ERASEBKGND => LRESULT(1),
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        crate::textinput::CHANGED
        | crate::textinput::COMMIT
        | crate::textinput::CANCEL
        | crate::textinput::KEY
        | crate::textinput::SIZE => {
            let out = with_app(|a| {
                a.last_input = Instant::now();
                a.overlay
                    .as_mut()
                    .map(|o| o.on_text_input(HWND(wp.0 as *mut _), msg, lp.0))
                    .unwrap_or(Outcome::Nothing)
            });
            if let Some(out) = out {
                with_app(|a| a.handle(out));
            }
            LRESULT(0)
        }
        WM_APP_TEXT_READ => {
            // SAFETY: unboxed only when this process boxed exactly this pointer for this message.
            if let Some(r) = unsafe { unboxed::<Result<crate::ocr::Reading, crate::ocr::ReadError>>(WM_APP_TEXT_READ, lp.0) } {
                with_app(|a| a.text_read(r));
            }
            LRESULT(0)
        }
        WM_APP_TEXT_BUSY => {
            with_app(|a| {
                if a.overlay.is_some() {
                    a.show_card(&crate::textcore::TextOutcome::Busy);
                }
            });
            LRESULT(0)
        }
        crate::card::WM_APP_CARD_CLOSED => {
            with_app(|a| {
                if let Some(r) = a.deferred_finish.take() {
                    a.session_finish(r);
                }
            });
            LRESULT(0)
        }
        WM_APP_SELFTEST_DRAG => {
            let out = with_app(|a| {
                let (x0, y0, x1, y1) = a.opts.session.as_ref().and_then(|s| s.selftest_drag)?;
                let o = a.overlay.as_mut()?;
                Some(o.selftest_drag(x0, y0, x1, y1))
            });
            if let Some(Some(out)) = out {
                with_app(|a| a.handle(out));
            }
            LRESULT(0)
        }
        hook::WM_APP_TEXT_TRIGGER => {
            with_app(|a| {
                if a.recording_window.is_invalid() {
                    a.start_snip("text", lp.0 as i64);
                }
            });
            LRESULT(0)
        }
        hook::WM_APP_RECORDED_CHORD => {
            with_app(|a| {
                if !a.recording_window.is_invalid() {
                    post(a.recording_window, crate::uiwin::WM_APP_RECORDED_CHORD, wp.0, lp.0);
                }
            });
            LRESULT(0)
        }
        hook::WM_APP_TRIGGER => {
            with_app(|a| {
                if a.recording_window.is_invalid() {
                    a.start_snip("win+shift+s", lp.0 as i64);
                } else {
                    post(a.recording_window, crate::uiwin::WM_APP_HOTKEY_REFUSED, 0, 0);
                }
            });
            LRESULT(0)
        }
        WM_APP_RECORD_HOTKEY => {
            let owner = HWND(wp.0 as *mut _);
            let settings = unsafe { FindWindowW(crate::uiwin::SETTINGS_CLASS, PCWSTR::null()) }.ok();
            with_app(|a| {
                if !a.is_session() && (owner.is_invalid() || settings == Some(owner)) {
                    a.record_hotkey(owner);
                }
            });
            LRESULT(0)
        }
        WM_HOTKEY if wp.0 as i32 == recording::HOTKEY => {
            with_app(|a| a.record_toggle(false));
            LRESULT(0)
        }
        crate::record_win::WM_STOP => { with_app(|a| a.record_stop_event()); LRESULT(0) }
        WM_HOTKEY if wp.0 as i32 == HOTKEY_PRTSCN => {
            with_app(|a| { if a.recording_window.is_invalid() { a.start_snip("prtscn", qpc()); } });
            LRESULT(0)
        }
        WM_HOTKEY if wp.0 as i32 == HOTKEY_ALL || wp.0 as i32 == HOTKEY_ALL2 => {
            with_app(|a| { if a.recording_window.is_invalid() { a.start_snip("all", qpc()); } });
            LRESULT(0)
        }
        WM_HOTKEY if wp.0 as i32 == HOTKEY_EDITOR || wp.0 as i32 == HOTKEY_EDITOR2 => {
            with_app(|a| { if a.recording_window.is_invalid() { a.start_snip("editor", qpc()); } });
            LRESULT(0)
        }
        crate::uiwin::WM_APP_RELOAD => {
            with_app(|a| {
                if !a.is_session() {
                    a.reload();
                }
            });
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            with_app(|a| {
                if !a.is_session() {
                    a.register_hotkeys();
                    a.refresh_tray();
                    a.write_state();
                }
            });
            LRESULT(0)
        }
        WM_APP_COMMAND if wp.0 == 5 || wp.0 == 6 => {
            with_app(|a| { if wp.0 == 6 || a.recorder.child.is_none() { a.record_toggle(wp.0 == 6); } }); LRESULT(0)
        }
        WM_APP_COMMAND => {
            let source = match wp.0 {
                2 => "editor",
                3 => "text",
                4 => "all",
                _ => "tray",
            };
            with_app(|a| match lp.0 {
                3 | 5 => a.start_countdown(lp.0 as u32, if wp.0 == 2 { "editor" } else { "delay" }),
                _ => a.start_snip(source, qpc()),
            });
            LRESULT(0)
        }
        WM_APP_CUSTOM_COLOR => {
            // The colour dialog sits above the overlay (it is owned by an overlay window). While it
            // is open, Esc and Enter belong to the dialog, not to the overlay.
            let info = with_app(|a| {
                a.overlay_keys(false);
                a.dialog_open = true;
                a.overlay.as_ref().map(|o| {
                    o.set_modal(true);
                    (o.owner(), o.editor_color().unwrap_or_default())
                })
            });
            let picked = match info {
                Some(Some((owner, cur))) => crate::actions::choose_color(owner, cur),
                _ => None,
            };
            with_app(|a| {
                a.last_input = Instant::now();
                a.dialog_open = false;
                if let Some(o) = &a.overlay {
                    o.set_modal(false);
                    a.overlay_keys(true);
                }
                if let (Some(c), Some(o)) = (picked, a.overlay.as_mut()) {
                    o.set_color(c);
                }
                if let Some(o) = &mut a.overlay
                    && !o.restore_editor_focus() {
                        // Never throw the work away for this: the first click takes the keyboard.
                        logf!("editor focus denied after colour dialog");
                        o.focused = false;
                        a.report_focus();
                }
            });
            LRESULT(0)
        }
        WM_APP_SAVE_AS => {
            // The dialog sits over the open editor and runs outside any borrow of the app.
            let info = with_app(|a| {
                a.overlay_keys(false);
                a.dialog_open = true;
                let dir = paths::output_dir(&a.cfg.folder);
                a.overlay.as_ref().map(|o| {
                    o.set_modal(true);
                    (o.owner(), dir)
                })
            })
            .flatten();
            let Some((owner, dir)) = info else { return LRESULT(0) };
            let name = format!("{}.png", naming::base_name(&local_stamp()));
            let chosen = crate::actions::save_as_dialog(Some(owner), &dir, &name);
            with_app(|a| {
                a.last_input = Instant::now();
                a.dialog_open = false;
                if let Some(o) = &a.overlay {
                    o.set_modal(false);
                    a.overlay_keys(true);
                }
                match chosen {
                    Some(path) => a.finish_save_as(path),
                    None => {
                        logf!("save as: cancelled; the editor stays open");
                        if let Some(o) = &mut a.overlay
                            && !o.restore_editor_focus()
                        {
                            logf!("editor focus denied after save dialog; the first click takes it");
                            o.focused = false;
                            a.report_focus();
                        }
                    }
                }
            });
            LRESULT(0)
        }
        WM_APP_GO => {
            with_app(|a| {
                unsafe {
                    let _ = KillTimer(Some(a.hwnd), TIMER_COOL);
                }
                // A prewarmed child may predate a Settings change. Re-read before the capture.
                if a.is_session() && a.opts.session.as_ref().is_none_or(|s| s.selftest_dir.is_none()) {
                    let (cfg, note) = Config::load_or_create(&paths::config_file());
                    if let Some(n) = note { logf!("{n}"); }
                    a.cfg = cfg;
                }
                a.start_snip_here(source_name(wp.0), lp.0 as i64)
            });
            LRESULT(0)
        }
        WM_APP_CHECK_FOCUS if wp.0 != 0 => {
            // Activated: an editor that opened without the keyboard has it now.
            with_app(|a| {
                let gained = a.overlay.as_mut().is_some_and(|o| {
                    let gained = o.is_editor() && !o.focused;
                    if gained {
                        o.focused = true;
                    }
                    gained
                });
                if gained {
                    a.report_focus();
                }
            });
            LRESULT(0)
        }
        WM_APP_CHECK_FOCUS => {
            with_app(|a| {
                if !a.dialog_open && a.overlay.as_ref().is_some_and(|o| o.editor.is_some() || o.is_text()) {
                    let fg = unsafe { GetForegroundWindow() };
                    let mut pid = 0;
                    if !fg.is_invalid() {
                        unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)); }
                    }
                    // Our colour dialog is in this process too. Another program in front: drawn
                    // work stays and the first click takes the keyboard back; an editor with nothing
                    // to lose closes, so a stray selection does not sit over what he went to.
                    if pid != std::process::id() {
                        if a.overlay.as_ref().is_some_and(|o| o.has_work()) {
                            logf!("editor lost foreground with drawn work; it stays open");
                            if let Some(o) = &mut a.overlay {
                                o.focused = false;
                            }
                            a.report_focus();
                        } else {
                            logf!("editor lost foreground; cancelling");
                            a.prev_fg = HWND::default();
                            a.handle(Outcome::Cancel);
                        }
                    }
                }
            });
            LRESULT(0)
        }
        WM_APP_OUTCOME => {
            // SAFETY: as above.
            if let Some(out) = unsafe { unboxed::<Outcome>(WM_APP_OUTCOME, lp.0) } {
                with_app(|a| a.handle(out));
            }
            LRESULT(0)
        }
        hook::WM_APP_PREWARM => {
            with_app(|a| {
                if !a.recording_window.is_invalid() || a.recorder.child.is_some() { return; }
                if a.cfg.separate_process && !a.is_session() {
                    a.retire_used_child();
                    if a.child.is_none() {
                        a.spawn_child(None);
                    }
                    return;
                }
                if a.warm.is_none() && a.overlay.is_none() {
                    let t = qpc();
                    a.warm = capture::create_gpu().ok();
                    logf!("prewarm: device {} in {:.2} ms ({:.2} ms after the Win+Shift key-down)", if a.warm.is_some() { "ready" } else { "FAILED" }, crate::log::ms_since(t), crate::log::ms_since(lp.0 as i64));
                }
                if !a.keep_device() {
                    unsafe {
                        SetTimer(Some(a.hwnd), TIMER_COOL, COOL_MS, None);
                    }
                }
            });
            LRESULT(0)
        }
        WM_APP_SESSION_READY => {
            with_app(|a| {
                if a.record_ready(HWND(wp.0 as *mut _), lp.0 as u32) { return; }
                if let Some(c) = &mut a.child {
                    let h = HWND(wp.0 as *mut _);
                    let mut pid = 0;
                    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)); }
                    if pid != c.pid || lp.0 as u32 != c.pid { return; }
                    c.hwnd = Some(h);
                    if let Some((t0, src)) = c.pending.take() {
                        c.editor = src == "editor";
                        unsafe { let _ = AllowSetForegroundWindow(c.pid); }
                        post(h, WM_APP_GO, source_code(src), t0 as isize);
                    }
                }
            });
            LRESULT(0)
        }
        WM_APP_SESSION_OVERLAY => {
            with_app(|a| {
                if a.record_overlay(wp.0, lp.0 as u32) { return; }
                if let Some(c) = &mut a.child {
                    if lp.0 as u32 != c.pid { return; }
                    let open = wp.0 & 1 != 0;
                    if c.overlay && !open {
                        c.used = true;
                    }
                    c.overlay = open;
                    c.editor = wp.0 & 2 != 0;
                    hook::set_overlay_active(crate::hotkey::route_overlay_keys(
                        c.overlay, c.editor, false,
                    ));
                }
            });
            LRESULT(0)
        }
        WM_APP_SESSION_BUSY => {
            with_app(|a| {
                if let Some(c) = &mut a.child
                    && lp.0 as u32 == c.pid {
                        c.used = true;
                }
            });
            LRESULT(0)
        }
        WM_APP_SESSION_KEYS => {
            with_app(|a| {
                if let Some(c) = &a.child {
                    if lp.0 as u32 != c.pid { return; }
                    hook::set_overlay_active(crate::hotkey::route_overlay_keys(
                        c.overlay,
                        c.editor,
                        wp.0 == 0,
                    ));
                }
            });
            LRESULT(0)
        }
        WM_APP_SESSION_DONE => {
            with_app(|a| {
                let pid = lp.0 as u32;
                if a.record_done(pid) { return; }
                let current = a.child.as_ref().is_some_and(|c| c.pid == pid);
                if !current && !a.retired.iter().any(|c| c.pid == pid) { return; }
                use crate::textcore::{DONE_TEXT_BASE, TextOutcome};
                if wp.0 >= DONE_TEXT_BASE {
                    if wp.0 == DONE_TEXT_BASE + 1 {
                        a.texts += 1;
                    }
                    if TextOutcome::code_is_failure(wp.0) {
                        a.warn_text_problem(wp.0);
                    }
                } else if wp.0 >= 1 {
                    a.snips += 1;
                    if let Ok(p) = std::fs::read_to_string(paths::last_file())
                        && let Ok(mut g) = LAST_FILE.lock()
                    {
                        *g = p;
                    }
                }
                if wp.0 == 1 {
                    if let Ok(mut g) = SAVE_ERRORS.lock() {
                        *g += 1;
                    }
                    a.warn_save_problem();
                }
                if current {
                    if let Some(c) = &mut a.child {
                        c.overlay = false;
                        c.used = true;
                    }
                    hook::set_overlay_active(false);
                }
                a.write_state();
            });
            LRESULT(0)
        }
        hook::WM_APP_OVERLAY_KEY => {
            // In the tray process with a session child: forward the key to the child.
            let fwd = with_app(|a| {
                a.recorder.child.as_ref().or(a.child.as_ref())
                    .and_then(|c| if c.overlay { c.hwnd } else { None })
            })
            .flatten();
            if let Some(h) = fwd {
                post(h, hook::WM_APP_OVERLAY_KEY, wp.0, lp.0);
                return LRESULT(0);
            }
            let out = with_app(|a| {
                a.overlay
                    .as_mut()
                    .map(|o| o.on_key(wp.0 as u32, lp.0 != 0))
                    .unwrap_or(Outcome::Nothing)
            });
            if let Some(out) = out {
                with_app(|a| a.handle(out));
            }
            LRESULT(0)
        }
        hook::WM_APP_HOOK_STATUS => {
            with_app(|a| {
                let ok = wp.0 != 0;
                let changed = a.armed != Some(ok);
                a.armed = Some(ok);
                a.hook_err = lp.0 as i32;
                if changed || !ok {
                    logf!("hook {} (error {:#x}, rearms {})", if ok { "armed" } else { "NOT ARMED" }, a.hook_err, a.rearms);
                }
                a.refresh_tray();
                a.write_state();
            });
            LRESULT(0)
        }
        tray::WM_APP_TRAY => {
            let ev = (lp.0 & 0xFFFF) as u32;
            if ev == WM_RBUTTONUP || ev == WM_CONTEXTMENU {
                let info = with_app(|a| {
                    let text_key = a.text_key().map(|k| k.display()).unwrap_or_default();
                    (a.status_text(), a.lang(), a.cfg.snip_enabled, a.cfg.text_enabled, a.cfg.win_shift_s, a.cfg.editor_key_text(), a.cfg.all_key_text(), text_key, crate::sys::theme(&a.cfg).mode)
                });
                if let Some((status, lang, snip_on, text_on, snip_key, editor_key, all_key, text_key, mode)) = info {
                    let (record_on, record_busy) = with_app(|a| (a.cfg.record_enabled, a.recorder.child.is_some())).unwrap_or_default();
                    let cmd = tray::menu(hwnd, &tray::MenuInfo { record_on, record_busy, status: &status, lang, snip_on, text_on, snip_key, editor_key: &editor_key, all_key: &all_key, text_key: &text_key, mode });
                    run_command(cmd);
                }
            } else if ev == WM_LBUTTONDBLCLK {
                run_command(tray::CMD_HOME);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            match wp.0 {
                crate::record_win::TIMER => { with_app(|a| a.record_tick()); }
                TIMER_REARM => {
                    rearm("timer");
                    with_app(|a| a.retry_hotkeys());
                }
                TIMER_DELAY => {
                    thread_local! {
                        static SHOWN: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
                    }
                    let shown = SHOWN.with(|c| c.get());
                    let now = with_app(|a| a.tick_countdown(shown)).flatten();
                    SHOWN.with(|c| c.set(now));
                }
                TIMER_CHILD => {
                    with_app(|a| a.check_child());
                }
                TIMER_RECORDING => {
                    with_app(|a| {
                        if !a.recording_window.is_invalid() && unsafe { GetForegroundWindow() } != a.recording_window {
                            a.record_hotkey(HWND::default());
                        }
                    });
                }
                TIMER_COOL => {
                    with_app(|a| {
                        unsafe {
                            let _ = KillTimer(Some(a.hwnd), TIMER_COOL);
                        }
                        if a.is_session() {
                            if a.overlay.is_none() && a.steps.is_none() {
                                logf!("session: no snip within {} s, exiting", COOL_MS / 1000);
                                a.session_finish(0);
                            }
                        } else if a.overlay.is_none() && !a.keep_device() && a.warm.take().is_some() {
                            logf!("prewarm: device dropped unused");
                        }
                    });
                }
                TIMER_OVERLAY => {
                    let out = with_app(|a| {
                        // A safety net for a forgotten overlay; it must never throw away drawn work.
                        let idle = a.last_input.elapsed().as_secs() >= a.cfg.overlay_timeout_seconds as u64;
                        if idle && !a.dialog_open && a.overlay.as_ref().is_some_and(|o| !o.has_work()) {
                            logf!("overlay timed out after {} s without input", a.cfg.overlay_timeout_seconds);
                            Some(Outcome::Cancel)
                        } else {
                            None
                        }
                    });
                    if let Some(Some(out)) = out {
                        with_app(|a| a.handle(out));
                    }
                }
                TIMER_EXIT => {
                    idle_report();
                    post(hwnd, WM_CLOSE, 0, 0);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_WTSSESSION_CHANGE => {
            if wp.0 == 7 { with_app(|a| a.record_stop_event()); }
            if wp.0 == WTS_SESSION_UNLOCK {
                rearm("unlock");
            }
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            if wp.0 as u32 == PBT_APMSUSPEND { with_app(|a| a.record_stop_event()); }
            if wp.0 as u32 == PBT_APMRESUMEAUTOMATIC || wp.0 as u32 == PBT_APMRESUMESUSPEND {
                rearm("resume");
            }
            LRESULT(1)
        }
        WM_DISPLAYCHANGE => {
            with_app(|a| a.record_stop_event());
            // A mode change invalidates the frozen frames: close any overlay, then re-arm.
            if with_app(|a| a.overlay.is_some()) == Some(true) {
                with_app(|a| a.handle(Outcome::Cancel));
            }
            rearm("display change");
            LRESULT(0)
        }
        WM_APP_SAVED => {
            with_app(|a| {
                a.write_state();
                if let Some(r) = a.done_pending.take() {
                    let clean = wp.0 != 0 && !a.soft_fail;
                    a.session_finish(if clean { r + 1 } else { r });
                    if !clean && !a.is_session() {
                        a.warn_save_problem();
                    }
                    a.soft_fail = false;
                }
            });
            LRESULT(0)
        }
        WM_CLOSE if with_app(|a| a.recorder.session.as_ref().is_some_and(|s|s.busy) || a.recorder.child.is_some()) == Some(true) => {
            with_app(|a| { a.recorder.quitting = true; a.record_stop_event(); }); LRESULT(0)
        }
        WM_CLOSE if with_app(|a| a.is_session() && (a.done_pending.is_some() || a.text_pending || crate::card::is_open())) == Some(true) => {
            // The save worker will close this session after publishing its files.
            LRESULT(0)
        }
        WM_CLOSE | WM_ENDSESSION => {
            if msg == WM_ENDSESSION && wp.0 == 0 {
                return LRESULT(0);
            }
            shutdown(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        m if with_app(|a| a.taskbar_created != 0 && m == a.taskbar_created) == Some(true) => {
            with_app(|a| {
                a.tray_ok = false;
                a.refresh_tray();
            });
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn rearm(why: &str) {
    let go = with_app(|a| {
        if a.hook_thread.is_some() {
            a.rearms += 1;
            true
        } else {
            false
        }
    });
    if go == Some(true) {
        if why != "timer" {
            logf!("re-arming hook: {why}");
        }
        hook::rearm();
    }
}

fn open_path(p: &std::path::Path) {
    if crate::sys::is_elevated() {
        crate::actions::open_path(p);
        return;
    }
    let wide: Vec<u16> = p.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    unsafe {
        ShellExecuteW(None, w!("open"), PCWSTR(wide.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

fn run_command(cmd: u32) {
    match cmd {
        tray::CMD_RECORD => { with_app(|a| a.record_toggle(false)); }
        tray::CMD_SNIP => {
            with_app(|a| a.start_snip("tray", qpc()));
        }
        tray::CMD_EDIT => {
            with_app(|a| a.start_snip("editor", qpc()));
        }
        tray::CMD_ALL => {
            with_app(|a| a.start_snip("all", qpc()));
        }
        tray::CMD_TEXT => {
            with_app(|a| a.start_snip("text", qpc()));
        }
        tray::CMD_HOME => {
            crate::actions::spawn_self("home");
        }
        tray::CMD_SNIP3 => {
            with_app(|a| a.start_countdown(3, "delay"));
        }
        tray::CMD_SNIP5 => {
            with_app(|a| a.start_countdown(5, "delay"));
        }
        tray::CMD_HISTORY => {
            crate::actions::spawn_self("history");
        }
        tray::CMD_SETTINGS => {
            crate::actions::spawn_self("settings");
        }
        tray::CMD_FOLDER => {
            let dir = with_app(|a| paths::output_dir(&a.cfg.folder)).unwrap_or_else(paths::screenshots_dir);
            open_path(&dir);
        }
        tray::CMD_LOG => open_path(&paths::log_file()),
        tray::CMD_REARM => rearm("tray menu"),
        tray::CMD_QUIT => {
            if let Some(h) = with_app(|a| a.hwnd) {
                post(h, WM_CLOSE, 0, 0);
            }
        }
        _ => {}
    }
}

fn shutdown(hwnd: HWND) {
    let parts = with_app(|a| {
        if let Some(ov) = a.close_overlay() {
            drop(ov);
        }
        if a.tray_ok {
            tray::remove(a.hwnd);
            a.tray_ok = false;
        }
        if !a.is_session() {
            unsafe {
                let _ = UnregisterHotKey(Some(a.hwnd), recording::HOTKEY);
                let _ = UnregisterHotKey(Some(a.hwnd), HOTKEY_PRTSCN);
                let _ = UnregisterHotKey(Some(a.hwnd), HOTKEY_EDITOR);
                let _ = UnregisterHotKey(Some(a.hwnd), HOTKEY_EDITOR2);
                if !a.badge.is_invalid() {
                    let _ = DestroyWindow(a.badge);
                }
                let _ = WTSUnRegisterSessionNotification(a.hwnd);
            }
        }
        if let Some(c) = a.child.take() {
            if let Some(h) = c.hwnd { post(h, WM_CLOSE, 0, 0); }
            unsafe {
                let _ = CloseHandle(c.process);
            }
        }
        for c in a.retired.drain(..) {
            unsafe {
                let _ = CloseHandle(c.process);
            }
        }
        a.armed = Some(false);
        a.hook_thread.take()
    });
    if let Some(Some(h)) = parts {
        hook::stop(h);
    }
    let is_session = with_app(|a| a.is_session()).unwrap_or(false);
    if !is_session {
        let (ev, avg, max, trig) = hook::stats();
        logf!("snip stopping: hook events={ev} avg={avg:.3}us max={max:.3}us triggers={trig}");
        with_app(|a| a.write_state());
    }
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

/// Memory and CPU of this process, for the idle test.
pub fn self_usage() -> (u64, u64, f64) {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    unsafe {
        let mut pmc = PROCESS_MEMORY_COUNTERS_EX { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc as *mut _ as *mut PROCESS_MEMORY_COUNTERS, pmc.cb);
        let (mut c, mut e, mut k, mut u) = Default::default();
        let _ = GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
        let t = |f: windows::Win32::Foundation::FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 10_000.0;
        (pmc.PrivateUsage as u64, pmc.WorkingSetSize as u64, t(k) + t(u))
    }
}

fn idle_report() {
    let (private, ws, cpu_ms) = self_usage();
    let secs = with_app(|a| a.opts.exit_after.unwrap_or(0)).unwrap_or(0);
    logf!(
        "idle-test after {secs} s: private={:.2} MB working_set={:.2} MB cpu_total={:.1} ms (includes start-up)",
        private as f64 / 1048576.0,
        ws as f64 / 1048576.0,
        cpu_ms
    );
}

unsafe extern "system" fn badge_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Register the main window class `main` (with main_proc) and the overlay class.
pub fn register_classes(hinst: HINSTANCE, main: PCWSTR) -> windows::core::Result<()> {
    unsafe {
        let mc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(main_proc),
            hInstance: hinst,
            lpszClassName: main,
            ..Default::default()
        };
        if RegisterClassExW(&mc) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let ov = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(overlay_proc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
            lpszClassName: overlay::CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&ov) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        crate::textinput::register(hinst);
        // The card's class is registered when one is shown.
        let badge = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(badge_proc), hInstance: hinst, lpszClassName: BADGE_CLASS, ..Default::default() };
        // A second registration in the same process (self-test) is fine to fail.
        RegisterClassExW(&badge);
        Ok(())
    }
}

/// Run the tray process (or a session child) until it closes. Returns the process exit code.
pub fn run(cfg: Config, opts: RunOpts) -> i32 {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let hinst: HINSTANCE = GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default();
        let session = opts.session.is_some();
        let class = if session {
            SESSION_CLASS
        } else if opts.exit_after.is_some() {
            MAIN_CLASS_TEST
        } else {
            MAIN_CLASS
        };
        if let Err(e) = register_classes(hinst, class) {
            logf!("register classes failed: {e}");
            return 1;
        }
        // A top-level (not message-only) hidden window: it gets WM_DISPLAYCHANGE and TaskbarCreated.
        let hwnd = match CreateWindowExW(WS_EX_TOOLWINDOW, class, w!("snip"), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst), None) {
            Ok(h) => h,
            Err(e) => {
                logf!("main window failed: {e}");
                return 1;
            }
        };
        let taskbar_created = if session { 0 } else { RegisterWindowMessageW(w!("TaskbarCreated")) };
        if !session && opts.exit_after.is_none() {
            // Elevated: `snip quit`, `snip text`, the Settings window and Explorer's restart notice
            // come from normal level and would be dropped by Windows (UIPI).
            crate::sys::allow_normal_level_messages(hwnd, &[WM_CLOSE, WM_APP_COMMAND, crate::uiwin::WM_APP_RELOAD, taskbar_created]);
        }
        APP_HWND.with(|h| h.set(hwnd));
        let app = App {
            icon_ok: if session { HICON::default() } else { tray::make_icon(crate::theme::TRAY_ARMED_RGB, false) },
            icon_bad: if session { HICON::default() } else { tray::make_icon(crate::theme::TRAY_UNARMED_RGB, true) },
            cfg: cfg.clone(),
            opts: opts.clone(),
            hwnd,
            hinst,
            overlay: None,
            steps: None,
            source: "",
            prev_fg: HWND::default(),
            last_input: Instant::now(),
            armed: None,
            hook_err: 0,
            rearms: 0,
            prtscn: None,
            tray_ok: false,
            taskbar_created,
            snips: 0,
            sessions: 0,
            started: crate::log::now_text(),
            hook_thread: None,
            warm: None,
            child: None,
            done_pending: None,
            editor_hk: None,
            editor2_hk: None,
            all_hk: None,
            all2_hk: None,
            dialog_open: false,
            soft_fail: false,
            clip_seq: None,
            retired: Vec::new(),
            countdown: None,
            badge: HWND::default(),
            recording_window: HWND::default(),
            text_pending: false,
            deferred_finish: None,
            texts: 0,
            text_started: 0,
            recorder: recording::Recorder::default(),
        };
        APP.with(|a| *a.borrow_mut() = Some(app));

        if let Some(s) = &opts.session {
            // Session child: device first, then tell the tray process we are ready.
            let t = qpc();
            let g = capture::create_gpu();
            logf!("session pid={} device {} in {:.2} ms", std::process::id(), if g.is_ok() { "ready" } else { "FAILED" }, crate::log::ms_since(t));
            with_app(|a| a.warm = g.ok());
            if s.parent != 0 {
                let parent = HWND(s.parent as *mut _);
                if IsWindow(Some(parent)).as_bool() {
                    post(parent, WM_APP_SESSION_READY, hwnd.0 as usize, std::process::id() as isize);
                } else {
                    post(hwnd, WM_CLOSE, 0, 0);
                }
            }
            match s.go {
                Some((t0, src)) => post(hwnd, WM_APP_GO, src, t0 as isize),
                None => {
                    SetTimer(Some(hwnd), TIMER_COOL, COOL_MS, None);
                }
            }
        } else {
            with_app(|a| a.apply_text_key());
            let (hook_wanted, trigger) = with_app(|a| (a.hook_wanted(), a.snip_trigger_wanted())).unwrap_or((false, false));
            if opts.measure_hook {
                // Idle memory with the hook in place: it lets every key pass (no trigger, no key).
                hook::set_text_key(None);
                let h = hook::start(hwnd, false, false, false);
                with_app(|a| a.hook_thread = Some(h));
            } else if hook_wanted && !opts.no_hook {
                let h = hook::start(hwnd, trigger, cfg.mask_win_key, cfg.prewarm && !cfg.keep_gpu_device);
                with_app(|a| a.hook_thread = Some(h));
                SetTimer(Some(hwnd), TIMER_REARM, cfg.rearm_minutes.saturating_mul(60_000), None);
            }
            // A start entry whose program is gone is repaired; none is ever created here.
            if opts.exit_after.is_none() && !opts.no_hook {
                let exe = PathBuf::from(crate::actions::exe_path());
                match crate::autostart::reconcile_startup(&crate::autostart::HkcuRun::snip(), &exe, |p| p.exists()) {
                    Ok(crate::autostart::AutoStartAction::LeaveAsIs) => {}
                    Ok(a) => logf!("start entry: {}", a.name()),
                    Err(e) => logf!("start entry check FAILED: {e}"),
                }
            }
            with_app(|a| a.register_hotkeys());
            if !opts.no_hook {
                let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
            }
            if (cfg.keep_gpu_device || opts.warm_device) && !cfg.separate_process {
                let t = qpc();
                let g = capture::create_gpu();
                logf!("warm D3D11 device: {} in {:.2} ms", if g.is_ok() { "created" } else { "FAILED" }, crate::log::ms_since(t));
                with_app(|a| a.warm = g.ok());
            } else if opts.warm_device {
                let t = qpc();
                let g = capture::create_gpu();
                logf!("warm D3D11 device (idle-test variant): {} in {:.2} ms", if g.is_ok() { "created" } else { "FAILED" }, crate::log::ms_since(t));
                with_app(|a| a.warm = g.ok());
            }
            with_app(|a| {
                a.refresh_tray();
                a.write_state();
            });
            if let Some(s) = opts.exit_after {
                SetTimer(Some(hwnd), TIMER_EXIT, (s.max(1) * 1000) as u32, None);
            }
            let (private, ws, _) = self_usage();
            logf!(
                "snip {} started pid={} separate_process={} hook={} prtscn={} tray={} private={:.2}MB ws={:.2}MB",
                env!("CARGO_PKG_VERSION"),
                std::process::id(),
                cfg.separate_process,
                with_app(|a| a.hook_wanted()).unwrap_or(false) && !opts.no_hook,
                cfg.print_screen && !opts.no_hook,
                !opts.no_tray,
                private as f64 / 1048576.0,
                ws as f64 / 1048576.0
            );
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|a| a.borrow_mut().take());
        APP_HWND.with(|h| h.set(HWND::default()));
        CoUninitialize();
        0
    }
}

/// `snip quit`: ask the running instance to close. 0 stopped or not running, 1 still running.
pub fn quit_running() -> i32 {
    unsafe {
        let Ok(h) = FindWindowW(MAIN_CLASS, PCWSTR::null()) else {
            println!("snip is not running");
            return 0;
        };
        post(h, WM_CLOSE, 0, 0);
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if FindWindowW(MAIN_CLASS, PCWSTR::null()).is_err() {
                println!("snip stopped");
                return 0;
            }
        }
        println!("snip did not stop within 5 s");
        1
    }
}

pub fn is_running() -> bool {
    unsafe { FindWindowW(MAIN_CLASS, PCWSTR::null()).is_ok() }
}

/// Isolated hidden-HWND validation used only by --selftest-record, never the running tray.
pub fn record_ipc_selftest() -> Result<serde_json::Value, String> { recording::ipc_selftest() }
