//! Two measuring runs that the other self-tests cannot do.
//!
//! `--selftest-perf` (hidden desktop only): how long the press, each move and the release of each
//! drag take on a synthetic 3440x1440 monitor, the first-frame breakdown, and the memory of an open
//! editor. With a case name it runs one drawing session in one device (no extra devices to count):
//! `ultra` flat-out, `ultra-1khz` at a 1000 Hz mouse with the shipped frame limit, `small` a
//! 400x300 selection, `four` and `four-1khz` a four-monitor layout with a selection across them.
//!
//! `--selftest-real` (the visible desktop only, and only after 2 minutes without the user's input): real
//! capture on a still and on a moving screen, the overlay and the text box as the screen actually
//! shows them, the real clipboard. It never asks for the foreground and sends no keys. The clipboard is saved first, in memory only. It comes back with its original formats
//! byte for byte, plus 3 flag formats that keep this test out of the clipboard history and the
//! cloud. Its contents are never printed or written anywhere.

use windows::Win32::Foundation::{HGLOBAL, HINSTANCE, HWND};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use crate::capture::{self, Frame, Method, MonitorInfo, Pixels, Snapshot};
use crate::config::CaptureMethod;
use crate::editor::Prefs;
use crate::geometry::{Layout, Rect};
use crate::log::{ms_since, qpc};
use crate::overlay::{Opts, Overlay};
use crate::theme::{Look, Mode, Theme};
use crate::toolbar::Tool;

fn mb(v: u64) -> f64 {
    v as f64 / 1048576.0
}

fn private_mb() -> f64 {
    mb(crate::app::self_usage().0)
}

fn pump(ms: u32) {
    let t = qpc();
    let mut msg = MSG::default();
    while ms_since(t) < ms as f64 {
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn percentile(v: &[f64], p: f64) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    s.get(((s.len().saturating_sub(1)) as f64 * p).round() as usize).copied().unwrap_or(0.0)
}

fn gpu_snapshot(rects: &[Rect]) -> Result<Snapshot, String> {
    let gpu = capture::create_gpu().map_err(|e| e.to_string())?;
    let mut frames = Vec::new();
    let mut monitors = Vec::new();
    for (i, rect) in rects.iter().enumerate() {
        let (w, h) = (rect.width() as u32, rect.height() as u32);
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                px.extend_from_slice(&[(x % 251) as u8, (y % 241) as u8, ((x ^ y) % 239) as u8, 255]);
            }
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let init = D3D11_SUBRESOURCE_DATA { pSysMem: px.as_ptr() as *const _, SysMemPitch: w * 4, SysMemSlicePitch: 0 };
        let mut tex = None;
        unsafe { gpu.device.CreateTexture2D(&desc, Some(&init), Some(&mut tex)) }.map_err(|e| e.to_string())?;
        frames.push(Frame { width: w, height: h, pixels: Pixels::Gpu(tex.unwrap()), method: Method::Dd, ms: 0.0, zero_present: false, note: String::new() });
        monitors.push(MonitorInfo { hmon: HMONITOR::default(), rect: *rect, work: *rect, name: format!("perf{i}"), dpi: 96, primary: i == 0 });
    }
    Ok(Snapshot { monitors, frames, layout: Layout::new(rects.to_vec()), gpu: Some(gpu), windows: vec![], gpu_ms: 0.0 })
}

fn editor_opts() -> Opts {
    Opts { dim: 0.45, theme: Theme::new(Look::Graphite, Mode::Dark), lang: crate::i18n::Lang::Tr, editor: Some(Prefs::default()), text: false, text_action: false }
}

/// One timed drag: the press, each move, the release.
struct Drag {
    press: f64,
    moves: Vec<f64>,
    release: f64,
}

/// Drag with `moves` mouse moves along `path`. `gap_ms`: 0 flat-out, else a mouse this far apart,
/// with the messages (the frame timer) pumped in between.
fn timed_drag(ov: &mut Overlay, moves: usize, ctrl: bool, gap_ms: f64, path: impl Fn(f64) -> (i32, i32)) -> Drag {
    let (x, y) = path(0.0);
    let t = qpc();
    ov.on_mouse_mods(0, WM_LBUTTONDOWN, x, y, false, ctrl);
    let press = ms_since(t);
    let mut times = Vec::with_capacity(moves);
    for k in 1..=moves {
        let (x, y) = path(k as f64 / moves as f64);
        let t = qpc();
        ov.on_mouse_mods(0, WM_MOUSEMOVE, x, y, false, ctrl);
        times.push(ms_since(t));
        if gap_ms > 0.0 {
            let mut msg = MSG::default();
            while ms_since(t) < gap_ms {
                unsafe {
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            }
        }
    }
    let (x, y) = path(1.0);
    let t = qpc();
    ov.on_mouse_mods(0, WM_LBUTTONUP, x, y, false, ctrl);
    Drag { press, moves: times, release: ms_since(t) }
}

fn report_drag(name: &str, d: &Drag) {
    println!(
        "{name}: press={:.1}ms move p50={:.2} p95={:.2} max={:.2} release={:.1}ms private={:.1}MB",
        d.press,
        percentile(&d.moves, 0.5),
        percentile(&d.moves, 0.95),
        percentile(&d.moves, 1.0),
        d.release,
        private_mb()
    );
}

/// Select `a` to `b`, then arrow, box, pen, a pixelate of nearly the whole selection, arrow.
fn five_shapes(ov: &mut Overlay, a: (i32, i32), b: (i32, i32), gap_ms: f64) {
    ov.on_mouse_mods(0, WM_LBUTTONDOWN, a.0, a.1, false, false);
    ov.on_mouse_mods(0, WM_MOUSEMOVE, (a.0 + b.0) / 2, (a.1 + b.1) / 2, false, false);
    ov.on_mouse_mods(0, WM_LBUTTONUP, b.0, b.1, false, false);
    println!("selection {:?}, nothing drawn: private={:.1}MB", ov.editor.as_ref().and_then(|e| e.sel), private_mb());
    let (l, t) = (a.0 as f64 + 20.0, a.1 as f64 + 20.0);
    let (w, h) = ((b.0 - a.0) as f64 - 40.0, (b.1 - a.1) as f64 - 40.0);
    for (name, tool) in [("arrow", Tool::Arrow), ("box", Tool::Rect), ("pen", Tool::Pen), ("pixelate", Tool::Obscure), ("arrow2", Tool::Arrow)] {
        ov.editor.as_mut().unwrap().tool = Some(tool);
        let moves = if tool == Tool::Pen { 500 } else { 200 };
        let d = timed_drag(ov, moves, false, gap_ms, |k| {
            if tool == Tool::Pen {
                let turn = k * 40.0;
                ((l + w / 2.0 + w / 3.0 * k * turn.cos()) as i32, (t + h / 2.0 + h / 3.0 * k * turn.sin()) as i32)
            } else {
                ((l + w * k) as i32, (t + h * k) as i32)
            }
        });
        report_drag(name, &d);
    }
    println!("5 shapes: private={:.1}MB", private_mb());
    pump(1000);
    println!("1 s later: private={:.1}MB", private_mb());
    let t = qpc();
    let res = ov.editor_result();
    println!("final image: {:.1}ms ok={}", ms_since(t), res.is_some());
}

/// One drawing session in one device.
fn perf_case(case: &str, hinst: HINSTANCE) -> Result<(), String> {
    let ultra = [Rect::from_xywh(0, 0, 3440, 1440)];
    let four = [Rect::from_xywh(-1920, 0, 1920, 1080), Rect::from_xywh(-606, -1440, 3440, 1440), Rect::from_xywh(0, 0, 2560, 1440), Rect::from_xywh(2560, 0, 2560, 1440)];
    let (rects, a, b, gap): (&[Rect], _, _, f64) = match case {
        "ultra" => (&ultra, (20, 20), (3420, 1420), 0.0),
        "ultra-1khz" => (&ultra, (20, 20), (3420, 1420), 1.0),
        "small" => (&ultra, (1000, 500), (1400, 800), 0.0),
        // Window 0 is at -1920,0: from the top monitor's corner to the far right of the bottom row.
        "four" => (&four, (1314, -1440), (7039, 1439), 0.0),
        "four-1khz" => (&four, (1314, -1440), (7039, 1439), 1.0),
        _ => return Err(format!("unknown case {case}")),
    };
    let snap = gpu_snapshot(rects)?;
    println!("device and frames ready: private={:.1}MB", private_mb());
    let mut ov = Overlay::new(snap, hinst, &editor_opts()).map_err(|e| e.to_string())?;
    ov.no_throttle = gap == 0.0;
    ov.show_passive().map_err(|e| e.to_string())?;
    println!("overlay shown: private={:.1}MB", private_mb());
    pump(100);
    five_shapes(&mut ov, a, b, gap);
    Ok(())
}

pub fn run_perf(case: Option<&str>) -> i32 {
    let desk = crate::selftest::desktop_name();
    println!("desktop: {desk:?}");
    if desk.is_empty() || desk.eq_ignore_ascii_case("Default") || desk.eq_ignore_ascii_case("Winlogon") {
        println!("REFUSED: run it on a hidden desktop (not the visible one)");
        return 4;
    }
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
    let hinst: HINSTANCE = unsafe { GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default() };
    if let Err(e) = crate::app::register_classes(hinst, crate::app::MAIN_CLASS_TEST) {
        println!("FAIL register classes: {e}");
        return 1;
    }
    println!("before device: private={:.1}MB", private_mb());
    if let Some(case) = case {
        let device = capture::create_gpu().map(|_| private_mb());
        println!("a device alone: private={:.1}MB", device.unwrap_or(0.0));
        pump(50);
        return match perf_case(case, hinst) {
            Ok(()) => {
                println!("selftest-perf {case}: done");
                0
            }
            Err(e) => {
                println!("FAIL {case}: {e}");
                1
            }
        };
    }
    let rect = Rect::from_xywh(0, 0, 3440, 1440);
    for round in 0..3 {
        let snap = match gpu_snapshot(&[rect]) {
            Ok(s) => s,
            Err(e) => {
                println!("FAIL snapshot: {e}");
                return 1;
            }
        };
        println!("round {round}: device and frame ready: private={:.1}MB", private_mb());
        let t = qpc();
        let mut ov = match Overlay::new(snap, hinst, &editor_opts()) {
            Ok(o) => o,
            Err(e) => {
                println!("FAIL overlay: {e}");
                return 1;
            }
        };
        let setup = ms_since(t);
        ov.no_throttle = true;
        let t = qpc();
        let shown = ov.show_passive();
        let first = ms_since(t);
        let (a, b, c) = ov.show_ms;
        println!("round {round}: setup={setup:.1}ms first_frame={first:.1}ms (draw+present={a:.1} show={b:.1} activate={c:.1}) err={:?} private={:.1}MB", shown.err(), private_mb());
        if round < 2 {
            continue;
        }
        pump(100);
        let select = timed_drag(&mut ov, 200, false, 0.0, |k| (20 + (k * 3400.0) as i32, 20 + (k * 1400.0) as i32));
        report_drag("select the whole monitor", &select);
        let ed = ov.editor.as_mut().unwrap();
        ed.tool = None;
        let empty_move = timed_drag(&mut ov, 200, true, 0.0, |k| (1700, 700 - (k * 60.0) as i32));
        report_drag("ctrl-move, nothing drawn", &empty_move);
        // Back to a full selection for the drawing runs.
        let ed = ov.editor.as_mut().unwrap();
        ed.tool = Some(Tool::Arrow);
        let arrow = timed_drag(&mut ov, 300, false, 0.0, |k| (100 + (k * 3000.0) as i32, 100 + (k * 1100.0) as i32));
        report_drag("arrow across the monitor", &arrow);
        let ed = ov.editor.as_mut().unwrap();
        ed.tool = Some(Tool::Rect);
        let boxed = timed_drag(&mut ov, 300, false, 0.0, |k| (200 + (k * 2800.0) as i32, 150 + (k * 1000.0) as i32));
        report_drag("box across the monitor", &boxed);
        let ed = ov.editor.as_mut().unwrap();
        ed.tool = Some(Tool::Pen);
        let pen = timed_drag(&mut ov, 1000, false, 0.0, |k| {
            let a = k * 60.0;
            ((1700.0 + (200.0 + 1200.0 * k) * a.cos()) as i32, (700.0 + (100.0 + 500.0 * k) * a.sin()) as i32)
        });
        report_drag("long pen stroke (1000 moves)", &pen);
        let ed = ov.editor.as_mut().unwrap();
        ed.tool = Some(Tool::Obscure);
        let obscure = timed_drag(&mut ov, 200, false, 0.0, |k| (300 + (k * 1200.0) as i32, 300 + (k * 600.0) as i32));
        report_drag("pixelate area", &obscure);
        let ed = ov.editor.as_mut().unwrap();
        ed.tool = None;
        let full_move = timed_drag(&mut ov, 100, true, 0.0, |k| (1700, 700 + (k * 60.0) as i32));
        report_drag("ctrl-move with 4 shapes", &full_move);
        println!("editor open, 5 shapes, 3440x1440 selection: private={:.1}MB", private_mb());
        pump(1000);
        println!("after 1 s at rest: private={:.1}MB", private_mb());
        let t = qpc();
        let res = ov.editor_result();
        println!("final image: {:.1}ms ok={}", ms_since(t), res.is_some());
    }
    println!("selftest-perf: done");
    0
}

fn idle_seconds() -> f64 {
    let mut li = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    unsafe {
        let _ = GetLastInputInfo(&mut li);
        GetTickCount().wrapping_sub(li.dwTime) as f64 / 1000.0
    }
}

fn near(p: &[u8], c: crate::raster::Rgba, tol: i32) -> bool {
    (p[0] as i32 - c.b as i32).abs() <= tol && (p[1] as i32 - c.g as i32).abs() <= tol && (p[2] as i32 - c.r as i32).abs() <= tol
}

/// What the screen shows right now on monitor `m`. Through WGC: this test runs after 2 idle minutes,
/// and on a still screen Desktop Duplication gives no frame.
fn screen_of(m: &MonitorInfo) -> Result<Vec<u8>, String> {
    let gpu = capture::create_gpu().map_err(|e| e.to_string())?;
    let tex = capture::capture_wgc(&gpu, m.hmon).map_err(|e| e.to_string())?;
    crate::probe::readback(&gpu, &tex, m.rect.width() as u32, m.rect.height() as u32).ok_or_else(|| "readback failed".to_string())
}

/// `freeze` while something moves on every monitor (a small window of ours, never activated): the
/// path where Desktop Duplication has frames to give. A still screen never reaches it.
fn moving_screen_rounds(monitors: &[MonitorInfo], hinst: HINSTANCE) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let stop = Arc::new(AtomicBool::new(false));
    let at: Vec<(i32, i32)> = monitors.iter().map(|m| (m.rect.left + 40, m.rect.top + 40)).collect();
    let raw_hinst = hinst.0 as isize;
    let mover = {
        let stop = stop.clone();
        std::thread::spawn(move || unsafe {
            let hinst = HINSTANCE(raw_hinst as *mut _);
            let style = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST;
            let wins: Vec<HWND> = at.iter().filter_map(|&(x, y)| CreateWindowExW(style, crate::app::MAIN_CLASS_TEST, w!("snip moving test"), WS_POPUP, x, y, 48, 48, None, None, Some(hinst), None).ok()).collect();
            for &h in &wins {
                let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
            }
            let mut step = 0;
            let mut msg = MSG::default();
            while !stop.load(Ordering::Relaxed) {
                step = (step + 1) % 8;
                for (&h, &(x, y)) in wins.iter().zip(&at) {
                    let _ = SetWindowPos(h, Some(HWND_TOPMOST), x + step, y, 48, 48, SWP_NOACTIVATE);
                }
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
                std::thread::sleep(std::time::Duration::from_millis(3));
            }
            for h in wins {
                let _ = DestroyWindow(h);
            }
        })
    };
    pump(200);
    for round in 0..3 {
        let t = qpc();
        let snap = capture::freeze(CaptureMethod::Auto, None);
        let ms = ms_since(t);
        println!("moving screen, round {round}: freeze={ms:.1}ms | {}", snap.summary());
        let dd = snap.frames.iter().filter(|f| f.method == Method::Dd).count();
        let lit = snap.monitors.iter().all(|m| snap.crop(&m.rect).is_ok_and(|(_, _, px)| capture::mean_luma(&px) > 1.0));
        // Displays powered off (for example after 15 idle minutes) give no duplication frame at all.
        let verdict = if !lit { "FAIL" } else if dd > 0 { "PASS" } else { "NO ANSWER (displays off?)" };
        println!("{verdict} moving-screen-{round}: {dd} of {} monitors by Desktop Duplication, none black: {lit}", snap.frames.len());
    }
    stop.store(true, Ordering::Relaxed);
    let _ = mover.join();
}

fn count_in(px: &[u8], m: &MonitorInfo, r: Rect, mut f: impl FnMut(&[u8]) -> bool) -> (u64, u64) {
    let stride = m.rect.width() as usize * 4;
    let (mut hit, mut all) = (0, 0);
    for y in r.top.max(m.rect.top)..r.bottom.min(m.rect.bottom) {
        let row = (y - m.rect.top) as usize * stride;
        for x in r.left.max(m.rect.left)..r.right.min(m.rect.right) {
            let at = row + (x - m.rect.left) as usize * 4;
            all += 1;
            if f(&px[at..at + 4]) {
                hit += 1;
            }
        }
    }
    (hit, all)
}

/// The overlay and its text box on the real screen, in a small area of the primary monitor.
fn text_box_on_screen(hinst: HINSTANCE, m: &MonitorInfo) -> Result<(), String> {
    let (w, h) = (500, 300);
    let rect = Rect::from_xywh(m.rect.right - w - 60, m.rect.bottom - h - 60, w, h);
    let green = crate::raster::Rgba::rgb(10, 170, 60);
    let gpu = capture::create_gpu().map_err(|e| e.to_string())?;
    let mut frame = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        frame.extend_from_slice(&[green.b, green.g, green.r, 255]);
    }
    let snap = Snapshot {
        monitors: vec![MonitorInfo { hmon: m.hmon, rect, work: rect, name: "text-box-test".into(), dpi: m.dpi, primary: true }],
        frames: vec![Frame { width: w as u32, height: h as u32, pixels: Pixels::Cpu(frame), method: Method::Gdi, ms: 0.0, zero_present: false, note: String::new() }],
        layout: Layout::new(vec![rect]),
        gpu: Some(gpu),
        windows: vec![],
        gpu_ms: 0.0,
    };
    let opts = editor_opts();
    let theme = opts.theme;
    let mut ov = Overlay::new(snap, hinst, &opts).map_err(|e| e.to_string())?;
    let fg = unsafe { GetForegroundWindow() };
    ov.show_passive().map_err(|e| e.to_string())?;
    pump(150);
    ov.on_mouse_mods(0, WM_LBUTTONDOWN, 10, 10, false, false);
    ov.on_mouse_mods(0, WM_MOUSEMOVE, 480, 280, false, false);
    ov.on_mouse_mods(0, WM_LBUTTONUP, 480, 280, false, false);
    ov.editor.as_mut().unwrap().tool = Some(Tool::Text);
    ov.on_mouse_mods(0, WM_LBUTTONDOWN, 60, 60, false, false);
    ov.on_mouse_mods(0, WM_LBUTTONUP, 60, 60, false, false);
    let (edit, host) = ov.text_windows().ok_or("no text box")?;
    unsafe {
        SetWindowTextW(edit, w!("Deneme \u{0131}\u{011F}\u{00FC}\u{015F}")).map_err(|e| e.to_string())?;
    }
    pump(300);
    let box_rect = crate::overlay::window_rect(host);
    let shot = screen_of(m)?;
    let (bg, area) = count_in(&shot, m, box_rect, |p| near(p, theme.bg, 12));
    let (border, _) = count_in(&shot, m, box_rect, |p| near(p, theme.accent, 16));
    let (ink, _) = count_in(&shot, m, box_rect, |p| !near(p, theme.bg, 40) && !near(p, theme.accent, 40));
    let (under, under_area) = count_in(&shot, m, Rect::new(rect.left + 20, rect.top + 180, rect.right - 20, rect.bottom - 70), |p| near(p, green, 16));
    println!(
        "text-box: box={box_rect:?} area={area} bg={bg} border={border} ink={ink} | swap-chain frame under it: {under}/{under_area} green | foreground unchanged={}",
        fg == unsafe { GetForegroundWindow() }
    );
    let shown = area > 0 && bg * 2 > area && ink > 20;
    println!("{} text-box-visible: the screen shows the box with its background and text", if shown { "PASS" } else { "FAIL" });
    println!("{} overlay-frame-visible: the screen shows the flip-model frame", if under * 2 > under_area { "PASS" } else { "FAIL" });
    drop(ov);
    pump(100);
    Ok(())
}

fn global(bytes: &[u8]) -> Result<HGLOBAL, String> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).map_err(|e| e.to_string())?;
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            return Err("GlobalLock".into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(h);
        Ok(h)
    }
}

fn read_format(fmt: u32) -> Option<Vec<u8>> {
    unsafe {
        let h = GetClipboardData(fmt).ok()?;
        let g = HGLOBAL(h.0);
        let n = GlobalSize(g);
        if n == 0 {
            return None;
        }
        let p = GlobalLock(g) as *const u8;
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p, n).to_vec();
        let _ = GlobalUnlock(g);
        Some(v)
    }
}

/// CF_BITMAP, CF_METAFILEPICT, CF_PALETTE, CF_ENHMETAFILE, owner-display and GDI-object formats
/// are not plain memory: a backup could not restore them.
fn not_memory(fmt: u32) -> bool {
    matches!(fmt, 2 | 3 | 9 | 14 | 0x80 | 0x82 | 0x83 | 0x8E) || (0x300..=0x3FF).contains(&fmt)
}

fn no_history_formats() -> Vec<(u32, Vec<u8>)> {
    unsafe {
        vec![
            (RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")), vec![0u8; 4]),
            (RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")), vec![0u8; 4]),
            (RegisterClipboardFormatW(w!("CanUploadToCloudClipboard")), vec![0u8; 4]),
        ]
    }
}

fn clipboard_roundtrip(owner: HWND) -> Result<(), String> {
    // 1. Save. A format that cannot be saved exactly stops the test before anything changes.
    crate::clipboard::open(owner).map_err(|e| e.to_string())?;
    let mut saved: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut unsaved = Vec::new();
    let mut fmt = 0;
    let has_dib = unsafe { IsClipboardFormatAvailable(8).is_ok() || IsClipboardFormatAvailable(17).is_ok() };
    loop {
        fmt = unsafe { EnumClipboardFormats(fmt) };
        if fmt == 0 {
            break;
        }
        if fmt == 2 && has_dib {
            continue; // made by Windows from the DIB, comes back by itself
        }
        if not_memory(fmt) {
            unsaved.push(fmt);
            continue;
        }
        match read_format(fmt) {
            Some(v) => saved.push((fmt, v)),
            None => unsaved.push(fmt),
        }
    }
    let total: usize = saved.iter().map(|(_, v)| v.len()).sum();
    unsafe {
        let _ = CloseClipboard();
    }
    if !unsaved.is_empty() {
        println!("SKIP clipboard: {} format(s) cannot be saved exactly ({unsaved:?}); it stayed untouched", unsaved.len());
        return Ok(());
    }
    println!("clipboard saved in memory: {} formats, {total} bytes", saved.len());

    // 2. The real write, kept out of the clipboard history.
    let (w, h) = (640u32, 360u32);
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            px.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255]);
        }
    }
    let t = qpc();
    let wrote = crate::clipboard::set_image_with(owner, w, h, &px, &no_history_formats());
    let write_ms = ms_since(t);
    let check = (|| -> Result<(), String> {
        wrote.map_err(|e| e.to_string())?;
        crate::clipboard::open(owner).map_err(|e| e.to_string())?;
        let v5 = read_format(17);
        let dib = read_format(8);
        // A program that asks for a plain bitmap gets one Windows makes from the DIB.
        let bitmap = unsafe { GetClipboardData(2).is_ok_and(|h| !h.is_invalid()) };
        unsafe {
            let _ = CloseClipboard();
        }
        let want5 = crate::dib::dibv5_bytes(w, h, &px);
        let want = crate::dib::dib24_bytes(w, h, &px);
        // GlobalSize may round up; the image itself must be exact.
        let ok5 = v5.as_ref().is_some_and(|v| v.len() >= want5.len() && v[..want5.len()] == want5[..]);
        let ok = dib.as_ref().is_some_and(|v| v.len() >= want.len() && v[..want.len()] == want[..]);
        println!("{} clipboard-image: CF_DIBV5 exact={ok5} CF_DIB exact={ok} CF_BITMAP made={bitmap}; write took {write_ms:.1}ms", if ok5 && ok && bitmap { "PASS" } else { "FAIL" });
        Ok(())
    })();

    // 3. Restore, whatever happened above, then compare byte for byte.
    crate::clipboard::open(owner).map_err(|e| format!("RESTORE FAILED, clipboard open: {e}"))?;
    let mut restore_ok = true;
    unsafe {
        let _ = EmptyClipboard();
        for (fmt, v) in saved.iter().chain(no_history_formats().iter()) {
            match global(v) {
                Ok(g) => {
                    if SetClipboardData(*fmt, Some(windows::Win32::Foundation::HANDLE(g.0))).is_err() {
                        restore_ok = false;
                    }
                }
                Err(_) => restore_ok = false,
            }
        }
        let _ = CloseClipboard();
    }
    crate::clipboard::open(owner).map_err(|e| format!("RESTORE CHECK FAILED, clipboard open: {e}"))?;
    let same = saved.iter().all(|(fmt, v)| read_format(*fmt).is_some_and(|now| now.len() >= v.len() && now[..v.len()] == v[..]));
    unsafe {
        let _ = CloseClipboard();
    }
    println!("{} clipboard-restored: {} formats back, byte-exact={same}", if restore_ok && same { "PASS" } else { "FAIL" }, saved.len());
    check
}

pub fn run_real() -> i32 {
    let desk = crate::selftest::desktop_name();
    println!("desktop: {desk:?}");
    if !desk.eq_ignore_ascii_case("Default") {
        println!("REFUSED: this run measures the visible desktop");
        return 4;
    }
    let idle = idle_seconds();
    println!("idle: {idle:.0} s");
    if idle < 120.0 {
        println!("REFUSED: he used the PC in the last 2 minutes");
        return 5;
    }
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
    let hinst: HINSTANCE = unsafe { GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default() };
    if let Err(e) = crate::app::register_classes(hinst, crate::app::MAIN_CLASS_TEST) {
        println!("FAIL register classes: {e}");
        return 1;
    }
    println!("{} clipboard-guard: the visible desktop may write the clipboard", if crate::clipboard::on_a_hidden_desktop_of_his_station() { "FAIL" } else { "PASS" });
    for (name, vk) in [("PrtScn", 0x2Cu32), ("Pause", 0x13)] {
        let ok = unsafe { RegisterHotKey(None, 91, MOD_NOREPEAT, vk) }.is_ok();
        if ok {
            unsafe {
                let _ = UnregisterHotKey(None, 91);
            }
        }
        println!("hotkey {name}: {}", if ok { "free" } else { "held by another program" });
    }
    let fg = unsafe { GetForegroundWindow() };
    let mut brightness: Vec<f64> = Vec::new();
    for round in 0..3 {
        let t = qpc();
        let snap = capture::freeze(CaptureMethod::Auto, None);
        let freeze_ms = ms_since(t);
        println!("round {round}: freeze={freeze_ms:.1}ms private={:.1}MB | {}", private_mb(), snap.summary());
        if round == 0 {
            for m in snap.monitors.clone() {
                let t = qpc();
                match snap.crop(&m.rect) {
                    Ok((w, h, px)) => {
                        let l = capture::mean_luma(&px);
                        brightness.push(l);
                        println!("{} capture-{}: {w}x{h} mean brightness {l:.1} ({:.1}ms)", if l > 1.0 { "PASS" } else { "FAIL (black)" }, m.name, ms_since(t));
                    }
                    Err(e) => println!("FAIL capture-{}: {e}", m.name),
                }
            }
        }
        let monitors = snap.monitors.clone();
        let opts = Opts { editor: None, ..editor_opts() };
        let t = qpc();
        let mut ov = match Overlay::new(snap, hinst, &opts) {
            Ok(o) => o,
            Err(e) => {
                println!("FAIL overlay: {e}");
                return 1;
            }
        };
        let setup = ms_since(t);
        let t = qpc();
        let shown = ov.show_passive();
        let first = ms_since(t);
        let (a, b, _) = ov.show_ms;
        println!("round {round}: setup={setup:.1}ms first_frame={first:.1}ms (draw+present={a:.1} show={b:.1}) err={:?} private={:.1}MB", shown.err(), private_mb());
        pump(150);
        if round == 0
            && let Some(m) = monitors.first()
        {
            match screen_of(m) {
                Ok(px) => {
                    let dimmed = capture::mean_luma(&px);
                    println!("{} overlay-on-screen: {} brightness {:.1} before, {dimmed:.1} under the dim", if dimmed < brightness[0] * 0.85 { "PASS" } else { "FAIL" }, m.name, brightness[0]);
                }
                Err(e) => println!("FAIL overlay-on-screen: {e}"),
            }
        }
        drop(ov);
        pump(100);
    }
    println!("after the overlays: private={:.1}MB", private_mb());
    let primary = capture::monitors().into_iter().find(|m| m.primary);
    if let Some(m) = primary
        && let Err(e) = text_box_on_screen(hinst, &m)
    {
        println!("FAIL text-box: {e}");
    }
    moving_screen_rounds(&capture::monitors(), hinst);
    let owner = unsafe { CreateWindowExW(WS_EX_TOOLWINDOW, crate::app::MAIN_CLASS_TEST, w!("snip clipboard test"), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst), None) };
    match owner {
        Ok(h) => {
            if let Err(e) = clipboard_roundtrip(h) {
                println!("FAIL clipboard: {e}");
            }
            unsafe {
                let _ = DestroyWindow(h);
            }
        }
        Err(e) => println!("FAIL clipboard owner: {e}"),
    }
    println!("{} no-focus-theft: foreground window unchanged across the whole run", if fg == unsafe { GetForegroundWindow() } { "PASS" } else { "FAIL" });
    println!("selftest-real: done");
    0
}
