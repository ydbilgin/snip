//! Things the Windows side does for the editor, the pin, history and settings windows: dialogs,
//! starting our own subcommands, opening files, the history record of a capture.
//! Nothing here touches the network; "open" only ever gets a local file or folder path.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Threading::{CREATE_NO_WINDOW, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW};
use windows::Win32::UI::Controls::Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FOS_PICKFOLDERS, FileOpenDialog, FileSaveDialog, IFileOpenDialog, IFileSaveDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, SW_SHOWNORMAL};
use windows::core::{PCWSTR, PWSTR, w};

use crate::history::{Entry, Store};
use crate::raster::{Canvas, Rgba};
use crate::sys::wide;

pub fn exe_path() -> String {
    let mut buf = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(None, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// Start `snip <args>` (our own executable) without waiting.
pub fn spawn_self(args: &str) -> bool {
    let cmd = format!("\"{}\" {args}", exe_path());
    let mut wide_cmd: Vec<u16> = cmd.encode_utf16().chain(Some(0)).collect();
    let si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut pi = PROCESS_INFORMATION::default();
    let r = unsafe { CreateProcessW(PCWSTR::null(), Some(PWSTR(wide_cmd.as_mut_ptr())), None, None, false, CREATE_NO_WINDOW, None, PCWSTR::null(), &si, &mut pi) };
    if r.is_ok() {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(pi.hThread);
            let _ = windows::Win32::Foundation::CloseHandle(pi.hProcess);
        }
    }
    r.is_ok()
}

/// Quote one argument for `spawn_self` (paths with spaces; a trailing backslash is doubled).
pub fn quote(s: &str) -> String {
    let t = if s.ends_with('\\') { format!("{s}\\") } else { s.to_string() };
    format!("\"{}\"", t.replace('"', ""))
}

pub fn open_path(p: &Path) {
    // Snip runs elevated: what it opens must not. Explorer, started with the shell's token, opens it.
    if crate::sys::is_elevated() {
        if crate::sys::explorer_as_shell_user(&crate::sys::explorer_args(&p.to_string_lossy(), false)) {
            return;
        }
        // No fallback at the elevated level: `p` can come from config.toml, which a normal-level
        // process can edit, so opening it here would run a program with administrator rights.
        crate::logf!("open: no normal-level shell found; not opened");
        return;
    }
    let wp = wide(&p.to_string_lossy());
    unsafe {
        ShellExecuteW(None, w!("open"), PCWSTR(wp.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Open Explorer with `p` selected.
pub fn show_in_folder(p: &Path) {
    if crate::sys::is_elevated() {
        if crate::sys::explorer_as_shell_user(&crate::sys::explorer_args(&p.to_string_lossy(), true)) {
            return;
        }
        crate::logf!("show in folder: no normal-level shell found; not opened");
        return;
    }
    let args = wide(&format!("/select,\"{}\"", p.to_string_lossy()));
    unsafe {
        let explorer = wide(&crate::sys::explorer_exe());
        ShellExecuteW(None, w!("open"), PCWSTR(explorer.as_ptr()), PCWSTR(args.as_ptr()), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

fn item_path(item: &IShellItem) -> Option<PathBuf> {
    unsafe {
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// The system "Save as" dialog for a PNG. Returns the chosen path.
pub fn save_as_dialog(owner: Option<HWND>, dir: &Path, name: &str) -> Option<PathBuf> {
    unsafe {
        let d: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let spec = [COMDLG_FILTERSPEC { pszName: w!("PNG"), pszSpec: w!("*.png") }];
        let _ = d.SetFileTypes(&spec);
        let _ = d.SetDefaultExtension(w!("png"));
        let n = wide(name);
        let _ = d.SetFileName(PCWSTR(n.as_ptr()));
        let opts = d.GetOptions().unwrap_or_default();
        let _ = d.SetOptions(opts | FOS_FORCEFILESYSTEM | FOS_OVERWRITEPROMPT);
        let dw = wide(&dir.to_string_lossy());
        if let Ok(folder) = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(dw.as_ptr()), None) {
            let _ = d.SetFolder(&folder);
        }
        d.Show(owner).ok()?;
        item_path(&d.GetResult().ok()?)
    }
}

/// The system folder picker.
pub fn pick_folder(owner: Option<HWND>, start: &Path) -> Option<PathBuf> {
    unsafe {
        let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = d.GetOptions().unwrap_or_default();
        let _ = d.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);
        let sw = wide(&start.to_string_lossy());
        if let Ok(folder) = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(sw.as_ptr()), None) {
            let _ = d.SetFolder(&folder);
        }
        d.Show(owner).ok()?;
        item_path(&d.GetResult().ok()?)
    }
}

/// The system colour dialog, starting at `current`.
pub fn choose_color(owner: HWND, current: Rgba) -> Option<Rgba> {
    let mut custom = [COLORREF(0x00FF_FFFF); 16];
    for (i, c) in crate::theme::PALETTE.iter().enumerate() {
        custom[i] = COLORREF(c.r as u32 | (c.g as u32) << 8 | (c.b as u32) << 16);
    }
    let mut cc = CHOOSECOLORW {
        lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: owner,
        rgbResult: COLORREF(current.r as u32 | (current.g as u32) << 8 | (current.b as u32) << 16),
        lpCustColors: custom.as_mut_ptr(),
        Flags: CC_RGBINIT | CC_FULLOPEN,
        ..Default::default()
    };
    if unsafe { ChooseColorW(&mut cc) }.as_bool() {
        let v = cc.rgbResult.0;
        Some(Rgba::rgb(v as u8, (v >> 8) as u8, (v >> 16) as u8))
    } else {
        None
    }
}

/// Post a message to the running tray process, if any.
pub fn tell_tray(msg: u32, wp: usize, lp: isize) -> bool {
    unsafe {
        match FindWindowW(crate::app::MAIN_CLASS, PCWSTR::null()) {
            Ok(h) => PostMessageW(Some(h), msg, WPARAM(wp), LPARAM(lp)).is_ok(),
            Err(_) => false,
        }
    }
}

/// Add a capture to the history. `saved`: the PNG written to the screenshots folder, if any;
/// otherwise the history keeps its own copy (`png`). Returns the path the entry points at.
pub fn record_history(source: &str, w: u32, h: u32, px: &[u8], saved: Option<&Path>, png: Option<&[u8]>) -> Result<PathBuf, String> {
    let store = Store::new(crate::paths::history_dir());
    let img = Canvas::from_bgra(w, h, px.to_vec());
    let thumb = crate::thumb::make(&img);
    let e = Entry { id: 0, time: crate::sys::local_time_text(), path: saved.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(), owned: false, w, h, source: source.into() };
    let own = if saved.is_some() { None } else { png };
    match store.record(e, &thumb, own) {
        Ok(id) => {
            Ok(saved.map(PathBuf::from).unwrap_or_else(|| store.dir.join(crate::history::owned_name(id))))
        }
        Err(err) => {
            crate::logf!("history record FAILED: {err}");
            Err(format!("history: {err}"))
        }
    }
}

/// Decode a PNG file to a canvas (COM must be initialised on this thread).
pub fn load_png(p: &Path) -> Option<Canvas> {
    let bytes = std::fs::read(p).ok()?;
    let (w, h, px) = crate::png::decode_png(&bytes).ok()?;
    Some(Canvas::from_bgra(w, h, px))
}
