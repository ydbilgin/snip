//! The debug log (`%LOCALAPPDATA%/Laureth/snip/snip.log`) and QueryPerformanceCounter timings.
//!
//! Each snip writes one line with the time of every step in milliseconds, so "smooth" is a number.
//! The file rolls over to `snip.log.1` at 1 MB.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;

use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::SystemInformation::GetLocalTime;

struct Sink {
    path: PathBuf,
    echo: bool,
}

static SINK: Mutex<Option<Sink>> = Mutex::new(None);
static FREQ: OnceLock<i64> = OnceLock::new();
const MAX_BYTES: u64 = 1024 * 1024;

pub fn qpc() -> i64 {
    let mut t = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut t);
    }
    t
}

pub fn freq() -> i64 {
    *FREQ.get_or_init(|| {
        let mut f = 0i64;
        unsafe {
            let _ = QueryPerformanceFrequency(&mut f);
        }
        f.max(1)
    })
}

pub fn ticks_to_ms(ticks: i64) -> f64 {
    ticks as f64 * 1000.0 / freq() as f64
}

pub fn ms_since(t0: i64) -> f64 {
    ticks_to_ms(qpc() - t0)
}

/// `path`: None disables the file. `echo`: also print to stdout (probe, selftest).
pub fn init(path: Option<PathBuf>, echo: bool) {
    let mut g = SINK.lock().unwrap_or_else(|e| e.into_inner());
    *g = match path {
        Some(p) => {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            Some(Sink { path: p, echo })
        }
        None if echo => Some(Sink { path: PathBuf::new(), echo }),
        None => None,
    };
}

pub fn now_text() -> String {
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}

pub fn line(msg: &str) {
    let g = SINK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = g.as_ref() else { return };
    let text = format!("{} [{}] {}\n", now_text(), std::process::id(), msg);
    if s.echo {
        print!("{text}");
        let _ = std::io::stdout().flush();
    }
    if s.path.as_os_str().is_empty() {
        return;
    }
    if std::fs::metadata(&s.path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let mut old = s.path.as_os_str().to_owned();
        old.push(".1");
        let _ = std::fs::rename(&s.path, PathBuf::from(old));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&s.path) {
        let _ = f.write_all(text.as_bytes());
    }
}

#[macro_export]
macro_rules! logf {
    ($($arg:tt)*) => { $crate::log::line(&format!($($arg)*)) };
}

/// Step timings for one snip: `mark("freeze")` records the time since the previous mark.
pub struct Steps {
    t0: i64,
    last: i64,
    parts: Vec<(String, f64)>,
}

impl Steps {
    pub fn start_at(t0: i64) -> Self {
        Self { t0, last: t0, parts: Vec::new() }
    }

    pub fn start() -> Self {
        Self::start_at(qpc())
    }

    pub fn mark(&mut self, name: &str) -> f64 {
        let now = qpc();
        let ms = ticks_to_ms(now - self.last);
        self.last = now;
        self.parts.push((name.to_string(), ms));
        ms
    }

    pub fn total_ms(&self) -> f64 {
        ms_since(self.t0)
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        for (n, ms) in &self.parts {
            s.push_str(&format!("{n}={ms:.2} "));
        }
        s.push_str(&format!("total={:.2}ms", self.total_ms()));
        s
    }
}
