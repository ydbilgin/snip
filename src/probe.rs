//! `snip probe`: the P0 measurements (REPORT.txt section 1.10), written to a log.
//!
//! Default (non-interactive): no window, no focus change, no clipboard, no hook, no hotkey.
//!   monitors and HDR, D3D11 device creation time, Desktop Duplication and GDI capture time per
//!   monitor, first-frame behaviour, black-frame check, freeze-all time, crop + PNG + DIB time.
//!   It refuses to run while a full-screen window other than the desktop is in front (he may be
//!   gaming) unless --live is given.
//! --wgc adds Windows.Graphics.Capture timings and the yellow-border check (the border is visible
//!   on the user's screen for a moment, so it is not in the default set).
//! --live adds everything that needs a person at the desk: RegisterHotKey results for Win+Shift+S and
//!   PrtScn with a control key, the overlay setup cost (windows appear for a moment), and a
//!   pass-through keyboard hook that measures the callback cost while he types (--hook-seconds).
//! --save-images writes one PNG per monitor and method next to the log (the screen contents).

use std::path::PathBuf;

use windows::Win32::Foundation::{CloseHandle, HWND, MAX_PATH, RECT};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, SetPriorityClass,
};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PWSTR;

use crate::capture::{self, Method, Pixels};
use crate::config::CaptureMethod;
use crate::geometry::Rect;
use crate::log::{ms_since, qpc};
use crate::logf;

#[derive(Clone, Debug)]
pub struct ProbeOpts {
    pub out: PathBuf,
    pub runs: u32,
    pub wgc: bool,
    pub live: bool,
    pub save_images: bool,
    pub hook_seconds: u32,
}

fn stats(v: &[f64]) -> String {
    if v.is_empty() {
        return "n=0".into();
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p = |q: f64| s[((s.len() - 1) as f64 * q).round() as usize];
    format!("n={} min={:.2} median={:.2} p95={:.2} max={:.2} ms", s.len(), s[0], p(0.5), p(0.95), s[s.len() - 1])
}

/// Foreground window: (exe name, covers its whole monitor, is the desktop/shell).
fn foreground() -> (String, bool, bool) {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() {
            return ("<none>".into(), false, true);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        let mut exe = String::from("?");
        if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            let mut buf = [0u16; MAX_PATH as usize];
            let mut n = buf.len() as u32;
            if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n).is_ok() {
                exe = String::from_utf16_lossy(&buf[..n as usize]);
            }
            let _ = CloseHandle(h);
        }
        let mut class = [0u16; 64];
        let n = GetClassNameW(fg, &mut class) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        let shell = class == "Progman" || class == "WorkerW" || class == "Shell_TrayWnd";
        let mut r = RECT::default();
        let _ = GetWindowRect(fg, &mut r);
        let fr = Rect::new(r.left, r.top, r.right, r.bottom);
        let full = capture::monitors().iter().any(|m| fr.left <= m.rect.left && fr.top <= m.rect.top && fr.right >= m.rect.right && fr.bottom >= m.rect.bottom);
        (format!("{exe} class={class}"), full, shell)
    }
}

pub(crate) fn readback(snap_gpu: &capture::Gpu, tex: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D, w: u32, h: u32) -> Option<Vec<u8>> {
    let mon = crate::geometry::Rect::from_xywh(0, 0, w as i32, h as i32);
    let snap = capture::Snapshot {
        monitors: vec![capture::MonitorInfo { hmon: Default::default(), rect: mon, work: mon, name: String::new(), dpi: 96, primary: true }],
        layout: crate::geometry::Layout::new(vec![mon]),
        frames: vec![capture::Frame { width: w, height: h, pixels: Pixels::Gpu(tex.clone()), method: Method::Dd, ms: 0.0, zero_present: false, note: String::new() }],
        gpu: Some(capture::Gpu { device: snap_gpu.device.clone(), context: snap_gpu.context.clone(), adapter: snap_gpu.adapter.clone(), factory: snap_gpu.factory.clone(), warp: snap_gpu.warp }),
        windows: vec![],
        gpu_ms: 0.0,
    };
    snap.crop(&mon).ok().map(|(_, _, v)| v)
}

fn save(opts: &ProbeOpts, name: &str, w: u32, h: u32, px: &[u8]) {
    if !opts.save_images {
        return;
    }
    if let Ok(b) = crate::png::encode_png(w, h, px) {
        let _ = std::fs::write(opts.out.join(name), b);
    }
}

/// Share of the outermost 2-pixel ring that looks like the WGC border (yellow-orange).
fn border_share(w: u32, h: u32, px: &[u8]) -> f64 {
    let (w, h) = (w as usize, h as usize);
    let (mut hit, mut n) = (0u64, 0u64);
    let mut look = |x: usize, y: usize| {
        let p = &px[(y * w + x) * 4..(y * w + x) * 4 + 4];
        let (b, g, r) = (p[0] as i32, p[1] as i32, p[2] as i32);
        n += 1;
        if r > 200 && g > 140 && b < 90 {
            hit += 1;
        }
    };
    for x in (0..w).step_by(4) {
        look(x, 0);
        look(x, 1);
        look(x, h - 1);
        look(x, h - 2);
    }
    for y in (0..h).step_by(4) {
        look(0, y);
        look(1, y);
        look(w - 1, y);
        look(w - 2, y);
    }
    if n == 0 { 0.0 } else { hit as f64 / n as f64 }
}

pub fn run(opts: ProbeOpts) -> i32 {
    unsafe {
        let _ = SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let _ = std::fs::create_dir_all(&opts.out);
    let stamp = crate::app::local_stamp();
    let log = opts.out.join(format!(
        "probe-{:04}{:02}{:02}-{:02}{:02}{:02}.log",
        stamp.year, stamp.month, stamp.day, stamp.hour, stamp.minute, stamp.second
    ));
    crate::log::init(Some(log.clone()), true);
    logf!("snip probe {} runs={} wgc={} live={} save_images={} log={}", env!("CARGO_PKG_VERSION"), opts.runs, opts.wgc, opts.live, opts.save_images, log.display());

    let (fg, full, shell) = foreground();
    logf!("foreground: {fg} fullscreen={full} shell={shell}");
    if full && !shell && !opts.live {
        logf!("DEFERRED: a full-screen window is in front (he may be gaming); run again later, or with --live at the desk");
        return 75;
    }

    // Monitors.
    let mons = capture::monitors();
    if mons.is_empty() {
        logf!("FAIL: no monitors enumerated");
        return 1;
    }
    for m in &mons {
        logf!("monitor {} rect={:?} {}x{} dpi={} primary={}", m.name, m.rect, m.rect.width(), m.rect.height(), m.dpi, m.primary);
    }

    // Device creation.
    let mut dev_ms = Vec::new();
    for _ in 0..opts.runs {
        let t = qpc();
        let g = capture::create_gpu();
        dev_ms.push(ms_since(t));
        drop(g);
    }
    logf!("d3d11 device create: {}", stats(&dev_ms));
    let gpu = match capture::create_gpu() {
        Ok(g) => g,
        Err(e) => {
            logf!("FAIL: no D3D11 device: {e}");
            return 1;
        }
    };
    unsafe {
        if let Ok(d) = gpu.adapter.GetDesc1() {
            let n = d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len());
            logf!("adapter: {} warp={}", String::from_utf16_lossy(&d.Description[..n]), gpu.warp);
        }
    }
    let outputs = gpu.outputs();
    for (h, hdr, bits) in capture::hdr_outputs(&gpu) {
        let name = mons.iter().find(|m| m.hmon == h).map(|m| m.name.clone()).unwrap_or_default();
        logf!("output {name}: hdr={hdr} bits_per_color={bits}");
    }
    logf!("outputs on the device's adapter: {} of {} monitors", outputs.len(), mons.len());

    let mut methods = vec![Method::Dd, Method::Gdi];
    if opts.wgc || opts.live {
        methods.push(Method::Wgc);
    }
    for m in &mons {
        for &meth in &methods {
            let mut times = Vec::new();
            let mut zero = 0;
            let mut errors = Vec::new();
            let mut first: Option<(u32, u32, Vec<u8>)> = None;
            for _ in 0..opts.runs {
                let t = qpc();
                let r: Result<(Option<Vec<u8>>, bool), String> = match meth {
                    Method::Dd => match outputs.iter().find(|(h, _)| *h == m.hmon) {
                        Some((_, o)) => capture::capture_dd(&gpu, o).map(|(tex, z)| (if first.is_none() { readback(&gpu, &tex, m.rect.width() as u32, m.rect.height() as u32) } else { None }, z)).map_err(|e| format!("{:#010x} {}", e.code().0, e.message())),
                        None => Err("output not on this adapter".into()),
                    },
                    Method::Wgc => capture::capture_wgc(&gpu, m.hmon)
                        .map(|tex| (if first.is_none() { readback(&gpu, &tex, m.rect.width() as u32, m.rect.height() as u32) } else { None }, false))
                        .map_err(|e| format!("{:#010x} {}", e.code().0, e.message())),
                    Method::Gdi => capture::capture_gdi(m.rect).map(|v| (Some(v), false)).map_err(|e| format!("{:#010x} {}", e.code().0, e.message())),
                };
                let ms = ms_since(t);
                match r {
                    Ok((px, z)) => {
                        times.push(ms);
                        if z {
                            zero += 1;
                        }
                        if first.is_none()
                            && let Some(px) = px {
                                first = Some((m.rect.width() as u32, m.rect.height() as u32, px));
                            }
                    }
                    Err(e) => errors.push(e),
                }
            }
            let luma = first.as_ref().map(|(_, _, p)| capture::mean_luma(p));
            logf!(
                "capture {} {}: {} zero_present={} mean_luma={} errors={}{}",
                m.name,
                meth.name(),
                stats(&times),
                zero,
                luma.map(|l| format!("{l:.1}{}", if l < 2.0 { " (BLACK?)" } else { "" })).unwrap_or("-".into()),
                errors.len(),
                errors.first().map(|e| format!(" first_error={e}")).unwrap_or_default()
            );
            if let Some((w, h, px)) = &first {
                save(&opts, &format!("{}-{}.png", m.name.trim_start_matches("\\\\.\\"), meth.name()), *w, *h, px);
            }
        }
    }

    // The whole freeze as the app does it (device + all monitors + window list).
    let mut freeze_ms = Vec::new();
    let mut summary = String::new();
    for _ in 0..opts.runs {
        let t = qpc();
        let snap = capture::freeze(if opts.wgc || opts.live { CaptureMethod::Auto } else { CaptureMethod::Dd }, None);
        freeze_ms.push(ms_since(t));
        summary = snap.summary();
    }
    logf!("freeze all monitors (as the app does, new device each time): {}", stats(&freeze_ms));
    let mut warm_ms = Vec::new();
    for _ in 0..opts.runs {
        let t = qpc();
        let snap = capture::freeze(if opts.wgc || opts.live { CaptureMethod::Auto } else { CaptureMethod::Dd }, Some(&gpu));
        warm_ms.push(ms_since(t));
        drop(snap);
    }
    logf!("freeze all monitors with a warm device (keep_gpu_device = true): {}", stats(&warm_ms));
    logf!("last freeze: {summary}");

    // Commit costs on a 2560x1440 (or smaller) area of the primary.
    let snap = capture::freeze(CaptureMethod::Dd, None);
    for window in &snap.windows {
        logf!("window hwnd={} rect={:?}", window.hwnd, window.rect);
    }
    let prim = snap.monitors.iter().find(|m| m.primary).map(|m| m.rect).unwrap_or(snap.layout.bounds());
    let sel = Rect::from_xywh(prim.left, prim.top, prim.width().min(2560), prim.height().min(1440));
    let t = qpc();
    match snap.crop(&sel) {
        Ok((w, h, px)) => {
            let crop_ms = ms_since(t);
            let t = qpc();
            let v5 = crate::dib::dibv5_bytes(w, h, &px);
            let d = crate::dib::dib24_bytes(w, h, &px);
            let dib_ms = ms_since(t);
            let t = qpc();
            let png = crate::png::encode_png(w, h, &px);
            let png_ms = ms_since(t);
            logf!(
                "commit {}x{}: crop={:.2}ms dib_build={:.2}ms ({} + {} bytes) png_encode={:.2}ms png_bytes={}",
                w, h, crop_ms, dib_ms, v5.len(), d.len(), png_ms, png.as_ref().map(|b| b.len()).unwrap_or(0)
            );
        }
        Err(e) => logf!("commit crop FAILED: {e}"),
    }
    drop(snap);

    if opts.wgc || opts.live {
        wgc_border(&gpu, &mons, &outputs);
    }
    if opts.live {
        live(&opts);
    } else {
        logf!("not run (need --live at the desk): RegisterHotKey(Win+Shift+S) and PrtScn, overlay setup, hook callback cost, Start-menu masking");
    }
    let (private, ws, cpu) = crate::app::self_usage();
    logf!("probe done: private={:.1}MB ws={:.1}MB cpu={:.0}ms", private as f64 / 1048576.0, ws as f64 / 1048576.0, cpu);
    0
}

/// Does an unpackaged exe get the yellow WGC border? Start a WGC session on the primary, wait,
/// and look at the primary's edge through Desktop Duplication (which sees what DWM composes).
fn wgc_border(gpu: &capture::Gpu, mons: &[capture::MonitorInfo], outputs: &[(windows::Win32::Graphics::Gdi::HMONITOR, windows::Win32::Graphics::Dxgi::IDXGIOutput)]) {
    use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
    use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::core::Interface;
    let Some(m) = mons.iter().find(|m| m.primary) else { return };
    let Some((_, out)) = outputs.iter().find(|(h, _)| *h == m.hmon) else { return };
    let (w, h) = (m.rect.width() as u32, m.rect.height() as u32);
    let before = capture::capture_dd(gpu, out).ok().and_then(|(t, _)| readback(gpu, &t, w, h)).map(|p| border_share(w, h, &p));
    let r = (|| -> windows::core::Result<(bool, Option<f64>)> {
        unsafe {
            let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
            let item: GraphicsCaptureItem = interop.CreateForMonitor(m.hmon)?;
            let dx: IDXGIDevice = gpu.device.cast()?;
            let d3d: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dx)?.cast()?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 1, item.Size()?)?;
            let session = pool.CreateCaptureSession(&item)?;
            let border_off = session.SetIsBorderRequired(false).is_ok();
            session.StartCapture()?;
            std::thread::sleep(std::time::Duration::from_millis(400));
            let during = capture::capture_dd(gpu, out).ok().and_then(|(t, _)| readback(gpu, &t, w, h)).map(|p| border_share(w, h, &p));
            let _ = session.Close();
            let _ = pool.Close();
            Ok((border_off, during))
        }
    })();
    match r {
        Ok((off, during)) => logf!(
            "wgc border on {}: SetIsBorderRequired(false) accepted={} edge_yellow_share before={} during={} (a large rise means the border shows)",
            m.name, off, before.map(|b| format!("{b:.3}")).unwrap_or("-".into()), during.map(|b| format!("{b:.3}")).unwrap_or("-".into())
        ),
        Err(e) => logf!("wgc border check FAILED: {e}"),
    }
}

fn hotkey(id: i32, mods: HOT_KEY_MODIFIERS, vk: u32, what: &str) -> bool {
    unsafe {
        let r = RegisterHotKey(None, id, mods, vk);
        let ok = r.is_ok();
        logf!("RegisterHotKey {what}: {}", match &r { Ok(()) => "OK (free)".to_string(), Err(e) => format!("FAILED {:#010x} {}", e.code().0, e.message()) });
        if ok {
            let _ = UnregisterHotKey(None, id);
        }
        ok
    }
}

fn live(opts: &ProbeOpts) {
    // A control key that nobody holds proves the probe can tell "free" from "held".
    let control = hotkey(0x51, MOD_WIN | MOD_SHIFT | MOD_ALT | MOD_CONTROL | MOD_NOREPEAT, VK_F24.0 as u32, "control Win+Shift+Ctrl+Alt+F24");
    let ws = hotkey(0x52, MOD_WIN | MOD_SHIFT | MOD_NOREPEAT, 'S' as u32, "Win+Shift+S");
    let ps = hotkey(0x53, MOD_NOREPEAT, VK_SNAPSHOT.0 as u32, "PrtScn");
    logf!("hotkeys: control_free={control} win_shift_s_free={ws} prtscn_free={ps}");

    // Overlay setup cost: real windows appear for a moment (Esc not needed; closed by code).
    unsafe {
        let hinst: windows::Win32::Foundation::HINSTANCE = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default();
        let _ = crate::app::register_classes(hinst, crate::app::MAIN_CLASS_TEST);
        let t = qpc();
        let snap = capture::freeze(CaptureMethod::Auto, None);
        let f = ms_since(t);
        let t = qpc();
        match crate::overlay::Overlay::new(snap, hinst, &crate::overlay::Opts::plain(0.45)) {
            Ok(mut ov) => {
                let setup = ms_since(t);
                let t = qpc();
                let shown = ov.show();
                let first = ms_since(t);
                logf!("overlay: freeze={f:.2}ms setup={setup:.2}ms first_frame={first:.2}ms total={:.2}ms shown={:?}", f + setup + first, shown.err());
                let mut msg = MSG::default();
                let until = std::time::Instant::now() + std::time::Duration::from_millis(300);
                while std::time::Instant::now() < until {
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                drop(ov);
            }
            Err(e) => logf!("overlay setup FAILED: {e}"),
        }
    }

    // Hook callback cost while he types (pass-through: Trigger is disabled, nothing is swallowed
    // except while the overlay is up, and it is not up here).
    if opts.hook_seconds > 0 {
        logf!("hook: pass-through for {} s; type normally (Win+Shift+S goes to Windows during this test)", opts.hook_seconds);
        let h = crate::hook::start(HWND::default(), false, false, false);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(opts.hook_seconds as u64);
        while std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        crate::hook::stop(h);
        let (n, avg, max, _) = crate::hook::stats();
        logf!("hook callback: events={n} avg={avg:.3}us max={max:.3}us");
    }
    logf!("manual checks left: Start menu after Win+Shift+S (with and without an elevated window in front), and each game in its display mode (run this probe with --live while the game is in front)");
}
