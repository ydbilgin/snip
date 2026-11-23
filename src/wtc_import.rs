//! One-time import of WinTextCapture's settings (%APPDATA%\WinTextCapture\settings.json) into Snip's
//! config, for the switch. Started only by `snip import-wintextcapture [--write]`.
//!
//! Reads HotKey (the virtual key, 1..=0xFE), ModCtrl, ModAlt, ModShift, ModWin, OcrLanguage, Language
//! and Theme. AutoStart is not imported: the Run value decides. A value WinTextCapture itself would
//! reset is skipped and named in the report; the others still import. Without `--write` it only says
//! what it would set. The JSON file is only read, never changed.

use serde_json::Value;

use crate::config::Config;
use crate::hotkey::{HkMods, Hotkey};
use crate::i18n::LangChoice;
use crate::theme::ThemeChoice;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    pub text_hotkey: Option<String>,
    pub ocr_language: Option<String>,
    pub language: Option<LangChoice>,
    pub theme: Option<ThemeChoice>,
    /// "Key: why it was skipped".
    pub skipped: Vec<String>,
}

pub fn parse(json: &str) -> Result<Imported, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("settings.json is not valid JSON: {e}"))?;
    let o = v.as_object().ok_or("settings.json does not hold a JSON object")?;
    let mut out = Imported::default();

    if ["HotKey", "ModCtrl", "ModAlt", "ModShift", "ModWin"].iter().any(|k| o.contains_key(*k)) {
        let flag = |k: &str, default: bool| match o.get(k) {
            None => Ok(default),
            Some(Value::Bool(b)) => Ok(*b),
            Some(other) => Err(format!("HotKey: {k} {other} is not true or false")),
        };
        let vk = match o.get("HotKey") {
            None => Ok(0x54),
            Some(Value::Number(n)) => n.as_u64().filter(|v| (1..=0xFE).contains(v)).map(|v| v as u32).ok_or(format!("HotKey: {n} is outside 1..254")),
            Some(other) => Err(format!("HotKey: {other} is not a key number")),
        };
        let parts = (|| Ok::<_, String>((vk?, flag("ModCtrl", false)?, flag("ModAlt", false)?, flag("ModShift", true)?, flag("ModWin", true)?)))();
        match parts {
            Ok((vk, ctrl, alt, shift, win)) => {
                let hk = Hotkey::new(HkMods { ctrl, alt, shift, win }, vk);
                match hk.check() {
                    Ok(()) => out.text_hotkey = Some(hk.display()),
                    Err(e) => out.skipped.push(format!("HotKey: {e}")),
                }
            }
            Err(e) => out.skipped.push(e),
        }
    }

    match o.get("OcrLanguage") {
        None => {}
        Some(Value::String(s)) if crate::textcore::is_auto_language(s) => out.ocr_language = Some("auto".into()),
        Some(Value::String(s)) if crate::textcore::is_well_formed_tag(s) => out.ocr_language = Some(s.trim().to_string()),
        Some(other) => out.skipped.push(format!("OcrLanguage: {other} is not a language tag")),
    }
    match o.get("Language") {
        None => {}
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "auto" => out.language = Some(LangChoice::System),
            "en" => out.language = Some(LangChoice::En),
            "tr" => out.language = Some(LangChoice::Tr),
            _ => out.skipped.push(format!("Language: {s:?} is not auto, en or tr")),
        },
        Some(other) => out.skipped.push(format!("Language: {other} is not text")),
    }
    match o.get("Theme") {
        None => {}
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "system" => out.theme = Some(ThemeChoice::System),
            "light" => out.theme = Some(ThemeChoice::Light),
            "dark" => out.theme = Some(ThemeChoice::Dark),
            _ => out.skipped.push(format!("Theme: {s:?} is not System, Light or Dark")),
        },
        Some(other) => out.skipped.push(format!("Theme: {other} is not text")),
    }
    Ok(out)
}

/// Set the imported values on `c`; one report line per value.
pub fn apply(i: &Imported, c: &mut Config) -> Vec<String> {
    let mut r = Vec::new();
    if let Some(v) = &i.text_hotkey {
        r.push(format!("text_hotkey = {v} (was {})", c.text_hotkey));
        c.text_hotkey = v.clone();
    }
    if let Some(v) = &i.ocr_language {
        r.push(format!("ocr_language = {v} (was {})", c.ocr_language));
        c.ocr_language = v.clone();
    }
    if let Some(v) = i.language {
        r.push(format!("language = {} (was {})", v.name(), c.language.name()));
        c.language = v;
    }
    if let Some(v) = i.theme {
        r.push(format!("theme = {} (was {})", v.name(), c.theme.name()));
        c.theme = v;
    }
    r
}

/// The whole command without the console; `write` receives the new config text.
pub fn run(json: &str, current: &Config, do_write: bool, write: impl FnOnce(&str) -> Result<(), String>) -> Result<String, String> {
    let i = parse(json)?;
    let mut c = current.clone();
    let lines = apply(&i, &mut c);
    let mut out = String::new();
    for s in &i.skipped {
        out.push_str(&format!("skipped {s}\n"));
    }
    out.push_str("not imported AutoStart: the Windows start entry decides\n");
    for l in &lines {
        out.push_str(&format!("{} {l}\n", if do_write { "set" } else { "would set" }));
    }
    if do_write {
        write(&c.to_file_text())?;
    } else {
        out.push_str("dry run: nothing changed; add --write to set these\n");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn his_defaults_import_as_his_key() {
        let j = r#"{"HotKey":84,"ModCtrl":false,"ModAlt":false,"ModShift":true,"ModWin":true,"OcrLanguage":"tr","Language":"auto","Theme":"System","AutoStart":true}"#;
        let i = parse(j).unwrap();
        assert_eq!(i.text_hotkey.as_deref(), Some("Win+Shift+T"));
        assert_eq!(i.ocr_language.as_deref(), Some("tr"));
        assert_eq!(i.language, Some(LangChoice::System));
        assert_eq!(i.theme, Some(ThemeChoice::System));
        assert!(i.skipped.is_empty());
    }

    #[test]
    fn a_bad_value_is_named_and_the_rest_still_import() {
        let i = parse(r#"{"HotKey":70000,"OcrLanguage":"not a tag","Language":"de","Theme":"Dark"}"#).unwrap();
        assert_eq!(i.theme, Some(ThemeChoice::Dark));
        assert_eq!((i.text_hotkey.clone(), i.ocr_language.clone(), i.language), (None, None, None));
        assert_eq!(i.skipped.len(), 3, "{:?}", i.skipped);
        assert!(parse("[]").is_err() && parse("{").is_err());
    }

    #[test]
    fn snips_own_key_is_refused() {
        let i = parse(r#"{"HotKey":83,"ModShift":true,"ModWin":true}"#).unwrap();
        assert_eq!(i.text_hotkey, None);
        assert_eq!(i.skipped.len(), 1);
    }

    #[test]
    fn a_dry_run_writes_nothing_and_a_written_file_loads_back() {
        let j = r#"{"HotKey":81,"ModCtrl":true,"ModAlt":true,"ModShift":false,"ModWin":false,"OcrLanguage":"en-US"}"#;
        let base = Config::default();
        let report = run(j, &base, false, |_| panic!("wrote on a dry run")).unwrap();
        assert!(report.contains("would set text_hotkey = Ctrl+Alt+Q"), "{report}");
        let mut written = String::new();
        run(j, &base, true, |t| { written = t.to_string(); Ok(()) }).unwrap();
        let back = Config::parse(&written).unwrap();
        assert_eq!((back.text_hotkey.as_str(), back.ocr_language.as_str()), ("Ctrl+Alt+Q", "en-US"));
    }
}
