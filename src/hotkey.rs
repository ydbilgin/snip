//! The user-assignable hotkeys: text form ("Ctrl+Shift+A", "PrtScn") to modifiers and a virtual
//! key, back to text, and the rules for which ones are allowed.
//!
//! Win+Shift+S stays with the low-level hook (`hookstate.rs`); every assignable key goes through
//! RegisterHotKey, which never swallows anything but its own combination. Refused:
//! - Win+Shift+S (already ours) and Win+Shift+R (Snipping Tool's screen recording, never touched);
//! - a letter, digit, Space or navigation key without Ctrl, Alt or Win (it would block typing);
//! - Alt+F4 and Ctrl+Alt+Delete.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct HkMods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

impl HkMods {
    /// RegisterHotKey's MOD_ALT | MOD_CONTROL | MOD_SHIFT | MOD_WIN bits.
    pub fn bits(self) -> u32 {
        (self.alt as u32) | (self.ctrl as u32) << 1 | (self.shift as u32) << 2 | (self.win as u32) << 3
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub mods: HkMods,
    pub vk: u32,
}

pub const VK_SNAPSHOT: u32 = 0x2C;

/// Plain snipping can route its few navigation keys through the hook. The activated editor
/// owns keyboard focus, so its native text control must receive caret and IME keys directly.
pub fn route_overlay_keys(overlay_up: bool, editor: bool, dialog_open: bool) -> bool {
    overlay_up && !editor && !dialog_open
}

/// What a new snip request does with the current session child. A child serves one snip: once
/// its overlay has closed it may still be writing the PNG and then exits, so a request in that
/// moment must start a new child (reusing it lost the key press, or closed the new overlay when
/// the first save finished).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionStep {
    /// An overlay is up: ignore the request.
    Ignore,
    /// A prewarmed child is waiting: use it.
    Reuse,
    /// No usable child: start one (retire the old one first when `retire`).
    Spawn { retire: bool },
}

pub fn session_step(child: bool, overlay_up: bool, used: bool) -> SessionStep {
    match (child, overlay_up, used) {
        (false, _, _) => SessionStep::Spawn { retire: false },
        (true, true, _) => SessionStep::Ignore,
        (true, false, true) => SessionStep::Spawn { retire: true },
        (true, false, false) => SessionStep::Reuse,
    }
}

#[cfg(test)]
mod routing_tests {
    use crate::hookstate::{HookState, KeyAction, KeyEvent, Mods, OVERLAY_KEYS, VK_R, VK_S};

    #[test]
    fn a_used_child_never_gets_a_second_snip() {
        use super::{SessionStep, session_step};
        assert_eq!(session_step(false, false, false), SessionStep::Spawn { retire: false });
        assert_eq!(session_step(true, false, false), SessionStep::Reuse);
        assert_eq!(session_step(true, true, false), SessionStep::Ignore);
        assert_eq!(session_step(true, false, true), SessionStep::Spawn { retire: true });
    }

    #[test]
    fn plain_navigation_routes_but_editor_caret_and_ime_keys_pass() {
        for (name, overlay, editor, dialog, routes) in [
            ("closed", false, false, false, false),
            ("plain", true, false, false, true),
            ("native text/editor", true, true, false, false),
            ("plain system dialog", true, false, true, false),
            ("editor system dialog", true, true, true, false),
        ] {
            let mut hook = HookState::new();
            let route = super::route_overlay_keys(overlay, editor, dialog);
            for vk in OVERLAY_KEYS {
                let event = |down| KeyEvent { vk, down, injected_by_us: false, mods: Mods::default() };
                let want = if routes { KeyAction::OverlayKey(vk) } else { KeyAction::Pass };
                assert_eq!(hook.on_key(event(true), true, route), want, "{name}: {vk} down");
                let want = if routes { KeyAction::Swallow } else { KeyAction::Pass };
                assert_eq!(hook.on_key(event(false), true, route), want, "{name}: {vk} up");
            }
            // Narrowing editor navigation must not change either Windows screenshot shortcut.
            let mods = Mods { win: true, shift: true, ..Mods::default() };
            let event = |vk, down| KeyEvent { vk, down, injected_by_us: false, mods };
            assert_eq!(hook.on_key(event(VK_S, true), true, route), KeyAction::Trigger, "{name}");
            assert_eq!(hook.on_key(event(VK_S, false), true, route), KeyAction::Swallow, "{name}");
            assert_eq!(hook.on_key(event(VK_R, true), true, route), KeyAction::Pass, "{name}");
        }
    }
}

/// Named keys (canonical name first among aliases for the same code).
const NAMED: &[(&str, u32)] = &[
    ("PrtScn", VK_SNAPSHOT),
    ("PrintScreen", VK_SNAPSHOT),
    ("Print", VK_SNAPSHOT),
    ("PrtSc", VK_SNAPSHOT),
    ("Pause", 0x13),
    ("ScrollLock", 0x91),
    ("Insert", 0x2D),
    ("Ins", 0x2D),
    ("Delete", 0x2E),
    ("Del", 0x2E),
    ("Home", 0x24),
    ("End", 0x23),
    ("PageUp", 0x21),
    ("PgUp", 0x21),
    ("PageDown", 0x22),
    ("PgDn", 0x22),
    ("Space", 0x20),
];

fn key_name(vk: u32) -> Option<String> {
    match vk {
        0x41..=0x5A | 0x30..=0x39 => Some(char::from_u32(vk).unwrap().to_string()),
        0x70..=0x87 => Some(format!("F{}", vk - 0x6F)),
        _ => NAMED.iter().find(|(_, v)| *v == vk).map(|(n, _)| n.to_string()),
    }
}

fn key_code(name: &str) -> Option<u32> {
    let up = name.to_ascii_uppercase();
    if up.len() == 1 {
        let c = up.as_bytes()[0];
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(c as u32);
        }
    }
    if let Some(n) = up.strip_prefix('F')
        && let Ok(n) = n.parse::<u32>()
        && (1..=24).contains(&n)
    {
        return Some(0x6F + n);
    }
    NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| *v)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyError {
    Empty,
    UnknownKey(String),
    NoKey,
    Reserved(&'static str),
    BlocksTyping,
}

impl std::fmt::Display for HotkeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HotkeyError::Empty => write!(f, "empty"),
            HotkeyError::UnknownKey(k) => write!(f, "unknown key \"{k}\""),
            HotkeyError::NoKey => write!(f, "only modifiers, no key"),
            HotkeyError::Reserved(why) => write!(f, "reserved: {why}"),
            HotkeyError::BlocksTyping => write!(f, "a letter, digit or navigation key needs Ctrl, Alt or Win"),
        }
    }
}

impl Hotkey {
    pub const fn new(mods: HkMods, vk: u32) -> Self {
        Self { mods, vk }
    }

    /// "Ctrl+Alt+Shift+Win+Key" (only the modifiers that are set).
    pub fn text(&self) -> String {
        let mut s = String::new();
        for (on, n) in [(self.mods.ctrl, "Ctrl"), (self.mods.alt, "Alt"), (self.mods.shift, "Shift"), (self.mods.win, "Win")] {
            if on {
                s.push_str(n);
                s.push('+');
            }
        }
        s.push_str(&key_name(self.vk).unwrap_or_else(|| format!("VK{:#04X}", self.vk)));
        s
    }

    /// The way people say it: "Win+Shift+T", "Ctrl+Alt+P". Parses back to the same key.
    pub fn display(&self) -> String {
        let mut s = String::new();
        for (on, n) in [(self.mods.win, "Win"), (self.mods.ctrl, "Ctrl"), (self.mods.alt, "Alt"), (self.mods.shift, "Shift")] {
            if on {
                s.push_str(n);
                s.push('+');
            }
        }
        s.push_str(&key_name(self.vk).unwrap_or_else(|| format!("VK{:#04X}", self.vk)));
        s
    }

    /// Parse and check. Case and spaces do not matter; "Control" and "Windows" are accepted.
    pub fn parse(s: &str) -> Result<Hotkey, HotkeyError> {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        if s.is_empty() {
            return Err(HotkeyError::Empty);
        }
        let mut mods = HkMods::default();
        let mut key = None;
        for part in s.split('+') {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods.ctrl = true,
                "alt" => mods.alt = true,
                "shift" => mods.shift = true,
                "win" | "windows" | "super" => mods.win = true,
                "" => return Err(HotkeyError::UnknownKey(String::new())),
                _ => {
                    if key.is_some() {
                        return Err(HotkeyError::UnknownKey(part.to_string()));
                    }
                    key = Some(key_code(part).ok_or_else(|| HotkeyError::UnknownKey(part.to_string()))?);
                }
            }
        }
        let hk = Hotkey { mods, vk: key.ok_or(HotkeyError::NoKey)? };
        hk.check()?;
        Ok(hk)
    }

    pub fn check(&self) -> Result<(), HotkeyError> {
        let m = self.mods;
        if key_name(self.vk).is_none() {
            return Err(HotkeyError::UnknownKey(format!("VK{:#04X}", self.vk)));
        }
        let ws = m.win && m.shift && !m.ctrl && !m.alt;
        if ws && self.vk == 0x53 {
            return Err(HotkeyError::Reserved("Win+Shift+S is the plain snip"));
        }
        if m.win && m.shift && self.vk == 0x52 {
            return Err(HotkeyError::Reserved("Win+Shift+R belongs to Snipping Tool's screen recording"));
        }
        if m.alt && !m.ctrl && !m.shift && !m.win && self.vk == 0x73 {
            return Err(HotkeyError::Reserved("Alt+F4 closes windows"));
        }
        if m.ctrl && m.alt && self.vk == 0x2E {
            return Err(HotkeyError::Reserved("Ctrl+Alt+Delete"));
        }
        if m.win && !m.ctrl && !m.alt && !m.shift && self.vk == 0x4C {
            return Err(HotkeyError::Reserved("Win+L locks the PC"));
        }
        let typing = matches!(self.vk, 0x30..=0x39 | 0x41..=0x5A | 0x20..=0x24 | 0x2D | 0x2E);
        if typing && !(m.ctrl || m.alt || m.win) {
            return Err(HotkeyError::BlocksTyping);
        }
        Ok(())
    }

    /// Plain PrtScn without modifiers.
    pub fn is_plain_prtscn(&self) -> bool {
        self.vk == VK_SNAPSHOT && self.mods == HkMods::default()
    }
}

/// The settings window's key recorder: a key-down with the modifiers held at that moment. Returns
/// None while only modifiers are down (keep listening).
pub fn from_keydown(vk: u32, mods: HkMods) -> Option<Hotkey> {
    let modifier = matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5);
    if modifier { None } else { Some(Hotkey { mods, vk }) }
}

/// The text capture key the hook should take, from the config. A key the screenshot side already
/// uses stays with it, so an edited file cannot break the editor. Returns a line for the log too.
pub fn text_binding(text_enabled: bool, text_hotkey: &str, plan: &Plan, snip_enabled: bool) -> (Option<Hotkey>, Option<String>) {
    if !text_enabled || text_hotkey.trim().is_empty() {
        return (None, None);
    }
    match Hotkey::parse(text_hotkey) {
        Ok(h) if snip_enabled && plan.uses(h) => {
            (None, Some(format!("text_hotkey \"{text_hotkey}\" ignored: a screenshot key uses it")))
        }
        Ok(h) => (Some(h), None),
        Err(e) => (None, Some(format!("text_hotkey \"{text_hotkey}\" ignored: {e}"))),
    }
}

/// What the tray process registers, decided from the config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Plain snip on PrtScn (the v1 `print_screen` key).
    pub plain_prtscn: bool,
    pub editor: Option<Hotkey>,
    /// A second editor key. Missing it is not an error.
    pub editor2: Option<Hotkey>,
    /// All screens as one picture (default PrtScn).
    pub all: Option<Hotkey>,
    /// A second all-screens key (default Win+Shift+PrtScn).
    pub all2: Option<Hotkey>,
    /// Lines for the log.
    pub notes: Vec<String>,
}

impl Plan {
    /// Does any screenshot key of this plan use `h`?
    pub fn uses(&self, h: Hotkey) -> bool {
        [self.editor, self.editor2, self.all, self.all2].contains(&Some(h)) || (self.plain_prtscn && h.is_plain_prtscn())
    }
}

fn parse_key(name: &str, text: &str, notes: &mut Vec<String>) -> Option<Hotkey> {
    if text.trim().is_empty() {
        return None;
    }
    match Hotkey::parse(text) {
        Ok(h) => Some(h),
        Err(e) => {
            notes.push(format!("{name} \"{text}\" ignored: {e}"));
            None
        }
    }
}

/// One key does one job. The all-screens keys come first, then the editor's, so an old file that
/// still gives PrtScn to the editor loses it to all screens (`Config::parse` moves it to Pause).
pub fn plan(editor_hotkey: &str, editor_hotkey_2: &str, all_hotkey: &str, all_hotkey_2: &str, print_screen: bool) -> Plan {
    let mut notes = Vec::new();
    let all = parse_key("all_hotkey", all_hotkey, &mut notes);
    let all2 = parse_key("all_hotkey_2", all_hotkey_2, &mut notes).filter(|h| Some(*h) != all);
    let taken = |h: &Hotkey| Some(*h) == all || Some(*h) == all2;
    let mut editor = parse_key("editor_hotkey", editor_hotkey, &mut notes);
    if editor.as_ref().is_some_and(taken) {
        notes.push(format!("editor_hotkey \"{editor_hotkey}\" ignored: all screens uses it"));
        editor = None;
    }
    let mut plain_prtscn = print_screen;
    if print_screen && all.iter().chain(all2.iter()).any(|h| h.is_plain_prtscn()) {
        plain_prtscn = false;
        notes.push("print_screen ignored: PrtScn takes all screens (all_hotkey)".into());
    }
    if print_screen && editor.is_some_and(|h| h.is_plain_prtscn()) {
        plain_prtscn = false;
        notes.push("print_screen ignored: PrtScn opens the editor (editor_hotkey = \"PrtScn\")".into());
    }
    let mut editor2 = parse_key("editor_hotkey_2", editor_hotkey_2, &mut notes);
    if let Some(h) = editor2 {
        if editor == Some(h) {
            editor2 = None;
        } else if taken(&h) {
            notes.push(format!("editor_hotkey_2 \"{editor_hotkey_2}\" ignored: all screens uses it"));
            editor2 = None;
        } else if plain_prtscn && h.is_plain_prtscn() {
            notes.push("editor_hotkey_2 ignored: PrtScn takes a plain snip (print_screen = true)".into());
            editor2 = None;
        }
    }
    Plan { plain_prtscn, editor, editor2, all, all2, notes }
}

/// A keyboard shortcut needs Ctrl without Alt: to Windows AltGr is Ctrl+Alt, and on a Turkish Q
/// layout AltGr+A and AltGr+S type characters.
pub fn is_shortcut_ctrl(ctrl: bool, alt: bool) -> bool {
    ctrl && !alt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn altgr_is_not_a_shortcut() {
        assert!(is_shortcut_ctrl(true, false));
        assert!(!is_shortcut_ctrl(true, true), "AltGr");
        assert!(!is_shortcut_ctrl(false, false));
        assert!(!is_shortcut_ctrl(false, true));
    }

    fn m(ctrl: bool, alt: bool, shift: bool, win: bool) -> HkMods {
        HkMods { ctrl, alt, shift, win }
    }

    #[test]
    fn second_editor_key_is_lightshots_pause_and_never_a_duplicate() {
        let p = plan("Ctrl+Alt+E", "Pause", "PrtScn", "Win+Shift+PrtScn", false);
        assert_eq!(p.editor2.map(|h| h.vk), Some(0x13));
        assert_eq!(p.editor2.map(|h| h.mods), Some(HkMods::default()));
        assert_eq!(plan("Pause", "Pause", "", "", false).editor2, None, "same key twice registers once");
        assert_eq!(plan("Ctrl+A", "", "", "", false).editor2, None, "empty means none");
        let bad = plan("PrtScn", "Win+Shift+S", "", "", false);
        assert_eq!(bad.editor2, None);
        assert!(bad.notes.iter().any(|n| n.contains("editor_hotkey_2")), "{:?}", bad.notes);
    }

    #[test]
    fn parse_and_text_round_trip() {
        let cases = [
            ("PrtScn", "PrtScn", m(false, false, false, false), 0x2C),
            ("print screen", "PrtScn", m(false, false, false, false), 0x2C),
            ("ctrl+shift+a", "Ctrl+Shift+A", m(true, false, true, false), 0x41),
            ("Shift + Ctrl + A", "Ctrl+Shift+A", m(true, false, true, false), 0x41),
            ("Win+Alt+F1", "Alt+Win+F1", m(false, true, false, true), 0x70),
            ("F24", "F24", m(false, false, false, false), 0x87),
            ("Control+PrtSc", "Ctrl+PrtScn", m(true, false, false, false), 0x2C),
            ("Ctrl+Alt+9", "Ctrl+Alt+9", m(true, true, false, false), 0x39),
            ("Win+Shift+X", "Shift+Win+X", m(false, false, true, true), 0x58),
        ];
        assert!(!cases.is_empty());
        for (input, text, mods, vk) in cases {
            let h = Hotkey::parse(input).unwrap_or_else(|e| panic!("{input}: {e}"));
            assert_eq!(h, Hotkey { mods, vk }, "{input}");
            assert_eq!(h.text(), text, "{input}");
            assert_eq!(Hotkey::parse(&h.text()).unwrap(), h, "{input} round trip");
        }
    }

    #[test]
    fn refused_hotkeys() {
        let cases = [
            ("", HotkeyError::Empty),
            ("Ctrl+Shift", HotkeyError::NoKey),
            ("Ctrl+Banana", HotkeyError::UnknownKey("Banana".into())),
            ("Ctrl+A+B", HotkeyError::UnknownKey("B".into())),
            ("Ctrl++A", HotkeyError::UnknownKey(String::new())),
            ("A", HotkeyError::BlocksTyping),
            ("Shift+A", HotkeyError::BlocksTyping),
            ("Space", HotkeyError::BlocksTyping),
            ("Shift+Home", HotkeyError::BlocksTyping),
        ];
        assert!(!cases.is_empty());
        for (input, want) in cases {
            assert_eq!(Hotkey::parse(input), Err(want), "{input}");
        }
        let reserved = ["Win+Shift+S", "Shift+Win+s", "Win+Shift+R", "Ctrl+Win+Shift+R", "Alt+F4", "Ctrl+Alt+Del", "Win+L"];
        assert!(!reserved.is_empty());
        for r in reserved {
            assert!(matches!(Hotkey::parse(r), Err(HotkeyError::Reserved(_))), "{r}");
        }
    }

    #[test]
    fn modifier_bits_match_register_hotkey() {
        assert_eq!(m(false, true, false, false).bits(), 0x1);
        assert_eq!(m(true, false, false, false).bits(), 0x2);
        assert_eq!(m(false, false, true, false).bits(), 0x4);
        assert_eq!(m(false, false, false, true).bits(), 0x8);
        assert_eq!(m(true, true, true, true).bits(), 0xF);
    }

    #[test]
    fn recorder_waits_for_a_real_key() {
        let ctrl = m(true, false, false, false);
        for vk in [0x10, 0x11, 0x12, 0x5B, 0x5C, 0xA0, 0xA5] {
            assert_eq!(from_keydown(vk, ctrl), None, "{vk:#x}");
        }
        assert_eq!(from_keydown(0x41, ctrl), Some(Hotkey { mods: ctrl, vk: 0x41 }));
    }

    #[test]
    fn text_key_never_collides_with_his_other_keys() {
        // Win+Shift+S, Win+Shift+R, PrtScn, Pause and Ctrl+Alt+Home (a monitor-blanking key in another tool).
        let t = Hotkey::parse("Win+Shift+T").unwrap();
        for other in ["PrtScn", "Pause", "Ctrl+Alt+Home"] {
            assert_ne!(Hotkey::parse(other).unwrap(), t, "{other}");
        }
        assert!(t.vk != 0x53 && t.vk != 0x52);
        assert_eq!(t.display(), "Win+Shift+T");
        assert_eq!(Hotkey::parse(&t.display()).unwrap(), t);

        let p = plan("Pause", "", "PrtScn", "Win+Shift+PrtScn", false);
        assert_eq!(text_binding(true, "Win+Shift+T", &p, true).0, Some(t));
        assert_eq!(text_binding(false, "Win+Shift+T", &p, true).0, None, "off");
        assert_eq!(text_binding(true, "", &p, true), (None, None), "no key");
        let (none, note) = text_binding(true, "PrtScn", &p, true);
        assert!(none.is_none() && note.unwrap().contains("screenshot"), "the screenshot keys keep theirs");
        assert!(text_binding(true, "PrtScn", &p, false).0.is_some(), "with screenshots off the key is free");
        assert!(text_binding(true, "Win+Shift+S", &p, true).0.is_none(), "refused by the key rules");
        assert!(text_binding(true, "Win+Shift+R", &p, true).0.is_none(), "never touched");
    }

    #[test]
    fn plan_gives_prtscn_to_the_editor_when_both_want_it() {
        // No all-screens key: the old rules hold.
        let p = plan("PrtScn", "Pause", "", "", true);
        assert!(!p.plain_prtscn);
        assert_eq!(p.editor, Some(Hotkey { mods: HkMods::default(), vk: VK_SNAPSHOT }));
        assert_eq!(p.notes.len(), 1);
        let q = plan("Ctrl+PrtScn", "", "", "", true);
        assert!(q.plain_prtscn, "different combination: both stay");
        assert!(q.notes.is_empty());
        let off = plan("", "", "", "", true);
        assert!(off.plain_prtscn && off.editor.is_none() && off.notes.is_empty());
        let bad = plan("Win+Shift+S", "", "", "", false);
        assert!(bad.editor.is_none());
        assert!(bad.notes[0].contains("reserved"), "{:?}", bad.notes);
    }

    #[test]
    fn all_screens_keys_are_prtscn_and_win_shift_prtscn_and_win_prtscn_is_left_alone() {
        let p = plan("Pause", "", "PrtScn", "Win+Shift+PrtScn", false);
        let (a, a2) = (p.all.unwrap(), p.all2.unwrap());
        assert!(a.is_plain_prtscn());
        assert_eq!((a2.mods, a2.vk), (HkMods { win: true, shift: true, ..HkMods::default() }, VK_SNAPSHOT));
        assert_eq!(a2.mods.bits(), 0x8 | 0x4);
        assert_eq!(a2.display(), "Win+Shift+PrtScn");
        assert!(p.notes.is_empty() && p.editor.is_some_and(|h| h.vk == 0x13));
        // Windows' own Win+PrtScn is not ours: no default key is Win+PrtScn.
        let win_prtscn = Hotkey::parse("Win+PrtScn").unwrap();
        assert!(!p.uses(win_prtscn), "Win+PrtScn stays with Windows");
        // The same key twice registers once; all screens beats the editor and the plain snip.
        assert_eq!(plan("Pause", "", "PrtScn", "PrtScn", false).all2, None);
        let o = plan("PrtScn", "", "PrtScn", "Win+Shift+PrtScn", true);
        assert_eq!(o.editor, None);
        assert!(!o.plain_prtscn);
        assert_eq!(o.notes.len(), 2, "{:?}", o.notes);
        let o2 = plan("Pause", "Win+Shift+PrtScn", "PrtScn", "Win+Shift+PrtScn", false);
        assert_eq!(o2.editor2, None);
        // The text key cannot take them.
        for k in ["PrtScn", "Win+Shift+PrtScn"] {
            assert!(text_binding(true, k, &p, true).0.is_none(), "{k}");
        }
        // Empty keys mean none.
        let n = plan("Pause", "", "", "", false);
        assert!(n.all.is_none() && n.all2.is_none());
    }
}

/// Recorder uses RegisterHotKey and cannot take a key already used by screenshots/text.
pub fn record_binding(enabled: bool, text: &str, plan: &Plan, snip_enabled: bool, text_key: Option<Hotkey>) -> Option<Hotkey> {
    if !enabled || text.trim().is_empty() { return None; }
    let h = Hotkey::parse(text).ok()?;
    if (snip_enabled && plan.uses(h)) || text_key == Some(h) { None } else { Some(h) }
}

#[cfg(test)]
mod recording_tests {
    use super::*;
    #[test]
    fn recorder_key_obeys_reservations_and_other_keys() {
        let p = plan("Pause", "", "PrtScn", "Win+Shift+PrtScn", false);
        let key = |s| record_binding(true, s, &p, true, Some(Hotkey::parse("Win+Shift+T").unwrap()));
        assert!(key("Ctrl+Shift+F9").is_some());
        for s in ["Win+Shift+R", "Ctrl+Win+Shift+R", "Win+Shift+S", "A", "Pause", "PrtScn", "Win+Shift+T"] { assert!(key(s).is_none(), "{s}"); }
    }
}
