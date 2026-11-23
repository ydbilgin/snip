//! Freezing the screens: one frame per monitor, taken before any snip window exists.
//!
//! Order of methods (REPORT.txt section 1.3): DXGI Desktop Duplication (DuplicateOutput1, no border,
//! the duplication is closed at once so we never hold one of the four per-session slots), then
//! Windows.Graphics.Capture (CreateForMonitor), then GDI BitBlt. GPU frames stay on the GPU as our own
//! copy of the texture; only the selection is read back at commit.
//!
//! On a still screen Desktop Duplication gives no frame at all on the user's PC: measured 200 ms per monitor
//! for nothing, about 1 s over 4 monitors, then WGC at about 20 ms each. So `freeze` polls every
//! monitor's duplication together under one short budget instead of waiting for each in turn.

use std::time::{Duration, Instant};

use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, Interface, Result};

use crate::config::CaptureMethod;
use crate::geometry::{Layout, Rect};
use crate::log::{ms_since, qpc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Dd,
    Wgc,
    Gdi,
}

impl Method {
    pub fn name(self) -> &'static str {
        match self {
            Method::Dd => "dd",
            Method::Wgc => "wgc",
            Method::Gdi => "gdi",
        }
    }
}

#[derive(Clone, Debug)]
pub struct MonitorInfo {
    pub hmon: HMONITOR,
    pub rect: Rect,
    /// Desktop work area from rcWork (the full rect with an auto-hide taskbar).
    pub work: Rect,
    pub name: String,
    pub dpi: u32,
    pub primary: bool,
}

pub fn monitors() -> Vec<MonitorInfo> {
    unsafe extern "system" fn cb(hmon: HMONITOR, _dc: HDC, _r: *mut RECT, data: LPARAM) -> BOOL {
        unsafe {
            let v = &mut *(data.0 as *mut Vec<MonitorInfo>);
            let mut mi = MONITORINFOEXW::default();
            mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if GetMonitorInfoW(hmon, &mut mi.monitorInfo).as_bool() {
                let r = mi.monitorInfo.rcMonitor;
                let work = mi.monitorInfo.rcWork;
                let len = mi.szDevice.iter().position(|&c| c == 0).unwrap_or(mi.szDevice.len());
                let (mut dx, mut dy) = (96u32, 96u32);
                let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
                v.push(MonitorInfo {
                    hmon,
                    rect: Rect::new(r.left, r.top, r.right, r.bottom),
                    work: Rect::new(work.left, work.top, work.right, work.bottom),
                    name: String::from_utf16_lossy(&mi.szDevice[..len]),
                    dpi: dx,
                    primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                });
            }
            BOOL(1)
        }
    }
    let mut v: Vec<MonitorInfo> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

/// The D3D11 device on the adapter that drives the primary output (Desktop Duplication needs the
/// output's own adapter). Created per snip and dropped after commit, so idle holds no GPU state.
#[derive(Clone)]
pub struct Gpu {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub adapter: IDXGIAdapter1,
    pub factory: IDXGIFactory2,
    pub warp: bool,
}

pub fn create_gpu() -> Result<Gpu> {
    unsafe {
        let factory: IDXGIFactory2 = CreateDXGIFactory1()?;
        let levels = [D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0];
        let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
        let mut device = None;
        let mut context = None;
        let mut warp = false;
        let adapter: IDXGIAdapter1 = factory.EnumAdapters1(0)?;
        let r = D3D11CreateDevice(&adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), flags, Some(&levels), D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context));
        if r.is_err() {
            warp = true;
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), flags, Some(&levels), D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?;
        }
        let device = device.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
        let context = context.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
        let dxgi_dev: IDXGIDevice = device.cast()?;
        let real_adapter: IDXGIAdapter1 = dxgi_dev.GetAdapter()?.cast()?;
        let factory: IDXGIFactory2 = real_adapter.GetParent()?;
        Ok(Gpu { device, context, adapter: real_adapter, factory, warp })
    }
}

impl Gpu {
    /// DXGI outputs of this device's adapter, with their monitor handles.
    pub fn outputs(&self) -> Vec<(HMONITOR, IDXGIOutput)> {
        let mut v = Vec::new();
        unsafe {
            for i in 0.. {
                let Ok(o) = self.adapter.EnumOutputs(i) else { break };
                if let Ok(d) = o.GetDesc()
                    && d.AttachedToDesktop.as_bool() {
                        v.push((d.Monitor, o));
                    }
            }
        }
        v
    }

    fn own_copy(&self, src: &ID3D11Texture2D) -> Result<ID3D11Texture2D> {
        unsafe {
            let mut d = D3D11_TEXTURE2D_DESC::default();
            src.GetDesc(&mut d);
            d.Usage = D3D11_USAGE_DEFAULT;
            d.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
            d.CPUAccessFlags = 0;
            d.MiscFlags = 0;
            d.MipLevels = 1;
            d.ArraySize = 1;
            let mut t = None;
            self.device.CreateTexture2D(&d, None, Some(&mut t))?;
            let t = t.unwrap();
            self.context.CopyResource(&t, src);
            Ok(t)
        }
    }
}

pub enum Pixels {
    /// Our own BGRA texture on `Gpu::device`.
    Gpu(ID3D11Texture2D),
    /// Top-down BGRA, stride width*4, alpha 255.
    Cpu(Vec<u8>),
}

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Pixels,
    pub method: Method,
    pub ms: f64,
    /// Desktop Duplication only: the frame had LastPresentTime == 0 (possibly empty).
    pub zero_present: bool,
    pub note: String,
}

/// One monitor through Desktop Duplication.
pub fn capture_dd(gpu: &Gpu, output: &IDXGIOutput) -> Result<(ID3D11Texture2D, bool)> {
    unsafe {
        let dup: IDXGIOutputDuplication = match output.cast::<IDXGIOutput5>() {
            Ok(o5) => o5.DuplicateOutput1(&gpu.device, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM])?,
            Err(_) => output.cast::<IDXGIOutput1>()?.DuplicateOutput(&gpu.device)?,
        };
        let desc = dup.GetDesc();
        if desc.Rotation != DXGI_MODE_ROTATION_IDENTITY && desc.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED {
            return Err(windows::core::Error::new(DXGI_ERROR_UNSUPPORTED, "rotated output"));
        }
        let mut candidate: Option<ID3D11Texture2D> = None;
        for attempt in 0..3 {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            let timeout = if attempt == 0 { 200 } else { 50 };
            match dup.AcquireNextFrame(timeout, &mut info, &mut res) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => break,
                Err(e) => return Err(e),
            }
            let tex: ID3D11Texture2D = match res {
                Some(r) => r.cast()?,
                None => {
                    let _ = dup.ReleaseFrame();
                    continue;
                }
            };
            let copy = gpu.own_copy(&tex);
            let _ = dup.ReleaseFrame();
            let copy = copy?;
            if info.LastPresentTime != 0 {
                return Ok((copy, false));
            }
            if candidate.is_none() {
                candidate = Some(copy);
            }
        }
        match candidate {
            Some(c) => Ok((c, true)),
            None => Err(windows::core::Error::new(DXGI_ERROR_WAIT_TIMEOUT, "no frame")),
        }
    }
}

/// What Auto waits for Desktop Duplication, over all monitors together: two refreshes at 60 Hz.
const DD_BUDGET_AUTO_MS: f64 = 34.0;
/// `capture = "dd"` asked for this method by name, and its fallback is GDI: wait as long as ever.
const DD_BUDGET_ONLY_MS: f64 = 200.0;

/// One monitor's result from `capture_dd_all`, with the milliseconds it took.
type DdResult = (Result<(ID3D11Texture2D, bool)>, f64);

/// Desktop Duplication on every monitor at once, polled from this thread until `budget_ms` is gone.
/// A frame with LastPresentTime == 0 is kept as a candidate in case nothing better comes. The result
/// is in `monitors` order; every duplication is closed before this returns.
fn capture_dd_all(gpu: &Gpu, outputs: &[(HMONITOR, IDXGIOutput)], monitors: &[MonitorInfo], budget_ms: f64) -> Vec<DdResult> {
    struct Try {
        dup: Option<IDXGIOutputDuplication>,
        candidate: Option<ID3D11Texture2D>,
        result: Option<Result<(ID3D11Texture2D, bool)>>,
        started: i64,
        ms: f64,
    }
    let open = |m: &MonitorInfo| -> Result<IDXGIOutputDuplication> {
        let (_, output) = outputs.iter().find(|(h, _)| *h == m.hmon).ok_or_else(|| windows::core::Error::new(DXGI_ERROR_NOT_FOUND, "output not on the device's adapter"))?;
        unsafe {
            let dup: IDXGIOutputDuplication = match output.cast::<IDXGIOutput5>() {
                Ok(o5) => o5.DuplicateOutput1(&gpu.device, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM])?,
                Err(_) => output.cast::<IDXGIOutput1>()?.DuplicateOutput(&gpu.device)?,
            };
            let desc = dup.GetDesc();
            if desc.Rotation != DXGI_MODE_ROTATION_IDENTITY && desc.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED {
                return Err(windows::core::Error::new(DXGI_ERROR_UNSUPPORTED, "rotated output"));
            }
            Ok(dup)
        }
    };
    let mut tries: Vec<Try> = monitors
        .iter()
        .map(|m| {
            let started = qpc();
            match open(m) {
                Ok(dup) => Try { dup: Some(dup), candidate: None, result: None, started, ms: 0.0 },
                Err(e) => Try { dup: None, candidate: None, result: Some(Err(e)), started, ms: ms_since(started) },
            }
        })
        .collect();
    let t = qpc();
    loop {
        for x in tries.iter_mut().filter(|x| x.result.is_none()) {
            let Some(dup) = &x.dup else { continue };
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            match unsafe { dup.AcquireNextFrame(0, &mut info, &mut res) } {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => continue,
                Err(e) => {
                    x.ms = ms_since(x.started);
                    x.result = Some(Err(e));
                    continue;
                }
            }
            let copy = match res.map(|r| r.cast::<ID3D11Texture2D>()) {
                Some(Ok(tex)) => gpu.own_copy(&tex),
                Some(Err(e)) => Err(e),
                None => {
                    let _ = unsafe { dup.ReleaseFrame() };
                    continue;
                }
            };
            let _ = unsafe { dup.ReleaseFrame() };
            match copy {
                Ok(c) if info.LastPresentTime != 0 => {
                    x.ms = ms_since(x.started);
                    x.result = Some(Ok((c, false)));
                }
                Ok(c) => {
                    if x.candidate.is_none() {
                        x.candidate = Some(c);
                    }
                }
                Err(e) => {
                    x.ms = ms_since(x.started);
                    x.result = Some(Err(e));
                }
            }
        }
        if tries.iter().all(|x| x.result.is_some()) || ms_since(t) >= budget_ms {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    tries
        .into_iter()
        .map(|mut x| {
            x.dup = None;
            let ms = if x.result.is_some() { x.ms } else { ms_since(x.started) };
            let r = x.result.take().unwrap_or_else(|| match x.candidate.take() {
                Some(c) => Ok((c, true)),
                None => Err(windows::core::Error::new(DXGI_ERROR_WAIT_TIMEOUT, "no frame")),
            });
            (r, ms)
        })
        .collect()
}

/// One monitor through Windows.Graphics.Capture. The capture border may flash on that monitor.
pub fn capture_wgc(gpu: &Gpu, hmon: HMONITOR) -> Result<ID3D11Texture2D> {
    unsafe {
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = interop.CreateForMonitor(hmon)?;
        let dxgi_dev: IDXGIDevice = gpu.device.cast()?;
        let d3d: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi_dev)?.cast()?;
        let size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 1, size)?;
        let session = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetIsCursorCaptureEnabled(false);
        session.StartCapture()?;
        let t0 = Instant::now();
        let result = loop {
            if let Ok(frame) = pool.TryGetNextFrame() {
                let surface = frame.Surface()?;
                let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
                let tex: ID3D11Texture2D = access.GetInterface()?;
                let copy = gpu.own_copy(&tex);
                let _ = frame.Close();
                break copy;
            }
            if t0.elapsed() > Duration::from_millis(1000) {
                break Err(windows::core::Error::new(DXGI_ERROR_WAIT_TIMEOUT, "WGC gave no frame in 1 s"));
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        let _ = session.Close();
        let _ = pool.Close();
        result
    }
}

/// One rectangle of the desktop through GDI BitBlt (CPU), alpha forced to 255.
pub fn capture_gdi(r: Rect) -> Result<Vec<u8>> {
    let (w, h) = (r.width(), r.height());
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(windows::core::Error::from_thread());
        }
        let mem = CreateCompatibleDC(Some(screen));
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -h; // top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let result = (|| -> Result<Vec<u8>> {
            let bmp = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let old = SelectObject(mem, bmp.into());
            let ok = BitBlt(mem, 0, 0, w, h, Some(screen), r.left, r.top, SRCCOPY | CAPTUREBLT);
            SelectObject(mem, old);
            let out = if ok.is_ok() && !bits.is_null() {
                let mut v = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize).to_vec();
                for px in v.as_chunks_mut::<4>().0 {
                    px[3] = 255;
                }
                Ok(v)
            } else {
                Err(ok.err().unwrap_or_else(windows::core::Error::from_thread))
            };
            let _ = DeleteObject(bmp.into());
            out
        })();
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        result
    }
}

/// A top-level window the overlay can select, in z-order (topmost first).
#[derive(Clone, Debug)]
pub struct WindowInfo {
    /// Native identity retained for later selection providers.
    pub hwnd: isize,
    pub rect: Rect,
}

/// Visible, uncloaked, not minimised, not click-through top-level windows of other processes,
/// with their DWM frame bounds (no invisible resize border).
pub fn windows_in_z_order() -> Vec<WindowInfo> {
    unsafe extern "system" fn cb(hwnd: HWND, data: LPARAM) -> BOOL {
        unsafe {
            let v = &mut *(data.0 as *mut Vec<WindowInfo>);
            if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                return BOOL(1);
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == std::process::id() {
                return BOOL(1);
            }
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TRANSPARENT.0 != 0 {
                return BOOL(1);
            }
            let mut cloaked = 0u32;
            if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, 4).is_ok() && cloaked != 0 {
                return BOOL(1);
            }
            let mut class = [0u16; 64];
            let n = GetClassNameW(hwnd, &mut class) as usize;
            let class = String::from_utf16_lossy(&class[..n]);
            if class == "Progman" || class == "WorkerW" {
                return BOOL(1);
            }
            let mut r = RECT::default();
            if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, std::mem::size_of::<RECT>() as u32).is_err()
                && GetWindowRect(hwnd, &mut r).is_err() {
                    return BOOL(1);
                }
            let rect = Rect::new(r.left, r.top, r.right, r.bottom);
            if rect.width() > 1 && rect.height() > 1 {
                v.push(WindowInfo { hwnd: hwnd.0 as isize, rect });
            }
            BOOL(1)
        }
    }
    let mut v: Vec<WindowInfo> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

/// All monitors frozen, plus what the overlay needs.
pub struct Snapshot {
    pub monitors: Vec<MonitorInfo>,
    pub layout: Layout,
    pub frames: Vec<Frame>,
    pub gpu: Option<Gpu>,
    pub windows: Vec<WindowInfo>,
    pub gpu_ms: f64,
}

/// Freeze every monitor with `method` (Auto = DD, then WGC, then GDI per monitor). `warm` is a
/// device kept from start-up (config keep_gpu_device); without it a device is created now.
pub fn freeze(method: CaptureMethod, warm: Option<&Gpu>) -> Snapshot {
    let monitors = monitors();
    let layout = Layout::new(monitors.iter().map(|m| m.rect).collect());
    let t = qpc();
    let gpu = match warm {
        Some(g) => Some(g.clone()),
        None => create_gpu().ok(),
    };
    let gpu_ms = ms_since(t);
    let budget = match method {
        CaptureMethod::Auto => Some(DD_BUDGET_AUTO_MS),
        CaptureMethod::Dd => Some(DD_BUDGET_ONLY_MS),
        CaptureMethod::Wgc | CaptureMethod::Gdi => None,
    };
    let dd: Vec<Option<DdResult>> = match (&gpu, budget) {
        (Some(g), Some(ms)) if !g.warp => capture_dd_all(g, &g.outputs(), &monitors, ms).into_iter().map(Some).collect(),
        _ => monitors.iter().map(|_| None).collect(),
    };
    let mut frames = Vec::with_capacity(monitors.len());
    for (m, d) in monitors.iter().zip(dd) {
        frames.push(freeze_one(gpu.as_ref(), d, m, method));
    }
    let windows = windows_in_z_order();
    Snapshot { monitors, layout, frames, gpu, windows, gpu_ms }
}

/// `dd`: this monitor's Desktop Duplication result from `capture_dd_all`.
fn freeze_one(gpu: Option<&Gpu>, mut dd: Option<DdResult>, m: &MonitorInfo, method: CaptureMethod) -> Frame {
    let (w, h) = (m.rect.width() as u32, m.rect.height() as u32);
    let mut notes = Vec::new();
    let order: &[Method] = match method {
        CaptureMethod::Auto => &[Method::Dd, Method::Wgc, Method::Gdi],
        CaptureMethod::Dd => &[Method::Dd, Method::Gdi],
        CaptureMethod::Wgc => &[Method::Wgc, Method::Gdi],
        CaptureMethod::Gdi => &[Method::Gdi],
    };
    for &meth in order {
        let t = qpc();
        let mut dd_ms = None;
        let r: Result<(Pixels, bool)> = match (meth, gpu) {
            (Method::Dd, Some(_)) => match dd.take() {
                Some((r, ms)) => {
                    dd_ms = Some(ms);
                    r.map(|(t, z)| (Pixels::Gpu(t), z))
                }
                None => Err(windows::core::Error::new(DXGI_ERROR_UNSUPPORTED, "no hardware device")),
            },
            (Method::Wgc, Some(g)) => capture_wgc(g, m.hmon).map(|t| (Pixels::Gpu(t), false)),
            (Method::Gdi, _) => capture_gdi(m.rect).map(|v| (Pixels::Cpu(v), false)),
            _ => Err(windows::core::Error::new(DXGI_ERROR_UNSUPPORTED, "no hardware device")),
        };
        let ms = dd_ms.unwrap_or_else(|| ms_since(t));
        match r {
            Ok((pixels, zero_present)) => {
                if let Pixels::Gpu(tex) = &pixels {
                    let mut d = D3D11_TEXTURE2D_DESC::default();
                    unsafe { tex.GetDesc(&mut d) };
                    if d.Width != w || d.Height != h {
                        notes.push(format!("{}: size {}x{} != monitor {}x{}", meth.name(), d.Width, d.Height, w, h));
                        continue;
                    }
                }
                return Frame { width: w, height: h, pixels, method: meth, ms, zero_present, note: notes.join("; ") };
            }
            Err(e) => notes.push(format!("{} failed {:.1}ms: {:#010x} {}", meth.name(), ms, e.code().0, e.message())),
        }
    }
    // Everything failed: a black frame, so the overlay still opens and Esc still works.
    let mut v = vec![0u8; (w * h * 4) as usize];
    for px in v.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    Frame { width: w, height: h, pixels: Pixels::Cpu(v), method: Method::Gdi, ms: 0.0, zero_present: false, note: format!("ALL FAILED, black frame; {}", notes.join("; ")) }
}

impl Snapshot {
    /// The selection as top-down BGRA (alpha 255). Gaps between monitors are black.
    pub fn crop(&self, sel: &Rect) -> Result<(u32, u32, Vec<u8>)> {
        let sel = self.layout.clip(sel).ok_or_else(|| windows::core::Error::new(DXGI_ERROR_INVALID_CALL, "selection is off the desktop"))?;
        let (w, h) = (sel.width() as usize, sel.height() as usize);
        let mut out = vec![0u8; w * h * 4];
        for px in out.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        for p in self.layout.pieces(&sel) {
            let f = &self.frames[p.monitor];
            let pw = p.src.width() as usize;
            let ph = p.src.height() as usize;
            match &f.pixels {
                Pixels::Cpu(v) => {
                    let fs = f.width as usize * 4;
                    for row in 0..ph {
                        let s = (p.src.top as usize + row) * fs + p.src.left as usize * 4;
                        let d = (p.dst_y as usize + row) * w * 4 + p.dst_x as usize * 4;
                        out[d..d + pw * 4].copy_from_slice(&v[s..s + pw * 4]);
                    }
                }
                Pixels::Gpu(tex) => {
                    let gpu = self.gpu.as_ref().expect("GPU frame without a device");
                    unsafe {
                        let mut d = D3D11_TEXTURE2D_DESC::default();
                        tex.GetDesc(&mut d);
                        d.Width = pw as u32;
                        d.Height = ph as u32;
                        d.Usage = D3D11_USAGE_STAGING;
                        d.BindFlags = 0;
                        d.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                        d.MiscFlags = 0;
                        let mut st = None;
                        gpu.device.CreateTexture2D(&d, None, Some(&mut st))?;
                        let st = st.unwrap();
                        let b = D3D11_BOX { left: p.src.left as u32, top: p.src.top as u32, front: 0, right: p.src.right as u32, bottom: p.src.bottom as u32, back: 1 };
                        gpu.context.CopySubresourceRegion(&st, 0, 0, 0, 0, tex, 0, Some(&b));
                        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
                        gpu.context.Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
                        let src = m.pData as *const u8;
                        for row in 0..ph {
                            let s = std::slice::from_raw_parts(src.add(row * m.RowPitch as usize), pw * 4);
                            let dpos = (p.dst_y as usize + row) * w * 4 + p.dst_x as usize * 4;
                            let dst = &mut out[dpos..dpos + pw * 4];
                            dst.copy_from_slice(s);
                            for px in dst.as_chunks_mut::<4>().0 {
                                px[3] = 255;
                            }
                        }
                        gpu.context.Unmap(&st, 0);
                    }
                }
            }
        }
        Ok((w as u32, h as u32, out))
    }

    /// Like `crop`, but a GPU frame is read back once, whole, into `cache`: moving or resizing an
    /// annotated selection then costs a copy per mouse move instead of a GPU readback.
    pub fn crop_cached(&self, sel: &Rect, cache: &mut Vec<Option<Vec<u8>>>) -> Result<(u32, u32, Vec<u8>)> {
        let sel = self.layout.clip(sel).ok_or_else(|| windows::core::Error::new(DXGI_ERROR_INVALID_CALL, "selection is off the desktop"))?;
        cache.resize_with(self.frames.len(), || None);
        let (w, h) = (sel.width() as usize, sel.height() as usize);
        let mut out = vec![0u8; w * h * 4];
        for px in out.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        for p in self.layout.pieces(&sel) {
            let f = &self.frames[p.monitor];
            if matches!(f.pixels, Pixels::Gpu(_)) && cache[p.monitor].is_none() {
                cache[p.monitor] = Some(self.crop(&self.monitors[p.monitor].rect)?.2);
            }
            let v: &[u8] = match &f.pixels {
                Pixels::Cpu(v) => v,
                Pixels::Gpu(_) => cache[p.monitor].as_deref().unwrap_or_default(),
            };
            let (pw, ph, fs) = (p.src.width() as usize, p.src.height() as usize, f.width as usize * 4);
            for row in 0..ph {
                let s = (p.src.top as usize + row) * fs + p.src.left as usize * 4;
                let d = (p.dst_y as usize + row) * w * 4 + p.dst_x as usize * 4;
                out[d..d + pw * 4].copy_from_slice(&v[s..s + pw * 4]);
            }
        }
        Ok((w as u32, h as u32, out))
    }

    pub fn summary(&self) -> String {
        let mut s = format!("gpu={:.2}ms{} ", self.gpu_ms, if self.gpu.as_ref().is_some_and(|g| g.warp) { "(warp)" } else { "" });
        for (m, f) in self.monitors.iter().zip(&self.frames) {
            s.push_str(&format!(
                "[{} {}x{}@{},{} dpi{} {} {:.2}ms{}{}] ",
                m.name, f.width, f.height, m.rect.left, m.rect.top, m.dpi, f.method.name(), f.ms,
                if f.zero_present { " zero-present" } else { "" },
                if f.note.is_empty() { String::new() } else { format!(" ({})", f.note) }
            ));
        }
        s.push_str(&format!("windows={}", self.windows.len()));
        s
    }
}

/// Mean brightness 0..255 of a BGRA buffer (sampled), to flag black frames in the probe.
pub fn mean_luma(bgra: &[u8]) -> f64 {
    let n = bgra.len() / 4;
    if n == 0 {
        return 0.0;
    }
    let step = (n / 50_000).max(1);
    let (mut sum, mut cnt) = (0u64, 0u64);
    let mut i = 0;
    while i < n {
        let p = &bgra[i * 4..i * 4 + 4];
        sum += (p[0] as u64 * 29 + p[1] as u64 * 150 + p[2] as u64 * 77) >> 8;
        cnt += 1;
        i += step;
    }
    sum as f64 / cnt as f64
}

/// HDR state of each output (DXGI_OUTPUT_DESC1 colour space is BT.2020 PQ when HDR is on).
pub fn hdr_outputs(gpu: &Gpu) -> Vec<(HMONITOR, bool, u32)> {
    let mut v = Vec::new();
    for (h, o) in gpu.outputs() {
        if let Ok(o6) = o.cast::<IDXGIOutput6>()
            && let Ok(d) = unsafe { o6.GetDesc1() } {
                v.push((h, d.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020, d.BitsPerColor));
            }
    }
    v
}

/// Read-only parts diagnostic: one monitor via DD, no capture window or WGC border.
/// A still/unavailable desktop may have no DD frame; report that error rather than fake pixels.
pub fn parts_monitor(point: crate::geometry::Point) -> Result<Snapshot> {
    let monitor = monitors().into_iter().find(|m| m.rect.contains(point))
        .ok_or_else(|| windows::core::Error::new(windows::Win32::Foundation::E_FAIL,"no monitor at point"))?;
    let gpu = create_gpu()?;
    let mut results = capture_dd_all(&gpu,&gpu.outputs(),std::slice::from_ref(&monitor),DD_BUDGET_AUTO_MS);
    let (frame,ms) = results.remove(0);
    let (texture,zero_present) = frame?;
    let rect = monitor.rect;
    Ok(Snapshot {
        monitors: vec![monitor], layout: Layout::new(vec![rect]),
        frames: vec![Frame {width:rect.width() as u32,height:rect.height() as u32,pixels:Pixels::Gpu(texture),method:Method::Dd,ms,zero_present,note:String::new()}],
        gpu:Some(gpu),windows:Vec::new(),gpu_ms:0.,
    })
}
