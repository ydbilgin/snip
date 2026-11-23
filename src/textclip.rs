//! Putting recognized text on the clipboard: CF_UNICODETEXT, read back to be sure, retried like
//! WinTextCapture (`textcore::set_text_with_retry`).
//!
//! Safety. The clipboard belongs to the window station, so a hidden desktop on the user's station shares
//! the user's real clipboard: a test overwrote it 12 times this way on 2026-11-07, and that is not
//! recoverable. On such a desktop `publish` never opens the clipboard; it counts instead (`SIMULATED`)
//! and answers as if it had worked. The real path is proved by `--selftest-clip`, which moves to a
//! private window station with its own clipboard first and compares the user's sequence number before and
//! after. The text itself is never logged.

use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;

use crate::textcore::{self, CLIP_ATTEMPTS, TextOutcome};

/// How many times `publish` would have written, on a hidden desktop of the user's station.
pub static SIMULATED: AtomicU64 = AtomicU64::new(0);

fn global_from(bytes: &[u8]) -> windows::core::Result<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            let e = windows::core::Error::from_thread();
            let _ = GlobalFree(Some(h));
            return Err(e);
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(h);
        Ok(h)
    }
}

fn write_unicode(owner: HWND, text: &str) -> windows::core::Result<()> {
    let bytes = textcore::unicode_text_bytes(text);
    crate::clipboard::open_quick(owner)?;
    let r = (|| unsafe {
        EmptyClipboard()?;
        let h = global_from(&bytes)?;
        if let Err(e) = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(h.0))) {
            let _ = GlobalFree(Some(h));
            return Err(e);
        }
        Ok(())
    })();
    unsafe {
        let _ = CloseClipboard();
    }
    r
}

fn read_unicode(owner: HWND) -> windows::core::Result<String> {
    crate::clipboard::open_quick(owner)?;
    let r = (|| unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_err() {
            return Ok(String::new());
        }
        let h = HGLOBAL(GetClipboardData(CF_UNICODETEXT.0 as u32)?.0);
        let p = GlobalLock(h) as *const u16;
        if p.is_null() {
            return Err(windows::core::Error::from_thread());
        }
        let s = textcore::decode_unicode_text(std::slice::from_raw_parts(p, GlobalSize(h) / 2));
        let _ = GlobalUnlock(h);
        Ok(s)
    })();
    unsafe {
        let _ = CloseClipboard();
    }
    r
}

/// Put `text` on the clipboard and say how it went.
pub fn publish(owner: HWND, text: &str) -> TextOutcome {
    if crate::clipboard::on_a_hidden_desktop_of_his_station() {
        SIMULATED.fetch_add(1, Ordering::SeqCst);
        crate::logf!("clipboard: text not written, this is a hidden desktop on the user's window station");
        return TextOutcome::Copied(text.to_string());
    }
    let result = textcore::set_text_with_retry(
        text,
        |t| write_unicode(owner, t),
        || read_unicode(owner),
        |ms| std::thread::sleep(std::time::Duration::from_millis(ms as u64)),
        CLIP_ATTEMPTS,
    );
    match result {
        Ok(true) => TextOutcome::Copied(text.to_string()),
        Ok(false) => {
            crate::logf!("clipboard: the text did not read back after {CLIP_ATTEMPTS} tries");
            TextOutcome::ClipboardUnchanged
        }
        Err(e) => {
            crate::logf!("clipboard FAILED {:#010x} {}", e.code().0, e.message());
            TextOutcome::Error
        }
    }
}

/// The real read path, for `--selftest-clip` only (it runs on a private station).
pub fn read_for_test(owner: HWND) -> windows::core::Result<String> {
    read_unicode(owner)
}
