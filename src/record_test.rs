//! Hidden synthetic acceptance through production GPU readback, NV12, MF, GIF and publication.
//! Never starts a tray/hook, selects a real monitor, or writes WinSta0's clipboard.
use crate::geometry::Rect;
use crate::record::{self, Output};
use crate::record_media::Media;
use serde_json::{Value, json};
use std::path::Path;
use windows::Win32::Foundation::{GENERIC_ALL, GENERIC_READ, HANDLE, HGLOBAL};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::StationsAndDesktops::*;
use windows::core::{PCWSTR, w};

type TestResult = Result<Value, String>;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
#[derive(Default)]
struct Report {
    passed: u32,
    failed: u32,
    skipped: u32,
}
impl Report {
    fn check(&mut self, name: &str, r: TestResult) {
        match r {
            Ok(metrics) => {
                self.passed += 1;
                println!(
                    "{}",
                    json!({"subsystem":name,"status":"PASS","metrics":metrics})
                );
            }
            Err(error) => {
                self.failed += 1;
                println!(
                    "{}",
                    json!({"subsystem":name,"status":"FAIL","error":error})
                );
            }
        }
    }
    fn skip(&mut self, name: &str, reason: &str) {
        self.skipped += 1;
        println!(
            "{}",
            json!({"subsystem":name,"status":"SKIP","reason":reason})
        );
    }
}
pub fn run(args: &[String]) -> i32 {
    let desktop = crate::selftest::desktop_name();
    if desktop.is_empty() || desktop.eq_ignore_ascii_case("Default") {
        println!(
            "{}",
            json!({"subsystem":"environment","status":"REFUSED","reason":"requires a hidden desktop, never Default"})
        );
        return 2;
    }
    let Some(dir) = args.first().map(std::path::PathBuf::from) else {
        println!(
            "{}",
            json!({"subsystem":"environment","status":"REFUSED","reason":"usage: snip --selftest-record DIR"})
        );
        return 2;
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        println!(
            "{}",
            json!({"subsystem":"environment","status":"FAIL","error":e.to_string()})
        );
        return 1;
    }
    // This entry point has started no threads/COM/windows yet; all app-state writes are redirected.
    let profile = dir.join(format!("profile-{}", std::process::id()));
    if std::fs::create_dir_all(profile.join("Desktop")).is_err() {
        return 1;
    }
    unsafe {
        for key in ["APPDATA", "LOCALAPPDATA", "USERPROFILE"] {
            std::env::set_var(key, &profile);
        }
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let sequence = crate::clipboard::sequence();
    let mut report = Report::default();
    report.check(
        "environment",
        Ok(json!({"desktop":desktop,"profile":profile,"live_capture":false})),
    );
    report.check(
        "optional-media",
        crate::record_media::test_missing_components()
            .map(|_| json!({"missing_dll":"rejected","missing_symbol":"rejected"})),
    );
    report.check("gpu-crop", gpu_test());
    report.check("stop-ipc", crate::app::record_ipc_selftest());
    match Media::start() {
        Ok(media) => {
            // Hardware-enabled is a requested path. The metrics report the actual MFT, including
            // software fallback by Windows; it is never called hardware success without evidence.
            report.check(
                "hardware-enabled",
                clip_test(&media, &dir.join("hardware-enabled"), true, false),
            );
            report.check(
                "forced-software",
                clip_test(&media, &dir.join("software"), false, false),
            );
            report.check(
                "static-coalescing",
                clip_test(&media, &dir.join("static"), false, true),
            );
        }
        Err(e) => {
            report.check("media-foundation", Err(e.to_string()));
            for n in ["hardware-enabled", "forced-software", "static-coalescing"] {
                report.skip(n, "Media Foundation unavailable");
            }
        }
    }
    report.check("publication-failures", publication_tests(&dir));
    report.check("clipboard-private-station", clipboard_test(&dir));
    report.check(
        "user-clipboard",
        if crate::clipboard::sequence() == sequence {
            Ok(json!({"sequence_unchanged":true}))
        } else {
            Err("the user's clipboard sequence changed".into())
        },
    );
    report.skip("live-WGC-and-chat","real monitor/cursor/HDR/lock/focus/performance and Discord paste need a separate manual check on a real desktop");
    println!(
        "record selftest: {} passed, {} failed, {} skipped",
        report.passed, report.failed, report.skipped
    );
    unsafe {
        CoUninitialize();
    }
    if report.failed == 0 { 0 } else { 1 }
}
/// 641x359 content in a larger, padded-stride source: colour patches, frame bits and moving marker.
fn synthetic(n: u32, static_image: bool) -> (Vec<u8>, usize) {
    let (w, h) = (649usize, 367usize);
    let stride = w * 4 + 28;
    let mut source = vec![0xcc; stride * h];
    let id = if static_image { 0 } else { n };
    for y in 0..359usize {
        for x in 0..641usize {
            let rgb = match x / 128 {
                0 => [0, 0, 0],
                1 => [255, 255, 255],
                2 => [255, 0, 0],
                3 => [0, 255, 0],
                _ => [0, 0, 255],
            };
            let mut px = if static_image {
                [0, 0, 0, 255]
            } else {
                [rgb[2], rgb[1], rgb[0], 255]
            };
            if !static_image && (270..294).contains(&y) && (24..248).contains(&x) {
                let bit = (x - 24) / 32;
                let c = if id & (1 << bit) != 0 { 255 } else { 0 };
                px = [c, c, c, 255];
            }
            if !static_image
                && (310..334).contains(&y)
                && x >= 16 + id as usize * 5
                && x < 40 + id as usize * 5
            {
                px = [255, 255, 0, 255];
            }
            let at = (y + 4) * stride + (x + 4) * 4;
            source[at..at + 4].copy_from_slice(&px);
        }
    }
    (source, stride)
}
fn gpu_frame(
    gpu: &crate::capture::Gpu,
    source: &[u8],
    stride: usize,
    staging: &mut Option<ID3D11Texture2D>,
) -> Result<Vec<u8>, String> {
    unsafe {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: 649,
            Height: 367,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let data = D3D11_SUBRESOURCE_DATA {
            pSysMem: source.as_ptr() as *const _,
            SysMemPitch: stride as u32,
            SysMemSlicePitch: 0,
        };
        let mut texture = None;
        gpu.device
            .CreateTexture2D(&desc, Some(&data), Some(&mut texture))
            .map_err(err)?;
        crate::record_capture::read_crop(
            &gpu.device,
            &gpu.context,
            &texture.ok_or("no GPU texture")?,
            Rect::from_xywh(4, 4, 641, 359),
            staging,
        )
        .map_err(err)
    }
}
fn gpu_test() -> TestResult {
    let gpu = crate::capture::create_gpu().map_err(err)?;
    let mut staging = None;
    for n in [0, 89] {
        let (source, stride) = synthetic(n, false);
        let actual = gpu_frame(&gpu, &source, stride, &mut staging)?;
        let wanted = record::crop_scale(
            &source,
            649,
            367,
            stride,
            Rect::from_xywh(4, 4, 641, 359),
            record::fit(641, 359, 1920, 1080, false)?,
        )?;
        if actual != wanted {
            return Err("D3D crop/readback differs from padded source".into());
        }
    }
    Ok(json!({"warp":gpu.warp,"source":[649,367],"crop":[641,359],"stride":2624,"byte_exact":true}))
}
fn clip_test(media: &Media, dir: &Path, hardware: bool, static_image: bool) -> TestResult {
    let output = Output::reserve(dir, crate::app::local_stamp()).map_err(err)?;
    let size = record::fit(641, 359, 1920, 1080, true)?;
    let gpu = crate::capture::create_gpu().map_err(err)?;
    let mut staging = None;
    let mut writer = media.writer(&output.part, size, hardware).map_err(|e| format!("writer: {}", err(e)))?;
    let backend = writer.backend.clone();
    let started = std::time::Instant::now();
    for n in 0..90 {
        let (source, stride) = synthetic(n, static_image);
        let crop = gpu_frame(&gpu, &source, stride, &mut staging)?;
        let padded = record::crop_scale(
            &crop,
            641,
            359,
            641 * 4,
            Rect::from_xywh(0, 0, 641, 359),
            size,
        )?;
        if padded[640 * 4..641 * 4] != padded[641 * 4..642 * 4] {
            return Err("odd right edge not replicated".into());
        }
        let nv12 = record::nv12(&padded, size.w, size.h)?;
        writer.write(&nv12).map_err(|e| format!("write: {}", err(e)))?;
    }
    writer.finish().map_err(|e| format!("finish: {}", err(e)))?;
    let encode_ms = started.elapsed().as_secs_f64() * 1000.;
    let bytes = record::publish(&output.part, &output.mp4, record::MP4_CAP).map_err(|e| format!("publish: {}", err(e)))?;
    let mut reader = media.reader(&output.mp4).map_err(|e| format!("reader: {}", err(e)))?;
    if (reader.w, reader.h) != (642, 360) {
        return Err(format!("decoded dimensions {}x{}", reader.w, reader.h));
    }
    let mut count = 0u32;
    let mut end = 0;
    let mut maximum_error = 0u8;
    while let Some(frame) = reader.next_frame().map_err(|e| format!("decode: {}", err(e)))? {
        if (frame.time - record::timestamp(count as u64)).abs() > 1 {
            return Err(format!("timestamp {} at frame {count}", frame.time));
        }
        let px = record::nv12_bgra(&frame.pixels, reader.w, reader.h)?;
        // Interior patch centres avoid H.264/chroma edges; declared RGB tolerance 24 / 255.
        for (x, want) in [
            (64, [0, 0, 0]),
            (192, [255, 255, 255]),
            (320, [0, 0, 255]),
            (448, [0, 255, 0]),
            (576, [255, 0, 0]),
        ] {
            let want = if static_image { [0, 0, 0] } else { want };
            let at = (100 * 642 + x) * 4;
            for c in 0..3 {
                let delta = px[at + c].abs_diff(want[c]);
                maximum_error = maximum_error.max(delta);
                if delta > 24 {
                    return Err(format!(
                        "patch mismatch at frame {count}, x{x}, channel {c}: {delta}>24"
                    ));
                }
            }
        }
        if count == 0 || count == 89 {
            let mut id = 0;
            for bit in 0..7 {
                let x = 40 + bit * 32;
                let at = (282 * 642 + x) * 4;
                if px[at] > 128 {
                    id |= 1 << bit;
                }
            }
            if id != if static_image { 0 } else { count } {
                return Err(format!("decoded frame ID {id}, expected {count}"));
            }
            // The replicated odd bottom/right must remain close after the lossy roundtrip.
            let right = (200 * 642 + 641) * 4;
            let edge = (200 * 642 + 640) * 4;
            if (0..3).any(|c| px[right + c].abs_diff(px[edge + c]) > 24) {
                return Err("decoded odd edge differs".into());
            }
        }
        end = frame.time + frame.duration;
        count += 1;
    }
    if count != 90 || (end - 30_000_000).abs() > 333_334 {
        return Err(format!("MP4 {count} frames, {end} ticks"));
    }
    let (gif_bytes, gif_frames) = crate::record_media::export_gif(media, &output)?;
    let wic = wic_gif(&output.gif, if static_image { 1 } else { 30 })?;
    Ok(
        json!({"requested_hardware":hardware,"actual_transform":backend,"frames":count,"duration_100ns":end,"size":[642,360],"rgb_tolerance":24,"max_patch_error":maximum_error,"encode_ms":encode_ms,"mp4_bytes":bytes,"gif_bytes":gif_bytes,"gif_frames":gif_frames,"wic":wic,"mp4":output.mp4,"gif":output.gif}),
    )
}
fn metadata_u32(reader: &IWICMetadataQueryReader, key: PCWSTR) -> Result<u32, String> {
    unsafe {
        let mut value = PROPVARIANT::default();
        reader.GetMetadataByName(key, &mut value).map_err(err)?;
        u32::try_from(&value).map_err(err)
    }
}
fn metadata_bytes(reader: &IWICMetadataQueryReader, key: PCWSTR) -> Result<Vec<u8>, String> {
    unsafe {
        use windows::Win32::System::Variant::{VT_UI1, VT_VECTOR};
        let mut value = PROPVARIANT::default();
        reader.GetMetadataByName(key, &mut value).map_err(err)?;
        if value.vt().0 != (VT_UI1.0 | VT_VECTOR.0) {
            return Err(format!("unexpected GIF metadata type {:?}", value.vt()));
        }
        let array = &value.Anonymous.Anonymous.Anonymous.caub;
        Ok(std::slice::from_raw_parts(array.pElems, array.cElems as usize).to_vec())
    }
}
fn wic_gif(path: &Path, expected: u32) -> TestResult {
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(err)?;
        let name: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let decoder = factory
            .CreateDecoderFromFilename(
                PCWSTR(name.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnLoad,
            )
            .map_err(err)?;
        let count = decoder.GetFrameCount().map_err(err)?;
        if count != expected {
            return Err(format!(
                "WIC decoded {count} GIF frames, expected {expected}"
            ));
        }
        let meta = decoder.GetMetadataQueryReader().map_err(err)?;
        let application = metadata_bytes(&meta, w!("/appext/application"))?;
        let data = metadata_bytes(&meta, w!("/appext/data"))?;
        if application != b"NETSCAPE2.0" || !data.windows(3).any(|x| x == [1, 0, 0]) {
            return Err(format!("WIC GIF loop metadata: {application:?} {data:?}"));
        }
        let mut total = 0;
        let mut first_marker = None;
        let mut last_marker = None;
        for i in 0..count {
            let frame = decoder.GetFrame(i).map_err(err)?;
            let (mut w, mut h) = (0, 0);
            frame.GetSize(&mut w, &mut h).map_err(err)?;
            if (w, h) != (642, 360) {
                return Err(format!("WIC GIF size {w}x{h}"));
            }
            let meta = frame.GetMetadataQueryReader().map_err(err)?;
            let delay = metadata_u32(&meta, w!("/grctlext/Delay"))?;
            let disposal = metadata_u32(&meta, w!("/grctlext/Disposal"))?;
            if disposal != 1 || (count > 1 && delay != 10) {
                return Err(format!("WIC GIF delay {delay}, disposal {disposal}"));
            }
            total += delay;
            let converter = factory.CreateFormatConverter().map_err(err)?;
            converter
                .Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.,
                    WICBitmapPaletteTypeCustom,
                )
                .map_err(err)?;
            let mut pixels = vec![0; w as usize * h as usize * 4];
            converter
                .CopyPixels(std::ptr::null(), w * 4, &mut pixels)
                .map_err(err)?;
            let marker = (0..w as usize).find(|&x| {
                let at = (320 * w as usize + x) * 4;
                pixels[at] > 180 && pixels[at + 1] > 180 && pixels[at + 2] < 80
            });
            if count > 1 && marker.is_none() {
                return Err("WIC GIF marker missing".into());
            }
            if i == 0 {
                first_marker = marker;
            }
            last_marker = marker;
        }
        if total != 300 || (count > 1 && first_marker == last_marker) {
            return Err(format!(
                "WIC GIF total delay {total}, marker {first_marker:?}->{last_marker:?}"
            ));
        }
        Ok(
            json!({"frames":count,"total_centiseconds":total,"loop":"infinite","disposal":"keep","marker_moves":count>1}),
        )
    }
}
fn publication_tests(dir: &Path) -> TestResult {
    let folder = dir.join("publication-errors");
    std::fs::create_dir_all(&folder).map_err(err)?;
    let unique = format!("{}-{}", std::process::id(), crate::log::qpc());
    let part = folder.join(format!("{unique}.part"));
    let dest = folder.join(format!("{unique}.mp4"));
    std::fs::write(&part, b"incomplete").map_err(err)?;
    std::fs::write(&dest, b"existing").map_err(err)?;
    if record::publish(&part, &dest, record::MP4_CAP).is_ok()
        || std::fs::read(&dest).map_err(err)? != b"existing"
    {
        return Err("publication replaced an existing file".into());
    }
    if record::publish(&part, &folder.join(format!("{unique}-cap.mp4")), 1).is_ok() {
        return Err("publication accepted oversized output".into());
    }
    Ok(json!({"non_replacing":true,"oversized_rejected":true,"parts_retained":part.exists()}))
}
fn clipboard_test(dir: &Path) -> TestResult {
    // A fresh thread/station, restored by RAII on EVERY return path, including an API failure.
    let station = unsafe { GetProcessWindowStation() }.map_err(err)?;
    let original = station.0 as usize;
    let file = dir.join("Ekran kaydı Şİ 😀.mp4");
    std::thread::spawn(move||->TestResult {unsafe {
        struct Restore {original:usize,station:Option<HWINSTA>,desktop:Option<HDESK>}
        impl Drop for Restore {fn drop(&mut self) {unsafe {let _=SetProcessWindowStation(HWINSTA(self.original as *mut _));if let Some(s)=self.station {let _=CloseWindowStation(s);}}}}
        let name:Vec<u16>=format!("SnipRecordTest{}",std::process::id()).encode_utf16().chain(Some(0)).collect();
        let private=CreateWindowStationW(PCWSTR(name.as_ptr()),0,GENERIC_ALL.0,None).map_err(err)?;
        let mut restore=Restore{original,station:Some(private),desktop:None};
        SetProcessWindowStation(private).map_err(err)?;
        let desk=CreateDesktopW(w!("record"),PCWSTR::null(),None,DESKTOP_CONTROL_FLAGS(0),GENERIC_ALL.0,None).map_err(err)?;restore.desktop=Some(desk);SetThreadDesktop(desk).map_err(err)?;
        let mut name=[0u16;128];GetUserObjectInformationW(HANDLE(GetProcessWindowStation().map_err(err)?.0),UOI_NAME,Some(name.as_mut_ptr() as *mut _),256,None).map_err(err)?;
        let actual=String::from_utf16_lossy(&name[..name.iter().position(|c|*c==0).unwrap_or(0)]);
        if !actual.starts_with("SnipRecordTest") {return Err("not on private station".into());}
        use windows::Win32::UI::WindowsAndMessaging::*;
        let hwnd=CreateWindowExW(WS_EX_TOOLWINDOW,w!("STATIC"),w!("record clipboard test"),WS_POPUP,0,0,0,0,None,None,None,None).map_err(err)?;
        crate::clipboard::set_image(hwnd,2,2,&[0,0,255,255].repeat(4)).map_err(err)?;
        let seq=crate::clipboard::sequence();let published=crate::clipboard::publish_recording(hwnd,seq,&file).map_err(err)?;
        use windows::Win32::System::DataExchange::{OpenClipboard,CloseClipboard,GetClipboardData,IsClipboardFormatAvailable};
        use windows::Win32::System::Memory::{GlobalLock,GlobalUnlock,GlobalSize};
        use windows::Win32::System::Ole::{CF_DIB,CF_DIBV5,CF_HDROP};
        if !published || IsClipboardFormatAvailable(CF_DIB.0 as u32).is_ok() || IsClipboardFormatAvailable(CF_DIBV5.0 as u32).is_ok() {return Err("recording clipboard kept an image format".into());}
        let _=DestroyWindow(hwnd); // Immediate-rendered DROPFILES must survive the producer HWND.
        OpenClipboard(None).map_err(err)?;let data=GetClipboardData(CF_HDROP.0 as u32).map_err(err)?;let global=HGLOBAL(data.0);let p=GlobalLock(global) as *const u8;
        let bytes=if p.is_null(){vec![]}else{let b=std::slice::from_raw_parts(p,GlobalSize(global)).to_vec();let _=GlobalUnlock(global);b};let _=CloseClipboard();
        let wanted=crate::dib::hdrop_bytes(&file.to_string_lossy());if bytes.get(..wanted.len())!=Some(wanted.as_slice()) {return Err("Unicode CF_HDROP roundtrip differs".into());}
        let stale=crate::clipboard::publish_recording(Default::default(),seq,&file).map_err(err)?;if stale {return Err("stale clipboard sequence was accepted".into());}
        Ok(json!({"station":actual,"unicode_hdrop":true,"dib_formats_absent":true,"stale_sequence_rejected":true,"survives_owner_window":true}))
    }}).join().map_err(|_|"clipboard worker panicked".to_string())?
}
