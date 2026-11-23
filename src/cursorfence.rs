//! Keeps the mouse on the monitor where a drag began, so a selection frame does not run onto the
//! neighbouring monitors (MouseFence cannot stop it: Snip runs elevated, MouseFence does not).
//!
//! Three layers:
//! - `Fence`: the pure rule. A drag has an area (the start monitor); the mouse is clipped to it
//!   while the crossing modifier (Alt) is not held; the clip is released when the drag ends.
//! - `Driver`: a `Fence` that sends its decisions to a `ClipSink`. It always releases on drop.
//! - `os` (Windows): the real sink, `ClipCursor(rect)` and `ClipCursor(NULL)`, and a process-wide
//!   `release_ours()` for the panic hook, the end of the process and a session that died.
//!
//! A stuck clip would trap the user's mouse. Every way out calls `end()`: button up, a finished outcome
//! (commit, cancel, Esc), the overlay closing (drop), focus or capture loss (`suspend()`), a lost
//! button-up (the overlay checks the physical buttons), a panic (hook), the end of the process,
//! and the tray process clearing the clip when a session child died with its overlay up.

use crate::geometry::{Point, Rect};

/// The key that lets a drag cross to another monitor. Alt is free in every overlay mode: Ctrl moves
/// the editor's selection, Shift constrains its shapes and steps the arrow keys by 10. AltGr
/// (Turkish keyboards) arrives as Ctrl+Alt, so it does not count.
pub const CROSSING_KEY_NAME: &str = "Alt";

/// Alt without Ctrl: AltGr is a letter key on a Turkish keyboard, not the crossing key.
pub fn crossing_held(alt: bool, ctrl: bool) -> bool {
    alt && !ctrl
}

/// What a decision asks the OS to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Keep,
    Clip(Rect),
    Release,
}

/// The pure rule. `area` is where the current drag may go.
#[derive(Debug, Default)]
pub struct Fence {
    area: Option<Rect>,
    clipped: bool,
    /// Focus or capture was lost mid-drag: no clip again until the drag is over.
    suspended: bool,
}

impl Fence {
    pub fn area(&self) -> Option<Rect> {
        self.area
    }

    pub fn clipped(&self) -> bool {
        self.clipped
    }

    /// A drag begins in `area`. `free`: the crossing key is held, so no clip yet.
    pub fn begin(&mut self, area: Rect, free: bool) -> Op {
        self.area = Some(area);
        if self.suspended {
            return Op::Keep;
        }
        if free {
            if std::mem::replace(&mut self.clipped, false) { Op::Release } else { Op::Keep }
        } else {
            self.clipped = true;
            Op::Clip(area)
        }
    }

    /// The mouse moved or the key changed. Key down releases the clip; key up clips again only
    /// when the mouse is inside the area (a mouse already on another monitor is not dragged back).
    pub fn update(&mut self, cursor: Point, free: bool) -> Op {
        let Some(area) = self.area else { return Op::Keep };
        if self.suspended {
            return Op::Keep;
        }
        if free && self.clipped {
            self.clipped = false;
            return Op::Release;
        }
        if !free && !self.clipped && area.contains(cursor) {
            self.clipped = true;
            return Op::Clip(area);
        }
        Op::Keep
    }

    /// Focus or capture lost: release now, and stay released until `end()`.
    pub fn suspend(&mut self) -> Op {
        // No drag: nothing to hold back, and a latch now would block the next drag.
        self.suspended = self.area.is_some();
        if std::mem::replace(&mut self.clipped, false) { Op::Release } else { Op::Keep }
    }

    /// The drag is over by any road. Always safe to call again.
    pub fn end(&mut self) -> Op {
        self.area = None;
        self.suspended = false;
        if std::mem::replace(&mut self.clipped, false) { Op::Release } else { Op::Keep }
    }
}

/// Where the clip really happens.
pub trait ClipSink {
    fn clip(&mut self, r: Rect);
    fn release(&mut self);
}

/// A `Fence` wired to a sink. Dropping it releases.
pub struct Driver {
    fence: Fence,
    sink: Box<dyn ClipSink>,
}

impl Driver {
    pub fn new(sink: Box<dyn ClipSink>) -> Driver {
        Driver { fence: Fence::default(), sink }
    }

    /// The real clip.
    #[cfg(windows)]
    pub fn real() -> Driver {
        Driver::new(Box::new(os::OsClip))
    }

    fn run(&mut self, op: Op) {
        match op {
            Op::Keep => {}
            Op::Clip(r) => self.sink.clip(r),
            Op::Release => self.sink.release(),
        }
    }

    pub fn area(&self) -> Option<Rect> {
        self.fence.area()
    }

    pub fn clipped(&self) -> bool {
        self.fence.clipped()
    }

    pub fn begin(&mut self, area: Rect, free: bool) {
        let op = self.fence.begin(area, free);
        self.run(op);
    }

    pub fn update(&mut self, cursor: Point, free: bool) {
        let op = self.fence.update(cursor, free);
        self.run(op);
    }

    pub fn suspend(&mut self) {
        let op = self.fence.suspend();
        self.run(op);
    }

    pub fn end(&mut self) {
        let op = self.fence.end();
        self.run(op);
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.end();
    }
}

/// The editor's drawing area: the monitors its selection touches. Anything drawn outside the
/// selection is invisible, so the mouse need not go further. None: the selection touches no monitor.
pub fn draw_area(sel: &Rect, monitors: &[Rect]) -> Option<Rect> {
    monitors.iter().filter(|m| m.intersect(sel).is_some()).fold(None, |acc: Option<Rect>, m| Some(acc.map_or(*m, |a| a.union(m))))
}

#[cfg(windows)]
pub mod os {
    use super::{ClipSink, Rect};
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{ClipCursor, GetClipCursor};

    /// True while this process holds a clip it set. Only a clip we set is ever released by
    /// `release_ours`, so a game's own clip is not stomped by a stray call.
    static OURS: AtomicBool = AtomicBool::new(false);

    pub struct OsClip;

    impl ClipSink for OsClip {
        fn clip(&mut self, r: Rect) {
            let rc = RECT { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
            // Set the flag first: if the call is cut short, the release still happens.
            OURS.store(true, Ordering::SeqCst);
            if unsafe { ClipCursor(Some(&rc)) }.is_err() {
                crate::logf!("ClipCursor failed; the drag is not fenced");
            }
        }

        fn release(&mut self) {
            release_ours();
        }
    }

    /// Release the clip if this process set it. Safe from a panic hook, a destructor or `main`.
    pub fn release_ours() {
        if OURS.swap(false, Ordering::SeqCst) {
            let _ = unsafe { ClipCursor(None) };
        }
    }

    /// Release whatever clip is set, ours or not: the tray process, when a session child died
    /// with its overlay up and cannot have released it.
    pub fn release_any() {
        OURS.store(false, Ordering::SeqCst);
        let _ = unsafe { ClipCursor(None) };
    }

    pub fn holds() -> bool {
        OURS.load(Ordering::SeqCst)
    }

    /// The clip rectangle now in force (the whole virtual desktop when none is set).
    pub fn read_back() -> Option<Rect> {
        let mut rc = RECT::default();
        unsafe { GetClipCursor(&mut rc) }.ok()?;
        Some(Rect::new(rc.left, rc.top, rc.right, rc.bottom))
    }

    /// Release at a panic, before the abort that `panic = "abort"` makes of it.
    pub fn install_panic_release() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            release_ours();
            previous(info);
        }));
    }
}

/// `snip --selftest-fence`: the real `ClipCursor` and `GetClipCursor`, read back, on a hidden
/// desktop (not the visible one). A private window station cannot do it (ClipCursor is refused there:
/// access denied, measured 2026-11-07), and the clip is shared with the user's real mouse, so the test is
/// short, never touches a clip somebody else holds (a game's), and ends with the clip released.
#[cfg(windows)]
pub fn run_selftest() -> i32 {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::ClipCursor;

    static FAILS: AtomicUsize = AtomicUsize::new(0);
    static PASSES: AtomicUsize = AtomicUsize::new(0);
    fn check(name: &str, ok: bool, detail: String) {
        if ok {
            PASSES.fetch_add(1, Ordering::SeqCst);
            println!("PASS {name}: {detail}");
        } else {
            FAILS.fetch_add(1, Ordering::SeqCst);
            println!("FAIL {name}: {detail}");
        }
    }

    let desk = crate::selftest::desktop_name();
    println!("desktop: {desk:?}");
    if desk.is_empty() || desk.eq_ignore_ascii_case("Default") || desk.eq_ignore_ascii_case("Winlogon") {
        println!("REFUSED: run this on a hidden desktop (not the visible one), never on the user's screen");
        return 4;
    }
    // The desktop of the whole virtual screen is what GetClipCursor returns when nothing is clipped.
    let Some(base) = os::read_back() else {
        println!("FAIL GetClipCursor failed");
        return 1;
    };
    println!("clip before: {base:?}");
    // A game's clip is not ours to touch: stop if the mouse is clipped to less than the virtual screen.
    let screen = {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN};
        let (x, y) = unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN)) };
        let (w, h) = unsafe { (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)) };
        Rect::new(x, y, x + w, y + h)
    };
    if base != screen {
        println!("SKIPPED: the mouse is already clipped to {base:?} (a game?); not touching it");
        return 0;
    }
    let r = Rect::new(base.left + 100, base.top + 100, base.left + 500, base.top + 400);
    let r2 = Rect::new(base.left + 600, base.top + 50, base.left + 900, base.top + 300);
    let inside = Point::new(r.left + 50, r.top + 50);

    // The raw sink.
    let mut sink = os::OsClip;
    sink.clip(r);
    check("clip-rect-read-back", os::read_back() == Some(r), format!("{:?}", os::read_back()));
    sink.release();
    check("release-restores", os::read_back() == Some(base) && !os::holds(), format!("{:?}", os::read_back()));
    if FAILS.load(Ordering::SeqCst) > 0 {
        // The clip does not work here: do not go on and leave one behind.
        os::release_any();
        println!("selftest-fence: {} passed, {} failed", PASSES.load(Ordering::SeqCst), FAILS.load(Ordering::SeqCst));
        return 1;
    }

    // Button up.
    let mut d = Driver::real();
    d.begin(r, false);
    check("drag-clips-to-start-monitor", os::read_back() == Some(r), format!("{:?}", os::read_back()));
    d.update(inside, false);
    check("moves-keep-the-clip", os::read_back() == Some(r), String::new());
    d.end();
    check("exit-button-up", os::read_back() == Some(base), format!("{:?}", os::read_back()));

    // Crossing key, pressed mid-drag and let go.
    d.begin(r, false);
    d.update(inside, true);
    check("crossing-key-down-frees-the-mouse", os::read_back() == Some(base), format!("{:?}", os::read_back()));
    d.update(inside, false);
    check("crossing-key-up-fences-again", os::read_back() == Some(r), format!("{:?}", os::read_back()));
    // Cancel, Esc and commit: the overlay calls end() for any outcome.
    d.end();
    check("exit-outcome-cancel-escape-commit", os::read_back() == Some(base), format!("{:?}", os::read_back()));

    // A second drag on another monitor replaces the first.
    d.begin(r, false);
    d.begin(r2, false);
    check("second-area-replaces-clip", os::read_back() == Some(r2), format!("{:?}", os::read_back()));
    d.end();

    // Focus loss.
    d.begin(r, false);
    d.suspend();
    check("exit-focus-loss", os::read_back() == Some(base), format!("{:?}", os::read_back()));
    d.update(inside, false);
    check("focus-loss-stays-free", os::read_back() == Some(base), String::new());
    d.end();

    // Overlay closed or session ended mid-drag: the driver drops.
    d.begin(r, false);
    drop(d);
    check("exit-overlay-closed-drop", os::read_back() == Some(base), format!("{:?}", os::read_back()));

    // The end of the process and the panic hook call this.
    let mut sink = os::OsClip;
    sink.clip(r);
    os::release_ours();
    check("exit-release-ours", os::read_back() == Some(base) && !os::holds(), format!("{:?}", os::read_back()));

    // A clip this process did not set (a game's) is left alone by release_ours.
    let rc = RECT { left: r2.left, top: r2.top, right: r2.right, bottom: r2.bottom };
    if unsafe { ClipCursor(Some(&rc)) }.is_ok() {
        os::release_ours();
        check("foreign-clip-left-alone", os::read_back() == Some(r2), format!("{:?}", os::read_back()));
        // The tray process clears a dead session's clip whoever set it.
        os::release_any();
        check("exit-session-died-release-any", os::read_back() == Some(base), format!("{:?}", os::read_back()));
    } else {
        check("foreign-clip-left-alone", false, "raw ClipCursor refused".into());
    }

    // The panic hook, last: the release build aborts at a panic, so a hook of this test looks at
    // the clip after ours ran and ends the process with the result.
    os::install_panic_release();
    let ours = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ours(info);
        let after = os::read_back();
        check("exit-panic-hook", after == Some(base), format!("{after:?}"));
        let (p, f) = (PASSES.load(Ordering::SeqCst), FAILS.load(Ordering::SeqCst));
        println!("selftest-fence: {p} passed, {f} failed");
        std::process::exit(if f == 0 && p > 0 { 0 } else { 1 });
    }));
    let mut sink = os::OsClip;
    sink.clip(r);
    panic!("test panic with the mouse clipped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    const A: Rect = Rect::new(0, 0, 1920, 1080);
    const B: Rect = Rect::new(1920, 0, 3840, 1080);

    #[derive(Clone, Default)]
    struct Log(Rc<RefCell<Vec<Option<Rect>>>>);

    impl Log {
        fn driver(&self) -> Driver {
            Driver::new(Box::new(self.clone()))
        }
        fn state(&self) -> Option<Rect> {
            self.0.borrow().last().copied().flatten()
        }
        fn calls(&self) -> usize {
            self.0.borrow().len()
        }
    }

    impl ClipSink for Log {
        fn clip(&mut self, r: Rect) {
            self.0.borrow_mut().push(Some(r));
        }
        fn release(&mut self) {
            self.0.borrow_mut().push(None);
        }
    }

    #[test]
    fn a_drag_clips_to_its_start_monitor_and_button_up_releases() {
        let log = Log::default();
        let mut d = log.driver();
        d.begin(A, false);
        assert_eq!(log.state(), Some(A));
        d.update(Point::new(100, 100), false);
        assert_eq!(log.calls(), 1, "no second clip while nothing changed");
        d.end();
        assert_eq!(log.state(), None);
        assert!(!d.clipped());
    }

    #[test]
    fn end_is_idempotent_and_never_releases_what_was_not_set() {
        let log = Log::default();
        let mut d = log.driver();
        d.end();
        assert_eq!(log.calls(), 0, "no drag: no OS call at all");
        d.begin(A, false);
        d.end();
        d.end();
        assert_eq!(log.calls(), 2);
    }

    #[test]
    fn holding_the_crossing_key_at_the_start_sets_no_clip() {
        let log = Log::default();
        let mut d = log.driver();
        d.begin(A, true);
        assert_eq!(log.calls(), 0);
        d.end();
        assert_eq!(log.calls(), 0);
    }

    #[test]
    fn pressing_the_key_mid_drag_releases_and_letting_go_inside_clips_again() {
        let log = Log::default();
        let mut d = log.driver();
        d.begin(A, false);
        d.update(Point::new(1919, 500), true);
        assert_eq!(log.state(), None, "free to cross");
        // The mouse crosses and the key is let go over the other monitor: not dragged back.
        d.update(Point::new(2500, 500), false);
        assert_eq!(log.state(), None);
        // Back inside the start monitor with the key up: fenced again.
        d.update(Point::new(1000, 500), false);
        assert_eq!(log.state(), Some(A));
        d.end();
        assert_eq!(log.state(), None);
    }

    #[test]
    fn a_finished_outcome_commit_cancel_or_escape_ends_the_drag() {
        // The overlay calls end() for every outcome that is not Nothing; this is that call.
        for _ in ["commit", "cancel", "escape"] {
            let log = Log::default();
            let mut d = log.driver();
            d.begin(B, false);
            assert_eq!(log.state(), Some(B));
            d.end();
            assert_eq!(log.state(), None);
            assert_eq!(d.area(), None);
        }
    }

    #[test]
    fn dropping_the_driver_releases_the_clip() {
        let log = Log::default();
        {
            let mut d = log.driver();
            d.begin(A, false);
            assert_eq!(log.state(), Some(A));
        }
        assert_eq!(log.state(), None, "overlay closed or session ended mid-drag");
    }

    #[test]
    fn dropping_without_a_clip_makes_no_os_call() {
        let log = Log::default();
        drop(log.driver());
        assert_eq!(log.calls(), 0);
    }

    #[test]
    fn focus_loss_releases_and_the_rest_of_the_drag_stays_free() {
        let log = Log::default();
        let mut d = log.driver();
        d.begin(A, false);
        d.suspend();
        assert_eq!(log.state(), None);
        d.update(Point::new(10, 10), false);
        assert_eq!(log.state(), None, "no re-clip while unfocused, even inside the area");
        d.begin(A, false);
        assert_eq!(log.state(), None, "nor by a repeated begin");
        d.end();
        // The next drag is fenced again.
        d.begin(A, false);
        assert_eq!(log.state(), Some(A));
    }

    #[test]
    fn focus_loss_between_drags_does_not_block_the_next_one() {
        let log = Log::default();
        let mut d = log.driver();
        d.suspend();
        d.begin(A, false);
        assert_eq!(log.state(), Some(A));
    }

    #[test]
    fn a_new_drag_in_another_area_replaces_the_clip() {
        let log = Log::default();
        let mut d = log.driver();
        d.begin(A, false);
        d.begin(B, false);
        assert_eq!(log.state(), Some(B));
    }

    #[test]
    fn altgr_is_not_the_crossing_key() {
        assert!(crossing_held(true, false));
        assert!(!crossing_held(true, true), "AltGr arrives as Ctrl+Alt");
        assert!(!crossing_held(false, false));
        assert!(!crossing_held(false, true));
    }

    #[test]
    fn the_drawing_area_is_the_monitors_the_selection_touches() {
        let c = Rect::new(0, 1080, 1920, 2160);
        let mons = [A, B, c];
        assert_eq!(draw_area(&Rect::new(100, 100, 200, 200), &mons), Some(A));
        assert_eq!(draw_area(&Rect::new(1800, 100, 2000, 200), &mons), Some(A.union(&B)));
        assert_eq!(draw_area(&Rect::new(1800, 1000, 2000, 1200), &mons), Some(A.union(&B).union(&c)));
        assert_eq!(draw_area(&Rect::new(9000, 9000, 9100, 9100), &mons), None);
    }

    #[test]
    fn clipped_is_true_only_between_a_clip_and_the_next_release() {
        let mut f = Fence::default();
        let mut set = false;
        let mut track = |op: Op, f: &Fence| {
            match op {
                Op::Clip(_) => set = true,
                Op::Release => set = false,
                Op::Keep => {}
            }
            assert_eq!(set, f.clipped());
        };
        let op = f.begin(A, false);
        track(op, &f);
        let op = f.update(Point::new(5, 5), true);
        track(op, &f);
        let op = f.update(Point::new(5, 5), false);
        track(op, &f);
        let op = f.suspend();
        track(op, &f);
        let op = f.update(Point::new(5, 5), false);
        track(op, &f);
        let op = f.end();
        track(op, &f);
        let op = f.begin(B, true);
        track(op, &f);
        let op = f.update(Point::new(2000, 5), false);
        track(op, &f);
        let op = f.end();
        track(op, &f);
        assert!(!set);
    }
}
