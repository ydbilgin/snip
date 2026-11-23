//! Timing of the core's heavy paths, for docs/CLOUD-RESULT-V2.md. Ignored by default (timings
//! depend on the machine): `cargo test --release --test budgets -- --ignored --nocapture`.

use std::time::Instant;

use snip::dpi::{Monitor, Monitors};
use snip::editor::{Editor, Prefs};
use snip::geometry::{Point, Rect};
use snip::raster::{BoxFont, Canvas, Rgba};
use snip::theme::{Look, Mode, Theme};

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn drag(e: &mut Editor, a: (i32, i32), b: (i32, i32)) {
    e.on_down(Point::new(a.0, a.1), false);
    for k in 1..=20 {
        let p = Point::new(a.0 + (b.0 - a.0) * k / 20, a.1 + (b.1 - a.1) * k / 20);
        e.on_move(p, false);
    }
    e.on_up(Point::new(b.0, b.1), false);
}

#[test]
#[ignore]
fn budgets() {
    let mons = Monitors::new(vec![Monitor::new(Rect::from_xywh(0, 0, 2560, 1440), 96)]);
    let theme = Theme::new(Look::Graphite, Mode::Light);
    let mut e = Editor::new(mons, vec![], theme, &Prefs::default(), Point::new(0, 0));
    drag(&mut e, (100, 100), (2019, 1179)); // 1920 x 1080
    e.tool = None; // the workload below explicitly selects each tool, including default Arrow
    for (k, a, b) in [(b'A', (200, 200), (900, 600)), (b'R', (1000, 200), (1600, 700)), (b'P', (200, 800), (900, 1000)), (b'H', (1000, 900), (1800, 900)), (b'B', (1200, 300), (1500, 500))] {
        e.on_key(k as u32, false, false);
        drag(&mut e, a, b);
    }
    assert_eq!(e.doc.shapes().len(), 5, "keep the measured workload intact");
    let base = Canvas::new(1920, 1080, Rgba::rgb(240, 240, 240));
    let n = 10;
    let t = Instant::now();
    for _ in 0..n {
        std::hint::black_box(e.compose_doc(&base, &BoxFont));
    }
    let doc_ms = ms(t) / n as f64;
    let cached = e.compose_doc(&base, &BoxFont);
    // One preview frame while drawing: copy the cached image, add the shape being dragged.
    e.on_key(b'A' as u32, false, false);
    e.on_down(Point::new(300, 300), false);
    e.on_move(Point::new(800, 700), false);
    let t = Instant::now();
    for _ in 0..n {
        let mut img = cached.clone();
        e.add_preview(&mut img, &BoxFont);
        std::hint::black_box(img);
    }
    let frame_ms = ms(t) / n as f64;
    let t = Instant::now();
    let v = e.view(&BoxFont);
    let ui_ms = ms(t);
    let t = Instant::now();
    let th = snip::thumb::make(&base);
    let thumb_ms = ms(t);
    let big = Canvas::new(2560, 1440, Rgba::rgb(1, 2, 3));
    let t = Instant::now();
    let _ = big.premultiplied();
    let pm_ms = ms(t);
    println!("compose_doc 1920x1080 with 5 shapes (incl. pixelate): {doc_ms:.1} ms");
    println!("preview frame while dragging an arrow (clone + 1 shape): {frame_ms:.1} ms");
    println!("toolbar + label + tooltip canvases: {ui_ms:.2} ms (bar {}x{})", v.bar.as_ref().map(|b| b.1.w).unwrap_or(0), v.bar.as_ref().map(|b| b.1.h).unwrap_or(0));
    println!("thumbnail 1920x1080 -> {}x{}: {thumb_ms:.1} ms, {} bytes", th.w, th.h, snip::thumb::encode(&th).len());
    println!("premultiply 2560x1440: {pm_ms:.1} ms");
}
