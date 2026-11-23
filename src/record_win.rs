//! Recorder child lifetime, worker and nonactivating controls. The tray never owns media objects.
use crate::geometry::Rect;
use crate::record::{self, Phase, State};
use crate::record_media::Media;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{
    COLORREF, CloseHandle, HWND, LPARAM, LRESULT, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};
pub const WM_STOP: u32 = WM_APP + 90;
pub const TIMER: usize = 90;
pub enum Event {
    Published {
        path: std::path::PathBuf,
        bytes: u64,
        frames: u64,
        w: u32,
        h: u32,
        warning: String,
    },
    Finished {
        gif: bool,
        error: String,
    },
}
pub struct Session {
    pub stop: Arc<AtomicBool>,
    pub state: Arc<Mutex<State>>,
    pub events: mpsc::Receiver<Event>,
    pub sequence: u32,
    windows: Vec<HWND>,
    panel: Option<HWND>,
    pub busy: bool,
    stop_label: &'static str,
}
impl Session {
    pub fn start(
        owner: HWND,
        parent: Option<HWND>,
        monitor: Rect,
        rect: Rect,
        cfg: crate::config::Config,
        sequence: u32,
        started_qpc: i64,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(State {
            phase: Phase::Starting,
            pid: std::process::id(),
            started_qpc,
            ..State::default()
        }));
        let stop_label = if crate::sys::lang(&cfg) == crate::i18n::Lang::Tr {
            "Durdur"
        } else {
            "Stop"
        };
        let (tx, rx) = mpsc::channel();
        let st = state.clone();
        let flag = stop.clone();
        let par = parent.map(|h| h.0 as usize);
        std::thread::spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            let r = work(par, monitor, rect, &cfg, &flag, &st, &tx);
            if let Err(e) = &r
                && let Ok(mut s) = st.lock()
            {
                s.phase = Phase::Error;
                s.error = e.clone();
            }
            save_state(&st);
            let _ = tx.send(Event::Finished {
                gif: r.as_ref().is_ok_and(|g| *g),
                error: r.err().unwrap_or_default(),
            });
            unsafe {
                CoUninitialize();
            }
        });
        let (windows, panel) = indicators(owner, rect, monitor);
        Self {
            stop,
            state,
            events: rx,
            sequence,
            windows,
            panel,
            busy: true,
            stop_label,
        }
    }
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
    pub fn tick(&mut self) {
        if self
            .state
            .lock()
            .is_ok_and(|s| matches!(s.phase, Phase::Finalizing | Phase::Ready | Phase::Error))
        {
            self.hide();
        }
        if let Some(h) = self.panel {
            let ms = self.state.lock().map(|s| s.elapsed_ms).unwrap_or(0);
            let text = format!(
                "{:02}:{:02}   {}",
                ms / 60000,
                ms / 1000 % 60,
                self.stop_label
            );
            let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
            unsafe {
                let _ = SetWindowTextW(h, PCWSTR(text.as_ptr()));
                let _ = InvalidateRect(Some(h), None, true);
            }
        }
    }
    pub fn hide(&mut self) {
        for h in self.windows.drain(..) {
            unsafe {
                let _ = DestroyWindow(h);
            }
        }
        self.panel = None;
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.request_stop();
        self.hide();
    }
}
pub fn progress_path(pid: u32) -> std::path::PathBuf {
    crate::paths::state_dir().join(format!("recorder-{pid}.json"))
}
fn save_state(state: &Mutex<State>) {
    if let Ok(s) = state.lock()
        && let Ok(bytes) = serde_json::to_vec(&*s)
    {
        let _ = crate::naming::write_small(&progress_path(s.pid), &bytes);
    }
}
struct Parent(windows::Win32::Foundation::HANDLE);
impl Drop for Parent {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
fn work(
    parent: Option<usize>,
    monitor: Rect,
    rect: Rect,
    cfg: &crate::config::Config,
    stop: &AtomicBool,
    state: &Mutex<State>,
    tx: &mpsc::Sender<Event>,
) -> Result<bool, String> {
    let parent = if let Some(h) = parent {
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(HWND(h as *mut _), Some(&mut pid));
        }
        Some(Parent(
            unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }.map_err(|e| e.to_string())?,
        ))
    } else {
        None
    };
    let media = Media::start().map_err(|e| e.to_string())?;
    let size = record::fit(rect.width() as u32, rect.height() as u32, 1920, 1080, true)?;
    let hw = qualify(&media, size, true).map(|_| true).or_else(|e| {
        crate::logf!("record hardware-enabled qualification failed: {e}");
        qualify(&media, size, false).map(|_| false)
    })?;
    if stop.load(Ordering::Acquire)
        || parent
            .as_ref()
            .is_some_and(|p| unsafe { WaitForSingleObject(p.0, 0) } == WAIT_OBJECT_0)
    {
        return Err("Recording cancelled before capture".into());
    }
    let output = record::Output::reserve(
        &crate::paths::output_dir(&cfg.folder),
        crate::app::local_stamp(),
    )
    .map_err(|e| e.to_string())?;
    let mut writer = media
        .writer(&output.part, size, hw)
        .map_err(|e| e.to_string())?;
    if let Ok(mut s) = state.lock() {
        s.backend = writer.backend.clone();
    }
    save_state(state);
    let mut capture =
        crate::record_capture::Capture::new(monitor, rect).map_err(|e| e.to_string())?;
    let mut control = record::Control::default();
    control.select();
    control.selected();
    let wait = Instant::now();
    let mut last = None;
    let mut origin = 0;
    let mut warning = String::new();
    let mut repeats = 0u64;
    let mut last_state = Instant::now();
    loop {
        if stop.load(Ordering::Acquire)
            || parent
                .as_ref()
                .is_some_and(|p| unsafe { WaitForSingleObject(p.0, 0) } == WAIT_OBJECT_0)
        {
            break;
        }
        if origin != 0
            && writer.frames > (crate::log::ms_since(origin) * 30. / 1000.).floor() as u64
        {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        let fresh = match capture.frame() {
            Ok(f) => f,
            Err(e) => {
                warning = format!("Capture stopped: {e}");
                break;
            }
        };
        let updated = fresh.is_some();
        if let Some(pixels) = fresh {
            if origin == 0 {
                origin = crate::log::qpc();
            }
            let bgra = record::crop_scale(
                &pixels,
                rect.width() as u32,
                rect.height() as u32,
                rect.width() as usize * 4,
                Rect::from_xywh(0, 0, rect.width(), rect.height()),
                size,
            )?;
            last = Some(record::nv12(&bgra, size.w, size.h)?);
            if control.first_frame()
                && let Ok(mut s) = state.lock()
            {
                s.phase = control.phase;
            }
        }
        let Some(px) = &last else {
            if wait.elapsed().as_millis() >= record::FIRST_FRAME_MS as u128 {
                return Err(
                    "No live WGC frame within 2 s / 2 saniyede canlı kare alınamadı".into(),
                );
            }
            std::thread::sleep(Duration::from_millis(1));
            continue;
        };
        let elapsed = crate::log::ms_since(origin);
        if elapsed >= cfg.record_max_seconds.clamp(1, 300) as f64 * 1000.
            || writer.bytes() >= record::STOP_BYTES
        {
            break;
        }
        let due = (elapsed * 30. / 1000.).floor() as u64;
        if due > writer.frames + 3 {
            warning = "Capture stopped: encoder overloaded (more than 3 frames late)".into();
            break;
        }
        if due >= writer.frames {
            if !updated && writer.frames > 0 {
                repeats += 1;
            }
            writer
                .write(px)
                .map_err(|e| format!("MP4 write failed: {e}"))?;
            if let Ok(mut s) = state.lock() {
                s.frames = writer.frames;
                s.repeats = repeats;
                s.elapsed_ms = elapsed as u64;
                s.bytes = writer.bytes();
            }
            if last_state.elapsed() >= Duration::from_millis(500) {
                save_state(state);
                last_state = Instant::now();
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(capture);
    let frames = writer.frames;
    if let Ok(mut s) = state.lock() {
        control.request_stop();
        control.finalize();
        s.phase = control.phase;
        s.error = warning.clone();
    }
    save_state(state);
    writer
        .finish()
        .map_err(|e| format!("MP4 finalize failed: {e}"))?;
    let bytes =
        record::publish(&output.part, &output.mp4, record::MP4_CAP).map_err(|e| e.to_string())?;
    if let Err(e) = crate::naming::write_small(
        &crate::paths::last_file(),
        output.mp4.to_string_lossy().as_bytes(),
    ) {
        if !warning.is_empty() {
            warning.push_str(" · ");
        }
        warning.push_str(&format!("MP4 saved, last.txt update failed: {e}"));
    }
    if let Ok(mut s) = state.lock() {
        s.output = output.mp4.to_string_lossy().into();
        s.bytes = bytes;
    }
    save_state(state);
    let _ = tx.send(Event::Published {
        path: output.mp4.clone(),
        bytes,
        frames,
        w: size.w,
        h: size.h,
        warning,
    });
    let gif = if cfg.record_gif {
        crate::record_media::export_gif(&media, &output).map(|_| true)?
    } else {
        false
    };
    if let Ok(mut s) = state.lock() {
        s.phase = Phase::Ready;
    }
    Ok(gif)
}
/// Qualification decodes every written sample. Enabling hardware is never reported as proof of use.
/// Its two throwaway files are removed afterwards, whatever the result.
pub fn qualify(media: &Media, size: record::Size, hardware: bool) -> Result<String, String> {
    let dir = crate::paths::state_dir().join("record-qualification");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!(
        "{}-{}-{}.mp4.part",
        std::process::id(),
        hardware,
        crate::log::qpc()
    ));
    let decoded_path = path.with_extension("");
    let result = qualify_at(media, size, hardware, &path, &decoded_path);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&decoded_path);
    result
}
fn qualify_at(
    media: &Media,
    size: record::Size,
    hardware: bool,
    path: &std::path::Path,
    decoded_path: &std::path::Path,
) -> Result<String, String> {
    std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let mut writer = media
        .writer(path, size, hardware)
        .map_err(|e| e.to_string())?;
    let backend = writer.backend.clone();
    for i in 0..3 {
        let p = if i == 0 {
            [0, 0, 0, 255]
        } else {
            [255, 255, 255, 255]
        };
        let nv = record::nv12(&p.repeat(size.w as usize * size.h as usize), size.w, size.h)?;
        writer.write(&nv).map_err(|e| e.to_string())?;
    }
    writer.finish().map_err(|e| e.to_string())?;
    record::publish(path, decoded_path, record::MP4_CAP).map_err(|e| e.to_string())?;
    let mut r = media.reader(decoded_path).map_err(|e| e.to_string())?;
    if (r.w, r.h) != (size.w, size.h) {
        return Err("qualification decode size mismatch".into());
    }
    let mut n = 0;
    while let Some(frame) = r.next_frame().map_err(|e| e.to_string())? {
        let expect = if n == 0 { 16 } else { 235 };
        if frame.pixels[0].abs_diff(expect) > 18 {
            return Err("qualification luma/orientation mismatch".into());
        }
        n += 1;
    }
    if n != 3 {
        return Err(format!("qualification decoded {n}/3 frames"));
    }
    crate::logf!("record qualified: {backend}");
    Ok(backend)
}
unsafe extern "system" fn indicator_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_NCHITTEST if unsafe { GetWindowTextLengthW(hwnd) } == 0 => {
            LRESULT(HTTRANSPARENT as isize)
        }
        WM_LBUTTONUP if unsafe { GetWindowTextLengthW(hwnd) } > 0 => {
            let owner = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
            unsafe {
                let _ = PostMessageW(Some(HWND(owner as *mut _)), WM_STOP, WPARAM(0), LPARAM(0));
            }
            LRESULT(0)
        }
        WM_PAINT => {
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut name = [0u16; 128];
                let n = GetWindowTextW(hwnd, &mut name);
                SetBkMode(dc, TRANSPARENT);
                if n > 0 {
                    let bg = CreateSolidBrush(COLORREF(0x00222222));
                    FillRect(dc, &ps.rcPaint, bg);
                    let _ = DeleteObject(bg.into());
                    let dot = CreateSolidBrush(COLORREF(0x003030ee));
                    let previous = SelectObject(dc, dot.into());
                    let _ = Ellipse(dc, 6, 10, 16, 20);
                    SelectObject(dc, previous);
                    let _ = DeleteObject(dot.into());
                }
                SetTextColor(dc, COLORREF(0x00ffffff));
                let mut rect = ps.rcPaint;
                rect.left += 18;
                DrawTextW(
                    dc,
                    &mut name[..n as usize],
                    &mut rect,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                );
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
fn indicators(owner: HWND, rect: Rect, monitor: Rect) -> (Vec<HWND>, Option<HWND>) {
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else {
            return (vec![], None);
        };
        let brush = CreateSolidBrush(COLORREF(0x003030c0));
        let class = w!("Laureth.snip.record.indicator");
        RegisterClassW(&WNDCLASSW {
            hInstance: module.into(),
            lpfnWndProc: Some(indicator_proc),
            lpszClassName: class,
            hbrBackground: brush,
            ..Default::default()
        });
        let (bars, panel) = record::indicator(rect, monitor);
        let mut windows = Vec::new();
        let mut control = None;
        for (r, is_panel) in bars
            .into_iter()
            .map(|r| (r, false))
            .chain(panel.map(|r| (r, true)))
        {
            if let Ok(h) = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class,
                if is_panel { w!("00:00   Stop") } else { w!("") },
                WS_POPUP,
                r.left,
                r.top,
                r.width(),
                r.height(),
                None,
                None,
                Some(module.into()),
                None,
            ) {
                SetWindowLongPtrW(h, GWLP_USERDATA, owner.0 as isize);
                if let Err(e) = SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE) {
                    crate::logf!("record indicator affinity failed: {e}");
                }
                let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                windows.push(h);
                if is_panel {
                    control = Some(h);
                }
            }
        }
        (windows, control)
    }
}
