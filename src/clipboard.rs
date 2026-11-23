//! Putting the snip on the clipboard: CF_DIBV5 and CF_DIB, both rendered now (no delayed rendering,
//! so the image survives snip quitting).

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP};
use windows::Win32::System::StationsAndDesktops::{GetProcessWindowStation, GetUserObjectInformationW, UOI_NAME};
use windows::core::Result;

fn global_from(bytes: &[u8]) -> Result<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return Err(windows::core::Error::from_thread());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(h);
        Ok(h)
    }
}

/// Open the clipboard, retrying for up to about 1 s while another program holds it.
pub fn open(owner: HWND) -> Result<()> {
    let mut last = None;
    for _ in 0..50 {
        match unsafe { OpenClipboard(Some(owner)) } {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Err(last.unwrap())
}

/// Open with a short wait: the text path has its own retries and must answer quickly.
pub fn open_quick(owner: HWND) -> Result<()> {
    let mut last = None;
    for _ in 0..20 {
        match unsafe { OpenClipboard(Some(owner)) } {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Err(last.unwrap())
}

/// The clipboard's sequence number: read right after `set_image`, it later tells whether the
/// picture is still the one we wrote.
pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

/// Registered format understood by PNG-aware paste targets.
pub fn png_format() -> u32 {
    unsafe { RegisterClipboardFormatW(windows::core::w!("PNG")) }
}

/// Append both optional formats in one clipboard opening. Check the sequence again while locked;
/// never empty the clipboard here. A stale capture must not modify the new owner's contents.
pub fn add_capture(owner: HWND, seq: u32, png: Option<&[u8]>, path: Option<&std::path::Path>) -> Result<bool> {
    if on_a_hidden_desktop_of_his_station() || sequence() != seq {
        return Ok(false);
    }
    let file = path.map(|p| crate::dib::hdrop_bytes(&p.to_string_lossy()));
    let png_id = png.map(|_| png_format());
    if png_id == Some(0) {
        return Err(windows::core::Error::from_thread());
    }
    open(owner)?;
    let r = (|| unsafe {
        if GetClipboardSequenceNumber() != seq {
            return Ok(false);
        }
        let formats = [(png_id.unwrap_or(0), png), (CF_HDROP.0 as u32, file.as_deref())];
        for (fmt, bytes) in formats {
            let Some(bytes) = bytes else { continue };
            let h = global_from(bytes)?;
            if let Err(e) = SetClipboardData(fmt, Some(HANDLE(h.0))) {
                let _ = GlobalFree(Some(h));
                return Err(e);
            }
        }
        Ok(true)
    })();
    unsafe { let _ = CloseClipboard(); }
    r
}

/// Compatibility helper for file-only callers.
pub fn add_file(owner: HWND, seq: u32, path: &std::path::Path) -> Result<bool> {
    add_capture(owner, seq, None, Some(path))
}

pub fn set_image(owner: HWND, width: u32, height: u32, bgra: &[u8]) -> Result<()> {
    set_image_with(owner, width, height, bgra, &[])
}

/// A hidden desktop on the user's window station shares the user's real clipboard: a test running there once
/// overwrote it (2026-11-07, 12 times, an older build). A private test station has its own.
pub fn on_a_hidden_desktop_of_his_station() -> bool {
    let mut name = [0u16; 64];
    let station = unsafe {
        GetProcessWindowStation().is_ok_and(|s| GetUserObjectInformationW(HANDLE(s.0), UOI_NAME, Some(name.as_mut_ptr() as *mut _), (name.len() * 2) as u32, None).is_ok())
    };
    let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    let desk = crate::selftest::desktop_name();
    station && String::from_utf16_lossy(&name[..len]).eq_ignore_ascii_case("WinSta0") && !desk.is_empty() && !desk.eq_ignore_ascii_case("Default")
}

/// `extra`: more formats to publish with the image (the real-screen test keeps itself out of the
/// clipboard history this way).
pub fn set_image_with(owner: HWND, width: u32, height: u32, bgra: &[u8], extra: &[(u32, Vec<u8>)]) -> Result<()> {
    if on_a_hidden_desktop_of_his_station() {
        crate::logf!("clipboard: not written, this is a hidden desktop on the user's window station");
        return Ok(());
    }
    let v5 = crate::dib::dibv5_bytes(width, height, bgra);
    let dib = crate::dib::dib24_bytes(width, height, bgra);
    open(owner)?;
    let r = (|| unsafe {
        EmptyClipboard()?;
        let formats = [(CF_DIBV5.0 as u32, &v5), (CF_DIB.0 as u32, &dib)];
        for (fmt, bytes) in formats.into_iter().chain(extra.iter().map(|(f, b)| (*f, b))) {
            let h = global_from(bytes)?;
            if let Err(e) = SetClipboardData(fmt, Some(HANDLE(h.0))) {
                let _ = GlobalFree(Some(h));
                return Err(e);
            }
        }
        Ok(())
    })();
    unsafe {
        let _ = CloseClipboard();
    }
    r
}

/// Publish a recording as a file only, rechecking the start sequence while the clipboard is held.
pub fn publish_recording(owner: HWND, seq: u32, path: &std::path::Path) -> Result<bool> {
    if on_a_hidden_desktop_of_his_station() || sequence() != seq { return Ok(false); }
    let bytes = crate::dib::hdrop_bytes(&path.to_string_lossy());
    let h = global_from(&bytes)?;
    if let Err(e) = open(owner) { unsafe { let _ = GlobalFree(Some(h)); } return Err(e); }
    let r = (|| unsafe {
        if sequence() != seq { let _ = GlobalFree(Some(h)); return Ok(false); }
        if let Err(e) = EmptyClipboard() { let _ = GlobalFree(Some(h)); return Err(e); }
        if let Err(e) = SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(h.0))) { let _ = GlobalFree(Some(h)); return Err(e); }
        Ok(true)
    })();
    unsafe { let _ = CloseClipboard(); }
    r
}
