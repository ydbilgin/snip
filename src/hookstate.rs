//! The decision the low-level keyboard hook makes for each key event, as a pure state machine.
//!
//! A swallowing hook with a wrong state machine eats keystrokes on the whole machine
//! (machine-panel's blank_session.rs warning), so every rule here has a test. Rules:
//! - Only the exact combination Win+Shift+S (no Ctrl, no Alt) triggers; its S key-down is swallowed,
//!   and so are its auto-repeats and its S key-up. Win and Shift themselves always pass.
//! - Win+Shift+R (Snipping Tool's video) and every other key always pass.
//! - Events this process injected (the Win-key mask) pass untouched.
//! - While the overlay is active (and only then), Esc, Enter and the arrow keys are swallowed and
//!   routed to the overlay, and so are their matching key-ups.
//! - Modifier state comes from the caller (GetAsyncKeyState at callback time), never from tracking
//!   key-ups: a lost key-up (secure desktop, Ctrl+Alt+Del) must not leave "Win held" stuck.

pub const VK_S: u32 = 0x53;
pub const VK_R: u32 = 0x52;
pub const VK_ESCAPE: u32 = 0x1B;
pub const VK_RETURN: u32 = 0x0D;
pub const VK_LEFT: u32 = 0x25;
pub const VK_UP: u32 = 0x26;
pub const VK_RIGHT: u32 = 0x27;
pub const VK_DOWN: u32 = 0x28;

/// The overlay keys, in a fixed order for the swallowed-key bitset.
pub const OVERLAY_KEYS: [u32; 6] = [VK_ESCAPE, VK_RETURN, VK_LEFT, VK_UP, VK_RIGHT, VK_DOWN];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub win: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    /// Let the event go on to the rest of the hook chain and the foreground window.
    Pass,
    /// Eat the event.
    Swallow,
    /// Eat the event and start a snip.
    Trigger,
    /// Eat the event and hand the key to the overlay.
    OverlayKey(u32),
}

impl KeyAction {
    pub fn swallows(self) -> bool {
        !matches!(self, KeyAction::Pass)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub vk: u32,
    pub down: bool,
    pub injected_by_us: bool,
    pub mods: Mods,
}

pub const VK_SHIFT: u32 = 0x10;
pub const VK_LSHIFT: u32 = 0xA0;
pub const VK_RSHIFT: u32 = 0xA1;
pub const VK_LWIN: u32 = 0x5B;
pub const VK_RWIN: u32 = 0x5C;

/// Win and Shift are both down now (the second of the two just went down, Ctrl and Alt are up):
/// Win+Shift+S may follow, so the UI thread can create the graphics device before S arrives.
/// `mods` is the state before this event (the key that is going down is not in it yet).
/// Never swallows anything; it is only a hint.
pub fn prewarm_hint(vk: u32, down: bool, injected_by_us: bool, mods: Mods) -> bool {
    if !down || injected_by_us || mods.ctrl || mods.alt {
        return false;
    }
    match vk {
        VK_LWIN | VK_RWIN => mods.shift,
        VK_SHIFT | VK_LSHIFT | VK_RSHIFT => mods.win,
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HookState {
    s_swallowed: bool,
    overlay_swallowed: u8,
}

fn overlay_bit(vk: u32) -> Option<u8> {
    OVERLAY_KEYS.iter().position(|&k| k == vk).map(|i| 1u8 << i)
}

impl HookState {
    pub fn new() -> Self {
        Self::default()
    }

    /// `trigger_enabled`: Win+Shift+S is on in the config. `overlay_active`: an overlay is showing.
    pub fn on_key(&mut self, ev: KeyEvent, trigger_enabled: bool, overlay_active: bool) -> KeyAction {
        if ev.injected_by_us {
            return KeyAction::Pass;
        }
        let m = ev.mods;
        let combo = m.win && m.shift && !m.ctrl && !m.alt;

        if ev.vk == VK_S {
            if ev.down {
                if self.s_swallowed && combo {
                    return KeyAction::Swallow; // auto-repeat of the trigger
                }
                self.s_swallowed = false;
                if trigger_enabled && combo {
                    self.s_swallowed = true;
                    return KeyAction::Trigger;
                }
                return KeyAction::Pass;
            }
            if self.s_swallowed {
                self.s_swallowed = false;
                return KeyAction::Swallow;
            }
            return KeyAction::Pass;
        }

        if let Some(bit) = overlay_bit(ev.vk) {
            if ev.down {
                if overlay_active {
                    self.overlay_swallowed |= bit;
                    return KeyAction::OverlayKey(ev.vk);
                }
                // A new down outside the overlay invalidates a swallowed up lost earlier.
                self.overlay_swallowed &= !bit;
                return KeyAction::Pass;
            }
            if self.overlay_swallowed & bit != 0 {
                self.overlay_swallowed &= !bit;
                return KeyAction::Swallow;
            }
            return KeyAction::Pass;
        }

        KeyAction::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { win: false, shift: false, ctrl: false, alt: false };
    const WS: Mods = Mods { win: true, shift: true, ctrl: false, alt: false };

    fn ev(vk: u32, down: bool, mods: Mods) -> KeyEvent {
        KeyEvent { vk, down, injected_by_us: false, mods }
    }

    /// Runs a sequence and returns the actions; panics if the sequence is empty (no zero-case test).
    fn run(seq: &[(KeyEvent, bool, bool)]) -> Vec<KeyAction> {
        assert!(!seq.is_empty(), "a test sequence with zero events proves nothing");
        let mut s = HookState::new();
        seq.iter().map(|&(e, en, ov)| s.on_key(e, en, ov)).collect()
    }

    #[test]
    fn win_shift_s_triggers_and_swallows_down_repeat_and_up() {
        let a = run(&[
            (ev(0x5B, true, Mods { win: true, ..NONE }), true, false),
            (ev(0xA0, true, WS), true, false),
            (ev(VK_S, true, WS), true, false),
            (ev(VK_S, true, WS), true, false), // auto-repeat
            (ev(VK_S, false, WS), true, false),
            (ev(0xA0, false, Mods { win: true, ..NONE }), true, false),
            (ev(0x5B, false, NONE), true, false),
        ]);
        assert_eq!(
            a,
            vec![KeyAction::Pass, KeyAction::Pass, KeyAction::Trigger, KeyAction::Swallow, KeyAction::Swallow, KeyAction::Pass, KeyAction::Pass]
        );
    }

    #[test]
    fn s_up_is_swallowed_even_after_win_was_released_first() {
        let a = run(&[(ev(VK_S, true, WS), true, false), (ev(VK_S, false, NONE), true, false)]);
        assert_eq!(a, vec![KeyAction::Trigger, KeyAction::Swallow]);
    }

    #[test]
    fn plain_typing_passes() {
        let shift = Mods { shift: true, ..NONE };
        let a = run(&[
            (ev(VK_S, true, NONE), true, false),
            (ev(VK_S, false, NONE), true, false),
            (ev(VK_S, true, shift), true, false),
            (ev(VK_S, false, shift), true, false),
            (ev(VK_S, true, Mods { win: true, ..NONE }), true, false),
            (ev(VK_S, false, Mods { win: true, ..NONE }), true, false),
        ]);
        assert!(a.iter().all(|x| *x == KeyAction::Pass), "{a:?}");
    }

    #[test]
    fn other_modifier_combinations_pass() {
        let cases = [
            Mods { ctrl: true, ..WS },
            Mods { alt: true, ..WS },
            Mods { ctrl: true, alt: true, ..WS },
        ];
        assert!(!cases.is_empty());
        for m in cases {
            let a = run(&[(ev(VK_S, true, m), true, false), (ev(VK_S, false, m), true, false)]);
            assert_eq!(a, vec![KeyAction::Pass, KeyAction::Pass], "{m:?}");
        }
    }

    #[test]
    fn win_shift_r_is_never_touched() {
        let a = run(&[
            (ev(VK_R, true, WS), true, false),
            (ev(VK_R, false, WS), true, false),
            (ev(VK_R, true, WS), true, true), // even with the overlay up
        ]);
        assert!(a.iter().all(|x| *x == KeyAction::Pass), "{a:?}");
    }

    #[test]
    fn disabled_trigger_passes_win_shift_s() {
        let a = run(&[(ev(VK_S, true, WS), false, false), (ev(VK_S, false, WS), false, false)]);
        assert_eq!(a, vec![KeyAction::Pass, KeyAction::Pass]);
    }

    #[test]
    fn our_injected_events_pass() {
        let mut e = ev(VK_S, true, WS);
        e.injected_by_us = true;
        let a = run(&[(e, true, true)]);
        assert_eq!(a, vec![KeyAction::Pass]);
    }

    #[test]
    fn lost_s_key_up_does_not_eat_the_next_plain_s() {
        // Trigger, then the S key-up is lost (secure desktop); later he types a plain "s".
        let a = run(&[(ev(VK_S, true, WS), true, false), (ev(VK_S, true, NONE), true, false), (ev(VK_S, false, NONE), true, false)]);
        assert_eq!(a, vec![KeyAction::Trigger, KeyAction::Pass, KeyAction::Pass]);
    }

    #[test]
    fn overlay_keys_route_only_while_the_overlay_is_active() {
        for vk in OVERLAY_KEYS {
            let a = run(&[(ev(vk, true, NONE), true, false), (ev(vk, false, NONE), true, false)]);
            assert_eq!(a, vec![KeyAction::Pass, KeyAction::Pass], "vk {vk:#x} without overlay");
            let b = run(&[(ev(vk, true, NONE), true, true), (ev(vk, false, NONE), true, true)]);
            assert_eq!(b, vec![KeyAction::OverlayKey(vk), KeyAction::Swallow], "vk {vk:#x} with overlay");
        }
    }

    #[test]
    fn shift_enter_routes_only_with_overlay_and_swallows_its_release() {
        let shift = Mods { shift: true, ..NONE };
        assert_eq!(run(&[
            (ev(VK_RETURN, true, shift), true, false),
            (ev(VK_RETURN, false, shift), true, false),
            (ev(VK_RETURN, true, shift), true, true),
            (ev(VK_RETURN, false, NONE), true, false),
        ]), vec![KeyAction::Pass, KeyAction::Pass, KeyAction::OverlayKey(VK_RETURN), KeyAction::Swallow]);
    }

    #[test]
    fn overlay_key_up_after_the_overlay_closed_is_still_swallowed_once() {
        let a = run(&[
            (ev(VK_ESCAPE, true, NONE), true, true),
            (ev(VK_ESCAPE, false, NONE), true, false), // overlay already gone
            (ev(VK_ESCAPE, true, NONE), true, false),
            (ev(VK_ESCAPE, false, NONE), true, false),
        ]);
        assert_eq!(a, vec![KeyAction::OverlayKey(VK_ESCAPE), KeyAction::Swallow, KeyAction::Pass, KeyAction::Pass]);
    }

    #[test]
    fn orphan_overlay_key_up_passes() {
        let a = run(&[(ev(VK_RETURN, false, NONE), true, true)]);
        assert_eq!(a, vec![KeyAction::Pass]);
    }

    #[test]
    fn audit_lost_overlay_key_up_does_not_eat_the_next_plain_key_up() {
        for vk in OVERLAY_KEYS {
            // The first key-up was lost during a desktop switch; a later plain key must pass.
            let a = run(&[
                (ev(vk, true, NONE), true, true),
                (ev(vk, true, NONE), true, false),
                (ev(vk, false, NONE), true, false),
            ]);
            assert_eq!(a, vec![KeyAction::OverlayKey(vk), KeyAction::Pass, KeyAction::Pass], "vk {vk:#x}");
        }
    }

    #[test]
    fn prewarm_hint_fires_on_the_second_of_win_and_shift() {
        let win = Mods { win: true, ..NONE };
        let shift = Mods { shift: true, ..NONE };
        let cases = [
            (VK_LWIN, true, false, shift, true),
            (VK_RWIN, true, false, shift, true),
            (VK_LSHIFT, true, false, win, true),
            (VK_RSHIFT, true, false, win, true),
            (VK_LWIN, true, false, NONE, false),             // Win alone
            (VK_LSHIFT, true, false, NONE, false),           // Shift alone (typing capitals)
            (VK_LSHIFT, false, false, win, false),           // key-up
            (VK_LSHIFT, true, true, win, false),             // our own injection
            (VK_LSHIFT, true, false, Mods { ctrl: true, ..win }, false),
            (VK_LWIN, true, false, Mods { alt: true, ..shift }, false),
            (VK_S, true, false, WS, false),                  // S is the trigger, not a hint
        ];
        assert!(!cases.is_empty());
        for (vk, down, inj, mods, want) in cases {
            assert_eq!(prewarm_hint(vk, down, inj, mods), want, "vk {vk:#x} down {down} inj {inj} {mods:?}");
        }
    }

    #[test]
    fn letters_and_digits_always_pass() {
        let mut s = HookState::new();
        let mut n = 0;
        for vk in (0x30..=0x39).chain(0x41..=0x5A) {
            if vk == VK_S {
                continue;
            }
            for mods in [NONE, WS] {
                for ov in [false, true] {
                    assert_eq!(s.on_key(ev(vk, true, mods), true, ov), KeyAction::Pass);
                    assert_eq!(s.on_key(ev(vk, false, mods), true, ov), KeyAction::Pass);
                    n += 1;
                }
            }
        }
        assert_eq!(n, 35 * 4);
    }
}
