//! What snip asks Windows about the session: dark mode, UI language, monitors with their DPI,
//! the local time. Each answer feeds a core module (`theme`, `i18n`, `dpi`).

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Globalization::GetUserDefaultUILanguage;
use windows::Win32::Graphics::Dwm::{DWMWINDOWATTRIBUTE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
use windows::core::{BOOL, w};

use crate::config::Config;
use crate::dpi::{Monitor, Monitors};
use crate::geometry::{Point, Rect};
use crate::i18n::Lang;
use crate::theme::{Mode, Theme};

/// Windows apps in dark mode (Settings > Personalisation > Colours: "Choose your app mode").
pub fn system_dark() -> bool {
    let mut v = 1u32;
    let mut n = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut _),
            Some(&mut n),
        )
    };
    r.is_ok() && v == 0
}

/// The app mode Windows is set to.
pub fn system_mode() -> Mode {
    if system_dark() { Mode::Dark } else { Mode::Light }
}

/// Whether Windows is configured to open Snipping Tool with the unmodified Print Screen key.
pub fn print_screen_snipping_enabled() -> Option<bool> {
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Control Panel\\Keyboard"),
            w!("PrintScreenKeyForSnippingEnabled"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    result.is_ok().then_some(value != 0)
}

/// Primary language id of the Windows display language (0x1F = Turkish).
pub fn ui_lang_primary() -> u16 {
    unsafe { GetUserDefaultUILanguage() & 0x3FF }
}

pub fn theme(cfg: &Config) -> Theme {
    Theme::new(cfg.look, cfg.theme.resolve(system_dark()))
}

pub fn lang(cfg: &Config) -> Lang {
    cfg.language.resolve(ui_lang_primary())
}

/// Dark title bar for our normal windows when the theme is dark (Windows 10 2004+ attribute 20).
pub fn title_bar_mode(hwnd: HWND, theme: &Theme) {
    let dark: BOOL = (theme.mode == Mode::Dark).into();
    let colorref = |c: crate::raster::Rgba| c.r as u32 | (c.g as u32) << 8 | (c.b as u32) << 16;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWINDOWATTRIBUTE(20),
            &dark as *const _ as *const _,
            std::mem::size_of::<BOOL>() as u32,
        );
        // Windows 11: the title bar takes the window's ground, so bar and content read as one surface.
        // Windows 10 refuses these two and keeps its own bar.
        for (attribute, value) in [(35, colorref(theme.window)), (34, colorref(theme.border))] {
            let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(attribute), &value as *const _ as *const _, 4);
        }
    }
}

/// Popup menus follow the theme. Windows has no documented call for this in a Win32 program;
/// these two uxtheme exports (ordinals 135 and 136, Windows 10 1903 and later) are what programs
/// use. Where they are missing the menu stays light.
pub fn menu_mode(mode: Mode) {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
    unsafe {
        let Ok(lib) = LoadLibraryExW(w!("uxtheme.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) else { return };
        let ordinal = |n: usize| windows::core::PCSTR(n as *const u8);
        if let Some(set) = GetProcAddress(lib, ordinal(135)) {
            let set: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(set);
            // 2 = always dark, 3 = always light.
            set(if mode == Mode::Dark { 2 } else { 3 });
        }
        if let Some(flush) = GetProcAddress(lib, ordinal(136)) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(flush);
            flush();
        }
    }
}

/// Local date, "2026-11-07": history captions say today and yesterday.
pub fn today_text() -> String {
    let t = unsafe { GetLocalTime() };
    format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
}

pub fn monitors() -> Monitors {
    unsafe extern "system" fn cb(hmon: HMONITOR, _dc: HDC, _r: *mut RECT, data: LPARAM) -> BOOL {
        unsafe {
            let v = &mut *(data.0 as *mut Vec<Monitor>);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(hmon, &mut mi).as_bool() {
                let r = mi.rcMonitor;
                let wk = mi.rcWork;
                let (mut dx, mut dy) = (96u32, 96u32);
                let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
                v.push(Monitor {
                    rect: Rect::new(r.left, r.top, r.right, r.bottom),
                    work: Rect::new(wk.left, wk.top, wk.right, wk.bottom),
                    dpi: dx,
                });
            }
            BOOL(1)
        }
    }
    let mut v: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    Monitors::new(v)
}

pub fn cursor() -> Point {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    Point::new(p.x, p.y)
}

/// "2026-11-04 19:50:12" (local time).
pub fn local_time_text() -> String {
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

/// The application icon's resource id (MAKEINTRESOURCE(1)).
pub fn app_icon() -> windows::core::PCWSTR {
    windows::core::PCWSTR(std::ptr::without_provenance(1))
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Milliseconds since boot (for the countdown).
pub fn tick_ms() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

// ---- Snip runs elevated (scheduled task \Laureth\Snip, Highest) ----

/// This process runs with the full (elevated) administrator token.
pub fn is_elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut n = 0u32;
        let ok = GetTokenInformation(token, TokenElevation, Some(&mut e as *mut _ as *mut _), std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut n).is_ok();
        let _ = CloseHandle(token);
        ok && e.TokenIsElevated != 0
    }
}

/// The command line that makes Explorer open `path`, or show it selected.
pub fn explorer_args(path: &str, select: bool) -> String {
    let p = path.replace('"', "");
    if select { format!("/select,\"{p}\"") } else { format!("\"{p}\"") }
}

/// Start `explorer.exe <args>` with the desktop shell's own (normal-level) token, so a file or a
/// folder opened from the elevated Snip does not open elevated. False: no shell, or no token.
pub fn explorer_as_shell_user(args: &str) -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{DuplicateTokenEx, SecurityImpersonation, TOKEN_ALL_ACCESS, TOKEN_DUPLICATE, TokenPrimary};
    use windows::Win32::System::Threading::{
        CREATE_NO_WINDOW, CreateProcessWithTokenW, LOGON_WITH_PROFILE, OpenProcess, OpenProcessToken, PROCESS_INFORMATION,
        PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
    use windows::core::{PCWSTR, PWSTR};
    unsafe {
        let shell = GetShellWindow();
        if shell.is_invalid() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(shell, Some(&mut pid));
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut token = HANDLE::default();
        let opened = OpenProcessToken(process, TOKEN_DUPLICATE, &mut token);
        let _ = CloseHandle(process);
        if opened.is_err() {
            return false;
        }
        let mut primary = HANDLE::default();
        let dup = DuplicateTokenEx(token, TOKEN_ALL_ACCESS, None, SecurityImpersonation, TokenPrimary, &mut primary);
        let _ = CloseHandle(token);
        if dup.is_err() {
            return false;
        }
        let exe = explorer_exe();
        let mut cmd = wide(&format!("\"{exe}\" {args}"));
        let si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let mut pi = PROCESS_INFORMATION::default();
        let app = wide(&exe);
        let r = CreateProcessWithTokenW(primary, LOGON_WITH_PROFILE, PCWSTR(app.as_ptr()), Some(PWSTR(cmd.as_mut_ptr())), CREATE_NO_WINDOW, None, PCWSTR::null(), &si, &mut pi);
        let _ = CloseHandle(primary);
        if r.is_ok() {
            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);
        }
        r.is_ok()
    }
}

/// The full path of Explorer in the Windows folder, so no copy beside snip is started instead. The
/// folder comes from the system, not from an environment variable a user-level setting could change.
pub fn explorer_exe() -> String {
    let mut buf = [0u16; 260];
    let n = unsafe { windows::Win32::System::SystemInformation::GetSystemWindowsDirectoryW(Some(&mut buf)) } as usize;
    let root = if n > 0 && n < buf.len() { String::from_utf16_lossy(&buf[..n]) } else { "C:\\Windows".into() };
    format!("{}\\explorer.exe", root.trim_end_matches('\\'))
}

/// Let the tray window hear a few messages from normal-level processes (`snip quit`, `snip text`,
/// the Settings window, the Explorer restart notice). Windows blocks them otherwise (UIPI), so
/// they would fail without a word. Only these messages; only while elevated.
pub fn allow_normal_level_messages(hwnd: HWND, messages: &[u32]) {
    use windows::Win32::UI::WindowsAndMessaging::{ChangeWindowMessageFilterEx, MSGFLT_ALLOW};
    if !is_elevated() {
        return;
    }
    for &m in messages {
        if unsafe { ChangeWindowMessageFilterEx(hwnd, m, MSGFLT_ALLOW, None) }.is_err() {
            crate::logf!("message {m:#x} could not be allowed from normal level");
        }
    }
}

#[cfg(test)]
mod elevation_tests {
    use super::explorer_args;

    #[test]
    fn explorer_arguments_are_quoted_and_cannot_break_out() {
        assert_eq!(explorer_args(r"C:\a b\x.png", false), "\"C:\\a b\\x.png\"");
        assert_eq!(explorer_args(r"C:\a\x.png", true), "/select,\"C:\\a\\x.png\"");
        assert_eq!(explorer_args("C:\\a\" /e,\"b", false), "\"C:\\a /e,b\"");
    }
}
