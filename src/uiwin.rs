//! The main window (`snip home`), the history window (`snip history`) and the settings window
//! (`snip settings`): ordinary
//! top-level windows whose client area is a core-drawn canvas. Each runs in its own short-lived
//! process (GDI only), one instance at a time; starting it again brings the open one to the front.

use std::cell::RefCell;
use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, HOT_KEY_MODIFIERS, RegisterHotKey, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::config::Config;
use crate::gdi::GdiGlyphs;
use crate::history::{History, Store};
use crate::historyview::{self, Act, Grid, Hit};
use crate::hotkey::HkMods;
use crate::i18n::{Lang, S};
use crate::raster::Canvas;
use crate::settingsview::{self, Change, KeyField, Part};
use crate::sys::wide;
use crate::theme::Theme;

pub const HISTORY_CLASS: PCWSTR = w!("Laureth.snip.history");
pub const SETTINGS_CLASS: PCWSTR = w!("Laureth.snip.settings");
pub const HOME_CLASS: PCWSTR = w!("Laureth.snip.home");
/// settings -> tray: config.toml changed, read it again.
pub const WM_APP_RELOAD: u32 = WM_APP + 60;
pub const WM_APP_HOTKEY_REFUSED: u32 = WM_APP + 66;
/// Tray -> settings: the recorder's chord from the hook (wParam vk, lParam modifier bits).
pub const WM_APP_RECORDED_CHORD: u32 = WM_APP + 67;
const TIMER_NOTE: usize = 1;
const TIMER_POLL: usize = 2;
const WM_MOUSELEAVE: u32 = 0x02A3;

trait Panel {
    fn render(&mut self, w: i32, h: i32, scale: f32) -> Canvas;
    /// Returns whether to repaint.
    fn mouse(&mut self, hwnd: HWND, msg: u32, x: i32, y: i32) -> bool;
    fn wheel(&mut self, _delta: i32) -> bool {
        false
    }
    fn key(&mut self, hwnd: HWND, vk: u32, up: bool) -> bool;
    fn activated(&mut self) -> bool {
        false
    }
    fn deactivated(&mut self, _hwnd: HWND) {}
    fn system_changed(&mut self, _hwnd: HWND) -> bool { false }
    fn hotkey_refused(&mut self, _hwnd: HWND) -> bool { false }
    fn clear_note(&mut self) -> bool {
        false
    }
    fn recorded_chord(&mut self, _hwnd: HWND, _vk: u32, _bits: u32) -> bool {
        false
    }
    /// The window exists and is about to show.
    fn started(&mut self, _hwnd: HWND) {}
    fn poll(&mut self, _hwnd: HWND) -> bool {
        false
    }
    /// `--shot`: what to show besides the defaults.
    fn shot_setup(&mut self, _args: &[String]) {}
}

struct Host {
    panel: Box<dyn Panel>,
    glyph_scale: f32,
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

fn with_host<R>(f: impl FnOnce(&mut Host) -> R) -> Option<R> {
    HOST.with(|h| h.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn scale_of(hwnd: HWND) -> f32 {
    crate::dpi::scale(unsafe { GetDpiForWindow(hwnd) })
}

fn client(hwnd: HWND) -> (i32, i32) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
    }
    (r.right - r.left, r.bottom - r.top)
}

fn repaint(hwnd: HWND) {
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

fn xy(l: LPARAM) -> (i32, i32) {
    (
        (l.0 & 0xFFFF) as i16 as i32,
        ((l.0 >> 16) & 0xFFFF) as i16 as i32,
    )
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = unsafe { BeginPaint(hwnd, &mut ps) };
            let (w, h) = client(hwnd);
            let sc = scale_of(hwnd);
            if w > 0
                && h > 0
                && let Some(cv) = with_host(|host| {
                    host.glyph_scale = sc;
                    host.panel.render(w, h, sc)
                })
            {
                crate::gdi::blit(dc, &cv);
            }
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_SIZE => {
            repaint(hwnd);
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let r = unsafe { &*(lp.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            repaint(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK | WM_RBUTTONUP => {
            let (x, y) = xy(lp);
            if msg == WM_MOUSEMOVE {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                unsafe {
                    let _ = TrackMouseEvent(&mut tme);
                }
            }
            if with_host(|host| host.panel.mouse(hwnd, msg, x, y)) == Some(true) {
                repaint(hwnd);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            if with_host(|host| host.panel.mouse(hwnd, WM_MOUSEMOVE, -1, -1)) == Some(true) {
                repaint(hwnd);
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
            if with_host(|host| host.panel.wheel(delta)) == Some(true) {
                repaint(hwnd);
            }
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
            let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
            if with_host(|host| host.panel.key(hwnd, wp.0 as u32, up)) == Some(true) {
                repaint(hwnd);
            }
            if msg == WM_SYSKEYDOWN || msg == WM_SYSKEYUP {
                // Alt+F4 still closes.
                return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            if wp.0 & 0xFFFF != 0 && with_host(|host| host.panel.activated()) == Some(true) {
                repaint(hwnd);
            } else if wp.0 & 0xFFFF == 0 {
                with_host(|host| host.panel.deactivated(hwnd));
            }
            LRESULT(0)
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            if with_host(|host| host.panel.system_changed(hwnd)) == Some(true) { repaint(hwnd); }
            LRESULT(0)
        }
        WM_APP_HOTKEY_REFUSED => {
            if with_host(|host| host.panel.hotkey_refused(hwnd)) == Some(true) { repaint(hwnd); }
            LRESULT(0)
        }
        WM_APP_RECORDED_CHORD => {
            if with_host(|host| host.panel.recorded_chord(hwnd, wp.0 as u32, lp.0 as u32)) == Some(true) { repaint(hwnd); }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == TIMER_POLL => {
            if with_host(|host| host.panel.poll(hwnd)) == Some(true) { repaint(hwnd); }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == TIMER_NOTE => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_NOTE);
            }
            if with_host(|host| host.panel.clear_note()) == Some(true) {
                repaint(hwnd);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            crate::app::wait_copy_png();
            unsafe { let _ = DestroyWindow(hwnd); }
            LRESULT(0)
        }
        WM_DESTROY => {
            with_host(|host| host.panel.deactivated(hwnd));
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn show_note_later(hwnd: HWND) {
    unsafe {
        SetTimer(Some(hwnd), TIMER_NOTE, 1600, None);
    }
}

/// Create and run one window until it closes.
fn run_window(
    class: PCWSTR,
    title: &str,
    w_dip: f32,
    h_dip: f32,
    resizable: bool,
    theme: &Theme,
    panel: Box<dyn Panel>,
) -> i32 {
    unsafe {
        if let Ok(h) = FindWindowW(class, PCWSTR::null()) {
            let _ = ShowWindow(h, SW_RESTORE);
            let _ = SetForegroundWindow(h);
            return 0;
        }
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
        let hinst = GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            hIcon: LoadIconW(Some(hinst), crate::sys::app_icon()).unwrap_or_default(),
            style: CS_DBLCLKS,
            lpfnWndProc: Some(proc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let mons = crate::sys::monitors();
        let place = mons
            .centered_window(crate::sys::cursor(), w_dip, h_dip)
            .unwrap_or(crate::geometry::Rect::from_xywh(
                100,
                100,
                w_dip as i32,
                h_dip as i32,
            ));
        let style = if resizable {
            WS_OVERLAPPEDWINDOW
        } else {
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX
        };
        let t = wide(title);
        // Create at the client size; the frame is added around it.
        let mut r = RECT {
            left: place.left,
            top: place.top,
            right: place.right,
            bottom: place.bottom,
        };
        let _ = AdjustWindowRectEx(&mut r, style, false, WINDOW_EX_STYLE(0));
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR(t.as_ptr()),
            style,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            None,
            None,
            Some(hinst),
            None,
        ) else {
            return 1;
        };
        crate::sys::title_bar_mode(hwnd, theme);
        HOST.with(|h| {
            *h.borrow_mut() = Some(Host {
                panel,
                glyph_scale: 1.0,
            })
        });
        with_host(|host| host.panel.started(hwnd));
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--shot") {
            return take_shot(hwnd, &args, i);
        }
        // `--passive`: an automated check looking at the window on the real screen must not take the keyboard.
        if std::env::args().any(|a| a == "--passive") {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        HOST.with(|h| h.borrow_mut().take());
        0
    }
}

/// `snip settings --shot FILE [--tab N] [...]`: the real window, as pixels, then exit. Only on a
/// hidden desktop: a look test must not open windows on the user's screen.
fn take_shot(hwnd: HWND, args: &[String], at: usize) -> i32 {
    let desk = crate::selftest::desktop_name();
    if desk.is_empty() || desk.eq_ignore_ascii_case("Default") || desk.eq_ignore_ascii_case("Winlogon") {
        println!("REFUSED: --shot requires a hidden desktop");
        return 4;
    }
    let Some(path) = args.get(at + 1) else { return 2 };
    with_host(|host| host.panel.shot_setup(args));
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = windows::Win32::Graphics::Gdi::UpdateWindow(hwnd);
        // Let the frame and the first paints finish.
        let mut msg = MSG::default();
        for _ in 0..25 {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    let r = crate::overlay::window_rect(hwnd);
    let mut code = 1;
    let framed = crate::looktest::print_window(hwnd, r.width(), r.height());
    let (w, h) = client(hwnd);
    let client_canvas = with_host(|host| host.panel.render(w, h, scale_of(hwnd)));
    // A window on a hidden desktop may print black: then the client area is the honest picture.
    let usable = framed.filter(|c| c.px.as_chunks::<4>().0.iter().any(|p| p[0] != 0 || p[1] != 0 || p[2] != 0));
    if let Some(cv) = usable.or(client_canvas)
        && let Ok(png) = crate::png::encode_png(cv.w, cv.h, &cv.px)
        && std::fs::write(path, png).is_ok()
    {
        println!("wrote {path} {}x{}", cv.w, cv.h);
        code = 0;
    }
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    HOST.with(|h| h.borrow_mut().take());
    code
}

// ---- History ----

struct HistoryPanel {
    store: Store,
    hist: History,
    thumbs: Vec<Option<Canvas>>,
    missing: Vec<bool>,
    grid: Grid,
    hover: Option<usize>,
    focus: Option<usize>,
    note: Option<String>,
    theme: Theme,
    lang: Lang,
    glyphs: GdiGlyphs,
    folder: PathBuf,
    clipboard_png: bool,
}

impl HistoryPanel {
    fn reload(&mut self) {
        let (h, note) = self.store.load();
        if let Some(n) = note {
            crate::logf!("{n}");
        }
        if h == self.hist && !self.thumbs.is_empty() {
            return;
        }
        self.thumbs = h.entries.iter().map(|e| self.store.thumb(e.id)).collect();
        self.missing = h
            .entries
            .iter()
            .map(|e| !std::path::Path::new(&e.path).exists())
            .collect();
        self.hist = h;
        self.grid.count = self.hist.entries.len();
        self.grid.scroll_by(0);
        self.focus = self.focus.filter(|&i| i < self.grid.count);
    }

    fn path(&self, i: usize) -> Option<PathBuf> {
        self.hist.entries.get(i).map(|e| PathBuf::from(&e.path))
    }

    fn act(&mut self, hwnd: HWND, i: usize, a: Act) {
        let Some(p) = self.path(i) else { return };
        match a {
            Act::Copy => {
                if let Some(img) = crate::actions::load_png(&p) {
                    let ok = crate::app::copy_picture(hwnd, &img, self.clipboard_png).is_ok();
                    self.note = Some(if ok {
                        S::Copied.get(self.lang).to_string()
                    } else {
                        "?".into()
                    });
                } else {
                    self.note = Some(S::FileMissing.get(self.lang).to_string());
                }
                show_note_later(hwnd);
            }
            Act::Pin => {
                let c = crate::sys::cursor();
                if p.exists() {
                    crate::actions::spawn_self(&format!(
                        "--pin {} {} {}",
                        crate::actions::quote(&p.to_string_lossy()),
                        c.x,
                        c.y
                    ));
                }
            }
            Act::Remove => {
                if let Some(e) = self.hist.entries.get(i)
                    && let Err(err) = self.store.remove(e.id) {
                        crate::logf!("history remove FAILED: {err}");
                        self.note = Some(format!("{}: {err}", S::Remove.get(self.lang)));
                        show_note_later(hwnd);
                }
                self.thumbs.clear();
                self.reload();
            }
        }
    }

    fn context_menu(&mut self, hwnd: HWND, i: usize) {
        let Some(p) = self.path(i) else { return };
        let t = |s: S| s.get(self.lang).to_string();
        let items = [
            (1usize, t(S::Copy)),
            (2, t(S::Pin)),
            (3, t(S::Open)),
            (4, t(S::ShowInFolder)),
            (5, t(S::Remove)),
        ];
        let cmd = unsafe {
            let Ok(m) = CreatePopupMenu() else { return };
            let texts: Vec<Vec<u16>> = items.iter().map(|(_, s)| wide(s)).collect();
            for (k, (id, _)) in items.iter().enumerate() {
                let _ = AppendMenuW(m, MF_STRING, *id, PCWSTR(texts[k].as_ptr()));
            }
            let c = crate::sys::cursor();
            crate::sys::menu_mode(self.theme.mode);
            let r = TrackPopupMenu(
                m,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                c.x,
                c.y,
                Some(0),
                hwnd,
                None,
            );
            let _ = DestroyMenu(m);
            r.0 as usize
        };
        match cmd {
            1 => self.act(hwnd, i, Act::Copy),
            2 => self.act(hwnd, i, Act::Pin),
            3 => crate::actions::open_path(&p),
            4 => crate::actions::show_in_folder(&p),
            5 => self.act(hwnd, i, Act::Remove),
            _ => {}
        }
    }
}

impl Panel for HistoryPanel {
    fn render(&mut self, w: i32, h: i32, scale: f32) -> Canvas {
        if (self.grid.w, self.grid.h, self.grid.scale) != (w, h, scale) {
            let (scroll, folder_hover) = (self.grid.scroll, self.grid.folder_hover);
            self.grid = Grid::new(w, h, scale, self.hist.entries.len());
            self.grid.scroll_by(scroll);
            self.grid.folder_hover = folder_hover;
        }
        let items: Vec<historyview::Item> = self
            .hist
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| historyview::Item {
                entry: e,
                thumb: self.thumbs.get(i).and_then(|t| t.as_ref()),
                missing: self.missing.get(i).copied().unwrap_or(false),
            })
            .collect();
        let mut cv = historyview::render(
            &self.grid,
            &self.theme,
            self.lang,
            &items,
            self.hover,
            self.focus,
            &crate::sys::today_text(),
            &self.glyphs,
        );
        if let Some(n) = &self.note {
            let f = self.theme.font * scale;
            let tm = crate::raster::Glyphs::text(&self.glyphs, n, f, true);
            let fb = self.grid.folder_box();
            cv.text_mask(
                &tm,
                fb.x0 - tm.w as i32 - (crate::theme::HISTORY_NOTE_GAP * scale) as i32,
                fb.y0 + (fb.h() - tm.h as i32) / 2,
                historyview::note_color(&self.theme),
            );
        }
        cv
    }

    fn mouse(&mut self, hwnd: HWND, msg: u32, x: i32, y: i32) -> bool {
        let hit = self.grid.hit(x, y, self.hover);
        match msg {
            WM_MOUSEMOVE => {
                let h = match hit {
                    Some(Hit::Cell(i)) | Some(Hit::Action(i, _)) => Some(i),
                    _ => None,
                };
                let on_folder = hit == Some(Hit::OpenFolder);
                let changed = h != self.hover || on_folder != self.grid.folder_hover;
                self.hover = h;
                self.grid.folder_hover = on_folder;
                changed
            }
            WM_LBUTTONUP => match hit {
                Some(Hit::Action(i, a)) => {
                    self.act(hwnd, i, a);
                    true
                }
                Some(Hit::Cell(i)) => {
                    self.focus = Some(i);
                    true
                }
                Some(Hit::OpenFolder) => {
                    crate::actions::open_path(&self.folder);
                    false
                }
                None => false,
            },
            WM_LBUTTONDBLCLK => {
                if let Some(Hit::Cell(i)) = hit
                    && let Some(p) = self.path(i)
                {
                    crate::actions::open_path(&p);
                }
                false
            }
            WM_RBUTTONUP => {
                if let Some(Hit::Cell(i)) | Some(Hit::Action(i, _)) = hit {
                    self.focus = Some(i);
                    self.context_menu(hwnd, i);
                }
                true
            }
            _ => false,
        }
    }

    fn wheel(&mut self, delta: i32) -> bool {
        let before = self.grid.scroll;
        self.grid.scroll_by(-delta * self.grid.row_h() / 240);
        before != self.grid.scroll
    }

    fn key(&mut self, hwnd: HWND, vk: u32, up: bool) -> bool {
        if up {
            return false;
        }
        let ctrl = unsafe { GetKeyState(0x11) } < 0;
        let mv = |s: &mut Self, dx: i32, dy: i32| {
            s.focus = s.grid.step(s.focus, dx, dy);
            if let Some(i) = s.focus {
                s.grid.reveal(i);
            }
            true
        };
        match vk {
            0x25 => mv(self, -1, 0),
            0x27 => mv(self, 1, 0),
            0x26 => mv(self, 0, -1),
            0x28 => mv(self, 0, 1),
            0x0D | 0x43 if vk == 0x0D || ctrl => {
                if let Some(i) = self.focus {
                    self.act(hwnd, i, Act::Copy);
                }
                true
            }
            0x2E => {
                if let Some(i) = self.focus {
                    self.act(hwnd, i, Act::Remove);
                }
                true
            }
            0x1B => {
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                false
            }
            _ => false,
        }
    }

    fn activated(&mut self) -> bool {
        self.thumbs.clear();
        self.reload();
        true
    }

    fn clear_note(&mut self) -> bool {
        self.note.take().is_some()
    }
}

pub fn run_history() -> i32 {
    let (cfg, _) = Config::load_or_create(&crate::paths::config_file());
    let theme = crate::sys::theme(&cfg);
    let lang = crate::sys::lang(&cfg);
    let mut p = HistoryPanel {
        store: Store::new(crate::paths::history_dir()),
        hist: History::default(),
        thumbs: Vec::new(),
        missing: Vec::new(),
        grid: Grid::new(1, 1, 1.0, 0),
        hover: None,
        focus: None,
        note: None,
        theme,
        lang,
        glyphs: GdiGlyphs::new(),
        folder: crate::paths::output_dir(&cfg.folder),
        clipboard_png: cfg.clipboard_png,
    };
    p.reload();
    run_window(
        HISTORY_CLASS,
        &format!("{} - Snip", S::History.get(lang)),
        crate::theme::HISTORY_WINDOW_WIDTH,
        crate::theme::HISTORY_WINDOW_HEIGHT,
        true,
        &theme,
        Box::new(p),
    )
}

// ---- Settings ----

struct SettingsPanel {
    cfg: Config,
    theme: Theme,
    lang: Lang,
    glyphs: GdiGlyphs,
    layout: Option<settingsview::Layout>,
    tab: usize,
    hover: Option<Part>,
    recording: Option<KeyField>,
    message: Option<(String, bool)>,
    autostart: bool,
    /// (tag, label) of the language menu; made once, Windows is asked only here.
    languages: Vec<(String, String)>,
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

impl SettingsPanel {
    fn set_recording(&mut self, hwnd: HWND, field: Option<KeyField>) {
        if self.recording == field { return; }
        let changed_on = self.recording.is_some() != field.is_some();
        self.recording = field;
        if changed_on {
            unsafe {
                if let Ok(tray) = FindWindowW(crate::app::MAIN_CLASS, PCWSTR::null()) {
                    SendMessageW(tray, crate::app::WM_APP_RECORD_HOTKEY,
                        Some(WPARAM(if field.is_some() { hwnd.0 as usize } else { 0 })), Some(LPARAM(0)));
                }
            }
        }
    }

    fn refresh_appearance(&mut self, hwnd: HWND) {
        self.theme = crate::sys::theme(&self.cfg);
        self.lang = crate::sys::lang(&self.cfg);
        self.layout = None;
        self.languages = language_list(self.lang, &self.cfg.ocr_language);
        crate::sys::title_bar_mode(hwnd, &self.theme);
        let title = wide(&format!("{} - Snip", S::Settings.get(self.lang)));
        unsafe { let _ = SetWindowTextW(hwnd, PCWSTR(title.as_ptr())); }
    }

    fn save(&mut self) {
        let path = crate::paths::config_file();
        match crate::naming::write_small(&path, self.cfg.to_file_text().as_bytes()) {
            Ok(()) => {
                crate::actions::tell_tray(WM_APP_RELOAD, 0, 0);
                self.message = Some((S::SettingsSaved.get(self.lang).to_string(), false));
            }
            Err(e) => self.message = Some((format!("{}: {e}", path.display()), true)),
        }
        if !self.cfg.text_history {
            crate::texthistory::clear(&crate::paths::history_dir());
        }
        // Theme and language apply to this window at once.
        self.theme = crate::sys::theme(&self.cfg);
        self.lang = crate::sys::lang(&self.cfg);
        self.layout = None;
    }

    fn apply(&mut self, hwnd: HWND, change: Change) {
        match change {
            Change::None => {}
            Change::Config => {
                self.save();
                self.refresh_appearance(hwnd);
            }
            Change::StartRecording(f) => self.set_recording(hwnd, Some(f)),
            Change::ShowTab(i) => {
                self.tab = i;
                self.layout = None;
                self.hover = None;
                self.message = None;
            }
            Change::PickFolder => {
                let start = crate::paths::output_dir(&self.cfg.folder);
                if let Some(p) = crate::actions::pick_folder(Some(hwnd), &start) {
                    self.cfg.folder = p.to_string_lossy().into_owned();
                    self.save();
                }
            }
            Change::PickLanguage => self.pick_language(hwnd),
            Change::ToggleAutostart => {
                let exe = PathBuf::from(crate::actions::exe_path());
                let want = !self.autostart;
                match crate::autostart::apply_user_change(&crate::autostart::HkcuRun::snip(), want, &exe) {
                    Ok(_) => {
                        self.autostart = want;
                        self.message = Some((S::SettingsSaved.get(self.lang).to_string(), false));
                    }
                    Err(e) => {
                        crate::logf!("start entry FAILED: {e}");
                        self.message = Some((S::AutostartFailed.get(self.lang).to_string(), true));
                    }
                }
            }
            Change::Message(s) => self.message = Some((s.get(self.lang).to_string(), true)),
        }
    }

    fn pick_language(&mut self, hwnd: HWND) {
        let texts: Vec<Vec<u16>> = self.languages.iter().map(|(_, l)| wide(l)).collect();
        let chosen = unsafe {
            let Ok(m) = CreatePopupMenu() else { return };
            for (i, t) in texts.iter().enumerate() {
                let on = self.languages[i].0.eq_ignore_ascii_case(self.cfg.ocr_language.trim());
                let _ = AppendMenuW(m, if on { MF_STRING | MF_CHECKED } else { MF_STRING }, i + 1, PCWSTR(t.as_ptr()));
            }
            let c = crate::sys::cursor();
            crate::sys::menu_mode(self.theme.mode);
            let r = TrackPopupMenu(m, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, c.x, c.y, Some(0), hwnd, None);
            let _ = DestroyMenu(m);
            r.0 as usize
        };
        if chosen >= 1 && let Some((tag, _)) = self.languages.get(chosen - 1) {
            self.cfg.ocr_language = tag.clone();
            self.save();
        }
    }

    /// Is this combination free? Registering it for a moment tells (a key Snip holds itself counts
    /// as free). The text key lives in the hook, ahead of Windows, so a combination with Win that
    /// Windows keeps for itself is fine; one that another program holds is refused.
    fn hotkey_free(&self, hwnd: HWND, field: KeyField, before: &str) -> bool {
        let value = settingsview::field_value(&self.cfg, field);
        let Ok(hk) = crate::hotkey::Hotkey::parse(value) else {
            return true;
        };
        if before.trim().eq_ignore_ascii_case(value.trim()) {
            return true;
        }
        if field == KeyField::Text && hk.mods.win {
            return true;
        }
        unsafe {
            let ok = RegisterHotKey(Some(hwnd), 0x5E77, HOT_KEY_MODIFIERS(hk.mods.bits()), hk.vk).is_ok();
            if ok {
                let _ = UnregisterHotKey(Some(hwnd), 0x5E77);
            }
            ok
        }
    }

    /// A key arrived for the recorder, from the window or from the hook.
    fn record(&mut self, hwnd: HWND, vk: u32, mods: HkMods) -> bool {
        let Some(field) = self.recording else { return false };
        let before = settingsview::field_value(&self.cfg, field).to_string();
        match settingsview::record_key(&mut self.cfg, field, vk, mods) {
            None => false,
            Some(Change::Config) => {
                self.set_recording(hwnd, None);
                if self.hotkey_free(hwnd, field, &before) {
                    self.apply(hwnd, Change::Config);
                } else {
                    match field {
                        KeyField::Editor => self.cfg.editor_hotkey = before,
                        KeyField::Editor2 => self.cfg.editor_hotkey_2 = before,
                        KeyField::All => self.cfg.all_hotkey = before,
                        KeyField::All2 => self.cfg.all_hotkey_2 = before,
                        KeyField::Text => self.cfg.text_hotkey = before,
                    }
                    self.message = Some((S::HotkeyInUse.get(self.lang).to_string(), true));
                }
                true
            }
            Some(other) => {
                self.set_recording(hwnd, None);
                self.apply(hwnd, other);
                true
            }
        }
    }
}

fn language_list(lang: Lang, selection: &str) -> Vec<(String, String)> {
    let installed = crate::ocr::installed().unwrap_or_default();
    let unavailable = |tag: &str| format!("{} ({tag})", S::OcrUnavailable.get(lang));
    crate::textcore::language_options(S::OcrAuto.get(lang), unavailable, installed, selection)
}

impl Panel for SettingsPanel {
    fn render(&mut self, w: i32, h: i32, scale: f32) -> Canvas {
        let l = match &self.layout {
            Some(l) if (l.w, l.h, l.scale) == (w, h, scale) => l.clone(),
            _ => {
                let l = settingsview::layout(w, h, scale, self.lang, &self.glyphs, &self.theme, self.tab);
                self.layout = Some(l.clone());
                l
            }
        };
        let folder = if self.cfg.folder.trim().is_empty() {
            S::FolderDefault.get(self.lang).to_string()
        } else {
            self.cfg.folder.clone()
        };
        let print_screen_conflict = self.tab == settingsview::TAB_SNIP
            && self.cfg.snip_enabled
            && {
                let p = self.cfg.plan();
                [p.editor, p.editor2, p.all, p.all2].iter().flatten().any(|hk| hk.mods == HkMods::default() && hk.vk == crate::hotkey::VK_SNAPSHOT)
            }
            && crate::sys::print_screen_snipping_enabled().unwrap_or(true);
        let message = match self.message.as_ref() {
            Some((m, true)) => Some((m.as_str(), true)),
            _ if print_screen_conflict => Some((S::PrintScreenConflict.get(self.lang), true)),
            other => other.map(|(m, warn)| (m.as_str(), *warn)),
        };
        let language_text = self.languages.iter().find(|(t, _)| t.eq_ignore_ascii_case(self.cfg.ocr_language.trim())).map(|(_, l)| l.clone()).unwrap_or_else(|| self.cfg.ocr_language.clone());
        let st = settingsview::ViewState {
            recording: self.recording,
            hover: self.hover,
            message,
            folder_text: &folder,
            autostart: self.autostart,
            language_text: &language_text,
        };
        settingsview::render(&l, &self.theme, self.lang, &self.cfg, &st, &self.glyphs)
    }

    fn mouse(&mut self, hwnd: HWND, msg: u32, x: i32, y: i32) -> bool {
        let Some(l) = &self.layout else { return false };
        let hit = l.hit(x, y).filter(|p| p.active(&self.cfg));
        match msg {
            WM_MOUSEMOVE => {
                let changed = hit != self.hover;
                self.hover = hit;
                changed
            }
            WM_LBUTTONUP => {
                self.set_recording(hwnd, None);
                if let Some(p) = hit {
                    let ch = settingsview::apply(&mut self.cfg, p);
                    self.apply(hwnd, ch);
                }
                true
            }
            _ => false,
        }
    }

    fn key(&mut self, hwnd: HWND, vk: u32, up: bool) -> bool {
        // PrtScn sends only a key-up; everything else is taken on key-down.
        if self.recording.is_none()
            || (up && vk != crate::hotkey::VK_SNAPSHOT)
            || (!up && vk == crate::hotkey::VK_SNAPSHOT)
        {
            if !up && vk == 0x1B && self.recording.is_none() {
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
            return false;
        }
        let down = |k: i32| unsafe { GetKeyState(k) } < 0;
        let mods = HkMods {
            ctrl: down(0x11),
            alt: down(0x12),
            shift: down(0x10),
            win: down(0x5B) || down(0x5C),
        };
        self.record(hwnd, vk, mods)
    }

    fn recorded_chord(&mut self, hwnd: HWND, vk: u32, bits: u32) -> bool {
        let mods = HkMods { alt: bits & 1 != 0, ctrl: bits & 2 != 0, shift: bits & 4 != 0, win: bits & 8 != 0 };
        self.record(hwnd, vk, mods)
    }

    fn deactivated(&mut self, hwnd: HWND) { self.set_recording(hwnd, None); }

    fn system_changed(&mut self, hwnd: HWND) -> bool {
        self.refresh_appearance(hwnd);
        true
    }

    fn hotkey_refused(&mut self, hwnd: HWND) -> bool {
        if self.recording.is_none() { return false; }
        self.set_recording(hwnd, None);
        self.message = Some((S::HotkeyRefused.get(self.lang).into(), true));
        true
    }

    fn shot_setup(&mut self, args: &[String]) {
        self.tab = arg_value(args, "--tab").and_then(|s| s.parse().ok()).unwrap_or(0).min(settingsview::TABS - 1);
        self.layout = None;
        self.recording = match arg_value(args, "--record").as_deref() {
            Some("text") => Some(KeyField::Text),
            Some("editor") => Some(KeyField::Editor),
            _ => None,
        };
        if let Some(m) = arg_value(args, "--message") {
            self.message = Some((m, true));
        }
    }
}

pub fn run_settings() -> i32 {
    let (cfg, note) = Config::load_or_create(&crate::paths::config_file());
    let theme = crate::sys::theme(&cfg);
    let lang = crate::sys::lang(&cfg);
    let message = note.filter(|n| n.contains("broken")).map(|n| (n, true));
    let autostart = crate::autostart::is_enabled(&crate::autostart::HkcuRun::snip()).unwrap_or(false);
    let languages = language_list(lang, &cfg.ocr_language);
    let p = SettingsPanel {
        cfg,
        theme,
        lang,
        glyphs: GdiGlyphs::new(),
        layout: None,
        tab: 0,
        hover: None,
        recording: None,
        message,
        autostart,
        languages,
    };
    run_window(
        SETTINGS_CLASS,
        &format!("{} - Snip", S::Settings.get(lang)),
        settingsview::WIDTH,
        settingsview::height_dip(),
        false,
        &theme,
        Box::new(p),
    )
}

// ---- Main window ----

struct HomePanel {
    cfg: Config,
    theme: Theme,
    lang: Lang,
    glyphs: GdiGlyphs,
    store: Store,
    thumbs: Vec<Canvas>,
    paths: Vec<PathBuf>,
    texts: Vec<crate::texthistory::TextEntry>,
    tray_running: bool,
    hover: Option<crate::homeview::Part>,
    parts: Vec<(crate::homeview::Part, crate::raster::IBox)>,
    note: Option<String>,
    edit_key: String,
    all_key: String,
}

impl HomePanel {
    fn reload(&mut self) {
        let (cfg, _) = Config::load_or_create(&crate::paths::config_file());
        self.theme = crate::sys::theme(&cfg);
        self.lang = crate::sys::lang(&cfg);
        self.edit_key = cfg.editor_key_text();
        self.all_key = cfg.all_key_text();
        self.cfg = cfg;
        let (h, _) = self.store.load();
        let recent: Vec<_> = h.entries.iter().take(3).collect();
        self.thumbs = recent.iter().filter_map(|e| self.store.thumb(e.id)).collect();
        self.paths = recent.iter().map(|e| PathBuf::from(&e.path)).collect();
        self.texts = crate::texthistory::load(&crate::paths::history_dir()).entries;
        self.tray_running = crate::app::is_running();
    }

    fn data_parts(&self, w: i32, scale: f32, rows: &[crate::homeview::RecentText], thumbs: &[&Canvas]) -> Vec<(crate::homeview::Part, crate::raster::IBox)> {
        crate::homeview::parts(w, scale, &self.data(rows, thumbs), self.lang, &self.glyphs, &self.theme)
    }

    fn data<'a>(&'a self, rows: &'a [crate::homeview::RecentText], thumbs: &'a [&'a Canvas]) -> crate::homeview::Data<'a> {
        crate::homeview::Data {
            snip_on: self.cfg.snip_enabled,
            text_on: self.cfg.text_enabled,
            snip_key: if self.cfg.win_shift_s { "Win+Shift+S" } else { "" },
            edit_key: &self.edit_key,
            all_key: &self.all_key,
            text_key: &self.cfg.text_hotkey,
            tray_running: self.tray_running,
            thumbs,
            texts: rows,
            more_texts: self.texts.len() > rows.len(),
            keep_texts: self.cfg.text_history,
            hover: self.hover,
            note: self.note.as_deref(),
        }
    }

    fn rows(&self) -> Vec<crate::homeview::RecentText> {
        let today = crate::sys::today_text();
        self.texts.iter().take(3).map(|e| crate::homeview::RecentText { preview: crate::textcore::preview(&e.text, 120), when: crate::historyview::when_label(&e.time, &today, self.lang) }).collect()
    }

    /// Make sure the keys' process runs, so a button always does what it says.
    fn ensure_tray(&mut self) -> bool {
        if crate::app::is_running() {
            return true;
        }
        crate::actions::spawn_self("");
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if crate::app::is_running() {
                self.tray_running = true;
                return true;
            }
        }
        false
    }

    /// Out of the way first: the window must not be in the picture.
    fn start(&mut self, hwnd: HWND, what: usize, delay: isize) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
        std::thread::sleep(std::time::Duration::from_millis(120));
        if self.ensure_tray() {
            unsafe {
                if let Ok(tray) = FindWindowW(crate::app::MAIN_CLASS, PCWSTR::null()) {
                    let mut pid = 0;
                    GetWindowThreadProcessId(tray, Some(&mut pid));
                    let _ = AllowSetForegroundWindow(pid);
                }
            }
            crate::actions::tell_tray(crate::app::WM_APP_COMMAND, what, delay);
        }
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    fn click(&mut self, hwnd: HWND, part: crate::homeview::Part) {
        use crate::homeview::{Job, Part as P};
        match part {
            P::Settings => { crate::actions::spawn_self("settings"); }
            P::SeeAll => { crate::actions::spawn_self("history"); }
            P::AllTexts => {
                if let Some(i) = self.pick_text(hwnd) {
                    self.copy_text(hwnd, i);
                }
            }
            P::Start(Job::Snip) => self.start(hwnd, 1, 0),
            P::Start(Job::Edit) => self.start(hwnd, 2, 0),
            P::Start(Job::Text) => self.start(hwnd, 3, 0),
            P::Start(Job::All) => self.start(hwnd, 4, 0),
            P::Delay(s) => self.start(hwnd, 1, s as isize),
            P::StartTray => {
                self.ensure_tray();
                self.tray_running = crate::app::is_running();
                self.fit_height(hwnd);
            }
            P::TurnOn(snip) => {
                if snip { self.cfg.snip_enabled = true } else { self.cfg.text_enabled = true }
                let path = crate::paths::config_file();
                if crate::naming::write_small(&path, self.cfg.to_file_text().as_bytes()).is_ok() {
                    crate::actions::tell_tray(WM_APP_RELOAD, 0, 0);
                }
            }
            P::Thumb(i) => {
                if let Some(p) = self.paths.get(i).filter(|p| p.exists()) {
                    crate::actions::open_path(p);
                }
            }
            P::TextLine(i) => self.copy_text(hwnd, i),
        }
    }

    fn copy_text(&mut self, hwnd: HWND, i: usize) {
        if let Some(e) = self.texts.get(i) {
            let done = crate::textclip::publish(hwnd, &e.text);
            let word = if matches!(done, crate::textcore::TextOutcome::Copied(_)) { S::Copied } else { S::CardUnchanged };
            self.note = Some(word.get(self.lang).to_string());
            show_note_later(hwnd);
        }
    }

    /// Every kept text in a menu; choosing one copies it. The window shows only the newest three.
    fn pick_text(&self, hwnd: HWND) -> Option<usize> {
        let today = crate::sys::today_text();
        let labels: Vec<Vec<u16>> = self.texts.iter().map(|e| wide(&crate::textcore::menu_label(&e.text, &crate::historyview::when_label(&e.time, &today, self.lang)))).collect();
        unsafe {
            let m = CreatePopupMenu().ok()?;
            for (k, label) in labels.iter().enumerate() {
                let _ = AppendMenuW(m, MF_STRING, k + 1, PCWSTR(label.as_ptr()));
            }
            let c = crate::sys::cursor();
            crate::sys::menu_mode(self.theme.mode);
            let r = TrackPopupMenu(m, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, c.x, c.y, Some(0), hwnd, None);
            let _ = DestroyMenu(m);
            (r.0 as usize).checked_sub(1)
        }
    }

    /// The notice about a stopped Snip takes a line; the window fits what it shows.
    fn fit_height(&self, hwnd: HWND) {
        let (w, _) = client(hwnd);
        let sc = scale_of(hwnd);
        let want = (crate::homeview::height_dip(!self.tray_running) * sc).round() as i32;
        let (_, now) = client(hwnd);
        if want != now {
            let r = crate::overlay::window_rect(hwnd);
            unsafe {
                let _ = SetWindowPos(hwnd, None, 0, 0, r.width(), r.height() + want - now, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
        let _ = w;
    }
}

impl Panel for HomePanel {
    fn render(&mut self, w: i32, h: i32, scale: f32) -> Canvas {
        let rows = self.rows();
        let thumbs: Vec<&Canvas> = self.thumbs.iter().collect();
        self.parts = self.data_parts(w, scale, &rows, &thumbs);
        let d = self.data(&rows, &thumbs);
        crate::homeview::render(w, h, scale, &d, self.lang, &self.theme, &self.glyphs)
    }

    fn mouse(&mut self, hwnd: HWND, msg: u32, x: i32, y: i32) -> bool {
        let hit = crate::homeview::hit(&self.parts, x, y);
        match msg {
            WM_MOUSEMOVE => {
                let changed = hit != self.hover;
                self.hover = hit;
                changed
            }
            WM_LBUTTONUP => {
                if let Some(p) = hit {
                    self.click(hwnd, p);
                }
                true
            }
            _ => false,
        }
    }

    fn key(&mut self, hwnd: HWND, vk: u32, up: bool) -> bool {
        if !up && vk == 0x1B {
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        false
    }

    /// Coming back to the window shows what happened meanwhile.
    fn activated(&mut self) -> bool {
        self.reload();
        true
    }

    fn system_changed(&mut self, hwnd: HWND) -> bool {
        self.reload();
        crate::sys::title_bar_mode(hwnd, &self.theme);
        true
    }

    fn clear_note(&mut self) -> bool {
        self.note.take().is_some()
    }

    fn started(&mut self, hwnd: HWND) {
        unsafe {
            SetTimer(Some(hwnd), TIMER_POLL, 2000, None);
        }
    }

    fn poll(&mut self, hwnd: HWND) -> bool {
        let running = crate::app::is_running();
        if running == self.tray_running {
            return false;
        }
        self.tray_running = running;
        self.fit_height(hwnd);
        true
    }

    fn shot_setup(&mut self, args: &[String]) {
        use crate::homeview::{Job, Part as P};
        self.hover = match arg_value(args, "--hover").as_deref() {
            Some("snip") => Some(P::Start(Job::Snip)),
            Some("text") => Some(P::Start(Job::Text)),
            _ => None,
        };
        if args.iter().any(|a| a == "--tray-running") {
            self.tray_running = true;
        }
    }
}

pub fn run_home() -> i32 {
    let (cfg, _) = Config::load_or_create(&crate::paths::config_file());
    let theme = crate::sys::theme(&cfg);
    let lang = crate::sys::lang(&cfg);
    let mut p = HomePanel {
        cfg,
        theme,
        lang,
        glyphs: GdiGlyphs::new(),
        store: Store::new(crate::paths::history_dir()),
        thumbs: Vec::new(),
        paths: Vec::new(),
        texts: Vec::new(),
        tray_running: false,
        hover: None,
        parts: Vec::new(),
        note: None,
        edit_key: String::new(),
        all_key: String::new(),
    };
    p.reload();
    if std::env::args().any(|a| a == "--tray-running") {
        p.tray_running = true;
    }
    let notice = !p.tray_running;
    run_window(
        HOME_CLASS,
        "Snip",
        crate::theme::HOME_WIDTH,
        crate::homeview::height_dip(notice),
        false,
        &theme,
        Box::new(p),
    )
}
