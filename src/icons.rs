//! The one icon set: simple line icons on a 20 x 20 grid, 1.5 units stroke, round caps and joins.
//!
//! The same data draws the icons in the app (through `raster`) and writes `docs/design/icons.svg`
//! for the mockups, so the design and the program cannot drift apart: a test compares the file to
//! `svg_sprite()`. Regenerate it with `SNIP_WRITE_DESIGN=1 cargo test icons`.
//!
//! Only lines, circles, arcs and a few filled polygons: no gradients, no shading, no emoji, no
//! stock icon pack.

use crate::raster::{Canvas, Mask, Pt, Rgba};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Region,
    Window,
    Screen,
    Arrow,
    Rect,
    Line,
    Pen,
    Marker,
    Text,
    Steps,
    Obscure,
    Blur,
    Color,
    Size,
    Undo,
    Redo,
    Copy,
    Save,
    Pin,
    Close,
    History,
    Settings,
    Timer,
    Folder,
    Trash,
    Check,
    /// Text capture: selection corners around a T.
    TextCopy,
}

pub const ALL: [Icon; 27] = [
    Icon::Region,
    Icon::Window,
    Icon::Screen,
    Icon::Arrow,
    Icon::Rect,
    Icon::Line,
    Icon::Pen,
    Icon::Marker,
    Icon::Text,
    Icon::Steps,
    Icon::Obscure,
    Icon::Blur,
    Icon::Color,
    Icon::Size,
    Icon::Undo,
    Icon::Redo,
    Icon::Copy,
    Icon::Save,
    Icon::Pin,
    Icon::Close,
    Icon::History,
    Icon::Settings,
    Icon::Timer,
    Icon::Folder,
    Icon::Trash,
    Icon::Check,
    Icon::TextCopy,
];

/// One drawing step in grid units. `w` multiplies the 1.5 unit stroke.
#[derive(Clone, Debug, PartialEq)]
pub enum Prim {
    Poly { pts: Vec<(f32, f32)>, closed: bool, w: f32 },
    Circle { c: (f32, f32), r: f32 },
    /// Clockwise on screen from `a0` to `a1` degrees (0 = right, 90 = down).
    Arc { c: (f32, f32), r: f32, a0: f32, a1: f32 },
    Fill { pts: Vec<(f32, f32)> },
    Dot { c: (f32, f32), r: f32 },
}

pub const GRID: f32 = crate::theme::ICON_GRID;
pub const STROKE: f32 = crate::theme::ICON_STROKE;

fn poly(pts: &[(f32, f32)]) -> Prim {
    Prim::Poly { pts: pts.to_vec(), closed: false, w: 1.0 }
}

fn closed(pts: &[(f32, f32)]) -> Prim {
    Prim::Poly { pts: pts.to_vec(), closed: true, w: 1.0 }
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Prim {
    closed(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
}

fn fill_rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Prim {
    Prim::Fill { pts: vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)] }
}

impl Icon {
    pub fn id(self) -> &'static str {
        match self {
            Icon::Region => "region",
            Icon::Window => "window",
            Icon::Screen => "screen",
            Icon::Arrow => "arrow",
            Icon::Rect => "rect",
            Icon::Line => "line",
            Icon::Pen => "pen",
            Icon::Marker => "marker",
            Icon::Text => "text",
            Icon::Steps => "steps",
            Icon::Obscure => "obscure",
            Icon::Blur => "blur",
            Icon::Color => "color",
            Icon::Size => "size",
            Icon::Undo => "undo",
            Icon::Redo => "redo",
            Icon::Copy => "copy",
            Icon::Save => "save",
            Icon::Pin => "pin",
            Icon::Close => "close",
            Icon::History => "history",
            Icon::Settings => "settings",
            Icon::Timer => "timer",
            Icon::Folder => "folder",
            Icon::Trash => "trash",
            Icon::Check => "check",
            Icon::TextCopy => "text-copy",
        }
    }

    pub fn prims(self) -> Vec<Prim> {
        match self {
            Icon::Region => vec![
                poly(&[(3.0, 7.0), (3.0, 3.0), (7.0, 3.0)]),
                poly(&[(13.0, 3.0), (17.0, 3.0), (17.0, 7.0)]),
                poly(&[(17.0, 13.0), (17.0, 17.0), (13.0, 17.0)]),
                poly(&[(7.0, 17.0), (3.0, 17.0), (3.0, 13.0)]),
            ],
            Icon::Window => vec![rect(3.0, 4.0, 17.0, 16.0), poly(&[(3.0, 7.5), (17.0, 7.5)])],
            Icon::Screen => vec![rect(2.5, 3.5, 17.5, 13.5), poly(&[(7.0, 16.5), (13.0, 16.5)]), poly(&[(10.0, 13.5), (10.0, 16.5)])],
            Icon::Arrow => vec![poly(&[(4.0, 16.0), (15.0, 5.0)]), poly(&[(8.5, 5.0), (15.0, 5.0), (15.0, 11.5)])],
            Icon::Rect => vec![rect(3.5, 5.0, 16.5, 15.0)],
            Icon::Line => vec![poly(&[(4.0, 16.0), (16.0, 4.0)])],
            Icon::Pen => vec![
                closed(&[(4.0, 16.0), (5.0, 12.5), (13.0, 4.5), (15.5, 7.0), (7.5, 15.0)]),
                poly(&[(11.0, 6.5), (13.5, 9.0)]),
            ],
            Icon::Marker => vec![
                closed(&[(7.5, 12.5), (13.0, 4.0), (16.5, 6.5), (11.0, 15.0)]),
                poly(&[(7.5, 12.5), (6.5, 15.5), (11.0, 15.0)]),
                poly(&[(3.0, 17.5), (17.0, 17.5)]),
            ],
            Icon::Text => vec![poly(&[(4.5, 6.0), (4.5, 4.0), (15.5, 4.0), (15.5, 6.0)]), poly(&[(10.0, 4.0), (10.0, 16.0)]), poly(&[(7.5, 16.0), (12.5, 16.0)])],
            Icon::Steps => vec![Prim::Circle { c: (10.0, 10.0), r: 7.0 }, poly(&[(8.25, 7.75), (10.5, 6.25), (10.5, 13.75)])],
            Icon::Obscure => vec![
                rect(3.0, 3.0, 17.0, 17.0),
                fill_rect(5.5, 5.5, 8.5, 8.5),
                fill_rect(8.5, 8.5, 11.5, 11.5),
                fill_rect(11.5, 5.5, 14.5, 8.5),
                fill_rect(5.5, 11.5, 8.5, 14.5),
                fill_rect(11.5, 11.5, 14.5, 14.5),
            ],
            Icon::Blur => vec![
                Prim::Circle { c: (10.0, 10.0), r: 7.0 },
                Prim::Dot { c: (10.0, 10.0), r: 1.6 },
                Prim::Dot { c: (6.5, 10.0), r: 1.1 },
                Prim::Dot { c: (13.5, 10.0), r: 1.1 },
                Prim::Dot { c: (10.0, 6.5), r: 1.1 },
                Prim::Dot { c: (10.0, 13.5), r: 1.1 },
            ],
            Icon::Color => vec![Prim::Circle { c: (10.0, 10.0), r: 6.5 }],
            Icon::Size => vec![
                Prim::Poly { pts: vec![(4.0, 5.5), (16.0, 5.5)], closed: false, w: 0.8 },
                Prim::Poly { pts: vec![(4.0, 10.0), (16.0, 10.0)], closed: false, w: 1.5 },
                Prim::Poly { pts: vec![(4.0, 15.0), (16.0, 15.0)], closed: false, w: 2.4 },
            ],
            Icon::Undo => vec![
                poly(&[(4.0, 8.0), (12.0, 8.0)]),
                Prim::Arc { c: (12.0, 12.0), r: 4.0, a0: -90.0, a1: 90.0 },
                poly(&[(12.0, 16.0), (7.0, 16.0)]),
                poly(&[(7.0, 5.0), (4.0, 8.0), (7.0, 11.0)]),
            ],
            Icon::Redo => vec![
                poly(&[(16.0, 8.0), (8.0, 8.0)]),
                Prim::Arc { c: (8.0, 12.0), r: 4.0, a0: 90.0, a1: 270.0 },
                poly(&[(8.0, 16.0), (13.0, 16.0)]),
                poly(&[(13.0, 5.0), (16.0, 8.0), (13.0, 11.0)]),
            ],
            Icon::Copy => vec![rect(7.0, 7.0, 16.5, 16.5), poly(&[(3.5, 13.0), (3.5, 3.5), (13.0, 3.5)])],
            Icon::Save => vec![
                poly(&[(10.0, 3.5), (10.0, 12.5)]),
                poly(&[(6.5, 9.0), (10.0, 12.5), (13.5, 9.0)]),
                poly(&[(3.5, 13.0), (3.5, 16.5), (16.5, 16.5), (16.5, 13.0)]),
            ],
            Icon::Pin => vec![
                closed(&[(7.0, 3.5), (13.0, 3.5), (12.0, 8.5), (14.5, 11.5), (5.5, 11.5), (8.0, 8.5)]),
                poly(&[(10.0, 11.5), (10.0, 17.0)]),
            ],
            Icon::Close => vec![poly(&[(5.0, 5.0), (15.0, 15.0)]), poly(&[(15.0, 5.0), (5.0, 15.0)])],
            Icon::History => vec![Prim::Circle { c: (10.0, 10.0), r: 7.0 }, poly(&[(10.0, 6.0), (10.0, 10.0), (13.0, 12.0)])],
            Icon::Settings => vec![
                poly(&[(3.5, 6.0), (5.0, 6.0)]),
                poly(&[(9.0, 6.0), (16.5, 6.0)]),
                Prim::Circle { c: (7.0, 6.0), r: 2.0 },
                poly(&[(3.5, 14.0), (11.0, 14.0)]),
                poly(&[(15.0, 14.0), (16.5, 14.0)]),
                Prim::Circle { c: (13.0, 14.0), r: 2.0 },
            ],
            Icon::Timer => vec![
                Prim::Circle { c: (10.0, 11.0), r: 6.0 },
                poly(&[(10.0, 11.0), (10.0, 7.5)]),
                poly(&[(8.5, 3.0), (11.5, 3.0)]),
                poly(&[(10.0, 3.0), (10.0, 5.0)]),
            ],
            Icon::Folder => vec![closed(&[(3.0, 5.0), (8.0, 5.0), (9.5, 7.0), (17.0, 7.0), (17.0, 15.5), (3.0, 15.5)])],
            Icon::Trash => vec![
                poly(&[(3.5, 6.0), (16.5, 6.0)]),
                poly(&[(8.0, 6.0), (8.0, 4.0), (12.0, 4.0), (12.0, 6.0)]),
                poly(&[(5.0, 6.0), (6.0, 16.5), (14.0, 16.5), (15.0, 6.0)]),
            ],
            Icon::Check => vec![poly(&[(4.5, 10.5), (8.5, 14.5), (15.5, 6.0)])],
            Icon::TextCopy => vec![
                poly(&[(3.0, 6.5), (3.0, 3.0), (6.5, 3.0)]),
                poly(&[(13.5, 3.0), (17.0, 3.0), (17.0, 6.5)]),
                poly(&[(17.0, 13.5), (17.0, 17.0), (13.5, 17.0)]),
                poly(&[(6.5, 17.0), (3.0, 17.0), (3.0, 13.5)]),
                poly(&[(6.5, 7.0), (13.5, 7.0)]),
                poly(&[(10.0, 7.0), (10.0, 13.5)]),
            ],
        }
    }
}

fn arc_points(c: (f32, f32), r: f32, a0: f32, a1: f32) -> Vec<(f32, f32)> {
    let n = (((a1 - a0).abs() / 6.0).ceil() as usize).max(2);
    (0..=n)
        .map(|i| {
            let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
            (c.0 + r * a.cos(), c.1 + r * a.sin())
        })
        .collect()
}

/// Draw `icon` into a `size` x `size` pixel box with its top-left at (`x`, `y`).
pub fn draw(cv: &mut Canvas, icon: Icon, x: f32, y: f32, size: f32, color: Rgba) {
    let k = size / GRID;
    let p = |q: (f32, f32)| Pt::new(x + q.0 * k, y + q.1 * k);
    let mut m = Mask::new(crate::raster::IBox::around(x, y, x + size, y + size, 2.0));
    let sw = (STROKE * k).max(1.0);
    for prim in icon.prims() {
        match prim {
            Prim::Poly { pts, closed, w } => {
                let mut v: Vec<Pt> = pts.iter().map(|&q| p(q)).collect();
                if closed && let Some(&first) = v.first() {
                    v.push(first);
                }
                m.polyline(&v, sw * w);
            }
            Prim::Circle { c, r } => m.ring(p(c), r * k, sw),
            Prim::Arc { c, r, a0, a1 } => {
                let v: Vec<Pt> = arc_points(c, r, a0, a1).into_iter().map(p).collect();
                m.polyline(&v, sw);
            }
            Prim::Fill { pts } => m.convex(&pts.iter().map(|&q| p(q)).collect::<Vec<_>>()),
            Prim::Dot { c, r } => m.disc(p(c), r * k),
        }
    }
    cv.blend_mask(&m, color);
}

fn n(v: f32) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

fn pts_attr(pts: &[(f32, f32)]) -> String {
    pts.iter().map(|(x, y)| format!("{},{}", n(*x), n(*y))).collect::<Vec<_>>().join(" ")
}

/// One `<symbol>` (stroke and fill follow `currentColor`).
pub fn svg_symbol(icon: Icon) -> String {
    let mut s = format!("  <symbol id=\"i-{}\" viewBox=\"0 0 20 20\">\n", icon.id());
    for prim in icon.prims() {
        let line = match prim {
            Prim::Poly { pts, closed, w } => {
                let tag = if closed { "polygon" } else { "polyline" };
                let sw = if (w - 1.0).abs() > 1e-3 { format!(" stroke-width=\"{}\"", n(STROKE * w)) } else { String::new() };
                format!("    <{tag} points=\"{}\"{sw}/>\n", pts_attr(&pts))
            }
            Prim::Circle { c, r } => format!("    <circle cx=\"{}\" cy=\"{}\" r=\"{}\"/>\n", n(c.0), n(c.1), n(r)),
            Prim::Arc { c, r, a0, a1 } => {
                let (s0, s1) = (a0.to_radians(), a1.to_radians());
                let large = if (a1 - a0).abs() > 180.0 { 1 } else { 0 };
                format!(
                    "    <path d=\"M{},{} A{},{} 0 {} 1 {},{}\"/>\n",
                    n(c.0 + r * s0.cos()),
                    n(c.1 + r * s0.sin()),
                    n(r),
                    n(r),
                    large,
                    n(c.0 + r * s1.cos()),
                    n(c.1 + r * s1.sin())
                )
            }
            Prim::Fill { pts } => format!("    <polygon points=\"{}\" fill=\"currentColor\" stroke=\"none\"/>\n", pts_attr(&pts)),
            Prim::Dot { c, r } => format!("    <circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"currentColor\" stroke=\"none\"/>\n", n(c.0), n(c.1), n(r)),
        };
        s.push_str(&line);
    }
    s.push_str("  </symbol>\n");
    s
}

/// The whole set as one SVG sprite (`<use href="#i-arrow"/>`).
pub fn svg_sprite() -> String {
    let mut s = String::from(
        "<!-- Generated by src/icons.rs (svg_sprite). Do not edit by hand: SNIP_WRITE_DESIGN=1 cargo test icons -->\n<svg xmlns=\"http://www.w3.org/2000/svg\" style=\"display:none\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\">\n",
    );
    for i in ALL {
        s.push_str(&svg_symbol(i));
    }
    s.push_str("</svg>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::IBox;

    #[test]
    fn every_icon_draws_inside_its_box() {
        assert!(!ALL.is_empty());
        for i in ALL {
            let mut c = Canvas::new(40, 40, Rgba::rgb(255, 255, 255));
            draw(&mut c, i, 10.0, 10.0, 20.0, Rgba::rgb(0, 0, 0));
            let mut inside = 0;
            for y in 0..40 {
                for x in 0..40 {
                    if c.get(x, y) != Rgba::rgb(255, 255, 255) {
                        assert!(IBox::new(9, 9, 31, 31).contains(x, y), "{} draws outside at {x},{y}", i.id());
                        inside += 1;
                    }
                }
            }
            assert!(inside > 15, "{} drew only {inside} pixels", i.id());
        }
    }

    #[test]
    fn ids_are_unique_and_every_icon_is_listed() {
        let mut ids: Vec<&str> = ALL.iter().map(|i| i.id()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), ALL.len());
    }

    #[test]
    fn grid_coordinates_stay_on_the_grid() {
        for i in ALL {
            for p in i.prims() {
                let pts: Vec<(f32, f32)> = match p {
                    Prim::Poly { pts, .. } | Prim::Fill { pts } => pts,
                    Prim::Circle { c, r } | Prim::Dot { c, r } | Prim::Arc { c, r, .. } => vec![(c.0 - r, c.1 - r), (c.0 + r, c.1 + r)],
                };
                for (x, y) in pts {
                    assert!((1.0..=19.0).contains(&x) && (1.0..=19.0).contains(&y), "{} point {x},{y}", i.id());
                }
            }
        }
    }

    #[test]
    fn design_sprite_matches_the_code() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/design/icons.svg");
        let want = svg_sprite();
        if std::env::var_os("SNIP_WRITE_DESIGN").is_some() {
            std::fs::write(&path, &want).unwrap();
        }
        let got = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
        assert_eq!(got, want, "docs/design/icons.svg is stale: run SNIP_WRITE_DESIGN=1 cargo test icons");
    }
}
