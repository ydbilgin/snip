#![windows_subsystem = "windows"]
//! snip command line.
//!
//!   snip                      start the tray process (Win+Shift+S, the editor hotkey); a second start exits 0
//!   snip snip [--edit] [--delay 3|5]   ask the running tray process for a snip (or the editor)
//!   snip history              the history window (last 50 captures)
//!   snip settings             the settings window
//!   snip home                 the main window: what Snip does, its keys, recent results
//!   snip text                 ask the running tray process for a text capture
//!   snip all                  ask the running tray process for all screens as one picture
//!   snip import-wintextcapture [--write]   take over WinTextCapture's settings (reads its file only)
//!   snip --pin <png> <x> <y> [--temp]  pin an image on top of the screen
//!   snip quit                 stop the running tray process
//!   snip status               print whether it runs and its state file
//!   snip --selftest           checks without screen, window or clipboard; exit 0 = all passed
//!   snip probe [options]      P0 measurements into a log (see src/probe.rs)
//!       --out <dir>  --runs <n>  --wgc  --live  --save-images  --hook-seconds <n>
//!   snip --no-hook --no-tray --exit-after <s>   idle test: no hook, no tray, exits and logs memory
//!   snip --demo-gif DIR [--scene FILE]  the README films and stills, drawn by the app's own renderer from scripts (no window)
//!   snip --selftest-editor [--isolated-clipboard]  hidden synthetic editor acceptance; clipboard requires private SnipNightTest* station
//!   snip --selftest-perf [ultra|ultra-1khz|small|four|four-1khz]
//!                             frame times and memory on a synthetic monitor (hidden desktop only);
//!                             a case runs in a single device, one process per case
//!   snip --selftest-look DIR SCENE [--dpi N]  the overlay drawn on a synthetic frame and written as PNG (hidden desktop)
//!   snip --selftest-real      real capture, overlay, text box, clipboard (visible desktop, 2 min idle)
//!   snip --selftest-text DIR  text capture on synthetic screens: reader, overlay, session (hidden desktop)
//!   snip --selftest-ocr-bench [FILE]   the reader's error rate on rendered text (hidden desktop)
//!   snip --selftest-fence     the real mouse clip, rect read back (hidden desktop only; skips when a game holds a clip)
//!   snip --selftest-clip      the real clipboard path, on a private window station with its own clipboard
//!   snip --ocr-file PNG [auto|TAG]  read a picture file like a text capture; prints each reader's text and times
//!   snip --image-file PNG OUTDIR    what image_format = "auto" would save for this picture; writes it to OUTDIR
//!   snip --version

#[cfg(not(windows))]
fn main() {
    eprintln!("snip runs on Windows. On this OS only the core library and its tests build (cargo test).");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    win::main()
}

#[cfg(windows)]
mod win {
use std::path::PathBuf;

use snip::app::{self, RunOpts};
use snip::config::Config;
use snip::{logf, paths};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::core::w;

fn attach_console() {
    unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        if h.map(|h| h.is_invalid() || h.0.is_null()).unwrap_or(true) {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

const USAGE: &str = "usage: snip [record [--stop] | --selftest-record DIR | snip [--edit] [--delay 3|5] | all | text | history | settings | --pin PNG X Y [--temp] | quit | status | --selftest | parts --at X,Y [--log FILE] | probe [--out DIR] [--runs N] [--wgc] [--live] [--save-images] [--hook-seconds N] | --no-hook --no-tray --exit-after S | --version]";

pub fn main() {
    unsafe {
        // The manifest already declares per-monitor-v2; this is the fallback if it was stripped.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    // A mouse clip must never outlive the process: release it at a panic (the release build aborts
    // there) and again at the end.
    snip::cursorfence::os::install_panic_release();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = dispatch(&args);
    snip::cursorfence::os::release_ours();
    std::process::exit(code);
}

/// `snip --image-file PNG OUTDIR`: the save path's encoders on a picture file, with sizes and times.
fn image_file(args: &[String]) -> i32 {
    let (Some(input), Some(out)) = (args.first(), args.get(1)) else {
        println!("usage: snip --image-file PNG OUTDIR");
        return 2;
    };
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    let Ok((w, h, px)) = std::fs::read(input).map_err(|e| e.to_string()).and_then(|b| snip::png::decode_png(&b).map_err(|e| e.to_string())) else {
        println!("FAIL cannot read {input}");
        return 1;
    };
    let t = snip::log::qpc();
    let Ok(png) = snip::png::encode_png(w, h, &px) else { println!("FAIL png"); return 1 };
    let png_ms = snip::log::ms_since(t);
    let photo = snip::naming::photo_like(png.len(), w, h);
    let t = snip::log::qpc();
    let jpeg = match snip::png::encode_jpeg(w, h, &px, 0.95) {
        Ok(j) => j,
        Err(e) => {
            println!("FAIL jpeg {:#010x} {}", e.code().0, e.message());
            return 1;
        }
    };
    let jpeg_ms = snip::log::ms_since(t);
    let stem = std::path::Path::new(input).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let dir = std::path::Path::new(out);
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(format!("{stem}.png")), &png);
    let _ = std::fs::write(dir.join(format!("{stem}.jpg")), &jpeg);
    println!("{w}x{h} png {} bytes {:.2} bit/px {png_ms:.1} ms; photo_like={photo}; jpeg q95 4:4:4 {} bytes {jpeg_ms:.1} ms; auto saves {}", png.len(), png.len() as f64 * 8.0 / (w as f64 * h as f64), jpeg.len(), if photo { "JPEG" } else { "PNG" });
    0
}

fn dispatch(args: &[String]) -> i32 {
    let first = args.first().map(String::as_str).unwrap_or("");
    match first {
        "--version" | "version" => {
            attach_console();
            println!("snip {}", env!("CARGO_PKG_VERSION"));
            0
        }
        "--help" | "-h" | "help" => {
            attach_console();
            println!("{USAGE}");
            0
        }
        "--selftest" | "selftest" => {
            attach_console();
            snip::selftest::run()
        }
        "--demo-gif" => {
            // The README films: no window, no screen, no input; see src/demo.rs.
            attach_console();
            let Some(dir) = args.get(1).filter(|d| !d.starts_with("--")) else {
                println!("usage: snip --demo-gif DIR [--scene FILE]");
                return 2;
            };
            // The PNG encoder is WIC, a COM component.
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
            }
            let g = snip::gdi::GdiGlyphs::new();
            // `--scene FILE`: also write the made-up desktop for `--selftest-look` (u32 w, u32 h, BGRA).
            if let Some(file) = args.iter().skip(2).position(|a| a == "--scene").and_then(|i| args.get(i + 3)) {
                if let Some(parent) = std::path::Path::new(file).parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let s = snip::demo::scene(&g, 1.0, &snip::demo::Desk::default());
                let mut raw = Vec::with_capacity(8 + s.px.len());
                raw.extend_from_slice(&s.w.to_le_bytes());
                raw.extend_from_slice(&s.h.to_le_bytes());
                raw.extend_from_slice(&s.px);
                if let Err(e) = std::fs::write(file, raw) {
                    println!("FAIL scene: {e}");
                    return 1;
                }
            }
            // The text film's words come from the Windows OCR, as a text capture reads them.
            let ocr = |c: &snip::raster::Canvas| snip::ocr::read(c.w, c.h, &c.px, "auto").map(|r| r.text).map_err(|e| format!("{e:?}"));
            match snip::demo::write_all(std::path::Path::new(dir), &g, &ocr) {
                Ok((lines, stills)) => {
                    for l in lines {
                        println!("{l}");
                    }
                    for (name, still) in stills {
                        let path = std::path::Path::new(dir).join(format!("{name}.png"));
                        match snip::png::encode_png(still.w, still.h, &still.px).map_err(|e| e.to_string()).and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string())) {
                            Ok(()) => println!("{name}.png: {}x{}", still.w, still.h),
                            Err(e) => {
                                println!("FAIL still {name}: {e}");
                                return 1;
                            }
                        }
                    }
                    0
                }
                Err(e) => {
                    println!("FAIL {e}");
                    1
                }
            }
        }
        "--selftest-editor" => {
            attach_console();
            snip::selftest::run_editor(args.iter().any(|a| a == "--isolated-clipboard"))
        }
        "--selftest-parts" => { attach_console(); snip::parttest::run() }
        "--selftest-parts-target" => { attach_console(); snip::parttest::target() }
        "--selftest-overlay" => {
            attach_console();
            snip::selftest::run_overlay()
        }
        "--selftest-perf" => {
            attach_console();
            snip::realtest::run_perf(args.get(1).map(String::as_str))
        }
        "--selftest-look" => {
            attach_console();
            snip::looktest::run_look(&args[1..])
        }
        "--selftest-seed" => {
            attach_console();
            snip::looktest::run_seed(&args[1..])
        }
        "--selftest-menu" => {
            attach_console();
            snip::looktest::run_menu(&args[1..])
        }
        "--selftest-real" => {
            attach_console();
            snip::realtest::run_real()
        }
        "--selftest-text" => {
            attach_console();
            snip::texttest::run_text(&args[1..])
        }
        "--selftest-ocr-bench" => {
            attach_console();
            snip::texttest::run_bench(&args[1..])
        }
        "--selftest-fence" => {
            attach_console();
            snip::cursorfence::run_selftest()
        }
        "--selftest-clip" => {
            attach_console();
            snip::texttest::run_clip()
        }
        "--ocr-file" => {
            attach_console();
            snip::ocr::run_file(&args[1..])
        }
        "--image-file" => {
            attach_console();
            image_file(&args[1..])
        }
        "history" => snip::uiwin::run_history(),
        "settings" => snip::uiwin::run_settings(),
        "home" => snip::uiwin::run_home(),
        "all" => {
            attach_console();
            if snip::actions::tell_tray(app::WM_APP_COMMAND, 4, 0) {
                0
            } else {
                println!("snip is not running");
                3
            }
        }
        "record" => {
            attach_console();
            if args.len()>2 || args.get(1).is_some_and(|a|a!="--stop") { println!("usage: snip record [--stop]"); return 2; }
            if snip::actions::tell_tray(app::WM_APP_COMMAND, if args.iter().any(|a|a=="--stop") {6} else {5}, 0) {0} else {println!("snip is not running");3}
        }
        "--selftest-record" => {
            attach_console();
            snip::record_test::run(&args[1..])
        }
        "text" => {
            attach_console();
            if snip::actions::tell_tray(app::WM_APP_COMMAND, 3, 0) {
                0
            } else {
                println!("snip is not running");
                3
            }
        }
        "import-wintextcapture" => {
            attach_console();
            let from = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default().join("WinTextCapture").join("settings.json");
            let json = match std::fs::read_to_string(&from) {
                Ok(j) => j,
                Err(e) => {
                    println!("cannot read {}: {e}", from.display());
                    return 1;
                }
            };
            let path = paths::config_file();
            let (cfg, _) = Config::load_or_create(&path);
            let write = args.iter().any(|a| a == "--write");
            match snip::wtc_import::run(&json, &cfg, write, |text| snip::naming::write_small(&path, text.as_bytes()).map_err(|e| e.to_string())) {
                Ok(report) => {
                    print!("{report}");
                    if write {
                        snip::actions::tell_tray(snip::uiwin::WM_APP_RELOAD, 0, 0);
                    }
                    0
                }
                Err(e) => {
                    println!("{e}");
                    1
                }
            }
        }
        "--pin" => {
            let (cfg, _) = Config::load_or_create(&paths::config_file());
            snip::log::init(if cfg.debug_log { Some(paths::log_file()) } else { None }, false);
            let path = args.get(1).map(PathBuf::from);
            let x = args.get(2).and_then(|s| s.parse().ok());
            let y = args.get(3).and_then(|s| s.parse().ok());
            let temp = args.iter().any(|a| a == "--temp");
            match (path, x, y) {
                (Some(p), Some(x), Some(y)) => snip::pinwin::run(p, x, y, temp),
                _ => {
                    attach_console();
                    println!("{USAGE}");
                    2
                }
            }
        }
        "snip" => {
            attach_console();
            let edit = args.iter().any(|a| a == "--edit");
            let delay = match args.iter().position(|a| a == "--delay") {
                Some(i) => match args.get(i + 1).and_then(|s| snip::delay::parse_seconds(s)) {
                    Some(d) => d,
                    None => {
                        println!("--delay takes 3 or 5\n{USAGE}");
                        return 2;
                    }
                },
                None => 0,
            };
            if snip::actions::tell_tray(app::WM_APP_COMMAND, if edit { 2 } else { 1 }, delay as isize) {
                0
            } else {
                println!("snip is not running");
                3
            }
        }
        "quit" | "--quit" => {
            attach_console();
            app::quit_running()
        }
        "status" => {
            attach_console();
            let running = app::is_running();
            println!("running: {running}");
            println!("state file: {}", paths::state_file().display());
            match std::fs::read_to_string(paths::state_file()) {
                Ok(s) => print!("{s}"),
                Err(e) => println!("(no state file: {e})"),
            }
            println!("log: {}", paths::log_file().display());
            if running { 0 } else { 3 }
        }
        "parts" => {
            attach_console();
            snip::partcmd::run(&args[1..])
        }
        "probe" => {
            attach_console();
            let mut o = snip::probe::ProbeOpts {
                out: paths::state_dir().join("probe"),
                runs: 5,
                wgc: false,
                live: false,
                save_images: false,
                hook_seconds: 30,
            };
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--out" => o.out = PathBuf::from(it.next().cloned().unwrap_or_default()),
                    "--runs" => o.runs = it.next().and_then(|s| s.parse().ok()).unwrap_or(5).clamp(1, 100),
                    "--wgc" => o.wgc = true,
                    "--live" => o.live = true,
                    "--save-images" => o.save_images = true,
                    "--hook-seconds" => o.hook_seconds = it.next().and_then(|s| s.parse().ok()).unwrap_or(30),
                    other => {
                        println!("unknown probe option {other}\n{USAGE}");
                        return 2;
                    }
                }
            }
            snip::probe::run(o)
        }
        _ => run_app(args),
    }
}

fn run_app(args: &[String]) -> i32 {
    let mut opts = RunOpts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--no-hook" => opts.no_hook = true,
            "--no-tray" => opts.no_tray = true,
            "--warm-device" => opts.warm_device = true,
            "--session" => {
                let parent = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                opts.session = Some(app::SessionArgs { parent, ..Default::default() });
            }
            "--go" => {
                let t0 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                let src = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                if let Some(s) = opts.session.as_mut() {
                    s.go = Some((t0, src));
                }
            }
            "--selftest-dir" => {
                let d = it.next().map(PathBuf::from);
                if let Some(s) = opts.session.as_mut() {
                    s.selftest_dir = d;
                }
            }
            "--selftest-scene" => {
                let d = it.next().map(PathBuf::from);
                if let Some(s) = opts.session.as_mut() {
                    s.selftest_scene = d;
                }
            }
            "--selftest-drag" => {
                let v: Vec<i32> = it.by_ref().take(4).filter_map(|s| s.parse().ok()).collect();
                if let (Some(s), [a, b, c, d]) = (opts.session.as_mut(), v.as_slice()) {
                    s.selftest_drag = Some((*a, *b, *c, *d));
                }
            }
            "--measure-hook" => opts.measure_hook = true,
            "--exit-after" => opts.exit_after = it.next().and_then(|s| s.parse().ok()),
            other => {
                attach_console();
                println!("unknown argument {other}\n{USAGE}");
                return 2;
            }
        }
    }
    let test = opts.exit_after.is_some();
    if test {
        attach_console();
    }
    let session = opts.session.is_some();
    let first_run = !paths::config_file().exists();
    let (mut cfg, note) = Config::load_or_create(&paths::config_file());
    if opts.session.as_ref().is_some_and(|s| s.selftest_dir.is_some()) {
        // Self-test child on a hidden desktop: GDI only (WGC could draw its border on the user's screen).
        cfg.capture = snip::config::CaptureMethod::Gdi;
    }
    snip::log::init(if cfg.debug_log || test { Some(paths::log_file()) } else { None }, test);
    if let Some(n) = note {
        logf!("{n}");
    }
    if !test && !session {
        // One tray process per logon session.
        let m = unsafe { CreateMutexW(None, true, w!("Local\\Laureth.snip.instance")) };
        let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if m.is_err() || already {
            // Starting it again means he is looking for it: show what it does.
            logf!("snip already running; opening its window");
            snip::actions::spawn_self("home");
            return 0;
        }
        std::mem::forget(m);
        // The first time ever, it says what it is instead of hiding in the tray.
        if first_run {
            snip::actions::spawn_self("home");
        }
    }
    app::run(cfg, opts)
}
}
