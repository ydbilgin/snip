//! Where snip keeps things.
//! - config: %APPDATA%/Laureth/snip/config.toml
//! - log, state, last.txt: %LOCALAPPDATA%/Laureth/snip/
//! - PNGs: the config's `folder`, else Windows' Screenshots known folder.

use std::path::PathBuf;

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Screenshots, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

fn env_dir(name: &str) -> PathBuf {
    std::env::var_os(name).map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

pub fn config_dir() -> PathBuf {
    env_dir("APPDATA").join("Laureth").join("snip")
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn state_dir() -> PathBuf {
    env_dir("LOCALAPPDATA").join("Laureth").join("snip")
}

pub fn log_file() -> PathBuf {
    state_dir().join("snip.log")
}

pub fn last_file() -> PathBuf {
    state_dir().join("last.txt")
}

pub fn state_file() -> PathBuf {
    state_dir().join("state.json")
}

/// Windows' Screenshots folder (FOLDERID_Screenshots); falls back to %USERPROFILE%/Pictures/Screenshots.
pub fn screenshots_dir() -> PathBuf {
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Screenshots, KF_FLAG_DEFAULT, None) {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            if !s.is_empty() {
                return PathBuf::from(s);
            }
        }
    }
    env_dir("USERPROFILE").join("Pictures").join("Screenshots")
}

pub fn output_dir(configured: &str) -> PathBuf {
    if configured.trim().is_empty() { screenshots_dir() } else { PathBuf::from(configured.trim()) }
}

/// The capture history (`history.rs`).
pub fn history_dir() -> PathBuf {
    state_dir().join("history")
}

/// The editor's remembered colour and size (`editor::Prefs`, schema 1).
pub fn editor_prefs_file() -> PathBuf {
    state_dir().join("editor.toml")
}

/// Pinned images when the history is off (deleted when the pin closes).
pub fn pins_dir() -> PathBuf {
    state_dir().join("pins")
}
