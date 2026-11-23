//! Read-only `snip parts --at X,Y`: one JSON object, geometry and timings only.
use crate::geometry::{Point, Rect};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

pub(crate) fn rect_json(r: Rect) -> Value {
    json!([r.left, r.top, r.right, r.bottom])
}
pub(crate) fn rects_json(rs: &[Rect]) -> Value {
    Value::Array(rs.iter().copied().map(rect_json).collect())
}
fn parse(args: &[String]) -> Result<(Point, Option<PathBuf>), String> {
    let mut point = None;
    let mut log = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--at" if point.is_none() => {
                i += 1;
                let (x, y) = args
                    .get(i)
                    .and_then(|s| s.split_once(','))
                    .ok_or("--at requires X,Y")?;
                point = Some(Point::new(
                    x.parse().map_err(|_| "invalid X")?,
                    y.parse().map_err(|_| "invalid Y")?,
                ));
            }
            "--log" if log.is_none() => {
                i += 1;
                log = Some(PathBuf::from(args.get(i).ok_or("--log requires a file")?));
            }
            _ => return Err("usage: snip parts --at X,Y [--log FILE]".into()),
        }
        i += 1;
    }
    Ok((point.ok_or("--at is required")?, log))
}
fn probe(point: Point) -> (Value, i32) {
    let windows = crate::capture::windows_in_z_order();
    let Some(window) = windows.iter().find(|w| w.rect.contains(point)) else {
        return (json!({"source":"none","window":null}), 3);
    };
    let mut class = [0u16; 256];
    let n = unsafe { GetClassNameW(HWND(window.hwnd as *mut _), &mut class) } as usize;
    let class = String::from_utf16_lossy(&class[..n]);
    let automation = match crate::partworker::Automation::new() {
        Ok(a) => a,
        Err(e) => {
            return (
                json!({"error":"UI Automation initialization failed","hresult":e.code().0}),
                1,
            );
        }
    };
    let mut query = automation.query(window.hwnd, point, window.rect, || false);
    let start = Instant::now();
    let first = query.ms;
    let mut retries = Vec::new();
    let mut nodes = query.nodes_seen;
    for deadline in [300, 1000] {
        if query.levels.len() > 1 {
            break;
        }
        // Timer wait on this diagnostic MTA only; no input, focus, overlay or clipboard.
        let (_tx, rx) = std::sync::mpsc::channel::<()>();
        let _ = rx.recv_timeout(Duration::from_millis(deadline).saturating_sub(start.elapsed()));
        query = automation.query(window.hwnd, point, window.rect, || false);
        retries.push(query.ms);
        nodes += query.nodes_seen;
    }
    let capture = crate::capture::parts_monitor(point).and_then(|snap| {
        let rect = window
            .rect
            .intersect(&snap.monitors[0].rect)
            .unwrap_or_default();
        snap.crop(&rect).map(|(_, _, px)| (rect, px))
    });
    let t = Instant::now();
    let (edge, edge_error) = match capture {
        Ok((rect, px)) => (
            crate::edges::split(&px, rect.width() as usize * 4, rect),
            None,
        ),
        Err(e) => (crate::edges::split(&[], 0, window.rect), Some(e.code().0)),
    };
    let ms = t.elapsed().as_secs_f64() * 1000.;
    let edges = crate::parts::levels(window.rect, &edge.root.chain(point));
    let source = crate::parts::choose(Some(&query.levels), &edges, 400);
    let mut value = json!({
        "window":{"class":class,"rect":rect_json(window.rect)},
        "uia":{"levels":rects_json(&query.levels),"ms_first":first,"ms_retries":retries,"nodes_seen":nodes},
        "edges":{"levels":rects_json(&edges),"ms":ms,"separators":edge.separators},
        "source":source.name(),
    });
    if let Some(e) = edge_error {
        value["edges"]["capture_error"] = json!(e);
    }
    // A useful UIA answer remains available even when DD has no frame on a still screen.
    let exit = if edge_error.is_some() && source == crate::parts::Source::None {
        1
    } else {
        0
    };
    (value, exit)
}
pub fn run(args: &[String]) -> i32 {
    let (point, log) = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            println!("{}", json!({"error":e}));
            return 1;
        }
    };
    let (value, code) = std::thread::spawn(move || {
        let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if init.is_err() {
            return (
                json!({"error":"COM MTA initialization failed","hresult":init.0}),
                1,
            );
        }
        let result = probe(point);
        unsafe {
            CoUninitialize();
        }
        result
    })
    .join()
    .unwrap_or((json!({"error":"parts worker failed"}), 1));
    let text = value.to_string();
    println!("{text}");
    if let Some(log) = log
        && std::fs::write(log, format!("{text}\n")).is_err()
    {
        return 1;
    }
    code
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn negative_physical_coordinates_and_strict_read_only_arguments() {
        assert_eq!(
            parse(&["--at".into(), "-123,456".into()]).unwrap(),
            (Point::new(-123, 456), None)
        );
        for args in [
            vec![],
            vec!["--at", "1"],
            vec!["--at", "1,2,3"],
            vec!["--save-images"],
            vec!["--log"],
        ] {
            assert!(parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).is_err());
        }
    }
    #[test]
    fn geometry_json_keeps_negative_edges() {
        assert_eq!(
            rect_json(Rect::new(-30, -20, 10, 15)),
            json!([-30, -20, 10, 15])
        );
    }
}
