//! `snip --pin <png> <x> <y> [--temp]`: one pinned capture, always on top, in its own small
//! process (GDI only, no graphics device). The window is the image with a 1-DIP border, placed
//! where the capture was taken. Drag to move, wheel to zoom, Esc or double-click to close,
//! right-click for Copy, Save as, Opacity, Actual size, Close.

use std::cell::RefCell;
use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::config::Config;
use crate::i18n::{Lang, S};
use crate::pinview::Pin;
use crate::raster::Canvas;
use crate::sys::wide;
use crate::theme::Theme;

pub const CLASS: PCWSTR = w!("Laureth.snip.pin");

struct PinApp {
    hwnd: HWND,
    img: Canvas,
    pin: Pin,
    theme: Theme,
    lang: Lang,
    path: PathBuf,
    temp: bool,
    cfg: Config,
    scaled: Option<(usize, Canvas)>,
}

thread_local! {
    static PIN: RefCell<Option<PinApp>> = const { RefCell::new(None) };
}

fn with_pin<R>(f: impl FnOnce(&mut PinApp) -> R) -> Option<R> {
    PIN.with(|p| p.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

impl PinApp {
    fn present(&mut self) {
        let (w, h) = self.pin.window_size();
        let (iw, ih) = self.pin.image_size();
        if self
            .scaled
            .as_ref()
            .is_none_or(|(z, _)| *z != self.pin.zoom)
        {
            let s = if (iw as u32, ih as u32) == (self.img.w, self.img.h) {
                self.img.clone()
            } else {
                self.img.resample_box(iw as u32, ih as u32)
            };
            self.scaled = Some((self.pin.zoom, s));
        }
        let mut cv = Canvas::new(w as u32, h as u32, self.theme.border);
        if let Some((_, s)) = &self.scaled {
            cv.draw(s, self.pin.border, self.pin.border);
        }
        crate::gdi::update_layered(self.hwnd, &cv, self.pin.x, self.pin.y, self.pin.alpha());
    }

    fn copy(&self) {
        if let Err(e) = crate::app::copy_picture(self.hwnd, &self.img, self.cfg.clipboard_png)
        {
            crate::logf!("pin copy FAILED {:#010x} {}", e.code().0, e.message());
        }
    }

    fn save_as(&self) {
        let dir = crate::paths::output_dir(&self.cfg.folder);
        let name = format!(
            "{}.png",
            crate::naming::base_name(&crate::app::local_stamp())
        );
        if let Some(p) = crate::actions::save_as_dialog(Some(self.hwnd), &dir, &name) {
            match crate::png::encode_png(self.img.w, self.img.h, &self.img.px) {
                Ok(bytes) => {
                    if let Err(e) = crate::naming::write_atomic(&p, &bytes) {
                        crate::logf!("pin save {} FAILED: {e}", p.display());
                    }
                }
                Err(e) => crate::logf!("pin encode FAILED {:#010x}", e.code().0),
            }
        }
    }

    fn menu(&mut self, x: i32, y: i32) {
        const COPY: usize = 1;
        const SAVE: usize = 2;
        const OPACITY_BASE: usize = 10;
        const RESET: usize = 4;
        const CLOSE: usize = 5;
        let t = |s: S| s.get(self.lang).to_string();
        let items = [
            (COPY, format!("{}\tCtrl+C", t(S::Copy))),
            (SAVE, format!("{}\tCtrl+Shift+S", t(S::SaveAs))),
            (RESET, format!("{}\t0", t(S::ZoomReset))),
            (CLOSE, format!("{}\tEsc", t(S::Close))),
        ];
        crate::sys::menu_mode(crate::sys::system_mode());
        let cmd = unsafe {
            let Ok(m) = CreatePopupMenu() else { return };
            let texts: Vec<Vec<u16>> = items.iter().map(|(_, s)| wide(s)).collect();
            for (i, (id, _)) in items.iter().enumerate() {
                let _ = AppendMenuW(m, MF_STRING, *id, PCWSTR(texts[i].as_ptr()));
                if *id == SAVE
                    && let Ok(sub) = CreatePopupMenu()
                {
                    for (index, percent) in crate::theme::PIN_OPACITY_PERCENT.iter().enumerate() {
                        let label = wide(&format!("{percent} %"));
                        let flags = if index == self.pin.opacity {
                            MF_STRING | MF_CHECKED
                        } else {
                            MF_STRING
                        };
                        let _ =
                            AppendMenuW(sub, flags, OPACITY_BASE + index, PCWSTR(label.as_ptr()));
                    }
                    let label = wide(&t(S::Opacity));
                    if AppendMenuW(m, MF_POPUP, sub.0 as usize, PCWSTR(label.as_ptr())).is_err() {
                        let _ = DestroyMenu(sub);
                    }
                }
            }
            let c = TrackPopupMenu(
                m,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                x,
                y,
                Some(0),
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(m);
            c.0 as usize
        };
        match cmd {
            COPY => self.copy(),
            SAVE => self.save_as(),
            id if (OPACITY_BASE..OPACITY_BASE + crate::pinview::OPACITY.len()).contains(&id) => {
                self.pin.opacity = id - OPACITY_BASE;
                self.present();
            }
            RESET => {
                self.pin.reset_zoom();
                self.present();
            }
            CLOSE => unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            _ => {}
        }
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
        // The whole window drags like a title bar.
        WM_NCHITTEST => LRESULT(HTCAPTION as isize),
        WM_MOVE | WM_EXITSIZEMOVE => {
            let mut r = RECT::default();
            unsafe {
                let _ = GetWindowRect(hwnd, &mut r);
            }
            with_pin(|p| {
                p.pin.x = r.left;
                p.pin.y = r.top;
            });
            LRESULT(0)
        }
        WM_DPICHANGED => {
            with_pin(|p| {
                let border = crate::dpi::px(crate::theme::BORDER_WIDTH, (wp.0 & 0xFFFF) as u32).max(1);
                p.pin.x += p.pin.border - border;
                p.pin.y += p.pin.border - border;
                p.pin.border = border;
                p.present();
            });
            LRESULT(0)
        }
        WM_NCLBUTTONDBLCLK => {
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
            let (x, y) = xy(lp);
            with_pin(|p| {
                if p.pin.zoom_at(delta.signum(), x, y) {
                    p.present();
                }
            });
            LRESULT(0)
        }
        WM_NCRBUTTONUP | WM_CONTEXTMENU => {
            let mut pt = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut pt);
            }
            with_pin(|p| p.menu(pt.x, pt.y));
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
            let shift = unsafe { GetKeyState(0x10) } < 0;
            match wp.0 as u32 {
                0x1B => unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                },
                0x43 if ctrl => {
                    with_pin(|p| p.copy());
                }
                0x53 if ctrl && shift => {
                    with_pin(|p| p.save_as());
                }
                0x30 | 0x60 => {
                    with_pin(|p| {
                        p.pin.reset_zoom();
                        p.present();
                    });
                }
                0xBB | 0x6B | 0xBD | 0x6D => {
                    let dir = if matches!(wp.0, 0xBB | 0x6B) { 1 } else { -1 };
                    with_pin(|p| {
                        let (w, h) = p.pin.window_size();
                        if p.pin.zoom_at(dir, p.pin.x + w / 2, p.pin.y + h / 2) {
                            p.present();
                        }
                    });
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            crate::app::wait_copy_png();
            unsafe { let _ = DestroyWindow(hwnd); }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Run one pin until it is closed. Exit 0, or 2 when the image cannot be read.
pub fn run(path: PathBuf, x: i32, y: i32, temp: bool) -> i32 {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
    }
    let (cfg, _) = Config::load_or_create(&crate::paths::config_file());
    let Some(img) = crate::actions::load_png(&path) else {
        crate::logf!("pin: cannot read {}", path.display());
        return 2;
    };
    unsafe {
        let hinst = GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            hIcon: LoadIconW(Some(hinst), crate::sys::app_icon()).unwrap_or_default(),
            style: CS_DBLCLKS,
            lpfnWndProc: Some(proc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_SIZEALL).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            CLASS,
            w!("snip pin"),
            WS_POPUP,
            x,
            y,
            1,
            1,
            None,
            None,
            Some(hinst),
            None,
        ) else {
            return 1;
        };
        let dpi = GetDpiForWindow(hwnd).max(96);
        let border = crate::dpi::px(crate::theme::BORDER_WIDTH, dpi).max(1);
        let mut app = PinApp {
            hwnd,
            pin: Pin::new(img.w, img.h, x, y, border),
            img,
            theme: crate::sys::theme(&cfg),
            lang: crate::sys::lang(&cfg),
            path: path.clone(),
            temp,
            cfg,
            scaled: None,
        };
        app.present();
        PIN.with(|p| *p.borrow_mut() = Some(app));
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if let Some(a) = PIN.with(|p| p.borrow_mut().take())
            && a.temp
        {
            let _ = std::fs::remove_file(&a.path);
        }
        0
    }
}
