//! `%APPDATA%/Laureth/snip/config.toml`. Every key is optional; a missing file is written once with
//! the defaults and comments. A broken file keeps the defaults and the error goes to the log.
//! Unknown keys are an error (a typo must not be silently ignored).
//!
//! v2 added `editor_hotkey`, `history`, `theme`, `language` and `look`, each with a default, so a
//! v1 file loads unchanged. v2 also changed the default of `print_screen` to false: PrtScn now
//! opens the editor. A v1 file that says `print_screen = true` keeps loading; `hotkey::plan` then
//! gives PrtScn to the editor and logs that `print_screen` was ignored.

use serde::Deserialize;

use crate::i18n::LangChoice;
use crate::theme::{Look, ThemeChoice};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CaptureMethod {
    /// Desktop Duplication, then Windows.Graphics.Capture, then GDI.
    #[default]
    Auto,
    Dd,
    Wgc,
    Gdi,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Take Win+Shift+S with the in-process low-level keyboard hook.
    pub win_shift_s: bool,
    /// Also take a plain snip on PrtScn (RegisterHotKey, no hook). Off by default since v2.
    pub print_screen: bool,
    /// The editor's hotkey (RegisterHotKey). Empty: no editor hotkey. See `hotkey.rs`.
    pub editor_hotkey: String,
    /// A second key for the editor (RegisterHotKey). While another program holds it, snip
    /// keeps working on the first key and tries again every `rearm_minutes`.
    pub editor_hotkey_2: String,
    /// All screens as one picture, no overlay: the key (RegisterHotKey). Empty: none.
    pub all_hotkey: String,
    /// A second key for the same job. Default Win+Shift+PrtScn.
    pub all_hotkey_2: String,
    /// Keep the last 50 captures in the history grid.
    pub history: bool,
    pub theme: ThemeChoice,
    pub language: LangChoice,
    /// The design direction (docs/design/): graphite or paper.
    pub look: Look,
    /// After Win+Shift+S, send one harmless key (VK 0xE8) so releasing Win does not open Start.
    pub mask_win_key: bool,
    /// Where PNGs go. Empty: Windows' Screenshots known folder.
    pub folder: String,
    pub save_file: bool,
    pub copy_to_clipboard: bool,
    pub capture: CaptureMethod,
    /// Keep the D3D11 device between snips (no textures, no swap chains). Faster first frame,
    /// more idle memory. Decided from the probe numbers.
    pub keep_gpu_device: bool,
    /// Run each snip in a short-lived child process, so the tray process stays small.
    pub separate_process: bool,
    /// Create the graphics device as soon as Win and Shift are both held (before S), and drop
    /// it again after the snip or after 10 s unused.
    pub prewarm: bool,
    /// Darkening of the frozen screen outside the selection, 0.0 to 0.9.
    pub dim: f32,
    /// Re-install the hook this often (Windows can silently remove a slow hook).
    pub rearm_minutes: u32,
    /// The overlay cancels itself after this many seconds without input (safety net).
    pub overlay_timeout_seconds: u32,
    /// Write the step-timing log to %LOCALAPPDATA%/Laureth/snip/snip.log.
    pub debug_log: bool,
    /// The screenshot functions (Win+Shift+S, the editor keys, their menu entries). Off leaves
    /// only text capture; Win+Shift+S then opens Windows' own tool.
    pub snip_enabled: bool,
    /// Text capture (its key, its menu entry, the "copy text" actions).
    pub text_enabled: bool,
    /// The text capture key, taken in the keyboard hook. Empty: no key.
    pub text_hotkey: String,
    /// The reading language: "auto" or a language tag. A malformed tag reads as "auto".
    pub ocr_language: String,
    /// Keep the last 20 captured texts for the main window.
    pub text_history: bool,
    /// The saved file: "png" always (lossless), or "auto": PNG for text and UI, JPEG at quality 95
    /// with full colour (4:4:4) for photo-like captures (games, video), a third of the size.
    pub image_format: String,
    /// Also put the saved file on the clipboard (CF_HDROP) next to the picture, so a chat program
    /// that takes files uploads the saved file instead of encoding its own PNG.
    pub clipboard_file: bool,
    /// Add registered PNG bytes after the immediate DIB write, on the save worker.
    pub clipboard_png: bool,
    pub record_enabled: bool,
    pub record_hotkey: String,
    pub record_max_seconds: u32,
    pub record_gif: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            win_shift_s: true,
            print_screen: false,
            editor_hotkey: "Pause".into(),
            editor_hotkey_2: String::new(),
            all_hotkey: ALL_HOTKEY_DEFAULT.into(),
            all_hotkey_2: ALL_HOTKEY_2_DEFAULT.into(),
            history: true,
            theme: ThemeChoice::System,
            language: LangChoice::System,
            look: Look::Graphite,
            mask_win_key: true,
            folder: String::new(),
            save_file: true,
            copy_to_clipboard: true,
            capture: CaptureMethod::Auto,
            keep_gpu_device: KEEP_GPU_DEFAULT,
            separate_process: true,
            prewarm: true,
            dim: crate::theme::PLAIN_DIM_DEFAULT,
            rearm_minutes: 10,
            overlay_timeout_seconds: 300,
            debug_log: true,
            snip_enabled: true,
            text_enabled: true,
            text_hotkey: TEXT_HOTKEY_DEFAULT.into(),
            ocr_language: "auto".into(),
            text_history: true,
            image_format: "auto".into(),
            clipboard_file: false,
            clipboard_png: true,
            record_enabled: true,
            record_hotkey: "Ctrl+Shift+F9".into(),
            record_max_seconds: 60,
            record_gif: true,
        }
    }
}

pub const DEFAULT_FILE: &str = r#"# snip settings (Laureth). Every key is optional; delete the file to get these defaults back.
# The Settings window applies its changes at once. After editing this file by hand, restart snip
# (snip quit, then start it again).

# Take Win+Shift+S (in-process low-level keyboard hook; Win+Shift+R stays with Snipping Tool).
win_shift_s = true
# Also take a plain snip on PrtScn (no hook needed). Off. PrtScn takes all screens (all_hotkey), so
# this only works when all_hotkey is changed or emptied.
print_screen = false
# The editor: freeze the screen, select, then draw arrows, boxes, text, steps, pixelate; copy, save
# or pin. Pause is Lightshot's key on this PC. Examples: "Pause", "Ctrl+Shift+A", "Win+Alt+S".
# Empty = no editor hotkey. A key another program holds is tried again every rearm_minutes.
# Not allowed: Win+Shift+S, Win+Shift+R, and a letter or digit without Ctrl, Alt or Win.
editor_hotkey = "Pause"
# A second key for the editor. Empty = none. Written only when it differs from "".
# editor_hotkey_2 = ""
# All screens as one picture, however many there are: no overlay, no selection. The picture goes to
# the clipboard and to a PNG. Empty = no key. Written only when it differs from "PrtScn".
# all_hotkey = "PrtScn"
# A second key for the same job. Windows' own Win+PrtScn is left alone. Written only when it differs
# from "Win+Shift+PrtScn".
# all_hotkey_2 = "Win+Shift+PrtScn"
# Keep the last 50 captures for the history window (tray menu > History).
history = true
# system, light or dark.
theme = "system"
# system, en or tr.
language = "system"
# graphite or paper (the two designs in docs/design).
look = "graphite"
# Send one harmless key after Win+Shift+S so releasing Win does not open Start.
mask_win_key = true
# Folder for the PNGs. Empty = Windows' Screenshots folder.
folder = ""
save_file = true
copy_to_clipboard = true
# auto = Desktop Duplication, then Windows.Graphics.Capture, then GDI. Or: dd, wgc, gdi.
capture = "auto"
# Keep the graphics device between snips: about 15-90 ms faster to open, more memory while idle.
keep_gpu_device = KEEPDEFAULT
# Run each snip in a short-lived child process (the tray process then stays at about 2 MB).
separate_process = true
# Create the graphics device while Win+Shift are held, before S (dropped again after 10 s unused).
prewarm = true
# Darkening outside the selection (0.0 to 0.9).
dim = DIMDEFAULT
# Re-install the keyboard hook this often, in minutes.
rearm_minutes = 10
# The overlay closes itself after this many seconds without input.
overlay_timeout_seconds = 300
# Step timings in %LOCALAPPDATA%\Laureth\snip\snip.log
debug_log = true
# The lines below are written only when they differ from these defaults.
# Screenshots on or off: Win+Shift+S, the editor keys and their menu entries.
# snip_enabled = true
# Text capture on or off: drag over text on the screen, it arrives on the clipboard as text.
# text_enabled = true
# The text capture key. Empty = no key (the menu and the main window still start it).
# text_hotkey = "Win+Shift+T"
# The reading language: auto, or a language Windows has a reader for, such as tr-TR or en-US.
# auto reads with the Windows profile's reader; when that reading looks wrong, also with the
# installed Chinese, Japanese, Korean, Cyrillic, Greek and Arabic readers, and takes their words
# where they saw their own letters (a game nick in Chinese).
# A chosen language reads with that reader only.
# ocr_language = "auto"
# Keep the last 20 captured texts for the main window. They stay on this PC.
# text_history = true
# The saved file: png (always, lossless), or auto: PNG for text and UI, JPEG at quality 95 with full
# colour for photo-like captures such as games and video (about a third of the PNG's size).
# image_format = "auto"
# Also put the saved file on the clipboard next to the picture, for chat programs that upload files.
# clipboard_file = false
# Add lossless PNG bytes next to the DIB picture after encoding on the save worker.
# clipboard_png = true
# Short silent screen recordings, RegisterHotKey (never Win+Shift+R).
# record_enabled = true
# record_hotkey = "Ctrl+Shift+F9"
# record_max_seconds = 60
# record_gif = true
"#;

pub const KEEP_GPU_DEFAULT: bool = false;
pub const TEXT_HOTKEY_DEFAULT: &str = "Win+Shift+T";
pub const ALL_HOTKEY_DEFAULT: &str = "PrtScn";
pub const ALL_HOTKEY_2_DEFAULT: &str = "Win+Shift+PrtScn";

/// Keys written only when they differ from the default: an older build refuses a file with a key
/// it does not know, falls back to its defaults, and those turn the clipboard back on.
const WRITTEN_WHEN_CHANGED: [&str; 15] = ["editor_hotkey_2", "all_hotkey", "all_hotkey_2", "snip_enabled", "text_enabled", "text_hotkey", "ocr_language", "text_history", "image_format", "clipboard_file", "clipboard_png", "record_enabled", "record_hotkey", "record_max_seconds", "record_gif"];

pub fn default_file() -> String {
    DEFAULT_FILE.replace("KEEPDEFAULT", if KEEP_GPU_DEFAULT { "true" } else { "false" })
        .replace("DIMDEFAULT", &crate::theme::PLAIN_DIM_DEFAULT.to_string())
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, String> {
        let mut c: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        c.dim = if c.dim.is_finite() { c.dim.clamp(0.0, 0.9) } else { crate::theme::PLAIN_DIM_DEFAULT };
        c.record_max_seconds = c.record_max_seconds.clamp(1, 300);
        c.rearm_minutes = c.rearm_minutes.clamp(1, 24 * 60);
        c.overlay_timeout_seconds = c.overlay_timeout_seconds.clamp(10, 3600);
        if !crate::textcore::is_auto_language(&c.ocr_language) && !crate::textcore::is_well_formed_tag(&c.ocr_language) {
            c.ocr_language = "auto".into();
        }
        c.image_format = c.image_format.trim().to_ascii_lowercase();
        if c.image_format != "auto" {
            c.image_format = "png".into();
        }
        c.migrate_editor_keys();
        Ok(c)
    }

    /// The screenshot keys this config asks for, after the one-key-one-job rules.
    pub fn plan(&self) -> crate::hotkey::Plan {
        crate::hotkey::plan(&self.editor_hotkey, &self.editor_hotkey_2, &self.all_hotkey, &self.all_hotkey_2, self.print_screen)
    }

    /// The editor key as people say it, from the plan (what is really registered), or empty.
    pub fn editor_key_text(&self) -> String {
        let p = self.plan();
        p.editor.or(p.editor2).map(|h| h.display()).unwrap_or_default()
    }

    /// The all-screens key as people say it, or empty.
    pub fn all_key_text(&self) -> String {
        let p = self.plan();
        p.all.or(p.all2).map(|h| h.display()).unwrap_or_default()
    }

    /// Before 2026-11-08 PrtScn opened the editor, and the default file said so. PrtScn now takes
    /// all screens, so an editor key that is plain PrtScn while an all-screens key wants it moves to
    /// Pause (Lightshot's key, the editor's key since then). The file is not rewritten here; the
    /// next save from the settings window writes the new value.
    fn migrate_editor_keys(&mut self) {
        use crate::hotkey::Hotkey;
        let wants = |k: &str| Hotkey::parse(k).is_ok_and(|h| h.is_plain_prtscn());
        if !(wants(&self.all_hotkey) || wants(&self.all_hotkey_2)) {
            return;
        }
        if wants(&self.editor_hotkey) {
            self.editor_hotkey = "Pause".into();
        }
        if wants(&self.editor_hotkey_2) {
            self.editor_hotkey_2 = String::new();
        }
    }

    /// The value of `key` as it is written in config.toml.
    fn value_text(&self, key: &str) -> Option<String> {
        let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        Some(match key {
            "win_shift_s" => self.win_shift_s.to_string(),
            "print_screen" => self.print_screen.to_string(),
            "editor_hotkey" => q(&self.editor_hotkey),
            "editor_hotkey_2" => q(&self.editor_hotkey_2),
            "all_hotkey" => q(&self.all_hotkey),
            "all_hotkey_2" => q(&self.all_hotkey_2),
            "history" => self.history.to_string(),
            "theme" => q(self.theme.name()),
            "language" => q(self.language.name()),
            "look" => q(self.look.name()),
            "mask_win_key" => self.mask_win_key.to_string(),
            "folder" => q(&self.folder),
            "save_file" => self.save_file.to_string(),
            "copy_to_clipboard" => self.copy_to_clipboard.to_string(),
            "capture" => q(match self.capture {
                CaptureMethod::Auto => "auto",
                CaptureMethod::Dd => "dd",
                CaptureMethod::Wgc => "wgc",
                CaptureMethod::Gdi => "gdi",
            }),
            "keep_gpu_device" => self.keep_gpu_device.to_string(),
            "separate_process" => self.separate_process.to_string(),
            "prewarm" => self.prewarm.to_string(),
            "dim" => format!("{}", (self.dim * 100.0).round() / 100.0),
            "rearm_minutes" => self.rearm_minutes.to_string(),
            "overlay_timeout_seconds" => self.overlay_timeout_seconds.to_string(),
            "debug_log" => self.debug_log.to_string(),
            "snip_enabled" => self.snip_enabled.to_string(),
            "text_enabled" => self.text_enabled.to_string(),
            "text_hotkey" => q(&self.text_hotkey),
            "ocr_language" => q(&self.ocr_language),
            "text_history" => self.text_history.to_string(),
            "image_format" => q(&self.image_format),
            "clipboard_file" => self.clipboard_file.to_string(),
            "clipboard_png" => self.clipboard_png.to_string(),
            "record_enabled" => self.record_enabled.to_string(),
            "record_hotkey" => q(&self.record_hotkey),
            "record_max_seconds" => self.record_max_seconds.to_string(),
            "record_gif" => self.record_gif.to_string(),
            _ => return None,
        })
    }

    /// The full commented file with this config's values (what the settings window writes).
    pub fn to_file_text(&self) -> String {
        let mut out = String::new();
        let defaults = Config::default();
        for line in default_file().lines() {
            let optional = WRITTEN_WHEN_CHANGED.iter().find(|k| line.starts_with(&format!("# {k} = ")));
            if let Some(key) = optional {
                let (mine, default) = (self.value_text(key).unwrap_or_default(), defaults.value_text(key).unwrap_or_default());
                if mine != default {
                    out.push_str(&format!("{key} = {mine}\n"));
                    continue;
                }
            }
            let key = line.split(" = ").next().unwrap_or("");
            match (line.starts_with('#'), line.contains(" = "), self.value_text(key)) {
                (false, true, Some(v)) => out.push_str(&format!("{key} = {v}")),
                _ => out.push_str(line),
            }
            out.push('\n');
        }
        out
    }

    /// Load from `path`; write the default file when it is missing. Returns the config and an
    /// optional note for the log.
    pub fn load_or_create(path: &std::path::Path) -> (Config, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(t) => match Config::parse(&t) {
                Ok(c) => (c, None),
                Err(e) => (Config::default(), Some(format!("config {} is broken, using defaults: {e}", path.display()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let note = match std::fs::write(path, default_file()) {
                    Ok(()) => format!("wrote default config {}", path.display()),
                    Err(e) => format!("could not write default config {}: {e}", path.display()),
                };
                (Config::default(), Some(note))
            }
            Err(e) => (Config::default(), Some(format!("could not read config {}: {e}", path.display()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_uses_auto_and_explicit_png_stays_png() {
        assert_eq!(Config::parse("").unwrap().image_format, "auto");
        assert_eq!(Config::parse("image_format = \"png\"\n").unwrap().image_format, "png");
        assert!(Config::default().to_file_text().contains("# image_format = \"auto\""));
    }

    #[test]
    fn clipboard_png_defaults_on_and_only_writes_when_changed() {
        assert!(Config::parse("").unwrap().clipboard_png);
        let c = Config::parse("clipboard_png = false").unwrap();
        assert!(!c.clipboard_png);
        assert!(c.to_file_text().lines().any(|l| l == "clipboard_png = false"));
        assert!(!Config::default().to_file_text().lines().any(|l| l.starts_with("clipboard_png =")));
        assert!(Config::parse("clipboard_png = 1").is_err());
    }

    #[test]
    fn default_file_parses_to_the_defaults() {
        assert_eq!(Config::parse(&default_file()).unwrap(), Config::default());
    }

    /// An older build refuses a file with a key it does not know and falls back to its defaults,
    /// which turn the clipboard back on. A file holding only defaults must not have the new key.
    #[test]
    fn files_with_default_values_stay_readable_by_older_builds() {
        for key in WRITTEN_WHEN_CHANGED {
            assert!(!default_file().lines().chain(Config::default().to_file_text().lines()).any(|l| l.starts_with(key)), "{key}");
            assert!(default_file().contains(&format!("# {key} = ")), "{key} has no line to fill in");
        }
        let has_key = |text: &str| text.lines().any(|l| l.starts_with("editor_hotkey_2"));
        assert!(!has_key(&default_file()));
        assert!(!has_key(&Config::default().to_file_text()));
        let changed = Config { editor_hotkey_2: "Ctrl+Alt+O".into(), all_hotkey: String::new(), ..Config::default() };
        let text = changed.to_file_text();
        assert!(has_key(&text));
        assert_eq!(Config::parse(&text).unwrap().editor_hotkey_2, "Ctrl+Alt+O");
        assert_eq!(Config::parse(&text).unwrap().all_hotkey, "", "emptying a key is kept");
    }

    #[test]
    fn recording_config_migrates_and_roundtrips() {
        let old = Config::parse(include_str!("../tests/data/config-v1.toml")).unwrap();
        assert!(old.record_enabled && old.record_gif);
        assert_eq!(old.record_hotkey, "Ctrl+Shift+F9");
        assert_eq!(old.record_max_seconds, 60);
        let c = Config::parse("record_enabled = false\nrecord_gif = false\nrecord_max_seconds = 999\nrecord_hotkey = \"Ctrl+F10\"").unwrap();
        assert_eq!(c.record_max_seconds, 300);
        assert_eq!(Config::parse(&c.to_file_text()).unwrap(), c);
        assert_eq!(Config::parse("record_max_seconds = 0").unwrap().record_max_seconds, 1);
    }

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn values_are_read_and_clamped() {
        let c = Config::parse("dim = 5.0\ncapture = \"gdi\"\nprint_screen = false\nrearm_minutes = 0\n").unwrap();
        assert_eq!(c.dim, 0.9);
        assert_eq!(c.capture, CaptureMethod::Gdi);
        assert!(!c.print_screen);
        assert_eq!(c.rearm_minutes, 1);
    }

    #[test]
    fn unknown_keys_and_bad_values_are_errors() {
        let cases = ["win_shift_z = true", "capture = \"magic\"", "dim = \"dark\"", "folder = 3"];
        assert!(!cases.is_empty());
        for c in cases {
            assert!(Config::parse(c).is_err(), "{c}");
        }
    }

    #[test]
    fn a_v1_file_still_loads() {
        let v1 = include_str!("../tests/data/config-v1.toml");
        let c = Config::parse(v1).expect("the v1 default file must load");
        assert!(c.print_screen, "v1 wrote print_screen = true");
        assert_eq!(c.editor_hotkey, "Pause", "new key takes its default");
        let plan = c.plan();
        assert!(!plan.plain_prtscn && plan.editor.is_some(), "PrtScn goes to all screens, not to a plain snip");
        assert!(plan.all.is_some_and(|h| h.is_plain_prtscn()));
    }

    /// The user's file from before 2026-11-08 says editor_hotkey = "PrtScn". PrtScn now means all
    /// screens, and the editor must keep a key.
    #[test]
    fn an_old_prtscn_editor_key_moves_to_pause() {
        let old = Config::parse("editor_hotkey = \"PrtScn\"\n").unwrap();
        assert_eq!(old.editor_hotkey, "Pause");
        let p = old.plan();
        assert!(p.all.is_some_and(|h| h.is_plain_prtscn()) && p.editor.is_some_and(|h| h.vk == 0x13) && p.notes.is_empty(), "{:?}", p.notes);
        let both = Config::parse("editor_hotkey = \"PrtScn\"\neditor_hotkey_2 = \"Pause\"\n").unwrap();
        assert_eq!(both.plan().editor2, None, "Pause once");
        // He emptied the all-screens keys: the editor may keep PrtScn.
        let kept = Config::parse("editor_hotkey = \"PrtScn\"\nall_hotkey = \"\"\nall_hotkey_2 = \"\"\n").unwrap();
        assert_eq!(kept.editor_hotkey, "PrtScn");
        // The defaults write no new key into a file an older build must read.
        assert!(!Config::default().to_file_text().lines().any(|l| l.starts_with("all_hotkey")));
    }

    #[test]
    fn new_keys_are_read() {
        let c = Config::parse("editor_hotkey = \"Ctrl+Shift+A\"\ntheme = \"dark\"\nlanguage = \"tr\"\nlook = \"paper\"\nhistory = false\n").unwrap();
        assert_eq!(c.editor_hotkey, "Ctrl+Shift+A");
        assert_eq!(c.theme, ThemeChoice::Dark);
        assert_eq!(c.language, LangChoice::Tr);
        assert_eq!(c.look, Look::Paper);
        assert!(!c.history);
        for bad in ["theme = \"blue\"", "language = \"de\"", "look = \"glass\"", "history = 50"] {
            assert!(Config::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn written_file_round_trips_every_key() {
        let c = Config {
            win_shift_s: false,
            print_screen: true,
            editor_hotkey: "Ctrl+Alt+P".into(),
            editor_hotkey_2: "Ctrl+Alt+O".into(),
            all_hotkey: "Ctrl+Alt+A".into(),
            all_hotkey_2: "Ctrl+Alt+B".into(),
            history: false,
            theme: ThemeChoice::Light,
            language: LangChoice::En,
            look: Look::Paper,
            mask_win_key: false,
            folder: "D:\\shots \"x\"".into(),
            save_file: false,
            copy_to_clipboard: false,
            capture: CaptureMethod::Wgc,
            keep_gpu_device: true,
            separate_process: false,
            prewarm: false,
            dim: 0.3,
            rearm_minutes: 5,
            overlay_timeout_seconds: 60,
            debug_log: false,
            snip_enabled: false,
            text_enabled: false,
            text_hotkey: "Ctrl+Alt+T".into(),
            ocr_language: "tr-TR".into(),
            text_history: false,
            image_format: "png".into(),
            clipboard_file: true,
            clipboard_png: false,
            record_enabled: false,
            record_hotkey: "Ctrl+F10".into(),
            record_max_seconds: 120,
            record_gif: false,
        };
        let text = c.to_file_text();
        assert_eq!(Config::parse(&text).unwrap(), c, "{text}");
        assert_eq!(Config::default().to_file_text(), default_file());
        // Every key in the file has a writer (no key silently keeps its default).
        let file = default_file();
        let keys: Vec<&str> = file.lines().filter(|l| !l.starts_with('#') && l.contains(" = ")).map(|l| l.split(" = ").next().unwrap()).collect();
        assert_eq!(keys.len(), 19); // editor_hotkey_2 is written only when it differs from its default
        for k in keys {
            assert!(c.value_text(k).is_some(), "{k}");
        }
    }

    #[test]
    fn a_malformed_reading_language_means_automatic() {
        assert_eq!(Config::parse("ocr_language = \"not a tag\"\n").unwrap().ocr_language, "auto");
        assert_eq!(Config::parse("ocr_language = \"tr-TR\"\n").unwrap().ocr_language, "tr-TR");
        assert_eq!(Config::parse("image_format = \"webp\"\n").unwrap().image_format, "png", "unknown format reads as png");
        assert_eq!(Config::parse("image_format = \" Auto \"\n").unwrap().image_format, "auto");
    }

    #[test]
    fn load_or_create_writes_the_default_once() {
        let d = crate::naming::testdir::TestDir::new("config");
        let p = d.0.join("sub").join("config.toml");
        let (c, note) = Config::load_or_create(&p);
        assert_eq!(c, Config::default());
        assert!(note.unwrap().starts_with("wrote default config"));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), default_file());
        std::fs::write(&p, "dim = 0.2\n").unwrap();
        let (c2, note2) = Config::load_or_create(&p);
        assert_eq!(c2.dim, 0.2);
        assert!(note2.is_none());
        std::fs::write(&p, "dim = [\n").unwrap();
        let (c3, note3) = Config::load_or_create(&p);
        assert_eq!(c3, Config::default());
        assert!(note3.unwrap().contains("broken"));
    }
}
