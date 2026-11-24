//! `snip --selftest`: checks that need Windows but no screen, no window and no clipboard.
//! Exit 0 when every check passed and at least one ran; a run with zero checks fails.

use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

use crate::capture::{self, Frame, Method, MonitorInfo, Pixels, Snapshot};
use crate::geometry::{Layout, Rect, his_layout, his_layout_now};

type Check = (&'static str, fn() -> Result<String, String>);

fn pattern(x: i32, y: i32) -> [u8; 4] {
    // A pixel value that encodes its desktop position, so any crop offset error shows.
    [(x & 0xFF) as u8, (y & 0xFF) as u8, ((x ^ y) & 0xFF) as u8, 255]
}

fn monitor_pixels(r: Rect) -> Vec<u8> {
    let mut v = Vec::with_capacity(r.area() as usize * 4);
    for y in r.top..r.bottom {
        for x in r.left..r.right {
            v.extend_from_slice(&pattern(x, y));
        }
    }
    v
}

fn fake_monitor(r: Rect, name: &str) -> MonitorInfo {
    MonitorInfo { hmon: HMONITOR::default(), rect: r, work: r, name: name.into(), dpi: 96, primary: r.left == 0 && r.top == 0 }
}

fn check_crop(snap: &Snapshot, sel: Rect) -> Result<(), String> {
    let (w, h, px) = snap.crop(&sel).map_err(|e| format!("crop: {e}"))?;
    if (w as i32, h as i32) != (sel.width(), sel.height()) {
        return Err(format!("size {w}x{h} != {}x{}", sel.width(), sel.height()));
    }
    let mut on = 0u64;
    for y in sel.top..sel.bottom {
        for x in sel.left..sel.right {
            let i = (((y - sel.top) * sel.width() + (x - sel.left)) * 4) as usize;
            let got = &px[i..i + 4];
            let want = if snap.layout.monitor_at(crate::geometry::Point::new(x, y)).is_some() {
                on += 1;
                pattern(x, y)
            } else {
                [0, 0, 0, 255]
            };
            if got != want {
                return Err(format!("pixel ({x},{y}) = {got:?}, want {want:?}"));
            }
        }
    }
    if on == 0 {
        return Err("selection covered no monitor pixel (zero-case)".into());
    }
    Ok(())
}

fn small_layout() -> Layout {
    // The user's layout scaled down by 16, keeping negative coordinates and the gaps.
    Layout::new(his_layout().monitors.iter().map(|m| Rect::new(m.left / 16, m.top / 16, m.right / 16, m.bottom / 16)).collect())
}

fn cpu_crop() -> Result<String, String> {
    let l = small_layout();
    let frames = l
        .monitors
        .iter()
        .map(|r| Frame { width: r.width() as u32, height: r.height() as u32, pixels: Pixels::Cpu(monitor_pixels(*r)), method: Method::Gdi, ms: 0.0, zero_present: false, note: String::new() })
        .collect();
    let snap = Snapshot { monitors: l.monitors.iter().map(|r| fake_monitor(*r, "fake")).collect(), layout: l.clone(), frames, gpu: None, windows: vec![], gpu_ms: 0.0 };
    let sels = [Rect::new(-30, -20, 40, 30), l.bounds(), Rect::new(1, 1, 2, 2)];
    for s in sels {
        check_crop(&snap, s)?;
    }
    Ok(format!("{} selections over {} fake monitors with negative coordinates", sels.len(), l.monitors.len()))
}

fn gpu_crop() -> Result<String, String> {
    let gpu = capture::create_gpu().map_err(|e| format!("device: {e}"))?;
    let l = small_layout();
    let mut frames = Vec::new();
    for (i, r) in l.monitors.iter().enumerate() {
        let px = monitor_pixels(*r);
        // Odd monitors on the GPU, even ones on the CPU: a cross-monitor crop mixes both paths.
        let pixels = if i % 2 == 1 {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: r.width() as u32,
                Height: r.height() as u32,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let init = D3D11_SUBRESOURCE_DATA { pSysMem: px.as_ptr() as *const _, SysMemPitch: r.width() as u32 * 4, SysMemSlicePitch: 0 };
            let mut t = None;
            unsafe { gpu.device.CreateTexture2D(&desc, Some(&init), Some(&mut t)) }.map_err(|e| format!("texture: {e}"))?;
            Pixels::Gpu(t.unwrap())
        } else {
            Pixels::Cpu(px)
        };
        frames.push(Frame { width: r.width() as u32, height: r.height() as u32, pixels, method: Method::Dd, ms: 0.0, zero_present: false, note: String::new() });
    }
    let warp = gpu.warp;
    let snap = Snapshot { monitors: l.monitors.iter().map(|r| fake_monitor(*r, "fake")).collect(), layout: l.clone(), frames, gpu: Some(gpu), windows: vec![], gpu_ms: 0.0 };
    let sels = [Rect::new(-30, -20, 40, 30), Rect::new(150, 5, 200, 60), l.bounds()];
    for s in sels {
        check_crop(&snap, s)?;
    }
    Ok(format!("{} selections, GPU and CPU frames mixed{}", sels.len(), if warp { " (WARP device)" } else { "" }))
}

/// The desktop as a four-monitor layout on 2026-11-08, full size (7040 x 2880), each pixel encoding its
/// desktop position. `gpu`: every monitor is a GPU texture, so the cut includes the readback.
fn his_now_snapshot(gpu: bool) -> Result<Snapshot, String> {
    let l = his_layout_now();
    let device = if gpu { Some(capture::create_gpu().map_err(|e| format!("device: {e}"))?) } else { None };
    let mut frames = Vec::new();
    for r in &l.monitors {
        let px = monitor_pixels(*r);
        let pixels = match &device {
            Some(g) => {
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: r.width() as u32,
                    Height: r.height() as u32,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                let init = D3D11_SUBRESOURCE_DATA { pSysMem: px.as_ptr() as *const _, SysMemPitch: r.width() as u32 * 4, SysMemSlicePitch: 0 };
                let mut t = None;
                unsafe { g.device.CreateTexture2D(&desc, Some(&init), Some(&mut t)) }.map_err(|e| format!("texture: {e}"))?;
                Pixels::Gpu(t.unwrap())
            }
            None => Pixels::Cpu(px),
        };
        frames.push(Frame { width: r.width() as u32, height: r.height() as u32, pixels, method: Method::Dd, ms: 0.0, zero_present: false, note: String::new() });
    }
    Ok(Snapshot { monitors: l.monitors.iter().map(|r| fake_monitor(*r, "fake")).collect(), layout: l.clone(), frames, gpu: device, windows: vec![], gpu_ms: 0.0 })
}

/// All screens on the 7040 x 2880 test layout: the picture is exactly the desktop box, every monitor pixel
/// is where it belongs, the empty corners are black, one clipboard write is counted (a test
/// clipboard, never the user's), the PNG header says the same size. Times: cut, clipboard work, file.
fn all_screens(gpu: bool) -> Result<String, String> {
    use crate::allshot::{CountingClipboard, ImageSink};
    let snap = his_now_snapshot(gpu)?;
    let want = snap.layout.bounds();
    let mut best: Option<(f64, f64)> = None;
    let mut sink = CountingClipboard::default();
    let mut shot = None;
    for _ in 0..3 {
        let t = crate::log::qpc();
        let s = crate::allshot::take(&snap)?;
        let cut = crate::log::ms_since(t);
        let t = crate::log::qpc();
        sink.put(s.w, s.h, &s.px)?;
        let clip = crate::log::ms_since(t);
        if best.is_none_or(|(a, b)| cut + clip < a + b) {
            best = Some((cut, clip));
        }
        shot = Some(s);
    }
    let shot = shot.unwrap();
    if (shot.w as i32, shot.h as i32) != (7040, 2880) || (shot.w as i32, shot.h as i32) != (want.width(), want.height()) {
        return Err(format!("picture {}x{}, desktop {}x{}", shot.w, shot.h, want.width(), want.height()));
    }
    if sink.puts != 3 || sink.last_size != (7040, 2880) {
        return Err(format!("{} clipboard writes of {:?}", sink.puts, sink.last_size));
    }
    // Spot checks on every monitor corner and the four gaps.
    let at = |x: i32, y: i32| -> [u8; 4] {
        let i = (((y - want.top) as usize * shot.w as usize) + (x - want.left) as usize) * 4;
        [shot.px[i], shot.px[i + 1], shot.px[i + 2], shot.px[i + 3]]
    };
    for m in &snap.layout.monitors {
        for (x, y) in [(m.left, m.top), (m.right - 1, m.top), (m.left, m.bottom - 1), (m.right - 1, m.bottom - 1), ((m.left + m.right) / 2, (m.top + m.bottom) / 2)] {
            if at(x, y) != pattern(x, y) {
                return Err(format!("pixel ({x},{y}) = {:?}, want {:?}", at(x, y), pattern(x, y)));
            }
        }
    }
    let mut black = 0;
    for (x, y) in [(-1900, -1400), (-1900, 1400), (5100, -1400), (4000, 1439), (-1000, 1300)] {
        if snap.layout.monitor_at(crate::geometry::Point::new(x, y)).is_none() {
            black += 1;
            if at(x, y) != [0, 0, 0, 255] {
                return Err(format!("gap pixel ({x},{y}) = {:?}, want opaque black", at(x, y)));
            }
        }
    }
    if black < 3 {
        return Err("fewer than three gap pixels were checked".into());
    }
    // Saved like any other shot; its header must say 7040 x 2880.
    let dir = std::env::temp_dir().join(format!("snip-selftest-all-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let stamp = crate::naming::Stamp { year: 2026, month: 11, day: 8, hour: 1, minute: 0, second: 1 };
    let t = crate::log::qpc();
    let saved = crate::app::save_png(&dir, stamp, shot.w, shot.h, &shot.px);
    let png_ms = crate::log::ms_since(t);
    let dims = saved.as_ref().ok().and_then(|(p, ..)| std::fs::read(p).ok()).map(|b| (u32::from_be_bytes([b[16], b[17], b[18], b[19]]), u32::from_be_bytes([b[20], b[21], b[22], b[23]])));
    let _ = std::fs::remove_dir_all(&dir);
    saved?;
    if dims != Some((7040, 2880)) {
        return Err(format!("PNG header {dims:?}"));
    }
    let (cut, clip) = best.unwrap();
    Ok(format!(
        "7040x2880 from 4 monitors{}; cut {cut:.0} ms + clipboard work {clip:.0} ms = {:.0} ms (best of 3; capture itself not included); PNG {png_ms:.0} ms (after the clipboard); 3 counted writes, none to the real clipboard",
        if gpu { " (GPU textures)" } else { " (CPU frames)" },
        cut + clip
    ))
}

fn png_roundtrip() -> Result<String, String> {
    let (w, h) = (97u32, 61u32);
    let mut px = Vec::new();
    for y in 0..h {
        for x in 0..w {
            px.extend_from_slice(&[(x * 2) as u8, (y * 4) as u8, ((x + y) * 3) as u8, 255]);
        }
    }
    let t = crate::log::qpc();
    let bytes = crate::png::encode_png(w, h, &px).map_err(|e| format!("encode: {e}"))?;
    let enc_ms = crate::log::ms_since(t);
    if &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("no PNG signature".into());
    }
    let (dw, dh, back) = crate::png::decode_png(&bytes).map_err(|e| format!("decode: {e}"))?;
    if (dw, dh) != (w, h) || back != px {
        return Err(format!("decoded {dw}x{dh} differs from {w}x{h} input"));
    }
    Ok(format!("{w}x{h} -> {} bytes, lossless, encode {enc_ms:.2} ms", bytes.len()))
}

fn save_flow() -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("snip-selftest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let stamp = crate::naming::Stamp { year: 2026, month: 10, day: 8, hour: 12, minute: 0, second: 1 };
    let px = vec![200u8; 40 * 30 * 4];
    let r1 = crate::app::save_png(&dir, stamp, 40, 30, &px);
    let r2 = crate::app::save_png(&dir, stamp, 40, 30, &px);
    let names: Vec<String> = std::fs::read_dir(&dir).map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    let (p1, ..) = r1?;
    let (p2, ..) = r2?;
    let want1 = "Ekran görüntüsü 2026-10-08 120001.png";
    let want2 = "Ekran görüntüsü 2026-10-08 120001 (2).png";
    if !p1.ends_with(want1) || !p2.ends_with(want2) {
        return Err(format!("names {p1:?} {p2:?}"));
    }
    if names.len() != 2 || names.iter().any(|n| n.ends_with(".part")) {
        return Err(format!("folder holds {names:?}"));
    }
    Ok(format!("{want1} and {want2}, no .part left"))
}

fn dib_formats() -> Result<String, String> {
    let px = vec![10u8; 33 * 7 * 4];
    let v5 = crate::dib::dibv5_bytes(33, 7, &px);
    let d = crate::dib::dib24_bytes(33, 7, &px);
    if v5.len() != 124 + 33 * 7 * 4 || d.len() != 40 + crate::dib::stride24(33) * 7 {
        return Err(format!("sizes {} {}", v5.len(), d.len()));
    }
    Ok(format!("CF_DIBV5 {} bytes, CF_DIB {} bytes", v5.len(), d.len()))
}

fn hook_rules() -> Result<String, String> {
    use crate::hookstate::*;
    let ws = Mods { win: true, shift: true, ctrl: false, alt: false };
    let mut s = HookState::new();
    let seq = [
        (KeyEvent { vk: VK_S, down: true, injected_by_us: false, mods: ws }, KeyAction::Trigger),
        (KeyEvent { vk: VK_S, down: false, injected_by_us: false, mods: ws }, KeyAction::Swallow),
        (KeyEvent { vk: VK_R, down: true, injected_by_us: false, mods: ws }, KeyAction::Pass),
        (KeyEvent { vk: VK_S, down: true, injected_by_us: false, mods: Mods::default() }, KeyAction::Pass),
    ];
    for (i, (e, want)) in seq.iter().enumerate() {
        let got = s.on_key(*e, true, false);
        if got != *want {
            return Err(format!("step {i}: {got:?} != {want:?}"));
        }
    }
    Ok(format!("{} key events", seq.len()))
}

fn config_default() -> Result<String, String> {
    let c = crate::config::Config::parse(&crate::config::default_file())?;
    if c != crate::config::Config::default() {
        return Err("default file differs from built-in defaults".into());
    }
    Ok(format!("config file {}", crate::paths::config_file().display()))
}

fn folders() -> Result<String, String> {
    let d = crate::paths::screenshots_dir();
    Ok(format!("screenshots folder {} (exists: {})", d.display(), d.is_dir()))
}

fn d2d_ready() -> Result<String, String> {
    use windows::Win32::Graphics::Direct2D::*;
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    use windows::core::Interface;
    let t = crate::log::qpc();
    let gpu = capture::create_gpu().map_err(|e| format!("device: {e}"))?;
    let dev_ms = crate::log::ms_since(t);
    unsafe {
        let f: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).map_err(|e| format!("d2d factory: {e}"))?;
        let dx: IDXGIDevice = gpu.device.cast().map_err(|e| e.to_string())?;
        let d = f.CreateDevice(&dx).map_err(|e| format!("d2d device: {e}"))?;
        d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE).map_err(|e| format!("d2d context: {e}"))?;
    }
    Ok(format!("D3D11 device {:.2} ms{}, Direct2D context ok", dev_ms, if gpu.warp { " (WARP)" } else { "" }))
}

/// v2: GDI text has real coverage, and an editor image with every tool encodes to a PNG that
/// decodes back pixel for pixel.
fn editor_output() -> Result<String, String> {
    use crate::dpi::{Monitor, Monitors};
    use crate::editor::{Editor, Prefs};
    use crate::geometry::Point;
    use crate::raster::{Canvas, Glyphs, Rgba};
    let g = crate::gdi::GdiGlyphs::new();
    let t = g.text("Snip 123 ığüşöç", 22.0, false);
    let inked = t.cov.iter().filter(|&&v| v > 128).count();
    if inked < 50 || t.w < 60 {
        return Err(format!("GDI text too small: {}x{} with {inked} inked pixels", t.w, t.h));
    }
    // Wide enough for the whole bar in one row at 150 %: the drags below stay off it.
    let mons = Monitors::new(vec![Monitor::new(Rect::from_xywh(-800, -600, 1600, 1200), 144)]);
    let theme = crate::theme::Theme::new(crate::theme::Look::Graphite, crate::theme::Mode::Dark);
    let mut e = Editor::new(mons, vec![], theme, &Prefs::default(), Point::new(0, 0));
    let drag = |e: &mut Editor, a: (i32, i32), b: (i32, i32)| {
        e.on_down(Point::new(a.0, a.1), false);
        e.on_move(Point::new(b.0, b.1), false);
        e.on_up(Point::new(b.0, b.1), false);
    };
    drag(&mut e, (-300, -200), (299, 199));
    e.tool = None; // select each tool explicitly below; default editor starts with Arrow
    for (key, a, b) in [(b'A', (-250, -150), (-50, 0)), (b'R', (0, -150), (200, 0)), (b'L', (-250, 50), (-50, 50)), (b'P', (-250, 100), (-50, 150)), (b'H', (0, 100), (200, 100)), (b'B', (50, 120), (250, 180))] {
        e.on_key(key as u32, false, false);
        drag(&mut e, a, b);
    }
    e.on_key(b'N' as u32, false, false);
    e.on_down(Point::new(-200, 170), false);
    e.on_key(b'T' as u32, false, false);
    e.on_down(Point::new(100, -180), false);
    for c in "Selftest".chars() {
        e.on_char(c);
    }
    e.on_key(crate::hookstate::VK_RETURN, false, false);
    let n = e.doc.shapes().len();
    if n != 8 {
        return Err(format!("{n} shapes, want 8"));
    }
    let base = Canvas::new(600, 400, Rgba::rgb(250, 250, 250));
    let img = e.compose(&base, &g, false);
    let changed = img
        .px
        .as_chunks::<4>()
        .0
        .iter()
        .zip(base.px.as_chunks::<4>().0)
        .filter(|(a, b)| a != b)
        .count();
    let bytes =
        crate::png::encode_png(img.w, img.h, &img.px).map_err(|e| format!("encode: {e}"))?;
    let (w, h, back) = crate::png::decode_png(&bytes).map_err(|e| format!("decode: {e}"))?;
    if (w, h) != (img.w, img.h) || back != img.px {
        return Err("PNG round trip changed pixels".into());
    }
    Ok(format!("8 shapes at 150 %, {changed} pixels changed, PNG {} bytes, GDI text {}x{}", bytes.len(), t.w, t.h))
}

/// 2026-11-23: the new tools with the real GDI font at 150 %: ellipse and spotlight change the
/// picture, the select tool moves a shape, the magnifier and the frame render, and the framed
/// picture survives a PNG round trip.
fn editor_v2() -> Result<String, String> {
    use crate::annotate::Shape;
    use crate::dpi::{Monitor, Monitors};
    use crate::editor::{Editor, Prefs};
    use crate::geometry::Point;
    use crate::raster::{Canvas, Rgba};
    let g = crate::gdi::GdiGlyphs::new();
    let mons = Monitors::new(vec![Monitor::new(Rect::from_xywh(0, 0, 900, 600), 144)]);
    let theme = crate::theme::Theme::new(crate::theme::Look::Graphite, crate::theme::Mode::Dark);
    let mut e = Editor::new(mons, vec![], theme, &Prefs::default(), Point::new(0, 0));
    let drag = |e: &mut Editor, a: (i32, i32), b: (i32, i32)| {
        e.on_down(Point::new(a.0, a.1), false);
        e.on_move(Point::new(b.0, b.1), false);
        e.on_up(Point::new(b.0, b.1), false);
    };
    drag(&mut e, (50, 50), (649, 449));
    e.on_key(b'E' as u32, false, false);
    drag(&mut e, (100, 100), (300, 220));
    e.on_key(b'F' as u32, false, false);
    drag(&mut e, (350, 250), (550, 400));
    let base = Canvas::new(600, 400, Rgba::rgb(240, 240, 240));
    let img = e.compose(&base, &g, false);
    let lit = img.get(450 - 50, 320 - 50) == base.get(0, 0);
    let dimmed = img.get(10, 390).luminance() < base.get(0, 0).luminance() * 0.6;
    if !(lit && dimmed) {
        return Err(format!("spotlight: lit {lit}, dimmed {dimmed}"));
    }
    e.on_key(b'V' as u32, false, false);
    drag(&mut e, (100, 160), (140, 160));
    let moved = matches!(e.doc.shapes().first(), Some(Shape::Ellipse { a, .. }) if (a.x - 140.5).abs() < 1e-3);
    if !moved {
        return Err(format!("select tool did not move the ellipse: {:?}", e.doc.shapes().first()));
    }
    let mut sample = Canvas::new(crate::theme::LOUPE_PIXELS, crate::theme::LOUPE_PIXELS, Rgba::rgb(229, 56, 59));
    sample.set(0, 0, Rgba::default());
    let loupe = crate::loupe::render(&sample, Point::new(1234, 567), &theme, 1.5, &g);
    let inked = loupe.px.as_chunks::<4>().0.iter().filter(|p| p[3] > 0).count();
    let framed = crate::beautify::apply(&img, 1.5);
    let bytes = crate::png::encode_png(framed.w, framed.h, &framed.px).map_err(|e| format!("encode: {e}"))?;
    let (w, h, back) = crate::png::decode_png(&bytes).map_err(|e| format!("decode: {e}"))?;
    if (w, h) != (framed.w, framed.h) || back != framed.px {
        return Err("framed PNG round trip changed pixels".into());
    }
    Ok(format!("ellipse + spotlight, ellipse moved 40 px, magnifier {}x{} ({inked} px), frame {}x{} PNG {} bytes", loupe.w, loupe.h, framed.w, framed.h, bytes.len()))
}

/// v2: the history store in a temporary folder keeps 50 and deletes only its own files.
fn history_store() -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("snip-selftest-history-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = crate::history::Store::new(&dir);
    let thumb = crate::raster::Canvas::new(8, 5, crate::raster::Rgba::rgb(1, 2, 3));
    let mut err = None;
    for i in 0..52 {
        let e = crate::history::Entry { id: 0, time: format!("2026-11-04 12:00:{:02}", i % 60), path: String::new(), owned: false, w: 8, h: 5, source: "selftest".into() };
        if let Err(x) = store.record(e, &thumb, Some(b"png")) {
            err = Some(x.to_string());
            break;
        }
    }
    let (h, note) = store.load();
    let files: std::collections::BTreeSet<_> = std::fs::read_dir(&dir)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let expected: std::collections::BTreeSet<_> = [
        crate::history::INDEX.to_string(),
        crate::history::LOCK.to_string(),
    ]
    .into_iter()
    .chain(h.entries.iter().flat_map(|e| {
        [
            crate::history::owned_name(e.id),
            crate::history::thumb_name(e.id),
        ]
    }))
    .collect();
    let _ = std::fs::remove_dir_all(&dir);
    if let Some(e) = err.or(note) {
        return Err(e);
    }
    if h.entries.len() != crate::history::LIMIT || files != expected {
        return Err(format!(
            "{} entries, {} files; missing {:?}, unexpected {:?}",
            h.entries.len(),
            files.len(),
            expected.difference(&files).collect::<Vec<_>>(),
            files.difference(&expected).collect::<Vec<_>>()
        ));
    }
    Ok("52 recorded, 50 kept, 50 PNG + 50 thumbnails + index + lock = 102 files".into())
}

/// v2: the hotkeys the current config.toml asks for.
fn hotkey_plan() -> Result<String, String> {
    let (cfg, note) = crate::config::Config::load_or_create(&crate::paths::config_file());
    if let Some(n) = note.filter(|n| n.contains("broken")) {
        return Err(n);
    }
    let p = cfg.plan();
    let ed = p.editor.map(|h| h.text()).unwrap_or_else(|| "none".into());
    let all = p.all.map(|h| h.text()).unwrap_or_else(|| "none".into());
    let all2 = p.all2.map(|h| h.text()).unwrap_or_else(|| "none".into());
    Ok(format!("all screens {all} and {all2}, Win+Shift+S {}, editor {ed}, plain PrtScn {}{}", if cfg.win_shift_s { "on" } else { "off" }, p.plain_prtscn, if p.notes.is_empty() { String::new() } else { format!(" ({})", p.notes.join("; ")) }))
}

pub fn run() -> i32 {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let checks: [Check; 17] = [
        ("geometry", || {
            let l = his_layout();
            let n = l.pieces(&l.bounds()).len();
            if n == 4 { Ok(format!("bounds {:?}, 4 pieces", l.bounds())) } else { Err(format!("{n} pieces")) }
        }),
        ("monitor-workareas", || {
            let mons = capture::monitors();
            if !mons.is_empty() && mons.iter().all(|m| !m.work.is_empty() && m.work.intersect(&m.rect) == Some(m.work)) {
                Ok(format!("{} rcWork areas within rcMonitor", mons.len()))
            } else { Err("missing or invalid monitor work area".into()) }
        }),
        ("hook-rules", hook_rules),
        ("config", config_default),
        ("dib-formats", dib_formats),
        ("png-roundtrip", png_roundtrip),
        ("save-names-atomic", save_flow),
        ("crop-cpu", cpu_crop),
        ("crop-gpu", gpu_crop),
        ("direct2d", d2d_ready),
        ("folders", folders),
        ("editor-output", editor_output),
        ("editor-v2-tools", editor_v2),
        ("history-store", history_store),
        ("hotkey-plan", hotkey_plan),
        ("all-screens-cpu", || all_screens(false)),
        ("all-screens-gpu", || all_screens(true)),
    ];
    let (mut pass, mut fail) = (0, 0);
    for (name, f) in checks.iter() {
        match f() {
            Ok(d) => {
                pass += 1;
                println!("PASS {name}: {d}");
            }
            Err(e) => {
                fail += 1;
                println!("FAIL {name}: {e}");
            }
        }
    }
    println!("selftest: {pass} passed, {fail} failed");
    if pass == 0 || fail > 0 { 1 } else { 0 }
}

static SESSION_MSGS: std::sync::Mutex<Vec<(u32, usize)>> = std::sync::Mutex::new(Vec::new());

unsafe extern "system" fn fake_tray_proc(hwnd: windows::Win32::Foundation::HWND, msg: u32, wp: windows::Win32::Foundation::WPARAM, lp: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    use crate::app::{WM_APP_SESSION_DONE, WM_APP_SESSION_OVERLAY, WM_APP_SESSION_READY};
    if msg == WM_APP_SESSION_READY || msg == WM_APP_SESSION_OVERLAY || msg == WM_APP_SESSION_DONE {
        if let Ok(mut g) = SESSION_MSGS.lock() {
            g.push((msg, wp.0));
        }
        return windows::Win32::Foundation::LRESULT(0);
    }
    unsafe { windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wp, lp) }
}

/// The tray-process/session-child protocol: a fake tray window starts a real `snip --session`
/// child with `--go` (source "selftest": it presses Enter by itself), then checks the messages,
/// the child's exit code and the PNG it wrote into a temp folder (no clipboard, no last.txt).
/// `all`: the all-screens source instead of the plain snip (no overlay; the PNG must be the whole
/// desktop).
fn session_roundtrip(hinst: windows::Win32::Foundation::HINSTANCE, all: bool) -> Vec<(String, bool, String)> {
    use crate::app::{WM_APP_SESSION_DONE, WM_APP_SESSION_OVERLAY, WM_APP_SESSION_READY};
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
    use windows::Win32::System::Threading::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{PCWSTR, PWSTR, w};
    let mut out = Vec::new();
    let tag = if all { "session-all" } else { "session" };
    if let Ok(mut g) = SESSION_MSGS.lock() {
        g.clear();
    }
    let dir = std::env::temp_dir().join(format!("snip-selftest-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(fake_tray_proc),
            hInstance: hinst,
            lpszClassName: w!("Laureth.snip.selftest.tray"),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let Ok(hwnd) = CreateWindowExW(WS_EX_TOOLWINDOW, w!("Laureth.snip.selftest.tray"), w!("snip selftest"), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst), None) else {
            out.push(("session-start".into(), false, "fake tray window failed".into()));
            return out;
        };
        let mut buf = [0u16; 1024];
        let n = GetModuleFileNameW(None, &mut buf) as usize;
        let exe = String::from_utf16_lossy(&buf[..n]);
        let t0 = crate::log::qpc();
        let cmd = format!("\"{exe}\" --session {} --go {t0} {} --selftest-dir \"{}\"", hwnd.0 as isize, if all { 10 } else { 9 }, dir.display());
        let mut wide: Vec<u16> = cmd.encode_utf16().chain(Some(0)).collect();
        let si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let mut pi = PROCESS_INFORMATION::default();
        if let Err(e) = CreateProcessW(PCWSTR::null(), Some(PWSTR(wide.as_mut_ptr())), None, None, false, PROCESS_CREATION_FLAGS(0), None, PCWSTR::null(), &si, &mut pi) {
            out.push(("session-start".into(), false, e.to_string()));
            return out;
        }
        let _ = CloseHandle(pi.hThread);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut msg = MSG::default();
        let mut exited = false;
        while std::time::Instant::now() < deadline {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if WaitForSingleObject(pi.hProcess, 10) == WAIT_OBJECT_0 {
                exited = true;
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                break;
            }
        }
        let total_ms = crate::log::ms_since(t0);
        let mut code = 0u32;
        let _ = GetExitCodeProcess(pi.hProcess, &mut code);
        if !exited {
            let _ = TerminateProcess(pi.hProcess, 1);
        }
        let _ = CloseHandle(pi.hProcess);
        let _ = DestroyWindow(hwnd);
        let msgs = SESSION_MSGS.lock().map(|g| g.clone()).unwrap_or_default();
        let kinds: Vec<String> = msgs
            .iter()
            .map(|(m, wp)| match *m {
                x if x == WM_APP_SESSION_READY => "ready".to_string(),
                x if x == WM_APP_SESSION_OVERLAY => format!("overlay{wp}"),
                x if x == WM_APP_SESSION_DONE => format!("done{wp}"),
                _ => "?".into(),
            })
            .collect();
        let what = if all { "process start, device, freeze, whole-desktop cut, PNG, card" } else { "process start, device, freeze, overlay, Enter, crop, PNG" };
        out.push((format!("{tag}-exit"), exited && code == 0, format!("exited={exited} code={code} in {total_ms:.0} ms ({what})")));
        let want: &[&str] = if all { &["ready", "done2"] } else { &["ready", "overlay1", "overlay0", "done2"] };
        out.push((format!("{tag}-messages"), kinds == want, format!("{kinds:?}")));
        let files: Vec<String> = std::fs::read_dir(&dir).map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
        let ok = files.len() == 1 && files[0].starts_with(crate::naming::PREFIX) && files[0].ends_with(".png");
        out.push((format!("{tag}-png"), ok, format!("{files:?}")));
        if all {
            // The picture is the whole virtual desktop of this (hidden) desktop's monitors.
            let bounds = Layout::new(capture::monitors().iter().map(|m| m.rect).collect()).bounds();
            let dims = files.first().and_then(|f| std::fs::read(dir.join(f)).ok()).filter(|b| b.len() > 24).map(|b| (u32::from_be_bytes([b[16], b[17], b[18], b[19]]), u32::from_be_bytes([b[20], b[21], b[22], b[23]])));
            out.push(("session-all-size".into(), dims == Some((bounds.width() as u32, bounds.height() as u32)), format!("PNG {dims:?}, desktop {}x{}", bounds.width(), bounds.height())));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    out
}

/// Name of this thread's desktop ("Default" is the user's visible one).
pub fn desktop_name() -> String {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::StationsAndDesktops::{GetThreadDesktop, GetUserObjectInformationW, UOI_NAME};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    unsafe {
        let Ok(d) = GetThreadDesktop(GetCurrentThreadId()) else { return String::new() };
        let mut buf = [0u16; 256];
        let mut need = 0u32;
        if GetUserObjectInformationW(HANDLE(d.0), UOI_NAME, Some(buf.as_mut_ptr() as *mut _), (buf.len() * 2) as u32, Some(&mut need)).is_err() {
            return String::new();
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n])
    }
}

/// Stock EDIT checks only. Called after the hidden-desktop guard, never by --selftest.
/// This does not prove a real IME candidate window or visual smoothness.
fn native_text_roundtrip(hinst: windows::Win32::Foundation::HINSTANCE) -> Result<String, String> {
    use crate::editor::TextEdit;
    use crate::geometry::Point;
    use crate::raster::Pt;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::Controls::{EM_GETSEL, EM_SETSEL};
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::w;
    let theme = crate::theme::Theme::new(crate::theme::Look::Graphite, crate::theme::Mode::Dark);
    let mut text = TextEdit {
        id: 1,
        at: Pt::new(150.0, 150.0),
        text: "İı ığüşöç 😀\nikinci satır".into(),
        color: theme.text,
        size: 22.0,
        original: None,
    };
    unsafe {
        let parent = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            crate::overlay::CLASS,
            w!("snip native text selftest"),
            WS_POPUP,
            0,
            0,
            1000,
            800,
            None,
            None,
            Some(hinst),
            None,
        )
        .map_err(|e| e.to_string())?;
        // The parent is never shown. Destroy it on every path after the child has dropped.
        let result = (|| {
            let bounds = Rect::new(100, 100, 900, 700);
            let mut input = crate::textinput::TextInput::new(
                parent,
                parent,
                &text,
                &theme,
                bounds,
                1.0,
                Point::new(150, 150),
            )
            .map_err(|e| e.to_string())?;
            // The box must be a top-level window with its own surface, owned by the overlay: a GDI
            // child of the overlay's no-redirection window has nothing to draw into.
            if GetAncestor(input.hwnd, GA_ROOT) != input.host
                || (GetWindowLongW(input.host, GWL_STYLE) as u32 & WS_CHILD.0) != 0
                || GetWindow(input.host, GW_OWNER).ok() != Some(parent)
            {
                return Err("native text box is not a top-level window owned by the overlay".into());
            }
            if crate::textinput::read(input.hwnd) != text.text {
                return Err("native Unicode buffer changed Turkish, emoji or newline".into());
            }
            SendMessageW(input.hwnd, EM_SETSEL, Some(WPARAM(2)), Some(LPARAM(7)));
            let before = crate::overlay::window_rect(input.hwnd);
            let hwnd = input.hwnd;
            text.size = 72.0;
            input.update(&text, bounds, 1.0);
            let after = crate::overlay::window_rect(input.hwnd);
            let (mut start, mut end) = (0u32, 0u32);
            SendMessageW(
                hwnd,
                EM_GETSEL,
                Some(WPARAM(&mut start as *mut _ as usize)),
                Some(LPARAM(&mut end as *mut _ as isize)),
            );
            if input.hwnd != hwnd
                || crate::textinput::read(hwnd) != text.text
                || (start, end) != (2, 7)
            {
                return Err("font update replaced HWND, buffer or caret selection".into());
            }
            if before.left != after.left
                || before.top != after.top
                || after.height() <= before.height()
                || after.width() <= before.width()
            {
                return Err(format!(
                    "text box did not grow at its fixed anchor: {before:?} -> {after:?}"
                ));
            }
            SendMessageW(hwnd, WM_RBUTTONUP, None, Some(LPARAM(0x000A000A)));
            let mut right = MSG::default();
            if !PeekMessageW(&mut right, Some(parent), WM_RBUTTONUP, WM_RBUTTONUP, PM_REMOVE).as_bool() {
                return Err("native text right-click was not forwarded to Snip".into());
            }
            drop(input);
            if IsWindow(Some(hwnd)).as_bool() {
                return Err("native edit HWND survived drop".into());
            }
            Ok("Turkish + emoji + multiline buffer; 22 -> 72 DIP grows the same HWND at its anchor, caret selection preserved; child destroyed (real IME and visual timing untested)".into())
        })();
        let _ = DestroyWindow(parent);
        result
    }
}

/// `snip --selftest-overlay`: the whole overlay path (freeze, windows, swap chains, Direct2D,
/// simulated drag, Enter, Esc, crop, release) on an invisible desktop. It refuses to run on the user's
/// visible "Default" desktop; start it on a separate hidden desktop.
pub fn run_overlay() -> i32 {
    use crate::overlay::{Kind, Outcome, Overlay};
    use windows::Win32::Foundation::HINSTANCE;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
    let desk = desktop_name();
    println!("desktop: {desk:?}");
    if desk.is_empty()
        || desk.eq_ignore_ascii_case("Default")
        || desk.eq_ignore_ascii_case("Winlogon")
    {
        println!("REFUSED: this test opens overlay windows; run it on a hidden desktop (not the visible one), never on the user's screen");
        return 4;
    }
    let hinst: HINSTANCE = unsafe { GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default() };
    if let Err(e) = crate::app::register_classes(hinst, crate::app::MAIN_CLASS_TEST) {
        println!("FAIL register classes: {e}");
        return 1;
    }
    let (private, ws, _) = crate::app::self_usage();
    println!("before: private={:.1}MB ws={:.1}MB", private as f64 / 1048576.0, ws as f64 / 1048576.0);
    let (mut pass, mut fail) = (0, 0);
    let mut check = |name: &str, ok: bool, detail: String| {
        if ok {
            pass += 1;
            println!("PASS {name}: {detail}");
        } else {
            fail += 1;
            println!("FAIL {name}: {detail}");
        }
    };
    check("clipboard-guard", crate::clipboard::on_a_hidden_desktop_of_his_station(), "this hidden desktop shares the user's real clipboard: writes refused".into());
    match native_text_roundtrip(hinst) {
        Ok(detail) => check("native-text-input", true, detail),
        Err(detail) => check("native-text-input", false, detail),
    }
    for (round, action) in ["drag", "enter", "escape"].iter().enumerate() {
        let t = crate::log::qpc();
        // GDI only: on a hidden desktop Desktop Duplication has no access, and WGC could draw its
        // border on the user's real screen. Round 0 then moves the frames to GPU textures, so both the
        // CPU-bitmap and the wrapped-texture paths of the overlay are exercised.
        let mut snap = capture::freeze(crate::config::CaptureMethod::Gdi, None);
        if round == 0
            && let Some(gpu) = snap.gpu.clone() {
                for f in snap.frames.iter_mut() {
                    if let Pixels::Cpu(px) = &f.pixels {
                        let desc = D3D11_TEXTURE2D_DESC {
                            Width: f.width,
                            Height: f.height,
                            MipLevels: 1,
                            ArraySize: 1,
                            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                            Usage: D3D11_USAGE_DEFAULT,
                            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                            CPUAccessFlags: 0,
                            MiscFlags: 0,
                        };
                        let init = D3D11_SUBRESOURCE_DATA { pSysMem: px.as_ptr() as *const _, SysMemPitch: f.width * 4, SysMemSlicePitch: 0 };
                        let mut t = None;
                        if unsafe { gpu.device.CreateTexture2D(&desc, Some(&init), Some(&mut t)) }.is_ok() {
                            f.pixels = Pixels::Gpu(t.unwrap());
                        }
                    }
                }
            }
        let freeze_ms = crate::log::ms_since(t);
        let summary = snap.summary();
        let n_mon = snap.monitors.len();
        let prim = snap.monitors.iter().position(|m| m.primary).unwrap_or(0);
        let mrect = snap.monitors.get(prim).map(|m| m.rect).unwrap_or_default();
        let t = crate::log::qpc();
        let mut ov = match Overlay::new(snap, hinst, &crate::overlay::Opts::plain(0.45)) {
            Ok(o) => o,
            Err(e) => {
                check("overlay-new", false, format!("{:#010x} {} | {}", e.code().0, e.message(), summary));
                break;
            }
        };
        let setup_ms = crate::log::ms_since(t);
        let t = crate::log::qpc();
        let shown = ov.show();
        let show_ms = crate::log::ms_since(t);
        check(
            &format!("overlay-open-{round}"),
            shown.is_ok() && ov.hwnds().len() == n_mon,
            format!("{} windows, freeze={freeze_ms:.2}ms setup={setup_ms:.2}ms first_frame={show_ms:.2}ms (draw+present={:.1} show={:.1} activate={:.1}) err={:?} | {summary}", ov.hwnds().len(), ov.show_ms.0, ov.show_ms.1, ov.show_ms.2, shown.err()),
        );
        let placed = ov.hwnds().iter().zip(ov.snap.monitors.iter()).all(|(h, m)| crate::overlay::window_rect(*h) == m.rect);
        check(&format!("windows-cover-monitors-{round}"), placed, "each window sits exactly on its monitor rect".into());
        let out = match *action {
            "drag" => {
                ov.on_mouse(prim, WM_LBUTTONDOWN, 100, 100);
                ov.on_mouse(prim, WM_MOUSEMOVE, 250, 180);
                ov.on_mouse(prim, WM_MOUSEMOVE, 400, 300);
                ov.on_mouse(prim, WM_LBUTTONUP, 400, 300)
            }
            "enter" => {
                ov.on_mouse(prim, WM_MOUSEMOVE, 10, 10);
                ov.on_key(crate::hookstate::VK_RETURN, false)
            }
            _ => ov.on_key(crate::hookstate::VK_ESCAPE, false),
        };
        let want = match *action {
            "drag" => Outcome::Commit(Rect::new(mrect.left + 100, mrect.top + 100, mrect.left + 401, mrect.top + 301), Kind::Rectangle),
            "enter" => Outcome::Commit(mrect, Kind::Monitor),
            _ => Outcome::Cancel,
        };
        check(&format!("outcome-{action}"), out == want, format!("{out:?}"));
        if let Outcome::Commit(r, _) = out {
            let t = crate::log::qpc();
            let c = ov.snap.crop(&r);
            let crop_ms = crate::log::ms_since(t);
            match c {
                Ok((w, h, px)) => check(&format!("crop-{action}"), w as i32 == r.width() && h as i32 == r.height() && px.len() == (w * h * 4) as usize, format!("{w}x{h} in {crop_ms:.2} ms")),
                Err(e) => check(&format!("crop-{action}"), false, e.to_string()),
            }
        }
        let hw = ov.hwnds();
        drop(ov);
        let gone = hw.iter().all(|h| unsafe { !windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(*h)).as_bool() });
        let (private, ws, _) = crate::app::self_usage();
        check(&format!("release-{round}"), gone, format!("windows destroyed, GPU objects dropped; private={:.1}MB ws={:.1}MB", private as f64 / 1048576.0, ws as f64 / 1048576.0));
    }
    let (private, ws, _) = crate::app::self_usage();
    println!("after release: private={:.1}MB ws={:.1}MB", private as f64 / 1048576.0, ws as f64 / 1048576.0);
    for (name, ok, detail) in plain_selection_roundtrip(hinst) {
        check(&name, ok, detail);
    }
    for (name, ok, detail) in fence_roundtrip(hinst) {
        check(&name, ok, detail);
    }
    for all in [false, true] {
        for (name, ok, detail) in session_roundtrip(hinst, all) {
            check(&name, ok, detail);
        }
    }
    println!("selftest-overlay: {pass} passed, {fail} failed");
    if pass == 0 || fail > 0 { 1 } else { 0 }
}

/// Controlled geometry: hidden desktops need not have any foreign windows.
fn plain_selection_roundtrip(hinst: windows::Win32::Foundation::HINSTANCE) -> Vec<(String, bool, String)> {
    use crate::overlay::{Kind, Opts, Outcome, Overlay};
    use windows::Win32::UI::WindowsAndMessaging::{WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONUP};
    let mut results = Vec::new();
    let bounds = Rect::new(0, 0, 800, 600);
    let work = Rect::new(0, 0, 800, 552);
    let other = Rect::new(-420, -120, -20, 480);
    let a = Rect::new(40, 140, 240, 340);
    let b = Rect::new(300, 200, 550, 500);
    let c = Rect::new(-380, 140, -80, 340);
    let mut snap = Snapshot {
        monitors: vec![fake_monitor(bounds, "plain-test"), fake_monitor(other, "negative-test")],
        frames: [bounds, other].iter().map(|r| Frame { width: r.width() as u32, height: r.height() as u32, pixels: Pixels::Cpu(monitor_pixels(*r)), method: Method::Gdi, ms: 0.0, zero_present: false, note: String::new() }).collect(),
        layout: Layout::new(vec![bounds, other]), gpu: capture::create_gpu().ok(),
        windows: [a, b, c].iter().enumerate().map(|(i, r)| capture::WindowInfo { hwnd: 101 + i as isize, rect: *r }).collect(), gpu_ms: 0.0,
    };
    snap.monitors[0].work = work;
    let mut opts = Opts::plain(0.45);
    opts.text_action = true;
    let mut ov = match Overlay::new(snap, hinst, &opts) {
        Ok(ov) => ov,
        Err(e) => return vec![("plain-selection-open".into(), false, e.to_string())],
    };
    ov.synthetic_input = true;
    ov.no_throttle = true;
    let click = |ov: &mut Overlay, x, y, shift| {
        ov.on_mouse_mods(0, WM_MOUSEMOVE, x, y, shift, false);
        ov.on_mouse_mods(0, WM_LBUTTONDOWN, x, y, shift, false);
        ov.on_mouse_mods(0, WM_LBUTTONUP, x, y, shift, false)
    };
    let drag = |ov: &mut Overlay, shift| {
        ov.on_mouse_mods(0, WM_LBUTTONDOWN, 600, 140, shift, false);
        ov.on_mouse_mods(0, WM_MOUSEMOVE, 700, 260, shift, false);
        ov.on_mouse_mods(0, WM_LBUTTONUP, 700, 260, shift, false)
    };
    let clear = |ov: &mut Overlay| {
        while ov.frame_count() > 0 { ov.on_key_mods(0x08, false, false); }
    };
    let mut check = |name: &str, ok, detail: String| results.push((name.into(), ok, detail));
    ov.on_mouse_mods(0, WM_MOUSEMOVE, 100, 100, false, false);
    for (name, shift, want, label) in [("enter-full-monitor", false, bounds, "monitor"), ("shift-enter-workarea", true, work, "workarea")] {
        let out = ov.on_key_mods(crate::hookstate::VK_RETURN, false, shift);
        check(name, out == Outcome::Commit(want, Kind::Monitor) && ov.commit_name(Kind::Monitor) == label, format!("{out:?}, {}", ov.commit_name(Kind::Monitor)));
    }
    ov.snap.monitors[0].work = bounds;
    let out = ov.on_key_mods(crate::hookstate::VK_RETURN, false, true);
    check("shift-enter-autohide", out == Outcome::Commit(bounds, Kind::Monitor), format!("{out:?}"));
    ov.snap.monitors[0].work = work;
    let first = click(&mut ov, 100, 200, true);
    let second = click(&mut ov, 400, 300, true);
    let out = ov.on_key_mods(crate::hookstate::VK_RETURN, false, false);
    check("frames-two-windows-enter", first == Outcome::Nothing && second == Outcome::Nothing && ov.frame_count() == 2 && out == Outcome::Commit(a.union(&b), Kind::Frames), format!("{out:?}, {} frames", ov.frame_count()));
    let out = ov.on_key_mods(crate::hookstate::VK_RETURN, false, true);
    check("frames-win-over-workarea", out == Outcome::Commit(a.union(&b), Kind::Frames) && ov.commit_name(Kind::Frames) == "frames", format!("{out:?}"));
    let rendering = ov.read_back(0).map(|cv| {
        let pixel = |x, y| { let p = cv.get(x, y); [p.b, p.g, p.r, p.a] };
        pixel(100, 200) == pattern(100, 200) && pixel(400, 300) == pattern(400, 300) && pixel(270, 300) != pattern(270, 300)
    });
    check("frames-undimmed-and-gap-dimmed", rendering.as_ref().is_ok_and(|v| *v), format!("{rendering:?}"));
    let cropped = check_crop(&ov.snap, a.union(&b));
    check("frames-union-keeps-between-pixels", cropped.is_ok(), format!("{cropped:?}"));
    ov.on_key_mods(0x08, false, false);
    let out = ov.on_key_mods(crate::hookstate::VK_RETURN, false, false);
    check("frames-backspace-last", ov.frame_count() == 1 && out == Outcome::Commit(a, Kind::Frames), format!("{out:?}"));
    clear(&mut ov);
    let first = click(&mut ov, 100, 200, true);
    let second = click(&mut ov, 100, 200, true);
    check("frames-toggle-same-window-empty", first == Outcome::Nothing && second == Outcome::Nothing && ov.frame_count() == 0, format!("{second:?}"));
    let added = drag(&mut ov, true);
    let out = click(&mut ov, 100, 200, false);
    let drawn = Rect::new(600, 140, 701, 261);
    check("frames-shift-drag-plain-click", added == Outcome::Nothing && ov.frame_count() == 2 && out == Outcome::Commit(a.union(&drawn), Kind::Frames), format!("{out:?}"));
    clear(&mut ov);
    click(&mut ov, 100, 200, true);
    let out = drag(&mut ov, false);
    check("frames-plain-drag-finishes", ov.frame_count() == 2 && out == Outcome::Commit(a.union(&drawn), Kind::Frames), format!("{out:?}"));
    clear(&mut ov);
    drag(&mut ov, true);
    let out = click(&mut ov, 650, 200, true);
    check("frames-toggle-region-no-window", out == Outcome::Nothing && ov.frame_count() == 0, format!("{out:?}"));
    click(&mut ov, 100, 200, true);
    let out = ov.on_key_mods(crate::hookstate::VK_ESCAPE, false, false);
    check("frames-escape-clears", out == Outcome::Cancel && ov.frame_count() == 0 && !ov.fence_clipped(), format!("{out:?}"));
    click(&mut ov, 100, 200, true);
    let out = ov.on_mouse_mods(0, WM_RBUTTONUP, 100, 200, false, false);
    check("frames-right-click-clears", out == Outcome::Cancel && ov.frame_count() == 0, format!("{out:?}"));
    // Different monitor, negative coordinates and a physical gap; reuse Snapshot::crop unchanged.
    click(&mut ov, 100, 200, true);
    click(&mut ov, -200, 200, true);
    let out = ov.on_key_mods(crate::hookstate::VK_RETURN, false, false);
    let cropped = check_crop(&ov.snap, a.union(&c));
    check("frames-cross-monitor-black-gap", out == Outcome::Commit(a.union(&c), Kind::Frames) && cropped.is_ok(), format!("{out:?}; {cropped:?}"));
    check("window-identities-survive-selection", ov.snap.windows.iter().map(|w| w.hwnd).collect::<Vec<_>>() == vec![101, 102, 103], "selection retains the native identities beside controlled bounds".into());
    clear(&mut ov);
    // Switching to text clears any frames; Shift keeps the existing text drag behavior.
    click(&mut ov, 100, 200, true);
    if let Some((x, y)) = ov.mode_item_center(crate::toolbar::Item::ModeText) {
        click(&mut ov, x, y, false);
    }
    let enter = ov.on_key_mods(crate::hookstate::VK_RETURN, false, true);
    let out = drag(&mut ov, true);
    check("frames-text-mode-unchanged", ov.is_text() && ov.frame_count() == 0 && enter == Outcome::Nothing && out == Outcome::TextArea(drawn, false), format!("{enter:?}; {out:?}"));
    results
}

/// The cursor fence through the real overlay: a recording sink stands in for ClipCursor, so
/// nothing touches the user's mouse. Every way a drag can end must leave the sink released.
fn fence_roundtrip(hinst: windows::Win32::Foundation::HINSTANCE) -> Vec<(String, bool, String)> {
    use crate::cursorfence::ClipSink;
    use crate::overlay::{Opts, Outcome, Overlay};
    use std::cell::RefCell;
    use std::rc::Rc;
    use windows::Win32::UI::WindowsAndMessaging::{WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};

    #[derive(Clone, Default)]
    struct Rec(Rc<RefCell<Vec<Option<Rect>>>>);
    impl ClipSink for Rec {
        fn clip(&mut self, r: Rect) {
            self.0.borrow_mut().push(Some(r));
        }
        fn release(&mut self) {
            self.0.borrow_mut().push(None);
        }
    }
    impl Rec {
        fn now(&self) -> Option<Rect> {
            self.0.borrow().last().copied().flatten()
        }
    }

    let mut results = Vec::new();
    let open = |text: bool| -> Option<(Overlay, Rec, Rect, usize)> {
        let snap = capture::freeze(crate::config::CaptureMethod::Gdi, None);
        let prim = snap.monitors.iter().position(|m| m.primary).unwrap_or(0);
        let rect = snap.monitors.get(prim)?.rect;
        let mut o = Opts::plain(0.45);
        o.text = text;
        let mut ov = Overlay::new(snap, hinst, &o).ok()?;
        let rec = Rec::default();
        ov.set_fence_sink(Box::new(rec.clone()));
        ov.synthetic_input = true;
        ov.show().ok()?;
        Some((ov, rec, rect, prim))
    };
    for (mode, text) in [("snip", false), ("text", true)] {
        let Some((mut ov, rec, rect, i)) = open(text) else {
            results.push((format!("fence-{mode}-open"), false, "overlay did not open".to_string()));
            continue;
        };
        // Hover with no button: no clip.
        ov.on_mouse(i, WM_MOUSEMOVE, 50, 50);
        results.push((format!("fence-{mode}-hover-free"), rec.now().is_none() && rec.0.borrow().is_empty(), "no button: no clip".into()));
        // Press clips to the start monitor, moves keep it, release frees it.
        ov.on_mouse(i, WM_LBUTTONDOWN, 100, 100);
        results.push((format!("fence-{mode}-press-clips-to-start-monitor"), rec.now() == Some(rect) && ov.fence_clipped(), format!("{:?} want {rect:?}", rec.now())));
        ov.on_mouse(i, WM_MOUSEMOVE, 300, 200);
        let out = ov.on_mouse(i, WM_LBUTTONUP, 400, 300);
        results.push((format!("fence-{mode}-exit-button-up"), rec.now().is_none() && !ov.fence_clipped() && out != Outcome::Nothing, format!("{out:?}")));
        // Esc in the middle of a drag.
        let Some((mut ov2, rec2, rect2, i2)) = open(text) else { continue };
        ov2.on_mouse(i2, WM_LBUTTONDOWN, 100, 100);
        let held = rec2.now() == Some(rect2);
        let out = ov2.on_key(crate::hookstate::VK_ESCAPE, false);
        results.push((format!("fence-{mode}-exit-escape"), held && rec2.now().is_none() && out == Outcome::Cancel, format!("{out:?}")));
        // Focus loss in the middle of a drag: freed, and not clipped again by the next move.
        let Some((mut ov3, rec3, rect3, i3)) = open(text) else { continue };
        ov3.on_mouse(i3, WM_LBUTTONDOWN, 100, 100);
        let held = rec3.now() == Some(rect3);
        ov3.release_fence();
        ov3.on_mouse(i3, WM_MOUSEMOVE, 150, 150);
        results.push((format!("fence-{mode}-exit-focus-loss"), held && rec3.now().is_none(), "freed, and the next move does not clip".into()));
        ov3.on_mouse(i3, WM_LBUTTONUP, 150, 150);
        ov3.on_mouse(i3, WM_LBUTTONDOWN, 100, 100);
        results.push((format!("fence-{mode}-next-drag-fenced-again"), rec3.now() == Some(rect3), format!("{:?}", rec3.now())));
        // The overlay closes in the middle of a drag (timeout, session end, a panic unwinding).
        let (windows_gone, released) = {
            let hw = ov3.hwnds();
            drop(ov3);
            (hw.iter().all(|h| unsafe { !windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(*h)).as_bool() }), rec3.now().is_none())
        };
        results.push((format!("fence-{mode}-exit-overlay-closed"), windows_gone && released, "dropped mid-drag".into()));
        // A lost button-up: the physical button is not down at the next move (synthetic off, no real button).
        let Some((mut ov4, rec4, rect4, i4)) = open(text) else { continue };
        ov4.on_mouse(i4, WM_LBUTTONDOWN, 100, 100);
        let held = rec4.now() == Some(rect4);
        ov4.synthetic_input = false;
        ov4.on_mouse(i4, WM_MOUSEMOVE, 120, 120);
        results.push((format!("fence-{mode}-exit-lost-button-up"), held && rec4.now().is_none(), "no button down: freed".into()));
        // The crossing key: down at the start, pressed mid-drag, let go.
        let Some((mut ov5, rec5, rect5, i5)) = open(text) else { continue };
        ov5.synthetic_crossing = true;
        ov5.on_mouse(i5, WM_LBUTTONDOWN, 100, 100);
        let free_at_start = rec5.now().is_none() && rec5.0.borrow().is_empty();
        ov5.synthetic_crossing = false;
        ov5.on_mouse(i5, WM_MOUSEMOVE, 120, 120);
        let fenced = rec5.now() == Some(rect5);
        ov5.synthetic_crossing = true;
        ov5.crossing_key_changed();
        let freed = rec5.now().is_none();
        results.push((format!("fence-{mode}-crossing-key"), free_at_start && fenced && freed, format!("at start free {free_at_start}, key up fenced {fenced}, key down freed {freed}")));
        ov5.on_mouse(i5, WM_LBUTTONUP, 120, 120);
    }
    // The editor: choosing a selection and drawing are fenced; moving the selection is not.
    {
        let snap = capture::freeze(crate::config::CaptureMethod::Gdi, None);
        let prim = snap.monitors.iter().position(|m| m.primary).unwrap_or(0);
        let rect = snap.monitors.get(prim).map(|m| m.rect).unwrap_or_default();
        let mut o = Opts::plain(0.45);
        o.editor = Some(crate::editor::Prefs::default());
        if let Ok(mut ov) = Overlay::new(snap, hinst, &o) {
            let rec = Rec::default();
            ov.set_fence_sink(Box::new(rec.clone()));
            ov.synthetic_input = true;
            let _ = ov.show();
            ov.on_mouse(prim, WM_LBUTTONDOWN, 100, 100);
            let select_fenced = rec.now() == Some(rect);
            ov.on_mouse(prim, WM_MOUSEMOVE, 300, 250);
            ov.on_mouse(prim, WM_LBUTTONUP, 400, 300);
            let select_freed = rec.now().is_none();
            results.push(("fence-editor-select".to_string(), select_fenced && select_freed, format!("fenced {select_fenced}, freed at button-up {select_freed}")));
            ov.on_mouse(prim, WM_LBUTTONDOWN, 200, 200);
            let draw_fenced = rec.now() == Some(rect);
            ov.on_mouse(prim, WM_MOUSEMOVE, 250, 220);
            ov.on_mouse(prim, WM_LBUTTONUP, 250, 220);
            let draw_freed = rec.now().is_none();
            results.push(("fence-editor-draw".to_string(), draw_fenced && draw_freed, format!("fenced {draw_fenced} (monitors the selection touches), freed {draw_freed}")));
            ov.on_mouse(prim, WM_LBUTTONDOWN, 200, 200);
            let held = rec.now() == Some(rect);
            ov.on_key(crate::hookstate::VK_ESCAPE, false);
            results.push(("fence-editor-exit-escape".to_string(), held && rec.now().is_none(), "Esc in the middle of a drawing".into()));
            ov.on_mouse(prim, WM_LBUTTONUP, 200, 200);
            let before = rec.0.borrow().len();
            ov.on_mouse_mods(prim, WM_LBUTTONDOWN, 200, 200, false, true); // Ctrl: move the selection
            ov.on_mouse_mods(prim, WM_LBUTTONUP, 210, 200, false, true);
            results.push(("fence-editor-move-not-fenced".to_string(), rec.0.borrow().len() == before, "Ctrl+drag moves the selection, no clip".into()));
        } else {
            results.push(("fence-editor-open".to_string(), false, "overlay did not open".into()));
        }
    }
    results
}

/// Synthetic image and owned HWNDs only. Physical input, actual screen pixels and IME stay separate.
/// Clipboard writes additionally require an explicitly isolated test window station.
pub fn run_editor(isolated_clipboard: bool) -> i32 {
    use crate::editor::{Effect, Prefs};
    use crate::overlay::{Opts, Outcome, Overlay};
    use crate::toolbar::Tool;
    use windows::Win32::Foundation::{HANDLE, HGLOBAL, HINSTANCE};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::StationsAndDesktops::{GetProcessWindowStation, GetUserObjectInformationW, UOI_NAME};
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::w;

    let desk = desktop_name();
    let station = unsafe {
        let mut name = [0u16; 256];
        GetProcessWindowStation().ok().and_then(|s| {
            GetUserObjectInformationW(HANDLE(s.0), UOI_NAME, Some(name.as_mut_ptr() as *mut _), (name.len() * 2) as u32, None).ok()?;
            Some(String::from_utf16_lossy(&name[..name.iter().position(|&x| x == 0).unwrap_or(name.len())]))
        }).unwrap_or_default()
    };
    println!("desktop={desk:?} station={station:?}; synthetic snapshot; no physical keys");
    if desk.is_empty() || desk.eq_ignore_ascii_case("Default") || desk.eq_ignore_ascii_case("Winlogon") {
        println!("REFUSED: --selftest-editor requires a hidden desktop");
        return 4;
    }
    if isolated_clipboard && !station.starts_with("SnipNightTest") {
        println!("REFUSED: clipboard acceptance requires a private SnipNightTest* window station");
        return 4;
    }
    unsafe { let _ = CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED); }
    let mut passed = 0;
    let mut check = |name: &str, ok: bool, detail: &str| -> Result<(), String> {
        if ok { passed += 1; println!("PASS {name}: {detail}"); Ok(()) }
        else { Err(format!("{name}: {detail}")) }
    };
    let result = (|| -> Result<(), String> {
        let hinst: HINSTANCE = unsafe { GetModuleHandleW(None).map_err(|e| e.to_string())?.into() };
        crate::app::register_classes(hinst, crate::app::MAIN_CLASS_TEST).map_err(|e| e.to_string())?;
        let detail = native_text_roundtrip(hinst)?;
        check("editor-native-text", true, &detail)?;
        let bounds = Rect::new(0, 0, 1000, 800);
        let gpu = capture::create_gpu().map_err(|e| e.to_string())?;
        let snap = Snapshot {
            monitors: vec![fake_monitor(bounds, "synthetic-editor")],
            layout: Layout::new(vec![bounds]),
            frames: vec![Frame { width: 1000, height: 800, pixels: Pixels::Cpu(monitor_pixels(bounds)), method: Method::Gdi, ms: 0.0, zero_present: false, note: "synthetic; never captured the screen".into() }],
            gpu: Some(gpu), windows: vec![], gpu_ms: 0.0,
        };
        let mut opts = Opts::plain(0.45); opts.editor = Some(Prefs::default());
        let mut ov = Overlay::new(snap, hinst, &opts).map_err(|e| e.to_string())?;
        // Keep all windows hidden. Methods below are the same production event path with explicit modifiers.
        // v2: while picking the area the magnifier shows the frozen pixel under the cursor.
        ov.on_mouse_mods(0, WM_MOUSEMOVE, 57, 61, false, false);
        let want = ov.frozen_pixel(crate::geometry::Point::new(57, 61)).ok_or("no frozen pixel")?;
        let (at, loupe) = ov.loupe_canvas().ok_or("no magnifier while selecting")?;
        let t = crate::theme::Theme::new(crate::theme::Look::Graphite, crate::theme::Mode::Light);
        let cell = (crate::theme::LOUPE_ZOOM).round() as i32;
        let mid = (t.pad).round() as i32 + (crate::theme::LOUPE_PIXELS as i32 / 2) * cell + cell / 2;
        let shown = loupe.get(mid, mid);
        check("editor-v2-loupe", shown == want && at.x > 57 && at.y > 61, &format!("middle cell {} = frozen pixel at 57,61, panel at {},{}", shown.to_hex(), at.x, at.y))?;
        ov.on_mouse_mods(0, WM_LBUTTONDOWN, 100, 100, false, false);
        ov.on_mouse_mods(0, WM_MOUSEMOVE, 600, 450, false, false);
        ov.on_mouse_mods(0, WM_LBUTTONUP, 600, 450, false, false);
        let selected = Rect::new(100, 100, 601, 451);
        check("editor-selection", ov.editor.as_ref().is_some_and(|e| e.sel == Some(selected)), "inclusive 501x351 synthetic selection")?;
        check("editor-v2-loupe-gone", ov.loupe_canvas().is_none(), "no magnifier once the area is picked and a drawing tool is out")?;
        for tool in [Tool::Arrow, Tool::Rect, Tool::Ellipse, Tool::Line, Tool::Pen, Tool::Marker, Tool::Steps, Tool::Spotlight, Tool::Obscure] {
            if ov.editor.as_ref().unwrap().tool != Some(tool) { ov.on_key_mods(tool.key() as u32, false, false); }
            let before = ov.editor.as_ref().unwrap().doc.shapes().len();
            ov.on_mouse_mods(0, WM_LBUTTONDOWN, 150, 150, false, false);
            ov.on_mouse_mods(0, WM_MOUSEMOVE, 240, 210, false, false);
            ov.on_mouse_mods(0, WM_LBUTTONUP, 240, 210, false, false);
            let e = ov.editor.as_ref().unwrap();
            check(&format!("editor-plain-{:?}", tool), e.sel == Some(selected) && e.doc.shapes().len() > before, "plain left adds annotation without moving selection")?;
        }
        // v2: the colour picker reads the frozen screen, then the tool before it comes back.
        ov.on_key_mods(Tool::Picker.key() as u32, false, false);
        let want = ov.frozen_pixel(crate::geometry::Point::new(420, 380)).ok_or("no frozen pixel")?;
        ov.on_mouse_mods(0, WM_LBUTTONDOWN, 420, 380, false, false);
        ov.on_mouse_mods(0, WM_LBUTTONUP, 420, 380, false, false);
        let e = ov.editor.as_ref().unwrap();
        check("editor-v2-picker", e.color == want && e.tool == Some(Tool::Obscure) && e.doc.shapes().len() == 9, &format!("ink {} from the frozen pixel, previous tool back, no shape added", want.to_hex()))?;
        // v2: the select tool drags the topmost shape (the pixelated box) 40 px right; one undo puts it back.
        ov.on_key_mods(Tool::Select.key() as u32, false, false);
        let before = ov.editor.as_ref().unwrap().doc.shapes().to_vec();
        ov.on_mouse_mods(0, WM_LBUTTONDOWN, 200, 180, false, false);
        ov.on_mouse_mods(0, WM_MOUSEMOVE, 240, 180, false, false);
        ov.on_mouse_mods(0, WM_LBUTTONUP, 240, 180, false, false);
        let e = ov.editor.as_ref().unwrap();
        let top = before.len() - 1;
        let ok = e.picked == Some(top) && e.doc.shapes()[top] == before[top].translated(crate::raster::Pt::new(40.0, 0.0)) && e.doc.shapes()[..top] == before[..top];
        check("editor-v2-select-move", ok, "the topmost shape moved by 40,0, the others stayed")?;
        ov.on_key_mods(0x5A, true, false);
        check("editor-v2-move-undo", ov.editor.as_ref().unwrap().doc.shapes() == before.as_slice(), "Ctrl+Z puts it back")?;
        ov.on_mouse_mods(0, WM_LBUTTONDOWN, 100, 100, false, true);
        ov.on_mouse_mods(0, WM_MOUSEMOVE, 130, 125, false, true);
        ov.on_mouse_mods(0, WM_LBUTTONUP, 130, 125, false, true);
        let moved = selected.offset(30, 25);
        check("editor-ctrl-left-corner", ov.editor.as_ref().unwrap().sel == Some(moved), "Ctrl+left corner moves by +30,+25 with drawing tool active")?;
        ov.on_key_mods(Tool::Arrow.key() as u32, false, false);
        ov.on_key_mods(0x31, false, false);
        let mut stroke_sizes = Vec::new();
        for _ in 0..6 {
            let e = ov.editor.as_ref().unwrap(); stroke_sizes.push(e.size_values()[e.size]);
            ov.on_wheel(120);
        }
        check("editor-six-stroke-sizes", stroke_sizes == crate::editor::STROKE, "2/4/7/10/14/20 DIP selected through key1 and wheel")?;
        ov.on_key_mods(Tool::Text.key() as u32, false, false);
        ov.on_key_mods(0x31, false, false);
        ov.on_mouse_mods(0, WM_LBUTTONDOWN, 300, 260, false, false);
        ov.on_mouse_mods(0, WM_LBUTTONUP, 300, 260, false, false);
        let (edit, _) = ov.text_windows().ok_or("native text box missing")?;
        unsafe { SetWindowTextW(edit, w!("İı ığüşöç 😀\nikinci satır")).map_err(|e| e.to_string())?; }
        let mut text_sizes = Vec::new();
        for _ in 0..6 {
            let e = ov.editor.as_ref().unwrap(); text_sizes.push(e.size_values()[e.size]);
            ov.on_wheel(120);
        }
        check("editor-six-text-sizes", text_sizes == crate::editor::TEXT, "16/22/32/40/56/72 DIP while native text remains open")?;
        check("editor-native-buffer", crate::textinput::read(edit) == "İı ığüşöç 😀\nikinci satır", "Turkish, emoji and multiline native buffer survives every size")?;
        // Ctrl+C while a native edit is focused belongs to text. Commit the annotation first.
        ov.on_text_input(edit, crate::textinput::COMMIT, 0);
        check("editor-text-commit", ov.editor.as_ref().unwrap().text.is_none(), "native annotation committed before whole-image copy")?;
        let out = ov.on_key_mods(0x43, true, false);
        check("editor-ctrl-c-effect", out == Outcome::Edited(Effect::Copy), "Ctrl+C routes to Edited(Copy)")?;
        let (rect, image, _) = ov.editor_result().ok_or("copy returned no edited bitmap")?;
        let (_, _, original) = ov.snap.crop(&rect).map_err(|e| e.to_string())?;
        check("editor-copy-has-edits", image.px != original && image.w == 501 && image.h == 351, "copied 501x351 bitmap contains real rendered annotations")?;
        let png = crate::png::encode_png(image.w, image.h, &image.px).map_err(|e| e.to_string())?;
        let (w, h, px) = crate::png::decode_png(&png).map_err(|e| e.to_string())?;
        check("editor-copy-png", (w, h) == (image.w, image.h) && px == image.px, "lossless PNG has exact edited pixels")?;
        if isolated_clipboard {
            crate::clipboard::set_image(ov.owner(), image.w, image.h, &image.px).map_err(|e| e.to_string())?;
            unsafe {
                use windows::Win32::System::DataExchange::{OpenClipboard, CloseClipboard, GetClipboardData};
                use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock, GlobalSize};
                use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5};
                OpenClipboard(Some(ov.owner())).map_err(|e| e.to_string())?;
                let copied = (|| -> Result<(), String> {
                    for (fmt, expected) in [(CF_DIBV5, crate::dib::dibv5_bytes(w, h, &px)), (CF_DIB, crate::dib::dib24_bytes(w, h, &px))] {
                        let handle = GetClipboardData(fmt.0 as u32).map_err(|e| e.to_string())?;
                        let global = HGLOBAL(handle.0);
                        let size = GlobalSize(global);
                        let ptr = GlobalLock(global) as *const u8;
                        if ptr.is_null() { return Err("clipboard GlobalLock failed".into()); }
                        let same = size >= expected.len() && std::slice::from_raw_parts(ptr, expected.len()) == expected;
                        let _ = GlobalUnlock(global);
                        if !same { return Err(format!("clipboard format {} differs from exact edited pixels", fmt.0)); }
                    }
                    Ok(())
                })();
                let _ = CloseClipboard(); copied?;
            }
            check("editor-isolated-clipboard", true, "real production clipboard helper; DIBV5 and DIB exact edited bytes on private station")?;
        } else { println!("SKIP clipboard: no --isolated-clipboard; user's clipboard never opened"); }
        // v2: the Frame button on the bar frames the result; the picture sits unchanged in the middle.
        let (fx, fy) = ov.editor.as_ref().unwrap().item_center(crate::toolbar::Item::Frame).ok_or("no Frame button")?;
        ov.on_mouse_mods(0, WM_LBUTTONDOWN, fx, fy, false, false);
        ov.on_mouse_mods(0, WM_LBUTTONUP, fx, fy, false, false);
        let (frect, framed, prefs) = ov.editor_result().ok_or("no framed bitmap")?;
        let pad = crate::beautify::FrameLayout::for_size(image.w, image.h, 1.0).pad;
        let same_middle = framed.get(pad + image.w as i32 / 2, pad + image.h as i32 / 2) == image.get(image.w as i32 / 2, image.h as i32 / 2);
        check("editor-v2-frame", prefs.frame && framed.w == image.w + 2 * pad as u32 && frect.left == rect.left - pad && same_middle, &format!("{}x{} framed from {}x{}, pad {pad}", framed.w, framed.h, image.w, image.h))?;
        let hwnds = ov.hwnds(); drop(ov);
        check("editor-owned-windows-close", hwnds.iter().all(|h| unsafe { !IsWindow(Some(*h)).as_bool() }) && unsafe { !IsWindow(Some(edit)).as_bool() }, "overlay and native text HWNDs destroyed after image extraction")?;
        Ok(())
    })();
    match result {
        Ok(()) => { println!("selftest-editor: {passed} passed, 0 failed; physical hotkeys, real screen and App/tray shutdown remain separate"); 0 }
        Err(error) => { println!("FAIL {error}\nselftest-editor: {passed} passed, 1 failed"); 1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn com() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
    }

    #[test]
    fn crop_with_cpu_frames_across_negative_coordinates() {
        cpu_crop().unwrap();
    }

    #[test]
    fn crop_with_mixed_gpu_and_cpu_frames() {
        gpu_crop().unwrap();
    }

    #[test]
    fn png_encode_decode_is_lossless() {
        com();
        png_roundtrip().unwrap();
    }

    #[test]
    fn save_writes_windows_names_atomically() {
        com();
        save_flow().unwrap();
    }

    #[test]
    fn crop_check_fails_on_a_selection_with_no_monitor_pixel() {
        // The zero-case guard: a selection entirely in a gap must not count as a pass.
        let l = small_layout();
        let frames = l
            .monitors
            .iter()
            .map(|r| Frame { width: r.width() as u32, height: r.height() as u32, pixels: Pixels::Cpu(monitor_pixels(*r)), method: Method::Gdi, ms: 0.0, zero_present: false, note: String::new() })
            .collect();
        let snap = Snapshot { monitors: l.monitors.iter().map(|r| fake_monitor(*r, "fake")).collect(), layout: l.clone(), frames, gpu: None, windows: vec![], gpu_ms: 0.0 };
        // x -119..-21, y -5..0 lies left of the MSI and above the BenQ: a gap inside the bounds.
        let gap = Rect::new(-119, -5, -21, 0);
        assert!(l.pieces(&gap).is_empty());
        assert!(check_crop(&snap, gap).is_err());
    }
}
