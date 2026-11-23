//! The text capture hotkey in the keyboard hook, as a pure state machine (WinTextCapture's rules).
//!
//! The key lives in the hook, not RegisterHotKey, for the same reason Win+Shift+S does: Windows
//! keeps Win+Shift+T for itself, and the hook is called before it. Rules, each with a test:
//! - Only the exact combination triggers; other modifiers pass.
//! - The trigger's auto-repeats and its key-up are eaten, whatever the modifiers are by then, and
//!   even if the binding changed while it was held.
//! - Every other key passes, also while the trigger is held.
//! - Events this process injected pass.
//! - No binding (text capture off, or a bad key in the config): nothing is ever eaten.
//!
//! The recorder (settings window, "press the new keys") also lives here: a key held with a modifier
//! goes to the recorder instead of Windows, so a combination Windows keeps can be recorded. Modifiers
//! pass, a bare key passes (the window gets it), task switching (Alt+Tab, Alt+Esc, Alt+F4, Ctrl+Esc) and
//! Esc always pass.

use crate::hookstate::Mods;
use crate::hotkey::{HkMods, Hotkey};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAction {
    Pass,
    Swallow,
    Trigger,
}

pub const MODIFIER_VKS: [u32; 11] = [0x10, 0x11, 0x12, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0x5B, 0x5C];

pub fn hk_mods(m: Mods) -> HkMods {
    HkMods { ctrl: m.ctrl, alt: m.alt, shift: m.shift, win: m.win }
}

/// The modifier bits one atomic carries from the settings to the hook.
pub fn pack(hk: Option<Hotkey>) -> u64 {
    match hk {
        Some(h) => (h.vk as u64) | (h.mods.bits() as u64) << 32,
        None => 0,
    }
}

pub fn unpack(v: u64) -> Option<Hotkey> {
    let vk = v as u32;
    if vk == 0 {
        return None;
    }
    let b = (v >> 32) as u32;
    Some(Hotkey::new(HkMods { alt: b & 1 != 0, ctrl: b & 2 != 0, shift: b & 4 != 0, win: b & 8 != 0 }, vk))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextKeyState {
    /// The trigger key still held (0: none).
    active: u32,
}

impl TextKeyState {
    pub const fn new() -> Self {
        Self { active: 0 }
    }

    /// Does this key concern the text rules at all? Two compares: all the cost other keys pay.
    pub fn routes(&self, vk: u32, binding: Option<Hotkey>) -> bool {
        vk == self.active || binding.is_some_and(|b| b.vk == vk)
    }

    pub fn evaluate(&mut self, binding: Option<Hotkey>, vk: u32, down: bool, injected_by_us: bool, mods: Mods) -> TextAction {
        if injected_by_us {
            return TextAction::Pass;
        }
        if !down {
            if vk == self.active {
                self.active = 0;
                return TextAction::Swallow;
            }
            return TextAction::Pass;
        }
        if self.active != 0 {
            return if vk == self.active { TextAction::Swallow } else { TextAction::Pass };
        }
        if binding.is_some_and(|b| b.vk == vk && b.mods == hk_mods(mods)) {
            self.active = vk;
            return TextAction::Trigger;
        }
        TextAction::Pass
    }

    /// The recorder. Trigger: this chord goes to the settings window.
    pub fn record(&mut self, vk: u32, down: bool, injected_by_us: bool, mods: Mods) -> TextAction {
        if injected_by_us || MODIFIER_VKS.contains(&vk) {
            return TextAction::Pass;
        }
        if !down {
            if vk == self.active {
                self.active = 0;
                return TextAction::Swallow;
            }
            return TextAction::Pass;
        }
        if self.active != 0 {
            return if vk == self.active { TextAction::Swallow } else { TextAction::Pass };
        }
        const TAB: u32 = 0x09;
        const ESC: u32 = 0x1B;
        const F4: u32 = 0x73;
        let task_switch = (mods.alt && matches!(vk, TAB | ESC | F4)) || (mods.ctrl && vk == ESC);
        if !(mods.ctrl || mods.alt || mods.shift || mods.win) || task_switch || vk == ESC {
            return TextAction::Pass;
        }
        self.active = vk;
        TextAction::Trigger
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hookstate::{HookState, KeyAction, KeyEvent, VK_R, VK_S};

    const WS: Mods = Mods { win: true, shift: true, ctrl: false, alt: false };
    const VK_T: u32 = 0x54;

    fn wst() -> Option<Hotkey> {
        Some(Hotkey::parse("Win+Shift+T").unwrap())
    }

    #[test]
    fn default_key_triggers_once_eats_repeats_and_keyup_then_works_again() {
        let mut s = TextKeyState::new();
        assert_eq!(s.evaluate(wst(), VK_T, true, false, WS), TextAction::Trigger);
        assert_eq!(s.evaluate(wst(), VK_T, true, false, WS), TextAction::Swallow, "repeat");
        assert_eq!(s.evaluate(wst(), VK_T, false, false, WS), TextAction::Swallow, "up");
        assert_eq!(s.evaluate(wst(), VK_T, true, false, WS), TextAction::Trigger, "the next press");
    }

    #[test]
    fn other_modifiers_pass() {
        for mods in [Mods::default(), Mods { win: true, ..Mods::default() }, Mods { shift: true, ..Mods::default() }, Mods { ctrl: true, ..WS }, Mods { alt: true, ..WS }] {
            let mut s = TextKeyState::new();
            assert_eq!(s.evaluate(wst(), VK_T, true, false, mods), TextAction::Pass, "{mods:?}");
            assert_eq!(s.evaluate(wst(), VK_T, false, false, mods), TextAction::Pass, "{mods:?}");
        }
    }

    #[test]
    fn no_binding_eats_nothing() {
        let mut s = TextKeyState::new();
        assert_eq!(s.evaluate(None, VK_T, true, false, WS), TextAction::Pass);
        assert_eq!(s.evaluate(None, VK_T, false, false, WS), TextAction::Pass);
        assert!(!s.routes(VK_T, None));
    }

    #[test]
    fn repeats_stay_eaten_after_the_modifiers_are_released() {
        let mut s = TextKeyState::new();
        assert_eq!(s.evaluate(wst(), VK_T, true, false, WS), TextAction::Trigger);
        assert_eq!(s.evaluate(wst(), VK_T, true, false, Mods::default()), TextAction::Swallow);
        assert_eq!(s.evaluate(wst(), VK_T, false, false, Mods::default()), TextAction::Swallow);
        assert_eq!(s.evaluate(wst(), VK_T, true, false, Mods::default()), TextAction::Pass, "a bare press after it");
    }

    #[test]
    fn the_held_key_stays_eaten_after_the_binding_changed() {
        let mut s = TextKeyState::new();
        let other = Some(Hotkey::parse("Ctrl+Alt+P").unwrap());
        assert_eq!(s.evaluate(wst(), VK_T, true, false, WS), TextAction::Trigger);
        assert!(s.routes(VK_T, other), "still held");
        assert_eq!(s.evaluate(other, VK_T, true, false, WS), TextAction::Swallow);
        assert_eq!(s.evaluate(other, VK_T, false, false, WS), TextAction::Swallow);
        assert_eq!(s.evaluate(other, 0x50, true, false, Mods { ctrl: true, alt: true, ..Mods::default() }), TextAction::Trigger, "the new one works");
    }

    #[test]
    fn every_other_key_passes_while_the_trigger_is_held() {
        let mut s = TextKeyState::new();
        assert_eq!(s.evaluate(wst(), VK_T, true, false, WS), TextAction::Trigger);
        assert_eq!(s.evaluate(wst(), 0x41, true, false, WS), TextAction::Pass);
        assert_eq!(s.evaluate(wst(), 0x41, false, false, WS), TextAction::Pass);
    }

    #[test]
    fn injected_events_pass() {
        let mut s = TextKeyState::new();
        assert_eq!(s.evaluate(wst(), VK_T, true, true, WS), TextAction::Pass);
        assert_eq!(s.evaluate(wst(), VK_T, false, true, WS), TextAction::Pass);
    }

    /// With a text key set, Win+Shift+S, Win+Shift+R and the overlay keys answer as before.
    #[test]
    fn snip_rules_are_untouched_by_the_text_key() {
        let mut h = HookState::new();
        let mut t = TextKeyState::new();
        let ev = |vk, down| KeyEvent { vk, down, injected_by_us: false, mods: WS };
        assert_eq!(h.on_key(ev(VK_S, true), true, false), KeyAction::Trigger);
        assert!(!t.routes(VK_S, wst()));
        assert_eq!(h.on_key(ev(VK_S, false), true, false), KeyAction::Swallow);
        assert_eq!(h.on_key(ev(VK_R, true), true, false), KeyAction::Pass);
        assert!(!t.routes(VK_R, wst()));
        assert_eq!(t.evaluate(wst(), VK_R, true, false, WS), TextAction::Pass, "Win+Shift+R is never eaten");
    }

    #[test]
    fn the_recorder_takes_chords_and_leaves_the_rest() {
        let mut s = TextKeyState::new();
        assert_eq!(s.record(0x5B, true, false, Mods { win: true, ..Mods::default() }), TextAction::Pass, "a modifier");
        assert_eq!(s.record(VK_T, true, false, WS), TextAction::Trigger);
        assert_eq!(s.record(VK_T, true, false, WS), TextAction::Swallow, "repeat");
        assert_eq!(s.record(VK_T, false, false, WS), TextAction::Swallow, "up");
        assert_eq!(s.record(0x2C, true, false, Mods::default()), TextAction::Pass, "bare PrtScn goes to the window");
        assert_eq!(s.record(0x1B, true, false, Mods::default()), TextAction::Pass, "Esc cancels in the window");
        let alt = Mods { alt: true, ..Mods::default() };
        for vk in [0x09, 0x1B, 0x73] {
            assert_eq!(s.record(vk, true, false, alt), TextAction::Pass, "task switching {vk:#x}");
        }
        assert_eq!(s.record(0x1B, true, false, Mods { ctrl: true, ..Mods::default() }), TextAction::Pass, "Ctrl+Esc");
    }

    #[test]
    fn a_binding_survives_the_atomic() {
        let h = Hotkey::parse("Ctrl+Alt+F9").unwrap();
        assert_eq!(unpack(pack(Some(h))), Some(h));
        assert_eq!(unpack(pack(None)), None);
        assert_eq!(unpack(pack(wst())), wst());
    }
}
