//! Additive recorder integration: one dedicated child, ordinary sessions retain their contract.
use super::*;
use crate::record::{Phase, State};
use crate::record_win::{self, Event};
pub(super) const HOTKEY: i32 = 6;
#[derive(Default)]
pub(super) struct Recorder {
    pub child: Option<Child>,
    pub session: Option<record_win::Session>,
    pub state: State,
    pub hotkey: Option<bool>,
    pub stop_pending: bool,
    pub quitting: bool,
    pub sequence: u32,
    pub card_body: String,
    parent_process: Option<ParentProcess>,
}
struct ParentProcess(HANDLE);
impl Drop for ParentProcess {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
impl App {
    pub(super) fn record_register(&mut self) {
        if self.recorder.child.is_some() {
            return;
        } // Keep the stop key while recording/finalizing.
        unsafe {
            let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY);
        }
        self.recorder.hotkey = None;
        if self.opts.no_hook || !self.recording_window.is_invalid() {
            return;
        }
        let key = crate::hotkey::record_binding(
            self.cfg.record_enabled,
            &self.cfg.record_hotkey,
            &self.cfg.plan(),
            self.cfg.snip_enabled,
            self.text_key(),
        );
        if let Some(key) = key {
            self.recorder.hotkey = Some(
                unsafe {
                    RegisterHotKey(
                        Some(self.hwnd),
                        HOTKEY,
                        MOD_NOREPEAT | HOT_KEY_MODIFIERS(key.mods.bits()),
                        key.vk,
                    )
                }
                .is_ok(),
            );
            logf!(
                "record key {} registered={:?}",
                key.text(),
                self.recorder.hotkey
            );
        }
    }
    pub(super) fn record_toggle(&mut self, stop_only: bool) {
        if self.is_session() {
            if let Some(s) = &self.recorder.session {
                s.request_stop();
            } else if self.source == "record" {
                self.handle(Outcome::Cancel);
            }
            return;
        }
        if self.recorder.child.is_some() {
            if !matches!(
                self.recorder.state.phase,
                Phase::Finalizing | Phase::Ready | Phase::Error
            ) {
                self.recorder.stop_pending = true;
                if let Some(c) = &self.recorder.child
                    && let Some(h) = c.hwnd
                    && valid(c, h)
                {
                    send_stop(c);
                }
            }
            return;
        }
        if stop_only
            || !self.cfg.record_enabled
            || self.overlay.is_some()
            || self.child.as_ref().is_some_and(|c| c.overlay)
        {
            return;
        }
        // Retain the ordinary prewarm/finishing child independently.
        let old = self.child.take();
        let started_qpc = qpc();
        let spawned = self.spawn_child(Some((started_qpc, "record")));
        self.recorder.child = self.child.take();
        self.child = old;
        if spawned {
            self.recorder.stop_pending = false;
            self.recorder.state = State {
                phase: Phase::Selecting,
                started_qpc,
                pid: self.recorder.child.as_ref().map_or(0, |c| c.pid),
                ..State::default()
            };
            unsafe {
                SetTimer(Some(self.hwnd), record_win::TIMER, 100, None);
            }
            self.write_state();
        } else {
            self.record_card(
                false,
                "Could not start recording / Kayıt başlatılamadı".into(),
            );
        }
    }
    pub(super) fn record_ready(&mut self, h: HWND, pid: u32) -> bool {
        let Some(c) = &mut self.recorder.child else {
            return false;
        };
        if pid != c.pid || !valid(c, h) {
            return false;
        }
        c.hwnd = Some(h);
        if self.recorder.stop_pending {
            post(h, WM_CLOSE, 0, 0);
        } else if let Some((t, src)) = c.pending.take() {
            unsafe {
                let _ = AllowSetForegroundWindow(c.pid);
            }
            post(h, WM_APP_GO, source_code(src), t as isize);
        }
        true
    }
    pub(super) fn record_overlay(&mut self, flags: usize, pid: u32) -> bool {
        let Some(c) = &mut self.recorder.child else {
            return false;
        };
        if pid != c.pid || c.hwnd.is_none_or(|h| !valid(c, h)) {
            return false;
        }
        c.overlay = flags & 1 != 0;
        hook::set_overlay_active(c.overlay); // Enter/Esc only during selection; never recording.
        true
    }
    pub(super) fn record_commit(&mut self, rect: crate::geometry::Rect) {
        let Some(ov) = self.close_overlay() else {
            return;
        };
        let monitor = ov
            .snap
            .monitors
            .iter()
            .find(|m| m.rect.intersect(&rect) == Some(rect))
            .map(|m| m.rect);
        drop(ov);
        self.warm = None;
        self.steps = None;
        self.restore_focus();
        let Some(monitor) = monitor else {
            self.record_card(false, "Recording area must fit one monitor".into());
            self.session_finish(0);
            return;
        };
        let session = record_win::Session::start(
            self.hwnd,
            self.parent(),
            monitor,
            rect,
            self.cfg.clone(),
            self.recorder.sequence,
            self.recorder.state.started_qpc,
        );
        self.recorder.session = Some(session);
        unsafe {
            SetTimer(Some(self.hwnd), record_win::TIMER, 100, None);
        }
    }
    fn record_card(&self, ok: bool, body: String) {
        use crate::cardview::{CardContent, Mark};
        let tr = self.lang() == crate::i18n::Lang::Tr;
        let title = if ok {
            if tr {
                "Kayıt kaydedildi"
            } else {
                "Recording saved"
            }
        } else if tr {
            "Kayıt sorunu"
        } else {
            "Recording problem"
        };
        let content = CardContent {
            mark: if ok { Mark::Done } else { Mark::Failed },
            title: title.into(),
            body,
            meta: None,
            tip: None,
        };
        crate::card::show(self.hwnd, &content, 5000, &crate::sys::theme(&self.cfg));
    }
    pub(super) fn record_tick(&mut self) {
        let events = if let Some(s) = &mut self.recorder.session {
            s.tick();
            s.events.try_iter().collect::<Vec<_>>()
        } else {
            vec![]
        };
        for event in events {
            match event {
                Event::Published {
                    path,
                    bytes,
                    frames,
                    w,
                    h,
                    warning,
                } => {
                    if let Some(s) = self.recorder.session.as_mut() {
                        s.hide();
                    }
                    let seq = self.recorder.session.as_ref().map_or(0, |s| s.sequence);
                    let copied = if self.cfg.copy_to_clipboard {
                        crate::clipboard::publish_recording(self.hwnd, seq, &path)
                    } else {
                        Ok(false)
                    };
                    let tr = self.lang() == crate::i18n::Lang::Tr;
                    let clip = match copied {
                        Ok(true) => {
                            if tr {
                                " · MP4 panoda"
                            } else {
                                " · MP4 on clipboard"
                            }
                        }
                        Ok(false) => {
                            if self.cfg.copy_to_clipboard {
                                if tr {
                                    " · Pano korundu"
                                } else {
                                    " · Clipboard preserved"
                                }
                            } else {
                                ""
                            }
                        }
                        Err(_) => {
                            if tr {
                                " · Pano yazılamadı"
                            } else {
                                " · Clipboard failed"
                            }
                        }
                    };
                    let seconds = frames / 30;
                    let mb = format!("{:.1}", bytes as f64 / 1_000_000.);
                    let mb = if tr { mb.replace('.', ",") } else { mb };
                    self.recorder.card_body = format!(
                        "{}:{:02} · {w}x{h} · {mb} MB{clip}{}",
                        seconds / 60,
                        seconds % 60,
                        if warning.is_empty() {
                            String::new()
                        } else {
                            format!(" · {warning}")
                        }
                    );
                    self.record_card(warning.is_empty(), self.recorder.card_body.clone());
                }
                Event::Finished { gif, error } => {
                    if let Some(s) = &mut self.recorder.session {
                        s.busy = false;
                        s.hide();
                    }
                    if gif {
                        self.record_card(
                            true,
                            format!(
                                "{} · {}",
                                self.recorder.card_body,
                                if self.lang() == crate::i18n::Lang::Tr {
                                    "GIF hazır"
                                } else {
                                    "GIF ready"
                                }
                            ),
                        );
                    }
                    if !error.is_empty() {
                        self.record_card(
                            false,
                            format!(
                                "{}{}{}",
                                self.recorder.card_body,
                                if self.recorder.card_body.is_empty() {
                                    ""
                                } else {
                                    " · "
                                },
                                error
                            ),
                        );
                    }
                    self.session_finish(if error.is_empty() { 2 } else { 0 });
                }
            }
        }
        if self.is_session() {
            if self
                .recorder
                .parent_process
                .as_ref()
                .is_some_and(|p| unsafe { WaitForSingleObject(p.0, 0) } == WAIT_OBJECT_0)
            {
                self.record_stop_event();
                if self.recorder.session.as_ref().is_none_or(|s| !s.busy) {
                    post(self.hwnd, WM_CLOSE, 0, 0);
                }
            }
            return;
        }
        let mut gone = false;
        if let Some(c) = &self.recorder.child {
            if let Ok(bytes) = std::fs::read(record_win::progress_path(c.pid))
                && let Ok(s) = serde_json::from_slice::<State>(&bytes)
                && s.pid == c.pid
                && s.started_qpc == self.recorder.state.started_qpc
            {
                self.recorder.state = s;
                if !self.recorder.state.output.is_empty()
                    && let Ok(mut last) = LAST_FILE.lock()
                {
                    *last = self.recorder.state.output.clone();
                }
            }
            gone = unsafe { WaitForSingleObject(c.process, 0) } == WAIT_OBJECT_0;
        }
        if gone {
            if let Some(c) = self.recorder.child.take() {
                unsafe {
                    let _ = CloseHandle(c.process);
                }
            }
            hook::set_overlay_active(false);
            if !matches!(self.recorder.state.phase, Phase::Ready | Phase::Error) {
                self.recorder.state.phase = Phase::Error;
                self.recorder.state.error =
                    "Recorder child exited before completion; inspect .part / recorder log".into();
            }
            self.record_register();
        }
        self.write_state();
        if self.recorder.quitting && self.recorder.child.is_none() {
            post(self.hwnd, WM_CLOSE, 0, 0);
        }
    }
    pub(super) fn record_done(&mut self, pid: u32) -> bool {
        if !self.recorder.child.as_ref().is_some_and(|c| c.pid == pid) {
            return false;
        }
        if self.recorder.state.phase == Phase::Selecting {
            self.recorder.state.phase = Phase::Ready;
        }
        hook::set_overlay_active(false);
        self.record_tick();
        true
    }
    pub(super) fn record_begin(&mut self, started_qpc: i64) {
        self.recorder.state = State {
            phase: Phase::Selecting,
            pid: std::process::id(),
            started_qpc,
            ..State::default()
        };
        if let Some(h) = self.parent() {
            let mut pid = 0;
            unsafe {
                GetWindowThreadProcessId(h, Some(&mut pid));
            }
            match unsafe {
                windows::Win32::System::Threading::OpenProcess(
                    windows::Win32::System::Threading::PROCESS_SYNCHRONIZE,
                    false,
                    pid,
                )
            } {
                Ok(handle) => self.recorder.parent_process = Some(ParentProcess(handle)),
                Err(_) => {
                    post(self.hwnd, WM_CLOSE, 0, 0);
                }
            }
        }
        unsafe {
            SetTimer(Some(self.hwnd), record_win::TIMER, 100, None);
        }
    }
    pub(super) fn record_stop_event(&mut self) {
        if self.recorder.child.is_some() {
            self.record_toggle(true);
        }
        if self.recorder.session.is_some() || self.source == "record" {
            self.record_toggle(true);
        }
    }
}
fn valid(c: &Child, h: HWND) -> bool {
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(h, Some(&mut pid));
    }
    pid == c.pid && unsafe { WaitForSingleObject(c.process, 0) } != WAIT_OBJECT_0
}

fn send_stop(c: &Child) -> bool {
    let Some(h) = c.hwnd else {
        return false;
    };
    if !valid(c, h) {
        return false;
    }
    unsafe { PostMessageW(Some(h), record_win::WM_STOP, WPARAM(0), LPARAM(0)).is_ok() }
}
pub(super) fn ipc_selftest() -> Result<serde_json::Value, String> {
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("STATIC"),
            w!("Snip recorder IPC test"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        )
        .map_err(|e| e.to_string())?;
        struct Window(HWND);
        impl Drop for Window {
            fn drop(&mut self) {
                unsafe {
                    let _ = DestroyWindow(self.0);
                }
            }
        }
        let window = Window(hwnd);
        let mut child = Child {
            pid: std::process::id(),
            process: windows::Win32::System::Threading::GetCurrentProcess(),
            hwnd: Some(hwnd),
            pending: None,
            overlay: false,
            editor: false,
            spawned: Instant::now(),
            used: false,
        };
        if !send_stop(&child) {
            return Err("validated HWND did not receive stop".into());
        }
        let mut message = MSG::default();
        if !PeekMessageW(
            &mut message,
            Some(hwnd),
            record_win::WM_STOP,
            record_win::WM_STOP,
            PM_REMOVE,
        )
        .as_bool()
        {
            return Err("numeric stop message missing".into());
        }
        child.pid = child.pid.wrapping_add(1);
        if send_stop(&child) {
            return Err("wrong HWND/PID accepted".into());
        }
        child.pid = std::process::id();
        child.hwnd = None;
        if send_stop(&child) {
            return Err("stop before READY was sent to no HWND".into());
        }
        child.hwnd = Some(hwnd);
        drop(window);
        if send_stop(&child) {
            return Err("destroyed HWND accepted".into());
        }
        Ok(
            serde_json::json!({"numeric_stop_delivered":true,"wrong_pid_rejected":true,"destroyed_hwnd_rejected":true,"unready_hwnd_rejected":true}),
        )
    }
}
