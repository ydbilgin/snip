//! The last text captures, for the main window. `history/texts.toml` (schema 1), newest first, at
//! most 20 entries. It holds what he captured, so it stays on this PC and `text_history = false`
//! turns it off and empties it. Screenshots go to history.toml; this is its plain-text neighbour.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const LIMIT: usize = 20;
/// A very long capture keeps its beginning here; the clipboard always got all of it.
pub const MAX_CHARS: usize = 20_000;
pub const FILE: &str = "texts.toml";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextEntry {
    pub id: u64,
    /// Local time, "2026-11-04 19:50:12".
    pub time: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TextHistory {
    pub schema: u32,
    pub next_id: u64,
    #[serde(rename = "entry")]
    pub entries: Vec<TextEntry>,
}

impl Default for TextHistory {
    fn default() -> Self {
        Self { schema: 1, next_id: 1, entries: Vec::new() }
    }
}

impl TextHistory {
    pub fn parse(text: &str) -> Result<TextHistory, String> {
        let h: TextHistory = toml::from_str(text).map_err(|e| e.to_string())?;
        if h.schema != 1 {
            return Err(format!("unknown schema {}", h.schema));
        }
        Ok(h)
    }

    pub fn add(&mut self, time: &str, text: &str) {
        let kept: String = text.chars().take(MAX_CHARS).collect();
        // The same text again moves to the top instead of filling the list.
        self.entries.retain(|e| e.text != kept);
        self.entries.insert(0, TextEntry { id: self.next_id, time: time.to_string(), text: kept });
        self.next_id += 1;
        self.entries.truncate(LIMIT);
    }
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// A missing or broken file reads as empty: this list is a convenience, never a reason to fail.
pub fn load(dir: &Path) -> TextHistory {
    std::fs::read_to_string(path_in(dir)).ok().and_then(|t| TextHistory::parse(&t).ok()).unwrap_or_default()
}

pub fn record(dir: &Path, time: &str, text: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut h = load(dir);
    h.add(time, text);
    let body = toml::to_string(&h).map_err(|e| e.to_string())?;
    crate::naming::write_small(&path_in(dir), body.as_bytes()).map_err(|e| e.to_string())
}

pub fn clear(dir: &Path) {
    let _ = std::fs::remove_file(path_in(dir));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_limited_and_no_duplicates() {
        let mut h = TextHistory::default();
        for i in 0..25 {
            h.add("2026-11-07 10:00:00", &format!("metin {i}"));
        }
        assert_eq!(h.entries.len(), LIMIT);
        assert_eq!(h.entries[0].text, "metin 24");
        h.add("2026-11-07 11:00:00", "metin 10");
        assert_eq!(h.entries[0].text, "metin 10");
        assert_eq!(h.entries.iter().filter(|e| e.text == "metin 10").count(), 1);
    }

    #[test]
    fn turkish_and_line_breaks_survive_the_file() {
        let d = crate::naming::testdir::TestDir::new("texthistory");
        let text = "Şirket çalışanları\r\nGörüşme \"14:30\", İıŞşĞğ \u{1F600}";
        record(&d.0, "2026-11-07 10:00:00", text).unwrap();
        assert_eq!(load(&d.0).entries[0].text, text);
        clear(&d.0);
        assert!(load(&d.0).entries.is_empty());
    }

    #[test]
    fn a_broken_file_reads_as_empty_and_a_long_text_is_cut() {
        let d = crate::naming::testdir::TestDir::new("texthistory-bad");
        std::fs::write(path_in(&d.0), "schema = [").unwrap();
        assert_eq!(load(&d.0), TextHistory::default());
        let mut h = TextHistory::default();
        h.add("t", &"x".repeat(MAX_CHARS + 5));
        assert_eq!(h.entries[0].text.chars().count(), MAX_CHARS);
    }
}
