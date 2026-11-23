//! Lazy per-overlay MTA worker. Only geometry crosses the COM/thread boundary.
use crate::geometry::{Point, Rect};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::{IsHungAppWindow, PostMessageW, WM_APP};
use windows::core::Result;

pub const READY: u32 = WM_APP + 160;
pub const TIMER: usize = 0x5350;

#[derive(Clone, Debug)]
pub struct Query {
    pub levels: Vec<Rect>,
    pub ms: f64,
    pub nodes_seen: usize,
}

pub struct Automation {
    api: IUIAutomation2,
}
impl Automation {
    /// Must be constructed and dropped inside the owning MTA.
    pub fn new() -> Result<Self> {
        unsafe {
            let api: IUIAutomation2 =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
            api.SetConnectionTimeout(1000)?;
            api.SetTransactionTimeout(500)?;
            api.SetAutoSetFocus(false)?;
            Ok(Self { api })
        }
    }
    pub fn query(
        &self,
        hwnd: isize,
        point: Point,
        frozen: Rect,
        cancelled: impl Fn() -> bool,
    ) -> Query {
        let started = Instant::now();
        let mut rects = Vec::new();
        let mut nodes_seen = 0;
        let _ = (|| -> Result<()> {
            unsafe {
                let hwnd = HWND(hwnd as *mut _);
                if IsHungAppWindow(hwnd).as_bool() || cancelled() {
                    return Ok(());
                }
                let mut current = self.api.ElementFromHandle(hwnd)?;
                let cache = self.api.CreateCacheRequest()?;
                cache.AddProperty(UIA_BoundingRectanglePropertyId)?;
                cache.SetTreeScope(TreeScope_Element)?;
                let condition = self.api.ControlViewCondition()?;
                let walker = self.api.ControlViewWalker()?;
                for _ in 0..30 {
                    if cancelled() || started.elapsed() >= Duration::from_millis(150) {
                        break;
                    }
                    let parent = current.CurrentBoundingRectangle()?;
                    let parent = Rect::new(parent.left, parent.top, parent.right, parent.bottom);
                    let children =
                        current.FindAllBuildCache(TreeScope_Children, &condition, &cache)?;
                    let mut best: Option<(Rect, IUIAutomationElement)> = None;
                    for i in 0..children.Length()? {
                        if cancelled() || started.elapsed() >= Duration::from_millis(150) {
                            break;
                        }
                        let child = children.GetElement(i)?;
                        nodes_seen += 1;
                        let Ok(r) = child.CachedBoundingRectangle() else {
                            continue;
                        };
                        let r = Rect::new(r.left, r.top, r.right, r.bottom);
                        if r.is_empty() || !r.contains(point) {
                            continue;
                        }
                        if r.area() * 10 > parent.area() * 9
                            && walker.GetFirstChildElement(&child).is_err()
                        {
                            continue;
                        }
                        if best.as_ref().is_none_or(|(b, _)| r.area() < b.area()) {
                            best = Some((r, child));
                        }
                    }
                    let Some((r, child)) = best else { break };
                    rects.push(r);
                    current = child;
                }
                Ok(())
            }
        })();
        Query {
            levels: crate::parts::levels(frozen, &rects),
            ms: started.elapsed().as_secs_f64() * 1000.,
            nodes_seen,
        }
    }
}

#[derive(Clone)]
pub struct Request {
    pub hwnd: isize,
    pub point: Point,
    pub frozen: Rect,
    pub id: u64,
    /// Bounds of the frozen pixels after clipping to the captured desktop.
    pub image_rect: Rect,
    /// Shared frozen pixels; sent on repeat hovers without another readback.
    pub pixels: Option<std::sync::Arc<Vec<u8>>>,
}
#[derive(Clone, Debug)]
pub struct Answer {
    pub id: u64,
    pub query: Option<Query>,
    pub first_ms: Option<f64>,
    pub edges: Option<crate::edges::Split>,
    pub edges_ms: f64,
    pub retry: usize,
}
#[derive(Default)]
struct State {
    latest: Option<Request>,
    answer: Option<Answer>,
    stopped: bool,
}
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}
pub struct Worker {
    shared: Arc<Shared>,
}
impl Worker {
    pub fn start(overlay: isize) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let task = shared.clone();
        std::thread::Builder::new()
            .name("snip-parts".into())
            .spawn(move || {
                let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
                if initialized {
                    // COM interfaces must be dropped before uninitialization.
                    run_worker(&task, overlay, true);
                    unsafe {
                        CoUninitialize();
                    }
                } else {
                    run_worker(&task, overlay, false);
                }
            })?;
        Ok(Self { shared })
    }
    pub fn request(&self, request: Request) {
        let mut state = self.shared.state.lock().unwrap();
        state.latest = Some(request);
        state.answer = None;
        self.shared.wake.notify_one();
    }
    pub fn cancel(&self) {
        let mut state = self.shared.state.lock().unwrap();
        state.latest = None;
        state.answer = None;
        self.shared.wake.notify_one();
    }
    pub fn take(&self) -> Option<Answer> {
        self.shared.state.lock().unwrap().answer.take()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().unwrap();
        state.stopped = true;
        state.latest = None;
        self.shared.wake.notify_one();
        // Never block overlay teardown on an out-of-process COM provider.
    }
}
fn stale(shared: &Shared, id: u64) -> bool {
    let state = shared.state.lock().unwrap();
    state.stopped || state.latest.as_ref().is_none_or(|r| r.id != id)
}
fn publish(shared: &Shared, overlay: isize, answer: Answer) {
    let mut state = shared.state.lock().unwrap();
    if state.stopped || state.latest.as_ref().is_none_or(|r| r.id != answer.id) {
        return;
    }
    state.answer = Some(answer);
    unsafe {
        let _ = PostMessageW(Some(HWND(overlay as *mut _)), READY, WPARAM(0), LPARAM(0));
    }
}
fn run_worker(shared: &Shared, overlay: isize, initialized: bool) {
    let mut automation = None;
    let mut attempted = false;
    let mut completed = 0;
    let mut cache = std::collections::HashMap::new();
    loop {
        let request = {
            let mut state = shared.state.lock().unwrap();
            while !state.stopped && state.latest.as_ref().is_none_or(|r| r.id == completed) {
                state = shared.wake.wait(state).unwrap();
            }
            if state.stopped {
                return;
            }
            state.latest.clone().unwrap()
        };
        completed = request.id;
        let (edges, edges_ms) = cache
            .entry(request.hwnd)
            .or_insert_with(|| {
                let t = Instant::now();
                let split = crate::edges::split(
                    request
                        .pixels
                        .as_deref()
                        .map(|v| v.as_slice())
                        .unwrap_or(&[]),
                    request.image_rect.width() as usize * 4,
                    request.image_rect,
                );
                (split, t.elapsed().as_secs_f64() * 1000.)
            })
            .clone();
        publish(
            shared,
            overlay,
            Answer {
                id: request.id,
                query: None,
                first_ms: None,
                edges: Some(edges.clone()),
                edges_ms,
                retry: 0,
            },
        );
        if !attempted {
            attempted = true;
            if initialized {
                automation = Automation::new().ok();
            }
        }
        let mut start = Instant::now();
        let mut first_ms = None;
        for (retry, delay) in [0, 300, 1000].into_iter().enumerate() {
            if delay != 0 {
                let state = shared.state.lock().unwrap();
                let _ = shared
                    .wake
                    .wait_timeout_while(
                        state,
                        Duration::from_millis(delay).saturating_sub(start.elapsed()),
                        |s| !s.stopped && s.latest.as_ref().is_some_and(|r| r.id == request.id),
                    )
                    .unwrap();
            }
            if stale(shared, request.id) {
                break;
            }
            let query = automation
                .as_ref()
                .map(|a| {
                    a.query(request.hwnd, request.point, request.frozen, || {
                        stale(shared, request.id)
                    })
                })
                .unwrap_or(Query {
                    levels: vec![request.frozen],
                    ms: 0.,
                    nodes_seen: 0,
                });
            if retry == 0 {
                start = Instant::now();
                first_ms = Some(query.ms);
            }
            let useful = query.levels.len() >= 2;
            publish(
                shared,
                overlay,
                Answer {
                    id: request.id,
                    query: Some(query),
                    first_ms,
                    edges: Some(edges.clone()),
                    edges_ms,
                    retry,
                },
            );
            if useful {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn latest_slot_drops_old_results_and_cancel_wakes_worker() {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let worker = Worker {
            shared: shared.clone(),
        };
        worker.request(Request {
            hwnd: 1,
            point: Point::new(-20, 30),
            frozen: Rect::new(-100, 0, 100, 200),
            id: 2,
            image_rect: Rect::new(-100, 0, 100, 200),
            pixels: None,
        });
        assert!(stale(&shared, 1));
        assert!(!stale(&shared, 2));
        publish(
            &shared,
            0,
            Answer {
                id: 1,
                query: None,
                first_ms: None,
                edges: None,
                edges_ms: 0.,
                retry: 0,
            },
        );
        assert!(worker.take().is_none());
        worker.cancel();
        assert!(stale(&shared, 2));
        drop(worker);
        assert!(shared.state.lock().unwrap().stopped);
    }
}
