//! The tray icon (Shell_NotifyIconW directly) and its menu. Two icons drawn at start: a blue frame
//! when the hotkey is armed, an orange frame with a cross when it is not.

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Shell::{NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

pub const WM_APP_TRAY: u32 = WM_APP + 20;
pub const CMD_SNIP: u32 = 1;
pub const CMD_FOLDER: u32 = 2;
pub const CMD_LOG: u32 = 3;
pub const CMD_REARM: u32 = 4;
pub const CMD_EDIT: u32 = 5;
pub const CMD_SNIP3: u32 = 6;
pub const CMD_SNIP5: u32 = 7;
pub const CMD_HISTORY: u32 = 8;
pub const CMD_QUIT: u32 = 9;
pub const CMD_SETTINGS: u32 = 10;
pub const CMD_TEXT: u32 = 11;
pub const CMD_HOME: u32 = 12;
pub const CMD_ALL: u32 = 13;
pub const CMD_RECORD: u32 = 14;

/// What the menu needs to know.
pub struct MenuInfo<'a> {
    pub status: &'a str,
    pub lang: crate::i18n::Lang,
    pub snip_on: bool,
    pub text_on: bool,
    pub record_on: bool,
    pub record_busy: bool,
    /// Win+Shift+S is taken, so the key can be shown.
    pub snip_key: bool,
    pub editor_key: &'a str,
    /// The all-screens key, as people say it (empty: none).
    pub all_key: &'a str,
    pub text_key: &'a str,
    pub mode: crate::theme::Mode,
}

/// A 16x16 (system small icon size) icon: a frame in `rgb`, with a cross when `cross`.
pub fn make_icon(rgb: (u8, u8, u8), cross: bool) -> HICON {
    unsafe {
        // Artwork embedded in the exe (build.rs): id 1 = armed, id 2 = not armed.
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
        let size = GetSystemMetrics(SM_CXSMICON).max(16);
        let id = windows::core::PCWSTR(std::ptr::without_provenance(if cross { 2 } else { 1 }));
        if let Ok(image) = LoadImageW(Some(instance.into()), id, IMAGE_ICON, size, size, IMAGE_FLAGS(0)) {
            return HICON(image.0);
        }
        let s = GetSystemMetrics(SM_CXSMICON).max(crate::theme::TRAY_ICON_MIN_SIZE);
        let mut px = vec![0u32; (s * s) as usize];
        let c = 0xFF00_0000 | (rgb.0 as u32) << 16 | (rgb.1 as u32) << 8 | rgb.2 as u32;
        let t = (s / 8).max(crate::theme::TRAY_ICON_MIN_STROKE);
        for y in 0..s {
            for x in 0..s {
                let edge = x < t || y < t || x >= s - t || y >= s - t;
                let corner_gap = (x < s / 3 || x >= s - s / 3) && (y < s / 3 || y >= s - s / 3);
                // Crop marks: the frame's edges, only near the four corners.
                let frame = edge && corner_gap;
                let diag = cross && ((x - y).abs() < t || (x + y - (s - 1)).abs() < t) && x > t && x < s - t;
                let center = !cross && x > s / 3 && x < s - 1 - s / 3 && y > s / 3 && y < s - 1 - s / 3;
                if frame || diag || center {
                    px[(y * s + x) as usize] = c;
                }
            }
        }
        let bmi = BITMAPV5HEADER {
            bV5Size: std::mem::size_of::<BITMAPV5HEADER>() as u32,
            bV5Width: s,
            bV5Height: -s,
            bV5Planes: 1,
            bV5BitCount: 32,
            bV5Compression: BI_BITFIELDS,
            bV5RedMask: 0x00FF_0000,
            bV5GreenMask: 0x0000_FF00,
            bV5BlueMask: 0x0000_00FF,
            bV5AlphaMask: 0xFF00_0000,
            ..Default::default()
        };
        let dc = GetDC(None);
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(Some(dc), &bmi as *const _ as *const BITMAPINFO, DIB_RGB_COLORS, &mut bits, None, 0);
        ReleaseDC(None, dc);
        let Ok(color) = color else { return LoadIconW(None, IDI_APPLICATION).unwrap_or_default() };
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u32, px.len());
        let mask = CreateBitmap(s, s, 1, 1, None);
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

fn wide_into(dst: &mut [u16], s: &str) {
    let v: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..v.len()].copy_from_slice(&v);
    dst[v.len()] = 0;
}

fn data(hwnd: HWND, icon: HICON, tip: &str) -> NOTIFYICONDATAW {
    let mut d = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_APP_TRAY,
        hIcon: icon,
        ..Default::default()
    };
    wide_into(&mut d.szTip, tip);
    d
}

pub fn add(hwnd: HWND, icon: HICON, tip: &str) -> bool {
    unsafe { Shell_NotifyIconW(NIM_ADD, &data(hwnd, icon, tip)).as_bool() }
}

pub fn update(hwnd: HWND, icon: HICON, tip: &str) -> bool {
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &data(hwnd, icon, tip)).as_bool() }
}

/// A warning next to the tray icon. Used only when a capture could not be written.
pub fn warn(hwnd: HWND, icon: HICON, tip: &str, title: &str, text: &str) -> bool {
    let mut d = data(hwnd, icon, tip);
    d.uFlags |= NIF_INFO;
    d.dwInfoFlags = NIIF_WARNING;
    wide_into(&mut d.szInfoTitle, title);
    wide_into(&mut d.szInfo, text);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &d).as_bool() }
}

pub fn remove(hwnd: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data(hwnd, HICON::default(), ""));
    }
}

fn wz(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// One menu line: (command, flags, text). A command of 0 with `MF_SEPARATOR` is a line.
pub fn menu_items(i: &MenuInfo) -> Vec<(u32, MENU_ITEM_FLAGS, String)> {
    use crate::i18n::S;
    let t = |x: S| x.get(i.lang).to_string();
    let with_key = |label: String, key: &str| if key.is_empty() { label } else { format!("{label}\t{key}") };
    let sep = || (0, MF_SEPARATOR, String::new());
    let mut v = vec![(0, MF_STRING | MF_GRAYED, i.status.to_string()), sep()];
    if i.snip_on {
        v.push((CMD_SNIP, MF_STRING, with_key(t(S::SnipNow), if i.snip_key { "Win+Shift+S" } else { "" })));
        v.push((CMD_ALL, MF_STRING, with_key(t(S::AllScreens), i.all_key)));
        v.push((CMD_EDIT, MF_STRING, with_key(t(S::OpenEditor), i.editor_key)));
        v.push((CMD_SNIP3, MF_STRING, t(S::SnipIn3)));
        v.push((CMD_SNIP5, MF_STRING, t(S::SnipIn5)));
        v.push(sep());
    }
    if i.record_on || i.record_busy {
        let tr = i.lang == crate::i18n::Lang::Tr;
        let label = match (tr, i.record_busy) { (true, false) => "Kayıt başlat", (true, true) => "Kaydı durdur", (false, false) => "Start recording", (false, true) => "Stop recording" };
        v.push((CMD_RECORD, MF_STRING, label.into())); v.push(sep());
    }
    if i.text_on {
        v.push((CMD_TEXT, MF_STRING, with_key(t(S::FnText), i.text_key)));
        v.push(sep());
    }
    v.push((CMD_HOME, MF_STRING | MF_DEFAULT, t(S::OpenSnip)));
    if i.snip_on {
        v.push((CMD_HISTORY, MF_STRING, t(S::History)));
    }
    v.push((CMD_SETTINGS, MF_STRING, t(S::Settings)));
    if i.snip_on {
        v.push((CMD_FOLDER, MF_STRING, t(S::OpenFolder)));
    }
    v
}

/// Show the menu at the cursor and return the chosen command (0 for none).
pub fn menu(hwnd: HWND, info: &MenuInfo) -> u32 {
    use crate::i18n::S;
    let t = |x: S| x.get(info.lang).to_string();
    unsafe {
        let Ok(m) = CreatePopupMenu() else { return 0 };
        let items = menu_items(info);
        let texts: Vec<Vec<u16>> = items.iter().map(|i| wz(&i.2)).collect();
        for (i, (id, flags, _)) in items.iter().enumerate() {
            if *flags == MF_SEPARATOR {
                let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            } else {
                let _ = AppendMenuW(m, *flags, *id as usize, PCWSTR(texts[i].as_ptr()));
                if *id == CMD_HOME {
                    let _ = SetMenuDefaultItem(m, CMD_HOME, 0);
                }
            }
        }
        // The rare ones stay out of the way, one step down.
        let sub = CreatePopupMenu().unwrap_or_default();
        let (log, rearm, trouble, quit) = (wz(&t(S::OpenLog)), wz(&t(S::Rearm)), wz(&t(S::Troubleshooting)), wz(&t(S::Quit)));
        let _ = AppendMenuW(sub, MF_STRING, CMD_LOG as usize, PCWSTR(log.as_ptr()));
        let _ = AppendMenuW(sub, MF_STRING, CMD_REARM as usize, PCWSTR(rearm.as_ptr()));
        let _ = AppendMenuW(m, MF_POPUP, sub.0 as usize, PCWSTR(trouble.as_ptr()));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(m, MF_STRING, CMD_QUIT as usize, PCWSTR(quit.as_ptr()));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        crate::sys::menu_mode(info.mode);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, Some(0), hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(m);
        cmd.0 as u32
    }
}
