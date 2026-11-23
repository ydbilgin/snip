//! The history of the last 50 captures.
//!
//! Folder: `%LOCALAPPDATA%/Laureth/snip/history/`. `history.toml` (schema 1) lists the entries,
//! newest first; each has a thumbnail `<id>.thumb` (`thumb.rs`). An entry points at the saved PNG
//! in the screenshots folder; when nothing was saved there (copy only, or `save_file = false`) the
//! history keeps its own copy as `<id>.png` (`owned = true`). Dropping an entry (the 51st capture,
//! or Remove) deletes its thumbnail and an owned copy, never a file in the screenshots folder.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const LIMIT: usize = 50;
pub const INDEX: &str = "history.toml";
pub const LOCK: &str = "history.lock";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: u64,
    /// Local time, "2026-11-04 19:50:12".
    pub time: String,
    pub path: String,
    pub owned: bool,
    pub w: u32,
    pub h: u32,
    /// "snip", "editor", "pin"...
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct History {
    pub schema: u32,
    pub next_id: u64,
    #[serde(rename = "entry")]
    pub entries: Vec<Entry>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            schema: 1,
            next_id: 1,
            entries: Vec::new(),
        }
    }
}

impl History {
    pub fn parse(text: &str) -> Result<History, String> {
        let h: History = toml::from_str(text).map_err(|e| e.to_string())?;
        if h.schema != 1 {
            return Err(format!("unknown history schema {}", h.schema));
        }
        Ok(h)
    }

    pub fn to_text(&self) -> String {
        format!(
            "# snip capture history (newest first). Managed by snip; safe to delete.\n{}",
            toml::to_string(self).unwrap_or_default()
        )
    }

    /// Give `e` the next id, put it first, and return the entries that fell off the end.
    pub fn add(&mut self, mut e: Entry) -> (u64, Vec<Entry>) {
        e.id = self
            .next_id
            .max(self.entries.iter().map(|x| x.id + 1).max().unwrap_or(1));
        self.next_id = e.id + 1;
        let id = e.id;
        self.entries.insert(0, e);
        let evicted = if self.entries.len() > LIMIT {
            self.entries.split_off(LIMIT)
        } else {
            Vec::new()
        };
        (id, evicted)
    }

    pub fn remove(&mut self, id: u64) -> Option<Entry> {
        let i = self.entries.iter().position(|e| e.id == id)?;
        Some(self.entries.remove(i))
    }

    pub fn get(&self, id: u64) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }
}

pub fn thumb_name(id: u64) -> String {
    format!("{id}.thumb")
}

pub fn owned_name(id: u64) -> String {
    format!("{id}.png")
}

/// The history folder on disk.
pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Store {
        Store { dir: dir.into() }
    }

    pub fn index(&self) -> PathBuf {
        self.dir.join(INDEX)
    }

    /// The history, empty when the index is missing or broken (the note says why).
    pub fn load(&self) -> (History, Option<String>) {
        match std::fs::read_to_string(self.index()) {
            Ok(t) => match History::parse(&t) {
                Ok(h) => (h, None),
                Err(e) => (
                    History::default(),
                    Some(format!("history index broken; mutations disabled: {e}")),
                ),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (History::default(), None),
            Err(e) => (
                History::default(),
                Some(format!("history index unreadable: {e}")),
            ),
        }
    }

    fn save(&self, h: &History) -> std::io::Result<()> {
        crate::naming::write_small(&self.index(), h.to_text().as_bytes())
    }

    fn delete_files(&self, e: &Entry) {
        let _ = std::fs::remove_file(self.dir.join(thumb_name(e.id)));
        if e.owned {
            let own = self.dir.join(owned_name(e.id));
            // Only a file inside the history folder is ever deleted.
            if Path::new(&e.path) == own {
                let _ = std::fs::remove_file(own);
            }
        }
    }

    /// Serialize index and file mutations across the tray and history processes.
    /// Keep this handle alive until the whole mutation has finished.
    fn mutation_lock(&self) -> std::io::Result<std::fs::File> {
        std::fs::create_dir_all(&self.dir)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.dir.join(LOCK))?;
        file.try_lock()
            .map_err(|e| std::io::Error::other(format!("history is busy: {e}")))?;
        Ok(file)
    }

    /// Record a capture. `png`: `Some(bytes)` when the history must keep its own copy (then
    /// `e.path` is ignored and set to it). Returns the new id.
    pub fn record(
        &self,
        mut e: Entry,
        thumb: &crate::raster::Canvas,
        png: Option<&[u8]>,
    ) -> std::io::Result<u64> {
        let _lock = self.mutation_lock()?;
        let (mut h, note) = self.load();
        if let Some(note) = note {
            return Err(std::io::Error::other(note));
        }
        e.owned = png.is_some();
        let (id, evicted) = h.add(e);
        if let Some(bytes) = png {
            let own = self.dir.join(owned_name(id));
            crate::naming::write_small(&own, bytes)?;
            if let Some(x) = h.entries.first_mut() {
                x.path = own.to_string_lossy().into_owned();
            }
        }
        crate::naming::write_small(&self.dir.join(thumb_name(id)), &crate::thumb::encode(thumb))?;
        self.save(&h)?;
        for e in &evicted {
            self.delete_files(e);
        }
        Ok(id)
    }

    pub fn remove(&self, id: u64) -> std::io::Result<bool> {
        let _lock = self.mutation_lock()?;
        let (mut h, note) = self.load();
        if let Some(note) = note {
            return Err(std::io::Error::other(note));
        }
        let Some(e) = h.remove(id) else {
            return Ok(false);
        };
        self.save(&h)?;
        self.delete_files(&e);
        Ok(true)
    }

    pub fn thumb(&self, id: u64) -> Option<crate::raster::Canvas> {
        std::fs::read(self.dir.join(thumb_name(id)))
            .ok()
            .and_then(|b| crate::thumb::decode(&b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::testdir::TestDir;
    use crate::raster::{Canvas, Rgba};

    fn entry(path: &str) -> Entry {
        Entry {
            id: 0,
            time: "2026-11-04 19:50:12".into(),
            path: path.into(),
            owned: false,
            w: 10,
            h: 10,
            source: "snip".into(),
        }
    }

    #[test]
    fn add_keeps_the_newest_fifty() {
        let mut h = History::default();
        let mut evicted = Vec::new();
        for i in 0..53 {
            let (id, ev) = h.add(entry(&format!("{i}.png")));
            assert_eq!(id, i + 1);
            evicted.extend(ev);
        }
        assert_eq!(h.entries.len(), LIMIT);
        assert_eq!(h.entries[0].id, 53, "newest first");
        assert_eq!(h.entries.last().unwrap().id, 4);
        assert_eq!(
            evicted.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn ids_never_repeat_after_removal() {
        let mut h = History::default();
        h.add(entry("a"));
        let (b, _) = h.add(entry("b"));
        h.remove(b);
        let (c, _) = h.add(entry("c"));
        assert_eq!(c, 3);
    }

    #[test]
    fn index_round_trips_and_old_or_broken_files_are_handled() {
        let mut h = History::default();
        h.add(entry("C:\\shots\\a \"b\".png"));
        let t = h.to_text();
        assert_eq!(History::parse(&t).unwrap(), h);
        assert_eq!(
            History::parse("").unwrap(),
            History::default(),
            "empty file is an empty history"
        );
        assert!(History::parse("schema = 2").is_err());
        assert!(History::parse("schema = 1\nbogus = 3").is_err());
    }

    #[test]
    fn store_records_evicts_and_removes_files() {
        let d = TestDir::new("history");
        let s = Store::new(d.0.join("history"));
        let thumb = Canvas::new(4, 3, Rgba::rgb(9, 9, 9));
        let shots = d.0.join("shots");
        std::fs::create_dir_all(&shots).unwrap();
        // First capture: a saved file outside the history folder.
        let saved = shots.join("one.png");
        std::fs::write(&saved, b"png").unwrap();
        let first = s
            .record(entry(&saved.to_string_lossy()), &thumb, None)
            .unwrap();
        // Then 50 copy-only captures: the history keeps its own PNG.
        for _ in 0..LIMIT {
            s.record(entry(""), &thumb, Some(b"owned")).unwrap();
        }
        let (h, note) = s.load();
        assert!(note.is_none());
        assert_eq!(h.entries.len(), LIMIT);
        assert!(h.get(first).is_none(), "the first one fell off");
        assert!(
            saved.exists(),
            "a file in the screenshots folder is never deleted"
        );
        assert!(
            !s.dir.join(thumb_name(first)).exists(),
            "its thumbnail is gone"
        );
        let newest = &h.entries[0];
        assert!(newest.owned);
        assert_eq!(std::fs::read(&newest.path).unwrap(), b"owned");
        assert_eq!(s.thumb(newest.id), Some(thumb.clone()));
        // Remove deletes the owned copy and the thumbnail.
        let id = newest.id;
        let own = PathBuf::from(&newest.path);
        assert!(s.remove(id).unwrap());
        assert!(!own.exists());
        assert!(s.thumb(id).is_none());
        assert!(!s.remove(id).unwrap(), "second removal finds nothing");
        // Exactly 49 thumbnails, 49 owned PNGs and the index remain.
        let files = std::fs::read_dir(&s.dir).unwrap().count();
        assert_eq!(files, 49 + 49 + 2);
    }

    #[test]
    fn owned_flag_never_deletes_a_foreign_path() {
        let d = TestDir::new("history-foreign");
        let s = Store::new(&d.0);
        let foreign = d.0.join("elsewhere.png");
        std::fs::write(&foreign, b"x").unwrap();
        let mut h = History::default();
        let mut e = entry(&foreign.to_string_lossy());
        e.owned = true; // a hand-edited index claims ownership of a foreign file
        h.add(e);
        std::fs::write(s.index(), h.to_text()).unwrap();
        assert!(s.remove(1).unwrap());
        assert!(foreign.exists());
    }

    #[test]
    fn concurrent_mutation_fails_busy_then_reloads_and_preserves_entries() {
        let d = TestDir::new("history-lock");
        let s = Store::new(&d.0);
        let thumb = Canvas::new(1, 1, Rgba::rgb(1, 2, 3));
        let first = s.record(entry("first"), &thumb, None).unwrap();
        let lock = s.mutation_lock().unwrap();
        let dir = d.0.clone();
        let blocked = std::thread::spawn(move || {
            let other = Store::new(dir);
            other.record(
                entry("second"),
                &Canvas::new(1, 1, Rgba::rgb(1, 2, 3)),
                None,
            )
        })
        .join()
        .unwrap();
        assert!(blocked.unwrap_err().to_string().contains("busy"));
        assert_eq!(s.load().0.entries.len(), 1);
        drop(lock);
        let second = s.record(entry("second"), &thumb, None).unwrap();
        assert_eq!(
            s.load().0.entries.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![second, first]
        );
    }

    #[test]
    fn broken_index_is_not_overwritten_by_mutations() {
        let d = TestDir::new("history-broken-write");
        let s = Store::new(&d.0);
        let broken = "entry = [";
        std::fs::write(s.index(), broken).unwrap();
        let thumb = Canvas::new(1, 1, Rgba::rgb(1, 2, 3));
        assert!(s.record(entry(""), &thumb, Some(b"png")).is_err());
        assert!(s.remove(1).is_err());
        assert_eq!(std::fs::read_to_string(s.index()).unwrap(), broken);
        assert_eq!(std::fs::read_dir(&s.dir).unwrap().count(), 2);
    }

    #[test]
    fn broken_index_loads_empty_with_a_note() {
        let d = TestDir::new("history-broken");
        let s = Store::new(&d.0);
        std::fs::write(s.index(), "entry = [").unwrap();
        let (h, note) = s.load();
        assert!(h.entries.is_empty());
        assert!(note.unwrap().contains("broken"));
    }
}
