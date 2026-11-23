//! Text capture's Windows tests. Nothing here writes to the user's clipboard, sends keys or clicks, or
//! captures the user's screen.
//!
//! - `--selftest-text DIR`: hidden desktop. The reader on known text; the text overlay on synthetic
//!   screens laid out like a four-monitor layout, each holding its own sentence, so a drag on each must
//!   read that sentence (the primary-monitor question); a whole session child end to end. The
//!   clipboard counts instead of writing, and the user's clipboard's sequence number must not move.
//! - `--selftest-ocr-bench`: hidden desktop. The error rate on rendered text, the same fonts, sizes,
//!   colours and sentences as WinTextCapture's OcrBench, to compare with its measurements.
//! - `--selftest-clip`: moves this process to a private window station with its own clipboard, runs
//!   the real clipboard path there, and proves the user's clipboard did not change.

use std::path::{Path, PathBuf};
use std::time::Instant;

use windows::Win32::Foundation::{COLORREF, HINSTANCE, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::capture::{self, Frame, Method, MonitorInfo, Pixels, Snapshot};
use crate::geometry::{Layout, Rect};
use crate::overlay::{Kind, Opts, Outcome, Overlay};
use crate::textcore::{cer, squeeze};

pub const TR_LINES: [&str; 4] = [
    "Şirket çalışanları için güvenlik güncellemesi yüklendi.",
    "Dosya açılamadı: İzin reddedildi, lütfen yöneticinize başvurun.",
    "Görüşme Çarşamba günü saat 14:30'da İstanbul ofisinde.",
    "Toplam tutar 1.234,56 TL, ödeme tarihi 21.11.2026.",
];
pub const EN_LINES: [&str; 4] = [
    "Error 0x80070643: Fatal error during installation.",
    "Contact your administrator or the package vendor.",
    "Press the hotkey, drag over the text, release the mouse.",
    "Total 4,800.00 USD due on 21 Oct 2026 (ref AB-1043).",
];
const FACES: [&str; 6] = ["Segoe UI", "Arial", "Tahoma", "Calibri", "Consolas", "Times New Roman"];
const SIZES: [i32; 5] = [11, 12, 14, 16, 20];
/// (name, background, ink) as (r, g, b).
type Rgb = (u8, u8, u8);
const STYLES: [(&str, Rgb, Rgb); 4] = [
    ("black-on-white", (255, 255, 255), (32, 32, 32)),
    ("white-on-dark", (32, 32, 32), (240, 240, 240)),
    ("yellow-on-blue", (20, 30, 70), (255, 212, 59)),
    ("grey-on-white", (255, 255, 255), (130, 130, 130)),
];
const MARGIN: i32 = 12;

fn on_his_screen() -> bool {
    let d = crate::selftest::desktop_name();
    d.is_empty() || d.eq_ignore_ascii_case("Default") || d.eq_ignore_ascii_case("Winlogon")
}

/// `lines` drawn with GDI, the way WinTextCapture's bench drew them (DrawText, no padding).
pub fn render_lines(lines: &[&str], face: &str, px: i32, bg: (u8, u8, u8), ink: (u8, u8, u8)) -> Result<(u32, u32, Vec<u8>), String> {
    let wide_lines: Vec<Vec<u16>> = lines.iter().map(|l| l.encode_utf16().collect()).collect();
    let face_w: Vec<u16> = face.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let dc = CreateCompatibleDC(None);
        let font = CreateFontW(-px, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, DEFAULT_QUALITY, 0, PCWSTR(face_w.as_ptr()));
        let old_font = SelectObject(dc, font.into());
        let (mut max_w, mut line_h) = (0, 0);
        for l in &wide_lines {
            let mut r = RECT::default();
            let mut t = l.clone();
            DrawTextW(dc, &mut t, &mut r, DT_CALCRECT | DT_NOPREFIX | DT_SINGLELINE);
            max_w = max_w.max(r.right - r.left);
            line_h = line_h.max(r.bottom - r.top);
        }
        let step = line_h + (px as f32 * 0.35) as i32;
        let (w, h) = (max_w + 2 * MARGIN, step * lines.len() as i32 + 2 * MARGIN);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).map_err(|e| e.to_string())?;
        let old_bmp = SelectObject(dc, dib.into());
        let n = (w * h * 4) as usize;
        let colour = |c: (u8, u8, u8)| COLORREF(c.0 as u32 | (c.1 as u32) << 8 | (c.2 as u32) << 16);
        let brush = CreateSolidBrush(colour(bg));
        let all = RECT { left: 0, top: 0, right: w, bottom: h };
        FillRect(dc, &all, brush);
        let _ = DeleteObject(brush.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, colour(ink));
        for (i, l) in wide_lines.iter().enumerate() {
            let top = MARGIN + i as i32 * step;
            let mut r = RECT { left: MARGIN, top, right: w, bottom: top + step };
            let mut t = l.clone();
            DrawTextW(dc, &mut t, &mut r, DT_NOPREFIX | DT_SINGLELINE);
        }
        let _ = GdiFlush();
        let mut px_out = std::slice::from_raw_parts(bits as *const u8, n).to_vec();
        for p in px_out.as_chunks_mut::<4>().0 {
            p[3] = 255;
        }
        SelectObject(dc, old_bmp);
        SelectObject(dc, old_font);
        let _ = DeleteObject(dib.into());
        let _ = DeleteObject(font.into());
        let _ = DeleteDC(dc);
        Ok((w as u32, h as u32, px_out))
    }
}

// ---- The reader's error rate ----

struct Row {
    lang: &'static str,
    face: &'static str,
    px: i32,
    style: &'static str,
    cer: f64,
    ms: f64,
}

/// Error rate in percent over the rows matching `f`.
fn mean(rows: &[Row], f: impl Fn(&Row) -> bool) -> f64 {
    let v: Vec<f64> = rows.iter().filter(|r| f(r)).map(|r| r.cer).collect();
    if v.is_empty() { f64::NAN } else { v.iter().sum::<f64>() / v.len() as f64 }
}

pub fn run_bench(args: &[String]) -> i32 {
    if on_his_screen() {
        println!("REFUSED: run this on a hidden desktop");
        return 4;
    }
    let out = args.iter().find(|a| !a.starts_with("--")).map(String::as_str);
    if args.iter().any(|a| a == "--wic") {
        crate::ocr::USE_WIC.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    println!("desktop: {:?}", crate::selftest::desktop_name());
    let sequence = unsafe { GetClipboardSequenceNumber() };
    let engine = match crate::ocr::engine_for("auto") {
        Ok(e) => e,
        Err(e) => {
            println!("FAIL no engine: {e:?}");
            return 1;
        }
    };
    let language = engine.RecognizerLanguage().and_then(|l| l.LanguageTag()).map(|t| t.to_string()).unwrap_or_default();
    println!("reader language: {language}; installed: {:?}", crate::ocr::installed().unwrap_or_default().iter().map(|(t, _)| t.clone()).collect::<Vec<_>>());
    let mut rows = Vec::new();
    let mut failures = 0;
    for (lang, lines) in [("tr", &TR_LINES), ("en", &EN_LINES)] {
        let truth = lines.join(" ");
        for face in FACES {
            for px in SIZES {
                for (style, bg, ink) in STYLES {
                    let Ok((w, h, img)) = render_lines(lines.as_slice(), face, px, bg, ink) else {
                        failures += 1;
                        continue;
                    };
                    let t0 = Instant::now();
                    match crate::ocr::read_with(&engine, &language, w, h, &img, t0) {
                        Ok(r) => rows.push(Row { lang, face, px, style, cer: cer(&r.text, &truth), ms: r.recognize_ms }),
                        Err(e) => {
                            failures += 1;
                            println!("FAIL {lang} {face} {px} {style}: {e:?}");
                        }
                    }
                }
            }
        }
    }
    let scaler = if crate::ocr::USE_WIC.load(std::sync::atomic::Ordering::Relaxed) { "WIC high quality cubic" } else { "GDI+ high quality bicubic" };
    let mut report = format!("Rust reader, language {language}, scaler {scaler}, {} images, {failures} failures. Error is percent of characters.\n", rows.len());
    for lang in ["tr", "en"] {
        report.push_str(&format!("\n{lang}: mean over styles and sizes\n"));
        let mut face_means = Vec::new();
        for face in FACES {
            let m = mean(&rows, |r| r.lang == lang && r.face == face);
            face_means.push(m);
            let by_size: Vec<String> = SIZES.iter().map(|px| format!("{px}px {:.2}", mean(&rows, |r| r.lang == lang && r.face == face && r.px == *px))).collect();
            report.push_str(&format!("  {face:<16} {m:>5.2}   {}\n", by_size.join("  ")));
        }
        report.push_str(&format!("  face mean        {:>5.2}\n", face_means.iter().sum::<f64>() / face_means.len() as f64));
        // WinTextCapture's table is per style too: this is the black-on-white row of each.
        for (style, _, _) in STYLES {
            report.push_str(&format!("  style {style:<16} {:>5.2}\n", mean(&rows, |r| r.lang == lang && r.style == style)));
        }
    }
    let mut times: Vec<f64> = rows.iter().map(|r| r.ms).collect();
    times.sort_by(f64::total_cmp);
    if let Some(mid) = times.get(times.len() / 2) {
        report.push_str(&format!("\nrecognize median {mid:.1} ms over {} images\n", times.len()));
    }
    print!("{report}");
    if let Some(path) = out {
        let _ = std::fs::write(path, &report);
    }
    let unchanged = sequence == unsafe { GetClipboardSequenceNumber() };
    println!("the user's clipboard sequence number {}: {}", sequence, if unchanged { "unchanged" } else { "CHANGED" });
    if failures > 0 || !unchanged { 1 } else { 0 }
}

// ---- The text path on synthetic screens ----

fn fill_frame(w: i32, h: i32, c: (u8, u8, u8)) -> Vec<u8> {
    let mut v = vec![255u8; (w * h * 4) as usize];
    for p in v.as_chunks_mut::<4>().0 {
        *p = [c.2, c.1, c.0, 255];
    }
    v
}

fn blit(dst: &mut [u8], dw: i32, x: i32, y: i32, src: &[u8], sw: i32, sh: i32) {
    for j in 0..sh {
        let from = (j * sw * 4) as usize;
        let to = (((y + j) * dw + x) * 4) as usize;
        dst[to..to + (sw * 4) as usize].copy_from_slice(&src[from..from + (sw * 4) as usize]);
    }
}

/// One monitor of the user's layout: where it is and the sentence on it.
struct Screen {
    name: &'static str,
    rect: Rect,
    primary: bool,
    line: &'static str,
    /// Where the text block lands inside the monitor.
    at: (i32, i32),
    block: (u32, u32),
}

/// A four-monitor layout measured on a real desk, each with its own sentence,
/// placed off centre so a wrong offset reads another line or nothing.
fn his_screens() -> Vec<Screen> {
    let s = |name, l, t, w, h, primary, line, at| Screen { name, rect: Rect::from_xywh(l, t, w, h), primary, line, at, block: (0, 0) };
    vec![
        s("DISPLAY2", -1920, 0, 1920, 1080, false, TR_LINES[0], (300, 240)),
        s("DISPLAY1", -606, -1440, 3440, 1440, false, TR_LINES[1], (900, 700)),
        s("DISPLAY4 (primary)", 0, 0, 2560, 1440, true, TR_LINES[2], (610, 455)),
        s("DISPLAY3", 2560, 0, 2560, 1440, false, TR_LINES[3], (1500, 1100)),
    ]
}

fn his_snapshot(screens: &mut [Screen]) -> Result<Snapshot, String> {
    let mut monitors = Vec::new();
    let mut frames = Vec::new();
    for s in screens.iter_mut() {
        let (tw, th, block) = render_lines(&[s.line], "Segoe UI", 20, (255, 255, 255), (32, 32, 32))?;
        s.block = (tw, th);
        // A page of a colour no text uses, so reading anything but the sentence is plain to see.
        let mut px = fill_frame(s.rect.width(), s.rect.height(), (243, 243, 243));
        blit(&mut px, s.rect.width(), s.at.0, s.at.1, &block, tw as i32, th as i32);
        monitors.push(MonitorInfo { hmon: HMONITOR::default(), rect: s.rect, work: s.rect, name: s.name.into(), dpi: 96, primary: s.primary });
        frames.push(Frame { width: s.rect.width() as u32, height: s.rect.height() as u32, pixels: Pixels::Cpu(px), method: Method::Gdi, ms: 0.0, zero_present: false, note: "synthetic".into() });
    }
    let layout = Layout::new(screens.iter().map(|s| s.rect).collect());
    Ok(Snapshot { monitors, layout, frames, gpu: Some(capture::create_gpu().map_err(|e| e.to_string())?), windows: vec![], gpu_ms: 0.0 })
}

type Checks<'a> = &'a mut dyn FnMut(&str, bool, String);

type Drag = (i32, i32, i32, i32);
fn scene_file(dir: &Path, lines: &[&str]) -> Result<(PathBuf, Drag), String> {
    let (w, h, block) = render_lines(lines, "Segoe UI", 16, (255, 255, 255), (32, 32, 32))?;
    let (sw, sh) = (900, 500);
    let mut px = fill_frame(sw, sh, (243, 243, 243));
    let (x, y) = (140, 120);
    blit(&mut px, sw, x, y, &block, w as i32, h as i32);
    let path = dir.join("scene.bin");
    let mut bytes = Vec::with_capacity(8 + px.len());
    bytes.extend_from_slice(&(sw as u32).to_le_bytes());
    bytes.extend_from_slice(&(sh as u32).to_le_bytes());
    bytes.extend_from_slice(&px);
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok((path, (x - 4, y - 4, x + w as i32 + 4, y + h as i32 + 4)))
}

fn reader_checks(check: Checks) {
    // Known text, each line break kept.
    let (w, h, px) = match render_lines(&TR_LINES, "Segoe UI", 16, (255, 255, 255), (32, 32, 32)) {
        Ok(v) => v,
        Err(e) => return check("reader-known-text", false, e),
    };
    let t0 = Instant::now();
    match crate::ocr::read(w, h, &px, "auto") {
        Ok(r) => {
            let e = cer(&r.text, &TR_LINES.join(" "));
            let lines = r.text.split("\r\n").count();
            check("reader-known-text", e < 1.0, format!("{:.2}% error, language {}, cold {:.0} ms", e, r.language, t0.elapsed().as_secs_f64() * 1000.0));
            check("reader-keeps-line-breaks", lines == 4, format!("{lines} lines, CRLF between them"));
        }
        Err(e) => check("reader-known-text", false, format!("{e:?}")),
    }
    // Latin and Chinese in one line (the nick of 2026-11-13): automatic asks a Chinese reader too.
    let has_cjk = crate::ocr::installed().unwrap_or_default().iter().any(|(t, _)| t.to_ascii_lowercase().starts_with("zh"));
    let nick = "EGO窥视#6240";
    match render_lines(&[nick], "Microsoft YaHei", 20, (40, 40, 40), (230, 230, 230)).map_err(|e| e.to_string()).and_then(|(w, h, px)| crate::ocr::read(w, h, &px, "auto").map_err(|e| format!("{e:?}"))) {
        Ok(r) if has_cjk => check("reader-latin-and-chinese", r.text == nick, format!("{:?}, language {}, second readers {:.0} ms", r.text, r.language, r.second_ms)),
        Ok(r) => check("reader-latin-and-chinese", r.text != nick, format!("no CJK reader installed: read {:?} (wrong, as expected)", r.text)),
        Err(e) => check("reader-latin-and-chinese", false, e),
    }
    // A language Windows has no reader for says so, it does not read in another.
    let no = crate::ocr::read(w, h, &px, "xx-XX");
    check("reader-missing-language", no == Err(crate::ocr::ReadError::NoEngine), format!("{:?}", no.as_ref().map(|r| r.text.len())));
    // Empty ground: no text, no error.
    let blank = fill_frame(400, 120, (255, 255, 255));
    match crate::ocr::read(400, 120, &blank, "auto") {
        Ok(r) => check("reader-blank", r.text.is_empty(), format!("{:?}", r.text)),
        Err(e) => check("reader-blank", false, format!("{e:?}")),
    }
    // The strip that broke the October build: wide and low, over the upscale limit.
    let long = "OCR 2026 TEXT ".repeat(18);
    if let Ok((w, h, px)) = render_lines(&[long.trim_end()], "Segoe UI", 24, (255, 255, 255), (32, 32, 32)) {
        match crate::ocr::read(w, h, &px, "auto") {
            Ok(r) => check("reader-wide-strip", cer(&r.text, long.trim_end()) < 5.0, format!("{w}x{h} prepared {:?}, {:.2}% error", r.prepared, cer(&r.text, long.trim_end()))),
            Err(e) => check("reader-wide-strip", false, format!("{e:?}")),
        }
    }
}

fn primary_monitor_checks(hinst: HINSTANCE, check: Checks) {
    let mut screens = his_screens();
    let snap = match his_snapshot(&mut screens) {
        Ok(s) => s,
        Err(e) => return check("four-monitor-layout", false, e),
    };
    let mut o = Opts::plain(0.45);
    o.text = true;
    o.text_action = true;
    o.lang = crate::i18n::Lang::Tr;
    let mut ov = match Overlay::new(snap, hinst, &o) {
        Ok(v) => v,
        Err(e) => return check("four-monitor-layout", false, e.to_string()),
    };
    ov.no_throttle = true;
    if let Err(e) = ov.show_passive() {
        return check("four-monitor-layout", false, e.to_string());
    }
    for s in &screens {
        let (x0, y0) = (s.rect.left + s.at.0 - 2, s.rect.top + s.at.1 - 2);
        let (x1, y1) = (x0 + s.block.0 as i32 + 4, y0 + s.block.1 as i32 + 4);
        let out = ov.selftest_drag(x0, y0, x1, y1);
        let Outcome::TextArea(rect, false) = out else {
            check(&format!("monitor {}", s.name), false, format!("{out:?}"));
            continue;
        };
        let read = ov.snap.crop(&rect).map_err(|e| e.to_string()).and_then(|(w, h, px)| crate::ocr::read(w, h, &px, "auto").map_err(|e| format!("{e:?}")));
        match read {
            Ok(r) => {
                let e = cer(&r.text, s.line);
                // This asks "was the right sentence read"; how well is the bench's question.
                check(&format!("monitor {}", s.name), e < 5.0, format!("drag at {},{} read {:?} ({e:.2}% error)", x0, y0, squeeze(&r.text)));
            }
            Err(e) => check(&format!("monitor {}", s.name), false, e),
        }
    }
    // A selection across the primary and its left neighbour reads each side.
    let (a, b) = (&screens[0], &screens[2]);
    let span = Rect::new(a.rect.right - 40, 200, b.rect.left + 40, 260);
    let across = ov.snap.crop(&span).map(|(w, h, _)| (w, h));
    check("selection-across-monitors", across.as_ref().is_ok_and(|(w, h)| (*w as i32, *h as i32) == (span.width(), span.height())), format!("{across:?}"));
    // Small drags, Esc and Enter.
    let (cx, cy) = (b.rect.left + 500, b.rect.top + 500);
    let tiny = ov.selftest_drag(cx, cy, cx + 8, cy + 200);
    check("selection-too-small-cancels", tiny == Outcome::Cancel, format!("{tiny:?}"));
    let enter = ov.on_key_mods(crate::hookstate::VK_RETURN, false, false);
    check("enter-does-nothing-in-text-mode", enter == Outcome::Nothing, format!("{enter:?}"));
    let esc = ov.on_key_mods(crate::hookstate::VK_ESCAPE, false, false);
    check("esc-cancels", esc == Outcome::Cancel, format!("{esc:?}"));
    check("no-window-picking", ov.current().is_none_or(|(_, k)| k == Kind::Rectangle), "only a dragged rectangle counts".into());
}

fn editor_action_check(hinst: HINSTANCE, check: Checks) {
    let mut screens = vec![Screen { name: "one", rect: Rect::from_xywh(0, 0, 1200, 700), primary: true, line: TR_LINES[0], at: (120, 200), block: (0, 0) }];
    let snap = match his_snapshot(&mut screens) {
        Ok(s) => s,
        Err(e) => return check("editor-copy-text", false, e),
    };
    let mut o = Opts::plain(0.45);
    o.editor = Some(crate::editor::Prefs::default());
    o.text_action = true;
    let mut ov = match Overlay::new(snap, hinst, &o) {
        Ok(v) => v,
        Err(e) => return check("editor-copy-text", false, e.to_string()),
    };
    ov.no_throttle = true;
    let s = &screens[0];
    let (x1, y1) = (s.at.0 + s.block.0 as i32 + 6, s.at.1 + s.block.1 as i32 + 6);
    ov.selftest_drag(s.at.0 - 6, s.at.1 - 6, x1, y1);
    let out = ov.on_key_mods(0x43, true, true);
    let Outcome::TextArea(rect, stay) = out else {
        return check("editor-copy-text", false, format!("Ctrl+Shift+C gave {out:?}"));
    };
    let read = ov.snap.crop(&rect).map_err(|e| e.to_string()).and_then(|(w, h, px)| crate::ocr::read(w, h, &px, "auto").map_err(|e| format!("{e:?}")));
    match read {
        Ok(r) => check("editor-copy-text", !stay && cer(&r.text, s.line) < 2.0, format!("read {:?}, nothing drawn so it closes", squeeze(&r.text))),
        Err(e) => check("editor-copy-text", false, e),
    }
    let copy = ov.on_key_mods(0x43, true, false);
    check("editor-ctrl-c-still-copies-the-picture", copy == Outcome::Edited(crate::editor::Effect::Copy), format!("{copy:?}"));
    // Without text capture the action is gone and Ctrl+Shift+C is plain copy again.
    let off = ov.editor.as_mut().map(|e| {
        e.text_action = false;
        e.item_center(crate::toolbar::Item::CopyText)
    });
    check("editor-action-follows-the-setting", off == Some(None), format!("{off:?}"));
}

fn session_check(dir: &Path, check: Checks) {
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::*;
    use windows::core::PWSTR;
    let lines = [TR_LINES[0], TR_LINES[3]];
    let (scene, drag) = match scene_file(dir, &lines) {
        Ok(v) => v,
        Err(e) => return check("session-end-to-end", false, e),
    };
    let result_dir = dir.join("session");
    let _ = std::fs::remove_dir_all(&result_dir);
    let t0 = crate::log::qpc();
    let cmd = format!("\"{}\" --session 0 --go {t0} 7 --selftest-dir \"{}\" --selftest-scene \"{}\" --selftest-drag {} {} {} {}", crate::actions::exe_path(), result_dir.display(), scene.display(), drag.0, drag.1, drag.2, drag.3);
    let mut wide: Vec<u16> = cmd.encode_utf16().chain(Some(0)).collect();
    let si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut pi = PROCESS_INFORMATION::default();
    unsafe {
        if let Err(e) = CreateProcessW(PCWSTR::null(), Some(PWSTR(wide.as_mut_ptr())), None, None, false, PROCESS_CREATION_FLAGS(0), None, PCWSTR::null(), &si, &mut pi) {
            return check("session-end-to-end", false, e.to_string());
        }
        let _ = CloseHandle(pi.hThread);
        let exited = WaitForSingleObject(pi.hProcess, 25_000) == WAIT_OBJECT_0;
        let mut code = 99u32;
        let _ = GetExitCodeProcess(pi.hProcess, &mut code);
        if !exited {
            let _ = TerminateProcess(pi.hProcess, 1);
        }
        let _ = CloseHandle(pi.hProcess);
        let ms = crate::log::ms_since(t0);
        let body = std::fs::read_to_string(result_dir.join("text-result.txt")).unwrap_or_default();
        let (state, text) = body.split_once('\n').unwrap_or(("", ""));
        let e = cer(text, &lines.join(" "));
        check("session-end-to-end", exited && code == 0 && state == "copied" && e < 2.0, format!("child exit {code}, {state:?}, {e:.2}% error, {:?}, whole run {ms:.0} ms including the card", squeeze(text)));
        let kept = crate::texthistory::load(&result_dir);
        check("session-recent-texts", kept.entries.first().is_some_and(|x| x.text == text), format!("{} entries", kept.entries.len()));
    }
}

fn card_check(check: Checks) {
    use crate::textcore::TextOutcome;
    let t = crate::theme::Theme::new(crate::theme::Look::Graphite, crate::theme::Mode::Dark);
    let o = TextOutcome::Copied("Şirket\r\nGörüşme".into());
    let Some(c) = crate::cardview::content(&o, crate::i18n::Lang::Tr) else { return check("card", false, "no content".into()) };
    unsafe {
        let host = CreateWindowExW(WS_EX_TOOLWINDOW, w!("STATIC"), w!("host"), WS_POPUP, 0, 0, 0, 0, None, None, None, None).unwrap_or_default();
        let fg_before = GetForegroundWindow();
        let shown = crate::card::show(host, &c, 200, &t);
        let card = FindWindowW(crate::card::CLASS, PCWSTR::null()).unwrap_or_default();
        let ex = GetWindowLongPtrW(card, GWL_EXSTYLE) as u32;
        let want = WS_EX_NOACTIVATE.0 | WS_EX_TOPMOST.0 | WS_EX_LAYERED.0 | WS_EX_TOOLWINDOW.0;
        check("card-never-takes-the-keyboard", shown && ex & want == want && GetForegroundWindow() == fg_before, format!("ex style {ex:#x}"));
        // It leaves by itself; the hold is short here.
        let until = Instant::now() + std::time::Duration::from_secs(4);
        let mut msg = MSG::default();
        while crate::card::is_open() && Instant::now() < until {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        check("card-goes-away-by-itself", !crate::card::is_open(), "closed within its time".into());
        let _ = DestroyWindow(host);
    }
}

pub fn run_text(args: &[String]) -> i32 {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
    if on_his_screen() {
        println!("REFUSED: this test opens overlay windows; run it on a hidden desktop (not the visible one), never on the user's screen");
        return 4;
    }
    println!("desktop: {:?}", crate::selftest::desktop_name());
    let dir = args.first().map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join(format!("snip-text-{}", std::process::id())));
    let _ = std::fs::create_dir_all(&dir);
    let sequence = unsafe { GetClipboardSequenceNumber() };
    let (mut pass, mut fail) = (0, 0);
    let mut c = |name: &str, ok: bool, detail: String| {
        if ok {
            pass += 1;
            println!("PASS {name}: {detail}");
        } else {
            fail += 1;
            println!("FAIL {name}: {detail}");
        }
    };
    c("clipboard-guard", crate::clipboard::on_a_hidden_desktop_of_his_station(), "this desktop shares the user's clipboard, so the text path only counts".into());
    let hinst: HINSTANCE = unsafe { GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default() };
    if let Err(e) = crate::app::register_classes(hinst, crate::app::MAIN_CLASS_TEST) {
        c("register-classes", false, e.to_string());
    }
    reader_checks(&mut c);
    primary_monitor_checks(hinst, &mut c);
    editor_action_check(hinst, &mut c);
    card_check(&mut c);
    // The real publish, on this desktop: it must count and not write.
    let before = crate::textclip::SIMULATED.load(std::sync::atomic::Ordering::SeqCst);
    let done = crate::textclip::publish(windows::Win32::Foundation::HWND::default(), "yazılmamalı");
    let after = crate::textclip::SIMULATED.load(std::sync::atomic::Ordering::SeqCst);
    c("publish-counts-instead-of-writing", after == before + 1 && matches!(done, crate::textcore::TextOutcome::Copied(_)), format!("{before} -> {after}"));
    session_check(&dir, &mut c);
    let now = unsafe { GetClipboardSequenceNumber() };
    c("user-clipboard-untouched", now == sequence, format!("sequence number {sequence} before, {now} after"));
    println!("{pass} passed, {fail} failed");
    if fail == 0 && pass > 0 { 0 } else { 1 }
}

// ---- The real clipboard path, on a private station ----

pub fn run_clip() -> i32 {
    use windows::Win32::Foundation::GENERIC_ALL;
    use windows::Win32::System::StationsAndDesktops::*;
    let his_before = unsafe { GetClipboardSequenceNumber() };
    println!("the user's clipboard sequence number before: {his_before}");
    let his_station = match unsafe { GetProcessWindowStation() } {
        Ok(s) => s,
        Err(e) => {
            println!("FAIL no station: {e}");
            return 1;
        }
    };
    let his_raw = his_station.0 as isize;
    // Everything happens on one fresh thread, moved to a station of its own before any window.
    let worker = std::thread::spawn(move || -> Result<String, String> {
        unsafe {
            let name: Vec<u16> = format!("SnipNightTest{}", std::process::id()).encode_utf16().chain(Some(0)).collect();
            let station = CreateWindowStationW(PCWSTR(name.as_ptr()), 0, GENERIC_ALL.0, None).map_err(|e| format!("station: {e}"))?;
            SetProcessWindowStation(station).map_err(|e| format!("set station: {e}"))?;
            let desk = CreateDesktopW(w!("Default"), PCWSTR::null(), None, DESKTOP_CONTROL_FLAGS(0), GENERIC_ALL.0, None).map_err(|e| format!("desktop: {e}"))?;
            SetThreadDesktop(desk).map_err(|e| format!("set desktop: {e}"))?;
            let mut buf = [0u16; 64];
            GetUserObjectInformationW(windows::Win32::Foundation::HANDLE(station.0), UOI_NAME, Some(buf.as_mut_ptr() as *mut _), 128, None).map_err(|e| e.to_string())?;
            let got = String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]);
            if !got.starts_with("SnipNightTest") {
                return Err(format!("not on a private station: {got}"));
            }
            if crate::clipboard::on_a_hidden_desktop_of_his_station() {
                return Err("the guard says this is still the user's station".into());
            }
            let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, w!("STATIC"), w!("clip test"), WS_POPUP, 0, 0, 0, 0, None, None, None, None).map_err(|e| format!("window: {e}"))?;
            let text = "Şirket çalışanları\r\nİstanbul 14:30 \u{1F600}";
            let local_before = GetClipboardSequenceNumber();
            let done = crate::textclip::publish(hwnd, text);
            if done != crate::textcore::TextOutcome::Copied(text.to_string()) {
                return Err(format!("publish: {done:?}"));
            }
            let back = crate::textclip::read_for_test(hwnd).map_err(|e| e.to_string())?;
            if back != text {
                return Err(format!("read back {back:?}"));
            }
            // The picture, then the saved file next to it (clipboard_file): both formats must stay.
            let (w, h) = (149u32, 44u32);
            let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, 40, 200, 255]).collect();
            crate::clipboard::set_image(hwnd, w, h, &px).map_err(|e| format!("set_image: {e}"))?;
            let seq = crate::clipboard::sequence();
            let file = std::env::temp_dir().join("snip-clip-test.jpg");
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
            let png = crate::png::encode_png(w, h, &px).map_err(|e| e.to_string())?;
            let added = crate::clipboard::add_capture(hwnd, seq, Some(&png), Some(&file)).map_err(|e| format!("add_capture: {e}"))?;
            use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard};
            use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
            use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP};
            let read = |fmt: u32| -> Option<Vec<u8>> {
                OpenClipboard(Some(hwnd)).ok()?;
                let r = GetClipboardData(fmt).ok().and_then(|hd| {
                    let g = windows::Win32::Foundation::HGLOBAL(hd.0);
                    let p = GlobalLock(g) as *const u8;
                    (!p.is_null()).then(|| {
                        let v = std::slice::from_raw_parts(p, GlobalSize(g)).to_vec();
                        let _ = GlobalUnlock(g);
                        v
                    })
                });
                let _ = CloseClipboard();
                r
            };
            let v5 = read(CF_DIBV5.0 as u32).unwrap_or_default();
            let drop = read(CF_HDROP.0 as u32).unwrap_or_default();
            let want_v5 = crate::dib::dibv5_bytes(w, h, &px);
            let want_drop = crate::dib::hdrop_bytes(&file.to_string_lossy());
            let both = IsClipboardFormatAvailable(CF_DIBV5.0 as u32).is_ok() && IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_ok();
            if !(added && both && v5.get(..want_v5.len()) == Some(&want_v5[..]) && drop.get(..want_drop.len()) == Some(&want_drop[..])) {
                return Err(format!("picture + file: added={added} both={both} dibv5 {} bytes, hdrop {} bytes", v5.len(), drop.len()));
            }
            let dib = read(CF_DIB.0 as u32).unwrap_or_default();
            let want_dib = crate::dib::dib24_bytes(w, h, &px);
            let png_back = read(crate::clipboard::png_format()).ok_or("PNG missing")?;
            if dib.get(..want_dib.len()) != Some(&want_dib[..]) {
                return Err("CF_DIB pixels changed after PNG/file append".into());
            }
            println!("PASS picture-formats: CF_DIBV5, CF_DIB, PNG and CF_HDROP coexist");
            let decoded = crate::png::decode_png(&png_back).map_err(|e| e.to_string())?;
            if decoded != (w, h, px.clone()) { return Err("WIC PNG pixels differ".into()); }
            println!("PASS png-pixels: WIC decoded exact capture pixels");
            let stale = crate::clipboard::add_file(hwnd, seq, &file).map_err(|e| format!("add_file again: {e}"))?;
            if stale {
                return Err("add_file wrote over a clipboard that had moved on".into());
            }
            // No save file and no history: the actual worker still encodes and appends PNG.
            crate::clipboard::set_image(hwnd, w, h, &px).map_err(|e| e.to_string())?;
            let seq = crate::clipboard::sequence();
            crate::app::test_save_copy(hwnd, seq, crate::raster::Canvas::from_bgra(w, h, px.clone()), None);
            let worker_png = read(crate::clipboard::png_format()).ok_or("PNG-only worker did not append")?;
            if crate::png::decode_png(&worker_png).map_err(|e| e.to_string())? != (w, h, px.clone()) {
                return Err("PNG-only worker changed pixels".into());
            }
            println!("PASS png-only-worker: production save worker, file/history off");
            // History/pin use a common asynchronous copy helper and hold their window on close.
            let image = crate::raster::Canvas::from_bgra(w, h, px.clone());
            crate::app::copy_picture(hwnd, &image, false).map_err(|e| e.to_string())?;
            if IsClipboardFormatAvailable(crate::clipboard::png_format()).is_ok() { return Err("PNG added despite clipboard_png=false".into()); }
            println!("PASS png-disabled-copy: DIB only when configured off");
            crate::app::copy_picture(hwnd, &image, true).map_err(|e| e.to_string())?;
            if !crate::app::wait_copy_png() { return Err("PNG copy worker did not finish within 3 s".into()); }
            let copied = read(crate::clipboard::png_format()).ok_or("history/pin PNG missing")?;
            if crate::png::decode_png(&copied).map_err(|e| e.to_string())? != (w, h, px.clone()) { return Err("history/pin PNG differs".into()); }
            println!("PASS png-history-pin-worker: asynchronous helper and bounded close wait, exact pixels");
            // A different owner replaces the capture before its worker finishes.
            crate::clipboard::set_image(hwnd, w, h, &px).map_err(|e| e.to_string())?;
            let seq = crate::clipboard::sequence();
            let other = CreateWindowExW(WS_EX_TOOLWINDOW, w!("STATIC"), w!("other owner"), WS_POPUP, 0, 0, 0, 0, None, None, None, None).map_err(|e| e.to_string())?;
            let done = crate::textclip::publish(other, "new owner");
            if done != crate::textcore::TextOutcome::Copied("new owner".into()) { return Err("other owner publish failed".into()); }
            let newer = crate::clipboard::sequence();
            crate::app::test_save_copy(hwnd, seq, crate::raster::Canvas::from_bgra(w, h, px), None);
            if IsClipboardFormatAvailable(crate::clipboard::png_format()).is_ok()
                || crate::clipboard::sequence() != newer
                || crate::textclip::read_for_test(other).map_err(|e| e.to_string())? != "new owner" {
                return Err("stale worker modified new owner's clipboard".into());
            }
            let _ = DestroyWindow(other);
            println!("PASS stale-png-worker: another owner's text and sequence unchanged; PNG absent");
            let local_after = GetClipboardSequenceNumber();
            let _ = DestroyWindow(hwnd);
            // Back to the user's station, so the number below is the user's and not this one's.
            SetProcessWindowStation(HWINSTA(his_raw as *mut _)).map_err(|e| format!("back to the user's station: {e}"))?;
            Ok(format!("real text write and read back, then picture (CF_DIBV5 exact) plus saved file (CF_HDROP exact, refused once the clipboard moved on) on {got}; that station's sequence {local_before} -> {local_after}"))
        }
    });
    let r = worker.join().map_err(|_| "panic".to_string()).and_then(|x| x);
    let his_after = unsafe { GetClipboardSequenceNumber() };
    match r {
        Ok(detail) => println!("PASS clipboard-real-path: {detail}"),
        Err(e) => {
            println!("FAIL clipboard-real-path: {e}");
            return 1;
        }
    }
    if his_after == his_before {
        println!("PASS user-clipboard-untouched: {his_before} before, {his_after} after");
        println!("selftest-clip: 8 passed, 0 failed");
        0
    } else {
        println!("FAIL user-clipboard-untouched: {his_before} before, {his_after} after");
        1
    }
}
