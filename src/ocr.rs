//! Reading text from a picture with the reader in Windows (Windows.Media.Ocr). Nothing leaves this
//! PC. The rules come from WinTextCapture (`textcore`): which engine, what size, how lines join.
//!
//! The caller passes the raw crop; `read` prepares it once. Preparing it twice broke reading in the
//! October build (the engine reads nothing under about 40 px high, so a shrink lost the text).
//!
//! `RecognizeAsync(..).join()` blocks until the answer comes: call this on a worker thread, never on
//! the UI thread. A thread without an apartment is fine: `ensure_com` makes the process-wide one.

use std::sync::OnceLock;
use std::time::Instant;

use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoIncrementMTAUsage};
use windows::core::HSTRING;

use crate::textcore::{self, EngineRequest};

/// Why nothing was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// Windows has no reader for that language.
    NoEngine,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    /// Lines joined with CRLF, each trimmed, empty ones dropped. Empty: no text found.
    pub text: String,
    /// The first reader's tag; "tr+zh-Hans-CN" when another script's reader gave part of the text.
    pub language: String,
    pub prepared: (u32, u32),
    pub engine_ms: f64,
    pub prepare_ms: f64,
    pub recognize_ms: f64,
    /// Making and running the other scripts' readers in automatic mode (about 0 when none ran).
    pub second_ms: f64,
    /// Share of the ink the first reader left outside its words (0 when no second reader exists).
    pub unread_ink: f64,
    /// What each reader read, first reader first (for the tests and `--ocr-file`).
    pub readings: Vec<textcore::EngineReading>,
}

fn ms(a: Instant, b: Instant) -> f64 {
    (b - a).as_secs_f64() * 1000.0
}

fn ensure_com() -> Result<(), String> {
    static MTA: OnceLock<Result<(), String>> = OnceLock::new();
    // The cookie is kept for the life of the process: the apartment must outlive these objects.
    MTA.get_or_init(|| unsafe { CoIncrementMTAUsage() }.map(|_| ()).map_err(|e| format!("CoIncrementMTAUsage: {e}"))).clone()
}

/// (tag, display name) of every reader Windows has.
pub fn installed() -> Result<Vec<(String, String)>, String> {
    let list = OcrEngine::AvailableRecognizerLanguages().map_err(|e| format!("AvailableRecognizerLanguages: {e}"))?;
    let n = list.Size().map_err(|e| e.to_string())?;
    let mut v = Vec::with_capacity(n as usize);
    for i in 0..n {
        let l = list.GetAt(i).map_err(|e| e.to_string())?;
        v.push((l.LanguageTag().map_err(|e| e.to_string())?.to_string(), l.DisplayName().map_err(|e| e.to_string())?.to_string()));
    }
    Ok(v)
}

fn create(req: &EngineRequest) -> Option<OcrEngine> {
    match req {
        EngineRequest::Profile => OcrEngine::TryCreateFromUserProfileLanguages().ok(),
        EngineRequest::Language(tag) => {
            let lang = Language::CreateLanguage(&HSTRING::from(tag.as_str())).ok()?;
            OcrEngine::TryCreateFromLanguage(&lang).ok()
        }
    }
}

/// (tag, ISO 15924 script) of every reader Windows has; the script is "" when Windows names none.
fn installed_scripts() -> Vec<(String, String)> {
    let Ok(list) = OcrEngine::AvailableRecognizerLanguages() else { return Vec::new() };
    let n = list.Size().unwrap_or(0);
    (0..n).filter_map(|i| list.GetAt(i).ok()).filter_map(|l| Some((l.LanguageTag().ok()?.to_string(), l.Script().map(|s| s.to_string()).unwrap_or_default()))).collect()
}

fn installed_tags() -> Vec<String> {
    installed().map(|v| v.into_iter().map(|(t, _)| t).collect()).unwrap_or_default()
}

pub fn engine_for(selection: &str) -> Result<OcrEngine, ReadError> {
    textcore::engine_requests(selection, &installed_tags()).iter().find_map(create).ok_or(ReadError::NoEngine)
}

fn tag_of(engine: &OcrEngine) -> String {
    engine.RecognizerLanguage().and_then(|l| l.LanguageTag()).map(|t| t.to_string()).unwrap_or_default()
}

fn script_of(engine: &OcrEngine) -> String {
    engine.RecognizerLanguage().and_then(|l| l.Script()).map(|t| t.to_string()).unwrap_or_default()
}

/// Which scaler `read` uses. GDI+ is what WinTextCapture's measurements used.
pub static USE_WIC: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// GDI+, loaded by hand when a capture needs it: an import would load it in the tray process too.
fn gdiplus_scale(px: &[u8], sw: u32, sh: u32, w: u32, h: u32) -> Result<Vec<u8>, String> {
    use std::ffi::c_void;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
    use windows::core::{PCSTR, s, w as wz};
    #[repr(C)]
    struct Startup {
        version: u32,
        callback: isize,
        no_thread: i32,
        no_codecs: i32,
    }
    const PIXEL_32BPP_RGB: i32 = 0x0002_2009;
    const PIXEL_32BPP_ARGB: i32 = 0x0026_200A;
    unsafe {
        let lib = LoadLibraryExW(wz!("gdiplus.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).map_err(|e| format!("gdiplus: {e}"))?;
        let get = |name: PCSTR| GetProcAddress(lib, name).ok_or_else(|| format!("gdiplus entry {}", name.to_string().unwrap_or_default()));
        type Bmp = unsafe extern "system" fn(i32, i32, i32, i32, *const u8, *mut *mut c_void) -> i32;
        type Ctx = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32;
        type SetI = unsafe extern "system" fn(*mut c_void, i32) -> i32;
        type Draw = unsafe extern "system" fn(*mut c_void, *mut c_void, i32, i32, i32, i32) -> i32;
        type Del = unsafe extern "system" fn(*mut c_void) -> i32;
        type Start = unsafe extern "system" fn(*mut usize, *const Startup, *mut c_void) -> i32;
        let start: Start = std::mem::transmute(get(s!("GdiplusStartup"))?);
        let bitmap: Bmp = std::mem::transmute(get(s!("GdipCreateBitmapFromScan0"))?);
        let context: Ctx = std::mem::transmute(get(s!("GdipGetImageGraphicsContext"))?);
        let compositing: SetI = std::mem::transmute(get(s!("GdipSetCompositingQuality"))?);
        let interpolation: SetI = std::mem::transmute(get(s!("GdipSetInterpolationMode"))?);
        let offset: SetI = std::mem::transmute(get(s!("GdipSetPixelOffsetMode"))?);
        let smoothing: SetI = std::mem::transmute(get(s!("GdipSetSmoothingMode"))?);
        let draw: Draw = std::mem::transmute(get(s!("GdipDrawImageRectI"))?);
        let del_graphics: Del = std::mem::transmute(get(s!("GdipDeleteGraphics"))?);
        let del_image: Del = std::mem::transmute(get(s!("GdipDisposeImage"))?);
        let mut gdip_cookie = 0usize;
        let input = Startup { version: 1, callback: 0, no_thread: 0, no_codecs: 1 };
        let status = start(&mut gdip_cookie, &input, std::ptr::null_mut());
        if status != 0 {
            return Err(format!("GdiplusStartup status {status}"));
        }
        let mut out = vec![0u8; w as usize * h as usize * 4];
        let (mut src, mut dst, mut g) = (std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
        // RGB source: the frames' alpha means nothing (GDI leaves it at 0).
        let mut status = bitmap(sw as i32, sh as i32, (sw * 4) as i32, PIXEL_32BPP_RGB, px.as_ptr(), &mut src);
        if status == 0 {
            status = bitmap(w as i32, h as i32, (w * 4) as i32, PIXEL_32BPP_ARGB, out.as_mut_ptr() as *const u8, &mut dst);
        }
        if status == 0 {
            status = context(dst, &mut g);
        }
        if status == 0 {
            // WinTextCapture's OcrImagePreprocessor, setting for setting.
            compositing(g, 2); // CompositingQualityHighQuality
            interpolation(g, 7); // InterpolationModeHighQualityBicubic
            offset(g, 2); // PixelOffsetModeHighQuality
            smoothing(g, 3); // SmoothingModeNone
            status = draw(g, src, 0, 0, w as i32, h as i32);
        }
        if !g.is_null() {
            del_graphics(g);
        }
        if !dst.is_null() {
            del_image(dst);
        }
        if !src.is_null() {
            del_image(src);
        }
        // GDI+ stays started: this process exits after the capture.
        if status != 0 {
            return Err(format!("GDI+ draw {sw}x{sh} to {w}x{h}: status {status}"));
        }
        Ok(out)
    }
}

/// Scale `px` (32bpp BGRA) to `w` x `h`.
pub fn resample(px: &[u8], sw: u32, sh: u32, w: u32, h: u32) -> Result<Vec<u8>, String> {
    if USE_WIC.load(std::sync::atomic::Ordering::Relaxed) {
        wic_scale(px, sw, sh, w, h)
    } else {
        gdiplus_scale(px, sw, sh, w, h)
    }
}

/// WIC's high quality cubic scaler.
fn wic_scale(px: &[u8], sw: u32, sh: u32, w: u32, h: u32) -> Result<Vec<u8>, String> {
    ensure_com()?;
    let stride = sw.checked_mul(4).ok_or("source too wide")?;
    let out_stride = w.checked_mul(4).ok_or("target too wide")?;
    let mut out = vec![0u8; out_stride as usize * h as usize];
    unsafe {
        let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(|e| format!("WIC factory: {e}"))?;
        let src = f.CreateBitmapFromMemory(sw, sh, &GUID_WICPixelFormat32bppBGRA, stride, px).map_err(|e| format!("WIC bitmap: {e}"))?;
        let scaler = f.CreateBitmapScaler().map_err(|e| format!("WIC scaler: {e}"))?;
        scaler.Initialize(&src, w, h, WICBitmapInterpolationModeHighQualityCubic).map_err(|e| format!("WIC scale {sw}x{sh} to {w}x{h}: {e}"))?;
        let fmt = scaler.GetPixelFormat().map_err(|e| e.to_string())?;
        if fmt == GUID_WICPixelFormat32bppBGRA {
            scaler.CopyPixels(std::ptr::null(), out_stride, &mut out).map_err(|e| format!("WIC copy: {e}"))?;
        } else {
            let conv = WICConvertBitmapSource(&GUID_WICPixelFormat32bppBGRA, &scaler).map_err(|e| format!("WIC convert: {e}"))?;
            conv.CopyPixels(std::ptr::null(), out_stride, &mut out).map_err(|e| format!("WIC copy: {e}"))?;
        }
    }
    Ok(out)
}

fn bitmap(w: u32, h: u32, px: &[u8]) -> Result<SoftwareBitmap, String> {
    let writer = DataWriter::new().map_err(|e| e.to_string())?;
    writer.WriteBytes(px).map_err(|e| e.to_string())?;
    let buffer = writer.DetachBuffer().map_err(|e| e.to_string())?;
    // Ignore: the frames' alpha means nothing (GDI leaves it at 0).
    SoftwareBitmap::CreateCopyWithAlphaFromBuffer(&buffer, BitmapPixelFormat::Bgra8, w as i32, h as i32, BitmapAlphaMode::Ignore).map_err(|e| format!("SoftwareBitmap {w}x{h}: {e}"))
}

/// One reader over the bitmap: each line's text as Windows joined it, and its words with their boxes.
fn recognize(engine: &OcrEngine, tag: &str, bmp: &SoftwareBitmap) -> Result<textcore::EngineReading, String> {
    let s = |e: windows::core::Error| e.to_string();
    let result = engine.RecognizeAsync(bmp).and_then(|op| op.join()).map_err(|e| format!("RecognizeAsync: {e}"))?;
    let lines = result.Lines().map_err(|e| format!("Lines: {e}"))?;
    let n = lines.Size().map_err(s)?;
    let mut out = textcore::EngineReading { tag: tag.to_string(), script: script_of(engine), lines: Vec::with_capacity(n as usize) };
    for i in 0..n {
        let line = lines.GetAt(i).map_err(s)?;
        let words = line.Words().map_err(s)?;
        let m = words.Size().map_err(s)?;
        let mut v = Vec::with_capacity(m as usize);
        for j in 0..m {
            let word = words.GetAt(j).map_err(s)?;
            let r = word.BoundingRect().map_err(s)?;
            v.push(textcore::OcrWord { text: word.Text().map_err(s)?.to_string(), x: r.X, y: r.Y, w: r.Width, h: r.Height, proof: 0.0 });
        }
        out.lines.push(textcore::OcrLine { text: line.Text().map_err(s)?.to_string(), words: v });
    }
    Ok(out)
}

/// Read the text in a raw crop (32bpp BGRA, top-down). `selection`: "auto" or a language tag.
/// Automatic mode may also ask the installed readers of other scripts (`textcore::second_readers`,
/// `needs_second_look`, `choose_reading`).
pub fn read(w: u32, h: u32, px: &[u8], selection: &str) -> Result<Reading, ReadError> {
    let t0 = Instant::now();
    let installed = installed_scripts();
    let tags: Vec<String> = installed.iter().map(|(t, _)| t.clone()).collect();
    let engine = textcore::engine_requests(selection, &tags).iter().find_map(create).ok_or(ReadError::NoEngine)?;
    let language = tag_of(&engine);
    let seconds = textcore::second_readers(selection, (&language, &script_of(&engine)), &installed);
    read_inner(&engine, &language, &seconds, w, h, px, t0)
}

/// `read` with an engine already made and no second readers (the measurements reuse one).
pub fn read_with(engine: &OcrEngine, language: &str, w: u32, h: u32, px: &[u8], t0: Instant) -> Result<Reading, ReadError> {
    read_inner(engine, language, &[], w, h, px, t0)
}

fn read_inner(engine: &OcrEngine, language: &str, seconds: &[String], w: u32, h: u32, px: &[u8], t0: Instant) -> Result<Reading, ReadError> {
    let fail = |e: String| ReadError::Failed(e);
    let t1 = Instant::now();
    let max = OcrEngine::MaxImageDimension().map_err(|e| fail(format!("MaxImageDimension: {e}")))?;
    let (pw, ph) = textcore::target_size(w, h, max);
    let scaled;
    let prepared: &[u8] = if (pw, ph) == (w, h) {
        px
    } else {
        scaled = resample(px, w, h, pw, ph).map_err(fail)?;
        &scaled
    };
    let t2 = Instant::now();
    let bmp = bitmap(pw, ph, prepared).map_err(fail)?;
    let first = recognize(engine, language, &bmp).map_err(fail)?;
    let t3 = Instant::now();
    // The other scripts' readers only when the first reading gives a reason (`needs_second_look`;
    // the unread ink is measured on the prepared picture the boxes belong to). One that cannot be
    // made or fails only costs its part: the first reading stands.
    let unread = if seconds.is_empty() { 0.0 } else { textcore::unread_ink(pw, ph, prepared, &first.lines.iter().flat_map(|l| &l.words).collect::<Vec<_>>()) };
    let others: Vec<textcore::EngineReading> = if seconds.is_empty() || !textcore::needs_second_look(&first, unread) {
        Vec::new()
    } else {
        seconds.iter().filter_map(|tag| create(&EngineRequest::Language(tag.clone())).and_then(|e| recognize(&e, &tag_of(&e), &bmp).ok())).collect()
    };
    let t4 = Instant::now();
    let (text, used) = textcore::choose_reading(&first, &others);
    let language = std::iter::once(language.to_string()).chain(used.iter().map(|&i| others[i].tag.clone())).collect::<Vec<_>>().join("+");
    let mut readings = vec![first];
    readings.extend(others);
    Ok(Reading { text, language, prepared: (pw, ph), engine_ms: ms(t0, t1), prepare_ms: ms(t1, t2), recognize_ms: ms(t2, t3), second_ms: ms(t3, t4), unread_ink: unread, readings })
}

/// (tag, first make+read ms, median of the next six, text) for every installed reader.
fn time_each_reader(w: u32, h: u32, px: &[u8]) -> Option<Vec<(String, f64, f64, String)>> {
    let max = OcrEngine::MaxImageDimension().ok()?;
    let (pw, ph) = textcore::target_size(w, h, max);
    let scaled = if (pw, ph) == (w, h) { px.to_vec() } else { resample(px, w, h, pw, ph).ok()? };
    let bmp = bitmap(pw, ph, &scaled).ok()?;
    let mut out = Vec::new();
    for tag in installed_tags() {
        let mut times = Vec::new();
        let mut text = String::new();
        for _ in 0..7 {
            let t = Instant::now();
            let Some(e) = create(&EngineRequest::Language(tag.clone())) else { break };
            let Ok(r) = recognize(&e, &tag, &bmp) else { break };
            times.push(ms(t, Instant::now()));
            text = r.text();
        }
        if times.is_empty() {
            continue;
        }
        let first = times[0];
        let mut rest = times[1..].to_vec();
        rest.sort_by(f64::total_cmp);
        out.push((tag, first, rest.get(rest.len() / 2).copied().unwrap_or(first), text));
    }
    Some(out)
}

/// `snip --ocr-file PNG [LANGUAGE]`: read a picture file the way a text capture reads its crop and
/// print every reader's text and the time. No window, no clipboard.
pub fn run_file(args: &[String]) -> i32 {
    let Some(path) = args.first() else {
        println!("usage: snip --ocr-file PNG [auto|TAG]");
        return 2;
    };
    let selection = args.get(1).map(String::as_str).unwrap_or("auto");
    if let Err(e) = ensure_com() {
        println!("FAIL {e}");
        return 1;
    }
    let decoded = std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| crate::png::decode_png(&b).map_err(|e| e.to_string()));
    let (w, h, px) = match decoded {
        Ok(v) => v,
        Err(e) => {
            println!("FAIL {path}: {e}");
            return 1;
        }
    };
    println!("installed: {:?}", installed_scripts());
    match read(w, h, &px, selection) {
        Ok(r) => {
            for x in &r.readings {
                println!("reader {}: {:?}", x.tag, x.text());
            }
            println!("chosen ({}): {:?}", r.language, r.text);
            println!("picture {w}x{h} prepared {}x{}; engine {:.1} ms, prepare {:.1} ms, recognize {:.1} ms, second readers {:.1} ms; unread ink {:.3}", r.prepared.0, r.prepared.1, r.engine_ms, r.prepare_ms, r.recognize_ms, r.second_ms, r.unread_ink);
            // What one more reader costs here: made and run 7 times each, on the same prepared picture.
            if let Some(per) = time_each_reader(w, h, &px) {
                for (tag, first, median, text) in per {
                    println!("cost {tag}: first {first:.1} ms (make + read), median of the next 6 {median:.1} ms; read {text:?}");
                }
            }
            0
        }
        Err(e) => {
            println!("FAIL {e:?}");
            1
        }
    }
}
