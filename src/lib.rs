//! snip: our own light screenshot and text capture tool for Windows. Win+Shift+S takes a plain snip;
//! a second, assignable hotkey opens the editor; Win+Shift+T reads text off the screen onto the
//! clipboard. Each works without the others.
//!
//! The crate has two halves:
//! - the core (every module without a `cfg`): geometry across monitors and DPI, the annotation
//!   model and its software renderer, the editor and toolbar state, history, settings, hotkeys,
//!   strings and theme. It has no OS calls, so `cargo test` runs it on Linux too.
//! - the Windows shell (`cfg(windows)`): hook, capture, overlay windows, clipboard, PNG, tray, and
//!   the pin, history and settings windows. It turns OS events into core calls and core output
//!   into pixels on screen.
//!
//! Nothing in either half opens a network connection (`tests/no_network.rs` checks the sources).

pub mod annotate;
pub mod autostart;
pub mod cardview;
pub mod config;
pub mod cursorfence;
pub mod delay;
pub mod dib;
pub mod dpi;
pub mod editor;
pub mod edges;
pub mod frame;
pub mod frameset;
pub mod geometry;
pub mod history;
pub mod historyview;
pub mod homeview;
pub mod hookstate;
pub mod hotkey;
pub mod i18n;
pub mod icons;
pub mod naming;
pub mod pinview;
pub mod parts;
pub mod raster;
pub mod record;
pub mod settingsview;
pub mod textcore;
pub mod texthistory;
pub mod textkey;
pub mod theme;
pub mod thumb;
pub mod toolbar;
pub mod wtc_import;

#[cfg(windows)]
pub mod log;

#[cfg(windows)]
pub mod actions;
#[cfg(windows)]
pub mod allshot;
#[cfg(windows)]
pub mod app;
#[cfg(windows)]
pub mod capture;
#[cfg(windows)]
pub mod card;
#[cfg(windows)]
pub mod clipboard;
#[cfg(windows)]
pub mod gdi;
#[cfg(windows)]
pub mod hook;
#[cfg(windows)]
pub mod looktest;
#[cfg(windows)]
pub mod ocr;
#[cfg(windows)]
pub mod overlay;
#[cfg(windows)]
pub mod paths;
#[cfg(windows)]
pub mod pinwin;
#[cfg(windows)]
pub mod png;
#[cfg(windows)]
pub mod probe;
#[cfg(windows)]
pub mod realtest;
#[cfg(windows)]
pub mod selftest;
#[cfg(windows)]
pub mod sys;
#[cfg(windows)]
pub mod textclip;
#[cfg(windows)]
pub mod textinput;
#[cfg(windows)]
pub mod texttest;
#[cfg(windows)]
pub mod tray;
#[cfg(windows)]
pub mod uiwin;

#[cfg(windows)]
pub mod partworker;

#[cfg(windows)]
pub mod partsoverlay;

#[cfg(windows)]
pub mod partcmd;

#[cfg(windows)]
pub mod parttest;

#[cfg(windows)]
pub mod record_media;
#[cfg(windows)]
pub mod record_capture;
#[cfg(windows)]
pub mod record_win;
#[cfg(windows)]
pub mod record_test;
