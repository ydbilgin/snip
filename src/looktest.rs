//! `snip --selftest-look DIR SCENE [--dpi N]`: the overlay's real drawing path on a synthetic frame,
//! read back to PNG files so a person can look at it. Hidden desktop only, like --selftest-overlay.
//! SCENE: u32 LE width, u32 LE height, then BGRA, top-down. No screen, clipboard or keys are touched.

use std::path::Path;

use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::CoInitializeEx;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::capture::{self, Frame, Method, MonitorInfo, Pixels, Snapshot};
use crate::editor::Prefs;
use crate::geometry::{Layout, Rect};
use crate::i18n::Lang;
use crate::overlay::{Opts, Overlay, window_rect};
use crate::raster::Canvas;
use crate::theme::{Look, Mode, Theme};
use crate::toolbar::{Item, Tool};

/// The language of the rendered views: Turkish unless `--lang en` is given.
static LANG: std::sync::OnceLock<Lang> = std::sync::OnceLock::new();

fn lang() -> Lang {
    *LANG.get().unwrap_or(&Lang::Tr)
}

/// The Turkish or the English sample text, by `lang()`.
fn pick_text(tr: &'static str, en: &'static str) -> &'static str {
    if lang() == Lang::En { en } else { tr }
}

/// A scene file as a one-monitor snapshot (the session's text self-test).
pub fn scene_snapshot(path: &Path) -> Result<Snapshot, String> {
    let raw = std::fs::read(path).map_err(|e| e.to_string())?;
    let (w, h, px) = parse_scene(&raw)?;
    snapshot(px, w, h, 96)
}

fn parse_scene(raw: &[u8]) -> Result<(u32, u32, &[u8]), String> {
    if raw.len() < 8 {
        return Err("scene too short".into());
    }
    let w = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    let h = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
    if raw.len() != 8 + (w * h * 4) as usize {
        return Err("scene size does not match its header".into());
    }
    Ok((w, h, &raw[8..]))
}

fn snapshot(scene: &[u8], w: u32, h: u32, dpi: u32) -> Result<Snapshot, String> {
    let bounds = Rect::new(0, 0, w as i32, h as i32);
    let gpu = capture::create_gpu().map_err(|e| e.to_string())?;
    Ok(Snapshot {
        monitors: vec![MonitorInfo { hmon: HMONITOR::default(), rect: bounds, work: bounds, name: "look".into(), dpi, primary: true }],
        layout: Layout::new(vec![bounds]),
        frames: vec![Frame { width: w, height: h, pixels: Pixels::Cpu(scene.to_vec()), method: Method::Gdi, ms: 0.0, zero_present: false, note: "synthetic".into() }],
        gpu: Some(gpu),
        windows: vec![],
        gpu_ms: 0.0,
    })
}

/// A window of ours as pixels: PrintWindow, full content.
pub(crate) fn print_window(hwnd: windows::Win32::Foundation::HWND, w: i32, h: i32) -> Option<Canvas> {
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bm = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bm.into());
        let ok = windows::Win32::Storage::Xps::PrintWindow(hwnd, mem, windows::Win32::Storage::Xps::PRINT_WINDOW_FLAGS(2)).as_bool();
        let mut px = vec![0u8; (w * h * 4) as usize];
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
            ..Default::default()
        };
        GetDIBits(mem, bm, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
        SelectObject(mem, old);
        let _ = DeleteObject(bm.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        for p in px.as_chunks_mut::<4>().0 {
            p[3] = 255;
        }
        ok.then(|| Canvas::from_bgra(w as u32, h as u32, px))
    }
}

fn save(ov: &Overlay, dir: &Path, name: &str) -> Result<(), String> {
    let mut cv = ov.read_back(0).map_err(|e| format!("{name}: {e}"))?;
    if let Some((_, host)) = ov.text_windows() {
        let r = window_rect(host);
        unsafe {
            let _ = ShowWindow(host, SW_SHOWNOACTIVATE);
            let _ = windows::Win32::Graphics::Gdi::UpdateWindow(host);
        }
        std::thread::sleep(std::time::Duration::from_millis(60));
        pump();
        if let Some(t) = print_window(host, r.width(), r.height()) {
            cv.draw(&t, r.left, r.top);
        }
    }
    let png = crate::png::encode_png(cv.w, cv.h, &cv.px).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{name}.png")), png).map_err(|e| e.to_string())?;
    println!("wrote {name}.png {}x{}", cv.w, cv.h);
    Ok(())
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

fn drag(ov: &mut Overlay, a: (i32, i32), b: (i32, i32)) {
    ov.on_mouse_mods(0, WM_LBUTTONDOWN, a.0, a.1, false, false);
    ov.on_mouse_mods(0, WM_MOUSEMOVE, (a.0 + b.0) / 2, (a.1 + b.1) / 2, false, false);
    ov.on_mouse_mods(0, WM_MOUSEMOVE, b.0, b.1, false, false);
    ov.on_mouse_mods(0, WM_LBUTTONUP, b.0, b.1, false, false);
}

fn pick(ov: &mut Overlay, t: Tool) {
    if ov.editor.as_ref().is_some_and(|e| e.tool != Some(t)) {
        ov.on_key_mods(t.key() as u32, false, false);
    }
}

fn click_item(ov: &mut Overlay, it: Item) -> Result<(), String> {
    let (x, y) = ov.editor.as_ref().and_then(|e| e.item_center(it)).ok_or("no such button")?;
    ov.on_mouse_mods(0, WM_MOUSEMOVE, x, y, false, false);
    ov.on_mouse_mods(0, WM_LBUTTONDOWN, x, y, false, false);
    ov.on_mouse_mods(0, WM_LBUTTONUP, x, y, false, false);
    Ok(())
}

fn hover_item(ov: &mut Overlay, it: Item) -> Result<(), String> {
    let (x, y) = ov.editor.as_ref().and_then(|e| e.item_center(it)).ok_or("no such button")?;
    ov.on_mouse_mods(0, WM_MOUSEMOVE, x, y, false, false);
    Ok(())
}

/// Draw `top` with its own alpha over `bg`.
fn over(bg: &mut Canvas, top: &Canvas, x: i32, y: i32) {
    for j in 0..top.h as i32 {
        for i in 0..top.w as i32 {
            let c = top.get(i, j);
            if c.a > 0 {
                bg.blend(x + i, y + j, c.with_alpha(255), c.a as f32 / 255.0);
            }
        }
    }
}

/// The windows' states that need a mouse over them, drawn with the real text route.
fn panels(scene: &[u8], w: u32, h: u32, t: Theme, tag: &str, dir: &Path) -> Result<(), String> {
    use crate::config::Config;
    use crate::gdi::GdiGlyphs;
    use crate::historyview::{self, Grid};
    use crate::history::Entry;
    use crate::settingsview::{self, KeyField, Part, Row, ViewState};
    let g = GdiGlyphs::new();
    let write = |cv: &Canvas, name: &str| -> Result<(), String> {
        let png = crate::png::encode_png(cv.w, cv.h, &cv.px).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(format!("{tag}-{name}.png")), png).map_err(|e| e.to_string())
    };
    let full = Canvas::from_bgra(w, h, scene.to_vec());
    let crops = [(120, 70, 1010, 790), (1050, 200, 1540, 780), (160, 120, 960, 300), (0, 0, 1600, 900), (160, 360, 960, 560), (1050, 200, 1400, 420), (180, 560, 900, 790)];
    let times = ["2026-11-07 17:52:12", "2026-11-07 17:45:12", "2026-11-07 09:12:40", "2026-11-06 22:31:12", "2026-11-06 08:24:12", "2026-11-04 17:17:12", "2026-10-30 16:10:12"];
    let thumbs: Vec<Canvas> = crops.iter().map(|c| crate::thumb::make(&full.crop(crate::raster::IBox::new(c.0, c.1, c.2, c.3)))).collect();
    let entries: Vec<Entry> = crops.iter().zip(times).enumerate().map(|(i, (c, tm))| Entry { id: i as u64 + 1, time: tm.into(), path: "x".into(), owned: false, w: (c.2 - c.0) as u32, h: (c.3 - c.1) as u32, source: "editor".into() }).collect();
    let items: Vec<historyview::Item> = entries.iter().zip(&thumbs).enumerate().map(|(i, (e, th))| historyview::Item { entry: e, thumb: Some(th), missing: i == 4 }).collect();
    let mut grid = Grid::new(900, 600, 1.0, items.len());
    grid.folder_hover = true;
    write(&historyview::render(&grid, &t, lang(), &items, Some(1), Some(3), "2026-11-07", &g), "history-hover")?;
    let empty = Grid::new(900, 600, 1.0, 0);
    write(&historyview::render(&empty, &t, lang(), &[], None, None, "2026-11-07", &g), "history-empty")?;
    let c = Config { editor_hotkey: "Ctrl+Shift+S".into(), history: false, ..Config::default() };
    let (lw, lh) = (settingsview::WIDTH as i32, settingsview::height_dip() as i32);
    let l = settingsview::layout(lw, lh, 1.0, lang(), &g, &t, settingsview::TAB_SNIP);
    let base = ViewState { recording: None, hover: None, message: None, folder_text: pick_text("Ekran Görüntüleri (Windows varsayılanı)", "Screenshots (Windows default)"), autostart: false, language_text: pick_text("Otomatik (Windows dili)", "Automatic (Windows language)") };
    let st = ViewState { hover: Some(Part::Toggle(Row::Clipboard)), folder_text: pick_text("D:\\Resimler\\Ekran görüntüleri", "D:\\Pictures\\Screenshots"), ..base };
    write(&settingsview::render(&l, &t, lang(), &c, &st, &g), "settings-hover")?;
    let base = ViewState { recording: Some(KeyField::Editor), hover: None, message: Some((pick_text("Başka bir program kullanıyor", "Another program uses it"), true)), folder_text: pick_text("Ekran Görüntüleri (Windows varsayılanı)", "Screenshots (Windows default)"), autostart: false, language_text: pick_text("Otomatik (Windows dili)", "Automatic (Windows language)") };
    write(&settingsview::render(&l, &t, lang(), &c, &base, &g), "settings-recording")?;
    // The recording control beside the area, on the scene.
    let ctl = crate::record::render_control(83_000, pick_text("Durdur", "Stop"), &t, 1.0, &g);
    let mut bg = full.crop(crate::raster::IBox::new(200, 120, 200 + ctl.w as i32 + 48, 120 + ctl.h as i32 + 48));
    bg.draw(&ctl, 24, 24);
    write(&bg, "record-control")?;
    // Result cards, on the scene, so the corners and the border show on something.
    use crate::textcore::TextOutcome;
    let sample = if lang() == Lang::En { "A security update was installed for company staff.\r\nThe meeting is on Wednesday at 14:30 in the Istanbul office." } else { "Şirket çalışanları için güvenlik güncellemesi yüklendi.\r\nGörüşme Çarşamba günü saat 14:30'da İstanbul ofisinde." };
    for (name, o) in [("card-copied", TextOutcome::Copied(sample.into())), ("card-notext", TextOutcome::NoText), ("card-noengine", TextOutcome::NoEngine), ("card-unchanged", TextOutcome::ClipboardUnchanged), ("card-error", TextOutcome::Error), ("card-busy", TextOutcome::Busy)] {
        let Some(content) = crate::cardview::content(&o, lang()) else { continue };
        let card = crate::cardview::render(&content, &t, 1.0, &g);
        let pad = 24;
        let (bw, bh) = (card.w as i32 + 2 * pad, card.h as i32 + 2 * pad);
        let mut bg = full.crop(crate::raster::IBox::new(200, 120, 200 + bw, 120 + bh));
        over(&mut bg, &card, pad, pad);
        write(&bg, name)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn one_theme(hinst: windows::Win32::Foundation::HINSTANCE, scene: &[u8], w: u32, h: u32, dpi: u32, t: Theme, tag: &str, dir: &Path) -> Result<(), String> {
    let k = dpi as f32 / 96.0;
    let p = |x: i32, y: i32| (x, y);
    {
        let mut opts = Opts::plain(0.45);
        opts.theme = t;
        opts.lang = lang();
        let mut ov = Overlay::new(snapshot(scene, w, h, dpi)?, hinst, &opts).map_err(|e| e.to_string())?;
        ov.no_throttle = true;
        let (a, b) = (p(160, 130), p(960, 480));
        ov.on_mouse_mods(0, WM_LBUTTONDOWN, a.0, a.1, false, false);
        ov.on_mouse_mods(0, WM_MOUSEMOVE, b.0, b.1, false, false);
        save(&ov, dir, &format!("{tag}-plain"))?;
        ov.on_mouse_mods(0, WM_LBUTTONUP, b.0, b.1, false, false);
        let m = (w as f32 / 2.0 - 34.0 * k) as i32;
        ov.on_mouse_mods(0, WM_MOUSEMOVE, m, (36.0 * k) as i32, false, false);
        save(&ov, dir, &format!("{tag}-plain-hover"))?;
    }
    // Plain key hints in both languages, using the constructor's very first UI pass.
    for (hint_lang, name) in [(Lang::Tr, "tr"), (Lang::En, "en")] {
        let mut opts = Opts::plain(0.45);
        opts.theme = t;
        opts.lang = hint_lang;
        let mut ov = Overlay::new(snapshot(scene, w, h, dpi)?, hinst, &opts).map_err(|e| e.to_string())?;
        ov.no_throttle = true;
        save(&ov, dir, &format!("{tag}-plain-hint-{name}"))?;
        ov.on_mouse_mods(0, WM_LBUTTONDOWN, 160, 180, true, false);
        ov.on_mouse_mods(0, WM_MOUSEMOVE, 420, 350, true, false);
        ov.on_mouse_mods(0, WM_LBUTTONUP, 420, 350, true, false);
        ov.on_mouse_mods(0, WM_LBUTTONDOWN, 600, 250, true, false);
        ov.on_mouse_mods(0, WM_MOUSEMOVE, 950, 550, true, false);
        ov.on_mouse_mods(0, WM_LBUTTONUP, 950, 550, true, false);
        save(&ov, dir, &format!("{tag}-plain-frames-{name}"))?;
    }
    // The text overlay: the hint and the guide lines, then dragging, then too small.
    {
        let mut opts = Opts::plain(0.45);
        opts.theme = t;
        opts.lang = lang();
        opts.text = true;
        opts.text_action = true;
        let mut ov = Overlay::new(snapshot(scene, w, h, dpi)?, hinst, &opts).map_err(|e| e.to_string())?;
        ov.no_throttle = true;
        ov.on_mouse_mods(0, WM_MOUSEMOVE, 520, 300, false, false);
        save(&ov, dir, &format!("{tag}-text-overlay"))?;
        ov.on_mouse_mods(0, WM_LBUTTONDOWN, 160, 130, false, false);
        ov.on_mouse_mods(0, WM_MOUSEMOVE, 674, 208, false, false);
        save(&ov, dir, &format!("{tag}-text-drag"))?;
        ov.on_mouse_mods(0, WM_MOUSEMOVE, 168, 137, false, false);
        save(&ov, dir, &format!("{tag}-text-toosmall"))?;
    }
    // Picking the area to record: the hint, then a drag.
    {
        let mut opts = Opts::plain(0.45);
        opts.theme = t;
        opts.lang = lang();
        let mut ov = Overlay::new(snapshot(scene, w, h, dpi)?, hinst, &opts).map_err(|e| e.to_string())?;
        ov.no_throttle = true;
        ov.set_record_mode();
        ov.on_mouse_mods(0, WM_MOUSEMOVE, 1010, 620, false, false);
        save(&ov, dir, &format!("{tag}-record-pick"))?;
    }
    // The recording control as its real window draws it (GDI, through WM_PRINTCLIENT), next to
    // the core's drawing of the same control: the two must match.
    {
        let k = dpi as f32 / 96.0;
        let area = Rect::new((300.0 * k) as i32, (300.0 * k) as i32, (700.0 * k) as i32, (500.0 * k) as i32);
        let stop = pick_text("Durdur", "Stop");
        let (r, win) = crate::record_win::look_indicator(t, k, stop, 83_000, area, Rect::new(0, 0, w as i32, h as i32)).ok_or("record control window")?;
        let core = crate::record::render_control(83_000, stop, &t, k, &crate::gdi::GdiGlyphs::new());
        if (win.w, win.h) != (core.w, core.h) || win.px != core.px {
            return Err(format!("record control window {}x{} differs from the core drawing {}x{}", win.w, win.h, core.w, core.h));
        }
        let mut bg = Canvas::from_bgra(w, h, scene.to_vec());
        bg.draw(&win, r.left, r.top);
        let crop = bg.crop(crate::raster::IBox::new(r.left - 20, r.top - 20, area.right + 20, area.bottom + 20));
        let png = crate::png::encode_png(crop.w, crop.h, &crop.px).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(format!("{tag}-record-window.png")), png).map_err(|e| e.to_string())?;
        println!("wrote {tag}-record-window.png (window pixels equal the core drawing)");
    }
    // The mode bar offers text too.
    {
        let mut opts = Opts::plain(0.45);
        opts.theme = t;
        opts.lang = lang();
        opts.text_action = true;
        let mut ov = Overlay::new(snapshot(scene, w, h, dpi)?, hinst, &opts).map_err(|e| e.to_string())?;
        ov.no_throttle = true;
        let (x, y) = ov.mode_item_center(Item::ModeText).ok_or("no text mode button")?;
        ov.on_mouse_mods(0, WM_MOUSEMOVE, x, y, false, false);
        save(&ov, dir, &format!("{tag}-mode-text-hover"))?;
    }
    let mut opts = Opts::plain(0.45);
    opts.theme = t;
    opts.lang = lang();
    opts.editor = Some(Prefs::default());
    opts.text_action = true;
    let mut ov = Overlay::new(snapshot(scene, w, h, dpi)?, hinst, &opts).map_err(|e| e.to_string())?;
    ov.no_throttle = true;
    let (a, b) = (p(130, 80), p(1000, 700));
    ov.on_mouse_mods(0, WM_LBUTTONDOWN, a.0, a.1, false, false);
    ov.on_mouse_mods(0, WM_MOUSEMOVE, b.0, b.1, false, false);
    save(&ov, dir, &format!("{tag}-selecting"))?;
    ov.on_mouse_mods(0, WM_LBUTTONUP, b.0, b.1, false, false);
    save(&ov, dir, &format!("{tag}-editor-start"))?;
    pick(&mut ov, Tool::Arrow);
    drag(&mut ov, p(620, 180), p(470, 150));
    pick(&mut ov, Tool::Rect);
    drag(&mut ov, p(165, 350), p(960, 440));
    pick(&mut ov, Tool::Steps);
    drag(&mut ov, p(900, 400), p(900, 400));
    pick(&mut ov, Tool::Marker);
    drag(&mut ov, p(160, 200), p(620, 200));
    pick(&mut ov, Tool::Pen);
    drag(&mut ov, p(700, 600), p(900, 640));
    pick(&mut ov, Tool::Arrow);
    hover_item(&mut ov, Item::CopyText)?;
    save(&ov, dir, &format!("{tag}-editor-text-hover"))?;
    hover_item(&mut ov, Item::Tool(Tool::Rect))?;
    save(&ov, dir, &format!("{tag}-editor-hover"))?;
    click_item(&mut ov, Item::Color)?;
    save(&ov, dir, &format!("{tag}-palette"))?;
    click_item(&mut ov, Item::Color)?;
    click_item(&mut ov, Item::Size)?;
    save(&ov, dir, &format!("{tag}-size"))?;
    click_item(&mut ov, Item::Size)?;
    pick(&mut ov, Tool::Text);
    let c = p(180, 620);
    ov.on_mouse_mods(0, WM_LBUTTONDOWN, c.0, c.1, false, false);
    ov.on_mouse_mods(0, WM_LBUTTONUP, c.0, c.1, false, false);
    if let Some((edit, _)) = ov.text_windows() {
        // Typed like keys arrive, so the box grows the way it does for a person.
        for ch in pick_text("Kargo tarihi kontrol edilecek", "Check the delivery date").encode_utf16() {
            unsafe {
                let _ = PostMessageW(Some(edit), WM_CHAR, windows::Win32::Foundation::WPARAM(ch as usize), windows::Win32::Foundation::LPARAM(1));
            }
        }
    }
    pump();
    ov.on_mouse_mods(0, WM_MOUSEMOVE, c.0, c.1, false, false);
    save(&ov, dir, &format!("{tag}-text"))?;
    if let Some((edit, _)) = ov.text_windows() {
        ov.on_text_input(edit, crate::textinput::COMMIT, 0);
    }
    // Dropping the tool shows the resize handles.
    pick(&mut ov, Tool::Text);
    ov.on_key_mods(Tool::Text.key() as u32, false, false);
    save(&ov, dir, &format!("{tag}-handles"))?;
    Ok(())
}

/// `--selftest-seed SCENE`: recent snips and texts for the look test, in this (throwaway) profile.
pub fn run_seed(args: &[String]) -> i32 {
    if crate::selftest::desktop_name().eq_ignore_ascii_case("Default") {
        println!("REFUSED: --selftest-seed is for a hidden desktop with a throwaway profile");
        return 4;
    }
    let Some(raw) = args.first().and_then(|p| std::fs::read(p).ok()) else { return 2 };
    let Ok((w, h, px)) = parse_scene(&raw) else { return 2 };
    unsafe {
        let _ = CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
    let full = Canvas::from_bgra(w, h, px.to_vec());
    let store = crate::history::Store::new(crate::paths::history_dir());
    let times = ["2026-11-06 22:31:12", "2026-11-07 09:12:40", "2026-11-07 17:45:12", "2026-11-07 17:52:12"];
    let crops = [(160, 360, 960, 560), (1050, 200, 1540, 780), (120, 70, 1010, 790), (160, 120, 960, 300)];
    for (c, t) in crops.iter().zip(times) {
        let img = full.crop(crate::raster::IBox::new(c.0, c.1, c.2, c.3));
        let png = match crate::png::encode_png(img.w, img.h, &img.px) {
            Ok(png) => png,
            Err(err) => {
                println!("seed: png failed: {err}");
                return 1;
            }
        };
        let e = crate::history::Entry { id: 0, time: t.into(), path: String::new(), owned: true, w: img.w, h: img.h, source: "editor".into() };
        if let Err(err) = store.record(e, &crate::thumb::make(&img), Some(&png)) {
            println!("seed: record failed: {err}");
            return 1;
        }
    }
    let dir = crate::paths::history_dir();
    for (t, text) in [
        ("2026-11-05 10:15:30", "Kargo takip numarası: 7312 4490 1187."),
        ("2026-11-06 22:02:10", "Toplam tutar 1.234,56 TL, ödeme tarihi 21.11.2026."),
        ("2026-11-07 17:40:02", "Görüşme Çarşamba günü saat 14:30'da İstanbul ofisinde."),
        ("2026-11-07 17:51:44", "Dosya açılamadı: İzin reddedildi, lütfen yöneticinize başvurun.\r\nError 0x80070643: Fatal error during installation."),
    ] {
        if let Err(err) = crate::texthistory::record(&dir, t, text) {
            println!("seed: text failed: {err}");
            return 1;
        }
    }
    println!("seeded {}", dir.display());
    0
}

/// The lines of a menu as Windows holds them: text, key, and marks. Windows draws a menu on a hidden
/// desktop into nothing (PrintWindow, WM_PRINT and its DC all give black), so this text is the
/// evidence that can be had there.
fn menu_lines(m: HMENU, depth: usize, out: &mut String) {
    unsafe {
        for i in 0..GetMenuItemCount(Some(m)).max(0) {
            let state = GetMenuState(m, i as u32, MF_BYPOSITION);
            let mut buf = [0u16; 128];
            let n = GetMenuStringW(m, i as u32, Some(&mut buf), MF_BYPOSITION);
            let text = String::from_utf16_lossy(&buf[..n.max(0) as usize]).replace('\t', "   [") ;
            let keyed = if text.contains("   [") { format!("{text}]") } else { text };
            let sub = GetSubMenu(m, i);
            let indent = "    ".repeat(depth);
            if state & MF_SEPARATOR.0 != 0 && sub.is_invalid() {
                out.push_str(&format!("{indent}----\n"));
            } else {
                let info = if state & MF_GRAYED.0 != 0 { " (gri)" } else { "" };
                let default = if GetMenuDefaultItem(m, 1, GET_MENU_DEFAULT_ITEM_FLAGS(0)) == i as u32 { " (kalin, cift tiklama)" } else { "" };
                out.push_str(&format!("{indent}{keyed}{info}{default}\n"));
            }
            if !sub.is_invalid() {
                menu_lines(sub, depth + 1, out);
            }
        }
    }
}

/// `--selftest-menu FILE`: the real tray menu, read from a helper thread and closed again. Hidden
/// desktop only.
pub fn run_menu(args: &[String]) -> i32 {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::core::{PCWSTR, w};
    let desk = crate::selftest::desktop_name();
    if desk.is_empty() || desk.eq_ignore_ascii_case("Default") || desk.eq_ignore_ascii_case("Winlogon") {
        println!("REFUSED: --selftest-menu requires a hidden desktop");
        return 4;
    }
    let Some(path) = args.first().cloned() else { return 2 };
    let (cfg, _) = crate::config::Config::load_or_create(&crate::paths::config_file());
    let lang = crate::sys::lang(&cfg);
    let mode = crate::sys::theme(&cfg).mode;
    let hwnd = unsafe { CreateWindowExW(WS_EX_TOOLWINDOW, w!("STATIC"), w!("menu test"), WS_POPUP, 0, 0, 0, 0, None, None, None, None) };
    let Ok(hwnd) = hwnd else { return 1 };
    let desk_raw = unsafe { windows::Win32::System::StationsAndDesktops::GetThreadDesktop(windows::Win32::System::Threading::GetCurrentThreadId()) }.map(|d| d.0 as isize).unwrap_or(0);
    let shooter = std::thread::spawn(move || -> bool {
        unsafe {
            // Same desktop as the menu: FindWindow and printing work per desktop.
            let _ = windows::Win32::System::StationsAndDesktops::SetThreadDesktop(windows::Win32::System::StationsAndDesktops::HDESK(desk_raw as *mut _));
            std::thread::sleep(std::time::Duration::from_millis(500));
            let Ok(menu) = FindWindowW(w!("#32768"), PCWSTR::null()) else {
                println!("menu window not found");
                return false;
            };
            let r = crate::overlay::window_rect(menu);
            const MN_GETHMENU: u32 = 0x01E1;
            let handle = SendMessageW(menu, MN_GETHMENU, Some(WPARAM(0)), Some(LPARAM(0)));
            let mut text = format!("Tepsi menusu, Windows'un tuttugu haliyle ({}x{} piksel):\n", r.width(), r.height());
            menu_lines(HMENU(handle.0 as *mut _), 0, &mut text);
            let done = handle.0 != 0 && std::fs::write(&path, text).is_ok();
            println!("menu {}x{} {}", r.width(), r.height(), if done { "written" } else { "not written" });
            let _ = PostMessageW(Some(menu), WM_KEYDOWN, WPARAM(0x1B), LPARAM(0));
            let _ = PostMessageW(Some(menu), WM_CLOSE, WPARAM(0), LPARAM(0));
            done
        }
    });
    let text_key = crate::hotkey::Hotkey::parse(&cfg.text_hotkey).map(|k| k.display()).unwrap_or_default();
    let cmd = crate::tray::menu(HWND(hwnd.0), &crate::tray::MenuInfo { record_on: cfg.record_enabled, record_busy: false, status: crate::i18n::S::StatusReady.get(lang), lang, snip_on: cfg.snip_enabled, text_on: cfg.text_enabled, snip_key: cfg.win_shift_s, editor_key: &cfg.editor_key_text(), all_key: &cfg.all_key_text(), text_key: &text_key, mode });
    println!("menu closed, command {cmd}");
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    if shooter.join().unwrap_or(false) { 0 } else { 1 }
}

pub fn run_look(args: &[String]) -> i32 {
    let desk = crate::selftest::desktop_name();
    if desk.is_empty() || desk.eq_ignore_ascii_case("Default") || desk.eq_ignore_ascii_case("Winlogon") {
        println!("REFUSED: --selftest-look requires a hidden desktop");
        return 4;
    }
    let (Some(dir), Some(scene)) = (args.first(), args.get(1)) else {
        println!("usage: snip --selftest-look DIR SCENE [--dpi N] [--lang en]");
        return 2;
    };
    if args.iter().position(|a| a == "--lang").and_then(|i| args.get(i + 1)).is_some_and(|l| l == "en") {
        let _ = LANG.set(Lang::En);
    }
    let dpi = args.iter().position(|a| a == "--dpi").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(96u32);
    let dir = Path::new(dir);
    let _ = std::fs::create_dir_all(dir);
    let Ok(raw) = std::fs::read(scene) else {
        println!("cannot read the scene");
        return 2;
    };
    let (w, h) = match parse_scene(&raw) {
        Ok((w, h, _)) => (w, h),
        Err(e) => {
            println!("{e}");
            return 2;
        }
    };
    unsafe {
        let _ = CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
    let result = (|| -> Result<(), String> {
        let hinst: windows::Win32::Foundation::HINSTANCE = unsafe { GetModuleHandleW(None).map_err(|e| e.to_string())?.into() };
        crate::app::register_classes(hinst, crate::app::MAIN_CLASS_TEST).map_err(|e| e.to_string())?;
        for (look, mode, name) in [(Look::Graphite, Mode::Dark, "dark"), (Look::Graphite, Mode::Light, "light"), (Look::Paper, Mode::Dark, "paper-dark"), (Look::Paper, Mode::Light, "paper-light")] {
            let t = Theme::new(look, mode);
            one_theme(hinst, &raw[8..], w, h, dpi, t, &format!("{name}-{dpi}"), dir)?;
            panels(&raw[8..], w, h, t, name, dir)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => 0,
        Err(e) => {
            println!("FAIL {e}");
            1
        }
    }
}
