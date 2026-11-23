//! Hidden-desktop acceptance: real foreign HWND/UIA, then explicit overlay input. No SendInput.
use crate::capture::{self, Frame, Method, MonitorInfo, Pixels, Snapshot, WindowInfo};
use crate::geometry::{Layout, Point, Rect};
use crate::overlay::{Kind, Opts, Outcome, Overlay};
use crate::parts::Source;
use std::io::{BufRead, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

fn hidden() -> bool {
    let name = crate::selftest::desktop_name();
    !name.is_empty()
        && !name.eq_ignore_ascii_case("Default")
        && !name.eq_ignore_ascii_case("Winlogon")
}
unsafe fn paint_edges(dc: HDC) {
    unsafe {
        for (r, color) in [
            (
                RECT {
                    left: 0,
                    top: 0,
                    right: 200,
                    bottom: 500,
                },
                30u32,
            ),
            (
                RECT {
                    left: 200,
                    top: 0,
                    right: 201,
                    bottom: 500,
                },
                240,
            ),
            (
                RECT {
                    left: 201,
                    top: 0,
                    right: 400,
                    bottom: 500,
                },
                70,
            ),
        ] {
            let brush = CreateSolidBrush(COLORREF(color * 0x010101));
            FillRect(dc, &r, brush);
            let _ = DeleteObject(brush.into());
        }
    }
}
unsafe extern "system" fn target_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if GetWindowLongPtrW(hwnd, GWLP_USERDATA) == 1 {
            match msg {
                WM_PRINT | WM_PRINTCLIENT => {
                    paint_edges(HDC(wp.0 as *mut _));
                    return LRESULT(0);
                }
                WM_PAINT => {
                    let mut ps = PAINTSTRUCT::default();
                    let dc = BeginPaint(hwnd, &mut ps);
                    paint_edges(dc);
                    let _ = EndPaint(hwnd, &ps);
                    return LRESULT(0);
                }
                _ => {}
            }
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}
/// Internal helper; also refuses the visible desktop before registering/creating any window.
pub fn target() -> i32 {
    if !hidden() {
        println!("REFUSED: --selftest-parts-target requires a hidden desktop");
        return 4;
    }
    let result = (|| -> windows::core::Result<()> {
        unsafe {
            let inst: HINSTANCE = GetModuleHandleW(None)?.into();
            let class = w!("Laureth.snip.parts-target");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(target_proc),
                hInstance: inst,
                lpszClassName: class,
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                return Err(windows::core::Error::from_thread());
            }
            let root = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class,
                w!(""),
                WS_POPUP,
                40,
                140,
                660,
                500,
                None,
                None,
                Some(inst),
                None,
            )?;
            let outer = CreateWindowExW(
                Default::default(),
                w!("STATIC"),
                w!("container"),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(0x100),
                20,
                20,
                620,
                460,
                Some(root),
                None,
                Some(inst),
                None,
            )?;
            let left = CreateWindowExW(
                Default::default(),
                w!("STATIC"),
                w!("left panel"),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(0x100),
                20,
                20,
                240,
                380,
                Some(outer),
                None,
                Some(inst),
                None,
            )?;
            let right = CreateWindowExW(
                Default::default(),
                w!("STATIC"),
                w!("right panel"),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(0x100),
                300,
                20,
                280,
                380,
                Some(outer),
                None,
                Some(inst),
                None,
            )?;
            let edges = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class,
                w!(""),
                WS_POPUP,
                760,
                140,
                400,
                500,
                None,
                None,
                Some(inst),
                None,
            )?;
            SetWindowLongPtrW(edges, GWLP_USERDATA, 1);
            for hwnd in [root, edges] {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                let _ = UpdateWindow(hwnd);
            }
            let rect = |h| crate::partcmd::rect_json(crate::overlay::window_rect(h));
            println!(
                "{}",
                serde_json::json!({"pid":std::process::id(),"root":root.0 as isize,"edges":edges.0 as isize,"root_rect":rect(root),"outer":rect(outer),"left":rect(left),"right":rect(right),"edges_rect":rect(edges)})
            );
            let _ = std::io::stdout().flush();
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            Ok(())
        }
    })();
    if let Err(e) = result {
        println!("FAIL parts target: {e}");
        1
    } else {
        0
    }
}
struct Helper {
    child: Child,
    pid: u32,
}
impl Drop for Helper {
    fn drop(&mut self) {
        // Child::kill targets this captured process handle/PID, never an image name.
        let killed = self.child.kill().is_ok();
        let waited = self.child.wait().is_ok();
        println!(
            "parts helper pid={} terminated={} reaped={}",
            self.pid, killed, waited
        );
    }
}
fn pump() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
fn wait_parts(ov: &mut Overlay, source: Source) -> bool {
    let until = Instant::now() + Duration::from_secs(4);
    loop {
        pump();
        ov.on_parts();
        if ov.parts.selection().0 == source {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        unsafe {
            MsgWaitForMultipleObjectsEx(None, 20, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        }
    }
}
fn rect(v: &serde_json::Value) -> Rect {
    let n = |i| v[i].as_i64().unwrap_or_default() as i32;
    Rect::new(n(0), n(1), n(2), n(3))
}
fn click(ov: &mut Overlay, p: Point, shift: bool) -> Outcome {
    ov.on_mouse_mods(0, WM_MOUSEMOVE, p.x, p.y, shift, false);
    ov.on_mouse_mods(0, WM_LBUTTONDOWN, p.x, p.y, shift, false);
    ov.on_mouse_mods(0, WM_LBUTTONUP, p.x, p.y, shift, false)
}
fn center(r: Rect) -> Point {
    Point::new((r.left + r.right) / 2, (r.top + r.bottom) / 2)
}
pub fn run() -> i32 {
    if !hidden() {
        println!("REFUSED: --selftest-parts requires a hidden desktop (not the visible one)");
        return 4;
    }
    let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    if !initialized {
        println!("parts selftest: 0 passed, 1 failed (COM MTA)");
        return 1;
    }
    let mut passed = 0;
    let mut failed = 0;
    let mut check = |name: &str, ok: bool| {
        if ok {
            passed += 1;
            println!("PASS {name}");
        } else {
            failed += 1;
            println!("FAIL {name}");
        }
    };
    let result = (|| -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut child = Command::new(exe)
            .arg("--selftest-parts-target")
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let output = child.stdout.take().ok_or("helper stdout missing")?;
        let pid = child.id();
        let _helper = Helper { child, pid };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::BufReader::new(output).read_line(&mut line);
            let _ = tx.send(line);
        });
        let line = rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        let ready: serde_json::Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        let root = ready["root"].as_i64().ok_or("root missing")? as isize;
        let edgehwnd = ready["edges"].as_i64().ok_or("edges missing")? as isize;
        let root_rect = rect(&ready["root_rect"]);
        let outer = rect(&ready["outer"]);
        let left = rect(&ready["left"]);
        let right = rect(&ready["right"]);
        let edge_rect = rect(&ready["edges_rect"]);
        check(
            "helper-ready",
            ready["pid"].as_u64() == Some(u64::from(pid))
                && root_rect == Rect::new(40, 140, 700, 640)
                && left == Rect::new(80, 180, 320, 560),
        );
        let automation = crate::partworker::Automation::new().map_err(|e| e.to_string())?;
        let ql = automation.query(root, center(left), root_rect, || false);
        let qr = automation.query(root, center(right), root_rect, || false);
        check(
            "uia-left-known-chain",
            ql.levels == vec![root_rect, outer, left],
        );
        check(
            "uia-right-known-chain",
            qr.levels == vec![root_rect, outer, right],
        );
        check(
            "edges-helper-no-children",
            unsafe { GetWindow(HWND(edgehwnd as *mut _), GW_CHILD) }.is_err(),
        );
        let image = crate::looktest::print_window(HWND(edgehwnd as *mut _), 400, 500)
            .ok_or("PrintWindow edges failed")?;
        let split = crate::edges::split(&image.px, 1600, edge_rect);
        let ep = Point::new(edge_rect.left + 50, edge_rect.top + 250);
        let chain = split.root.chain(ep);
        check(
            "edges-divider-two-panes",
            split.separators == 1
                && split.root.children.len() == 2
                && chain.len() == 2
                && (chain[1].right - (edge_rect.left + 200)).abs() <= 1,
        );
        let expected_edge = chain
            .get(1)
            .copied()
            .ok_or("edges helper produced no pane")?;
        let bounds = Rect::new(0, 0, 1200, 720);
        let mut pixels = vec![255; 1200 * 720 * 4];
        for y in 0..500 {
            for x in 0..400 {
                let dst = ((y + 140) * 1200 + x + 760) * 4;
                let src = (y * 400 + x) * 4;
                pixels[dst..dst + 4].copy_from_slice(&image.px[src..src + 4]);
            }
        }
        let snap = Snapshot {
            monitors: vec![MonitorInfo {
                hmon: HMONITOR::default(),
                rect: bounds,
                work: bounds,
                name: "parts-test".into(),
                dpi: 96,
                primary: true,
            }],
            layout: Layout::new(vec![bounds]),
            frames: vec![Frame {
                width: 1200,
                height: 720,
                pixels: Pixels::Cpu(pixels),
                method: Method::Gdi,
                ms: 0.,
                zero_present: false,
                note: String::new(),
            }],
            gpu: capture::create_gpu().ok(),
            windows: vec![
                WindowInfo {
                    hwnd: root,
                    rect: root_rect,
                },
                WindowInfo {
                    hwnd: edgehwnd,
                    rect: edge_rect,
                },
            ],
            gpu_ms: 0.,
        };
        let inst: HINSTANCE = unsafe { GetModuleHandleW(None) }
            .map_err(|e| e.to_string())?
            .into();
        crate::app::register_classes(inst, crate::app::MAIN_CLASS_TEST)
            .map_err(|e| e.to_string())?;
        let mut ov = Overlay::new(snap, inst, &Opts::plain(0.45)).map_err(|e| e.to_string())?;
        ov.synthetic_input = true;
        ov.no_throttle = true;
        check(
            "overlay-lazy-before-entry",
            !ov.parts.has_worker() && !ov.parts.mode.active(),
        );
        let p = center(left);
        ov.on_mouse_mods(0, WM_MOUSEMOVE, p.x, p.y, false, false);
        check("hover-does-not-start-uia", !ov.parts.has_worker());
        ov.on_key_mods(0x12, false, false);
        ov.on_alt_mods(false, false);
        check("alt-tap-latches", ov.parts.mode.active());
        check("overlay-worker-uia", wait_parts(&mut ov, Source::Uia));
        check(
            "default-innermost",
            ov.current() == Some((left, Kind::Part)),
        );
        ov.on_wheel(120);
        check("wheel-up-outer", ov.current() == Some((outer, Kind::Part)));
        ov.on_wheel(-120);
        check("wheel-down-inner", ov.current() == Some((left, Kind::Part)));
        check(
            "click-part",
            click(&mut ov, p, false) == Outcome::Commit(left, Kind::Part),
        );
        let metadata = ov.parts_metadata(Kind::Part);
        check(
            "part-log-fields",
            metadata.contains("source=uia")
                && metadata.contains("level=2")
                && metadata.contains("uia_ms="),
        );
        check(
            "shift-add-first-part",
            click(&mut ov, p, true) == Outcome::Nothing && ov.frame_count() == 1,
        );
        let p2 = center(right);
        ov.on_mouse_mods(0, WM_MOUSEMOVE, p2.x, p2.y, true, false);
        check(
            "same-window-keeps-depth",
            wait_parts(&mut ov, Source::Uia) && ov.current() == Some((right, Kind::Part)),
        );
        check(
            "shift-add-second-part",
            click(&mut ov, p2, true) == Outcome::Nothing && ov.frame_count() == 2,
        );
        check(
            "enter-frames-union",
            ov.on_key_mods(0x0d, false, false) == Outcome::Commit(left.union(&right), Kind::Frames),
        );
        check(
            "shift-toggle-exact-part",
            click(&mut ov, p2, true) == Outcome::Nothing && ov.frame_count() == 1,
        );
        while ov.frame_count() > 0 {
            ov.on_key_mods(0x08, false, false);
        }
        ov.on_key_mods(0x12, false, false);
        ov.on_alt_mods(false, false);
        check("alt-tap-turns-off", !ov.parts.mode.active());
        ov.on_mouse_mods(0, WM_LBUTTONDOWN, p.x, p.y, false, false);
        ov.on_key_mods(0x12, false, false);
        ov.on_alt_mods(false, false);
        ov.on_mouse_mods(0, WM_LBUTTONUP, p.x, p.y, false, false);
        check("alt-during-drag-does-not-toggle", !ov.parts.mode.active());
        ov.on_mouse_mods(0, WM_MOUSEMOVE, ep.x, ep.y, false, false);
        ov.on_wheel(-120);
        check("wheel-enters-parts", ov.parts.mode.active());
        check(
            "edges-selected-after-useless-uia",
            wait_parts(&mut ov, Source::Edges),
        );
        check(
            "edges-part-known-pane",
            ov.current() == Some((expected_edge, Kind::Part))
                && ov.parts_metadata(Kind::Part).contains("edges_ms="),
        );
        ov.on_wheel(120);
        ov.on_wheel(120);
        check("wheel-up-exits-at-zero", !ov.parts.mode.active());
        ov.on_key_mods(0x12, false, false);
        check("alt-held-active", ov.parts.mode.active());
        check(
            "held-alt-click-part",
            wait_parts(&mut ov, Source::Edges)
                && click(&mut ov, ep, false) == Outcome::Commit(expected_edge, Kind::Part),
        );
        ov.on_alt_mods(false, false);
        check("used-alt-release-does-not-latch", !ov.parts.mode.active());
        ov.on_key_mods(0x12, true, false);
        ov.on_alt_mods(false, true);
        check("altgr-does-not-enter", !ov.parts.mode.active());
        ov.on_key_mods(0x12, false, false);
        ov.on_mouse_mods(0, WM_MBUTTONDOWN, ep.x, ep.y, false, false);
        ov.on_mouse_mods(0, WM_MBUTTONUP, ep.x, ep.y, false, false);
        ov.on_alt_mods(false, false);
        check("middle-click-interrupts-alt-tap", !ov.parts.mode.active());
        drop(ov);
        drop(automation);
        Ok(())
    })();
    if let Err(e) = result {
        check("setup-or-runtime", false);
        println!("FAIL detail: {e}");
    }
    unsafe {
        CoUninitialize();
    }
    println!("parts selftest: {passed} passed, {failed} failed");
    if failed == 0 && passed > 0 { 0 } else { 1 }
}
