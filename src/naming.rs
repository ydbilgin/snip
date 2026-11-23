//! File names and the atomic write.
//!
//! Windows' own Snipping Tool names files `Ekran görüntüsü 2026-10-08 125520.png` on the user's PC (1,444 of
//! 1,446 files in the user's Screenshots folder on 2026-10-08 follow it). snip uses the same
//! pattern so existing habits and scripts that look for these names keep working. Within the same second it adds ` (2)`,
//! ` (3)`, ... The PNG is first written as `<name>.png.part` and then renamed, so a reader that
//! filters on `.png` never sees a half-written file.

use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

pub const PREFIX: &str = "Ekran görüntüsü";

pub fn base_name(s: &Stamp) -> String {
    format!(
        "{PREFIX} {:04}-{:02}-{:02} {:02}{:02}{:02}",
        s.year, s.month, s.day, s.hour, s.minute, s.second
    )
}

pub fn part_path(final_path: &Path) -> PathBuf {
    let mut s = final_path.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

/// The first free name for this second: `<base>.png`, then `<base> (2).png`, ... A name counts as
/// taken when either the `.png` or its `.png.part` exists.
pub fn pick_name(dir: &Path, stamp: &Stamp, exists: impl Fn(&Path) -> bool) -> PathBuf {
    pick_name_ext(dir, stamp, "png", exists)
}

/// `pick_name` with another extension ("jpg" for a photo-like capture in `image_format = "auto"`).
pub fn pick_name_ext(dir: &Path, stamp: &Stamp, ext: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let base = base_name(stamp);
    let mut n = 1u64;
    loop {
        let name = if n == 1 { format!("{base}.{ext}") } else { format!("{base} ({n}).{ext}") };
        let p = dir.join(name);
        if !exists(&p) && !exists(&part_path(&p)) {
            return p;
        }
        n += 1;
    }
}

/// Write `bytes` to `<final>.part`, flush it to disk, then rename it over `final` with
/// MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH). On any error the `.part`
/// file is removed and `final` is untouched.
pub fn write_atomic(final_path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_then(final_path, bytes, rename_replace)
}

/// An automatic capture must never replace an existing screenshot, even if two save workers
/// picked the same name before either finished. Retry only a name collision.
pub fn write_capture(dir: &Path, stamp: &Stamp, bytes: &[u8]) -> std::io::Result<PathBuf> {
    write_capture_ext(dir, stamp, "png", bytes)
}

pub fn write_capture_ext(dir: &Path, stamp: &Stamp, ext: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    loop {
        let path = pick_name_ext(dir, stamp, ext, |p| p.exists());
        match write_then(&path, bytes, rename_new) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

fn write_then(final_path: &Path, bytes: &[u8], publish: fn(&Path, &Path) -> std::io::Result<()>) -> std::io::Result<()> {
    let part = part_path(final_path);
    // create_new: a .part file that is not ours is never overwritten or removed.
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&part)?;
    let result = (|| {
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok::<(), std::io::Error>(())
    })();
    drop(f);
    let result = result.and_then(|()| publish(&part, final_path));
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// PNG bits per pixel above which a capture counts as photo-like (games, video, photos). Measured
/// 2026-11-13 on the user's saves: text and UI screens 0.14-2.3 bit/px, games 4.4-8.7 bit/px.
pub const PHOTO_BITS_PER_PIXEL: f64 = 3.0;

/// Whether `image_format = "auto"` saves this capture as JPEG: its PNG already costs more than
/// `PHOTO_BITS_PER_PIXEL`. The PNG is encoded anyway, so the measure is free.
pub fn photo_like(png_bytes: usize, width: u32, height: u32) -> bool {
    let pixels = width as f64 * height as f64;
    // Under 256x256 pixels the file is small whatever its kind: it stays PNG.
    pixels >= 256.0 * 256.0 && png_bytes as f64 * 8.0 / pixels > PHOTO_BITS_PER_PIXEL
}

/// For small state files only (state.json, last.txt): a stale `.part` left by a crash is ours,
/// so it is removed first. Never use this for screenshots.
pub fn write_small(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let _ = std::fs::remove_file(part_path(path));
    write_atomic(path, bytes)
}

#[cfg(not(windows))]
fn rename_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    // POSIX rename replaces atomically; the directory entry is flushed with the file's sync above.
    std::fs::rename(from, to)
}

#[cfg(not(windows))]
pub(crate) fn rename_new(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::hard_link(from, to)?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

#[cfg(windows)]
pub(crate) fn rename_new(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    use windows::core::PCWSTR;
    let a: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let b: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe { MoveFileExW(PCWSTR(a.as_ptr()), PCWSTR(b.as_ptr()), MOVEFILE_WRITE_THROUGH) }
        .map_err(|e| std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF))
}

#[cfg(windows)]
fn rename_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW};
    use windows::core::PCWSTR;
    let a: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let b: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe { MoveFileExW(PCWSTR(a.as_ptr()), PCWSTR(b.as_ptr()), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) }
        .map_err(|e| std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF))
}

#[cfg(test)]
pub(crate) mod testdir {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh empty folder under the system temp folder, removed on drop.
    pub struct TestDir(pub PathBuf);
    static N: AtomicU32 = AtomicU32::new(0);
    impl TestDir {
        pub fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("snip-test-{}-{}-{}", tag, std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testdir::TestDir;
    use super::*;

    #[test]
    fn photo_like_is_decided_by_the_png_cost() {
        assert!(!photo_like(104_791, 2560, 1392), "a text screen, 0.24 bit/px");
        assert!(photo_like(3_577_601, 2560, 1392), "a game screen, 8.0 bit/px");
        assert!(!photo_like(1_037_348, 2560, 1440), "2.25 bit/px stays PNG");
        assert!(!photo_like(7_922, 149, 44), "tiny crops stay PNG whatever their cost");
    }

    #[test]
    fn names_take_the_extension() {
        let dir = Path::new("C:/x");
        assert_eq!(pick_name_ext(dir, &STAMP, "jpg", |_| false), dir.join("Ekran görüntüsü 2026-10-08 125520.jpg"));
        let first = dir.join("Ekran görüntüsü 2026-10-08 125520.jpg");
        assert_eq!(pick_name_ext(dir, &STAMP, "jpg", |p| p == first), dir.join("Ekran görüntüsü 2026-10-08 125520 (2).jpg"));
    }

    const STAMP: Stamp = Stamp { year: 2026, month: 10, day: 8, hour: 12, minute: 55, second: 20 };

    #[test]
    fn base_name_matches_windows_turkish_pattern() {
        assert_eq!(base_name(&STAMP), "Ekran görüntüsü 2026-10-08 125520");
        let early = Stamp { year: 2026, month: 1, day: 2, hour: 3, minute: 4, second: 5 };
        assert_eq!(base_name(&early), "Ekran görüntüsü 2026-01-02 030405");
    }

    #[test]
    fn name_uses_the_same_turkish_letters_as_windows() {
        let n = base_name(&STAMP);
        assert!(n.starts_with("Ekran g\u{00F6}r\u{00FC}nt\u{00FC}s\u{00FC} "), "{n}");
        // The same UTF-8 (NFC) bytes as the names Snipping Tool wrote on the user's PC (checked 2026-10-08).
        let want = [0x45, 0x6b, 0x72, 0x61, 0x6e, 0x20, 0x67, 0xc3, 0xb6, 0x72, 0xc3, 0xbc, 0x6e, 0x74, 0xc3, 0xbc, 0x73, 0xc3, 0xbc, 0x20];
        assert_eq!(&n.as_bytes()[..20], &want);
    }

    #[test]
    fn pick_name_adds_a_counter_within_the_same_second() {
        let dir = Path::new("X:/shots");
        let taken = |n: usize| {
            move |p: &Path| {
                let names: Vec<PathBuf> = (1..=n)
                    .map(|i| if i == 1 { dir.join("Ekran görüntüsü 2026-10-08 125520.png") } else { dir.join(format!("Ekran görüntüsü 2026-10-08 125520 ({i}).png")) })
                    .collect();
                names.iter().any(|x| x == p)
            }
        };
        assert_eq!(pick_name(dir, &STAMP, taken(0)), dir.join("Ekran görüntüsü 2026-10-08 125520.png"));
        assert_eq!(pick_name(dir, &STAMP, taken(1)), dir.join("Ekran görüntüsü 2026-10-08 125520 (2).png"));
        assert_eq!(pick_name(dir, &STAMP, taken(2)), dir.join("Ekran görüntüsü 2026-10-08 125520 (3).png"));
    }

    #[test]
    fn a_pending_part_file_also_takes_the_name() {
        let dir = Path::new("X:/shots");
        let first = dir.join("Ekran görüntüsü 2026-10-08 125520.png");
        let part = part_path(&first);
        let got = pick_name(dir, &STAMP, |p| p == part);
        assert_eq!(got, dir.join("Ekran görüntüsü 2026-10-08 125520 (2).png"));
    }

    #[test]
    fn a_crowded_second_keeps_checking_names_after_9999() {
        let dir = Path::new("X:/shots");
        let free = dir.join(format!("{PREFIX} 2026-10-08 125520 (10000).png"));
        let free_part = part_path(&free);
        let checked_free = std::cell::Cell::new(false);
        // Every earlier PNG or pending write is occupied; no real files are created.
        let picked = pick_name(dir, &STAMP, |p| {
            if p == free {
                checked_free.set(true);
                false
            } else {
                p != free_part
            }
        });
        assert_eq!(picked, free);
        assert!(checked_free.get(), "the returned name must have been checked");
    }

    #[test]
    fn part_path_appends_part() {
        assert_eq!(part_path(Path::new("C:/a/b.png")), PathBuf::from("C:/a/b.png.part"));
    }

    #[test]
    fn atomic_write_leaves_only_the_final_file() {
        let d = TestDir::new("atomic");
        let p = pick_name(&d.0, &STAMP, |p| p.exists());
        write_atomic(&p, b"hello png").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hello png");
        assert!(!part_path(&p).exists());
        let entries: Vec<_> = std::fs::read_dir(&d.0).unwrap().collect();
        assert_eq!(entries.len(), 1);
        // Same second again: the next name gets " (2)".
        let p2 = pick_name(&d.0, &STAMP, |p| p.exists());
        assert!(p2.to_string_lossy().ends_with("125520 (2).png"), "{p2:?}");
        write_atomic(&p2, b"second").unwrap();
        assert_eq!(std::fs::read_dir(&d.0).unwrap().count(), 2);
    }

    #[test]
    fn atomic_write_replaces_an_existing_final_file() {
        let d = TestDir::new("replace");
        let p = d.0.join("x.png");
        std::fs::write(&p, b"old").unwrap();
        write_atomic(&p, b"new").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"new");
    }

    #[test]
    fn automatic_publish_never_overwrites_a_name_taken_after_selection() {
        let d = TestDir::new("capture-collision");
        let p = pick_name(&d.0, &STAMP, |p| p.exists());
        std::fs::write(&p, b"existing screenshot").unwrap();
        assert_eq!(write_then(&p, b"new screenshot", rename_new).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&p).unwrap(), b"existing screenshot");
        assert!(!part_path(&p).exists());
        let next = write_capture(&d.0, &STAMP, b"new screenshot").unwrap();
        assert_ne!(p, next);
        assert_eq!(std::fs::read(next).unwrap(), b"new screenshot");
    }

    #[test]
    fn concurrent_automatic_captures_all_keep_their_content() {
        let d = TestDir::new("capture-workers");
        let workers: Vec<_> = (0..8u8).map(|n| {
            let dir = d.0.clone();
            std::thread::spawn(move || {
                let path = write_capture(&dir, &STAMP, &[n; 64]).unwrap();
                (path, n)
            })
        }).collect();
        let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        for (path, n) in &results { assert_eq!(std::fs::read(path).unwrap(), vec![*n; 64]); }
        assert_eq!(std::fs::read_dir(&d.0).unwrap().count(), results.len());
    }

    #[test]
    fn failed_write_leaves_no_part_file_and_no_final_file() {
        let d = TestDir::new("fail");
        // A directory sits where the final file should go: the rename must fail.
        let p = d.0.join("blocked.png");
        std::fs::create_dir(&p).unwrap();
        std::fs::write(p.join("inside"), b"x").unwrap();
        assert!(write_atomic(&p, b"data").is_err());
        assert!(!part_path(&p).exists());
        assert!(p.is_dir());
    }

    #[test]
    fn stale_part_file_blocks_the_write_instead_of_being_overwritten() {
        let d = TestDir::new("stale");
        let p = d.0.join("y.png");
        std::fs::write(part_path(&p), b"someone else").unwrap();
        assert!(write_atomic(&p, b"data").is_err());
        assert!(!p.exists());
    }
}
