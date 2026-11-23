//! Optional Media Foundation. No MF function is statically linked: load only system DLLs on use.
use crate::record::{self, Size};
use std::ffi::c_void;
use std::path::Path;
use windows::Win32::Foundation::{E_FAIL, HMODULE};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::core::{GUID, HRESULT, Interface, PCSTR, PCWSTR, Result, Type};

fn error(s: impl AsRef<str>) -> windows::core::Error {
    windows::core::Error::new(E_FAIL, s.as_ref())
}
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect()
}
// The modules stay loaded until process exit. All COM objects can therefore outlive a wrapper.
// Each recording lives in its own short-lived child, so this retains no DLLs in the tray.
pub struct Media {
    plat: HMODULE,
    rw: HMODULE,
}
macro_rules! entry {
    ($module:expr,$name:literal,$ty:ty) => {{
        #[allow(unused_unsafe)]
        unsafe {
            let p = GetProcAddress($module, PCSTR(concat!($name, "\0").as_ptr()))
                .ok_or_else(|| error(concat!("Media Foundation entry missing: ", $name)))?;
            std::mem::transmute::<unsafe extern "system" fn() -> isize, $ty>(p)
        }
    }};
}
impl Media {
    pub fn start() -> Result<Self> {
        unsafe {
            let plat = LoadLibraryExW(
                windows::core::w!("mfplat.dll"),
                None,
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
            .map_err(|_| error("Recording needs the Windows Media Feature Pack (mfplat.dll)"))?;
            let rw = LoadLibraryExW(
                windows::core::w!("mfreadwrite.dll"),
                None,
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
            .map_err(|_| {
                error("Recording needs the Windows Media Feature Pack (mfreadwrite.dll)")
            })?;
            let startup = entry!(
                plat,
                "MFStartup",
                unsafe extern "system" fn(u32, u32) -> HRESULT
            );
            startup(MF_VERSION, MFSTARTUP_FULL).ok()?;
            Ok(Self { plat, rw })
        }
    }
    fn attributes(&self) -> Result<IMFAttributes> {
        let f = entry!(
            self.plat,
            "MFCreateAttributes",
            unsafe extern "system" fn(*mut *mut c_void, u32) -> HRESULT
        );
        let mut p = std::ptr::null_mut();
        unsafe {
            f(&mut p, 12).ok()?;
            IMFAttributes::from_abi(p)
        }
    }
    fn media_type(&self) -> Result<IMFMediaType> {
        let f = entry!(
            self.plat,
            "MFCreateMediaType",
            unsafe extern "system" fn(*mut *mut c_void) -> HRESULT
        );
        let mut p = std::ptr::null_mut();
        unsafe {
            f(&mut p).ok()?;
            IMFMediaType::from_abi(p)
        }
    }
    fn sample(&self, data: &[u8], n: u64) -> Result<IMFSample> {
        let f = entry!(
            self.plat,
            "MFCreateSample",
            unsafe extern "system" fn(*mut *mut c_void) -> HRESULT
        );
        let b = entry!(
            self.plat,
            "MFCreateMemoryBuffer",
            unsafe extern "system" fn(u32, *mut *mut c_void) -> HRESULT
        );
        unsafe {
            let mut p = std::ptr::null_mut();
            f(&mut p).ok()?;
            let sample = IMFSample::from_abi(p)?;
            let mut p = std::ptr::null_mut();
            b(data.len() as u32, &mut p).ok()?;
            let buffer = IMFMediaBuffer::from_abi(p)?;
            let mut dst = std::ptr::null_mut();
            buffer.Lock(&mut dst, None, None)?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(data.len() as u32)?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(record::timestamp(n))?;
            sample.SetSampleDuration(record::duration(n))?;
            Ok(sample)
        }
    }
    fn video_type(&self, size: Size, subtype: &GUID) -> Result<IMFMediaType> {
        let t = self.media_type()?;
        unsafe {
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
            t.SetUINT64(&MF_MT_FRAME_SIZE, (size.w as u64) << 32 | size.h as u64)?;
            t.SetUINT64(&MF_MT_FRAME_RATE, 30u64 << 32 | 1)?;
            t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, 1u64 << 32 | 1)?;
            t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            t.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
            t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
            t.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
            t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
        }
        Ok(t)
    }
    /// Opens the exclusively reserved .part as a seekable stream with an explicit MP4 container.
    pub fn writer<'a>(&'a self, path: &Path, size: Size, hardware: bool) -> Result<Writer<'a>> {
        let file = entry!(
            self.plat,
            "MFCreateFile",
            unsafe extern "system" fn(
                MF_FILE_ACCESSMODE,
                MF_FILE_OPENMODE,
                MF_FILE_FLAGS,
                PCWSTR,
                *mut *mut c_void,
            ) -> HRESULT
        );
        let create = entry!(
            self.rw,
            "MFCreateSinkWriterFromURL",
            unsafe extern "system" fn(
                PCWSTR,
                *mut c_void,
                *mut c_void,
                *mut *mut c_void,
            ) -> HRESULT
        );
        unsafe {
            let mut p = std::ptr::null_mut();
            let name = wide(path);
            file(
                MF_ACCESSMODE_READWRITE,
                MF_OPENMODE_RESET_IF_EXIST,
                MF_FILEFLAGS_NONE,
                PCWSTR(name.as_ptr()),
                &mut p,
            )
            .ok()?;
            let stream = IMFByteStream::from_abi(p)?;
            let attrs = self.attributes()?;
            attrs.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
            attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, hardware as u32)?;
            attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
            // No B-frames: the Microsoft software encoder otherwise reorders frames and the
            // first decoded sample starts one frame late (measured on Windows 2026-11-17).
            attrs.SetUINT32(&MF_LOW_LATENCY, 1)?;
            let mut p = std::ptr::null_mut();
            create(PCWSTR::null(), stream.as_raw(), attrs.as_raw(), &mut p).ok()?;
            let writer = IMFSinkWriter::from_abi(p)?;
            let output = self.video_type(size, &MFVideoFormat_H264)?;
            output.SetUINT32(&MF_MT_AVG_BITRATE, record::bitrate(size.w, size.h))?;
            output.SetUINT32(&MF_MT_MPEG2_PROFILE, 77)?; // H.264 Main; encoder chooses level.
            let index = writer.AddStream(&output)?;
            let input = self.video_type(size, &MFVideoFormat_NV12)?;
            input.SetUINT32(&MF_MT_DEFAULT_STRIDE, size.w)?;
            writer.SetInputMediaType(index, &input, None)?;
            let mut backend = String::from("transform unavailable");
            if let Ok(ex) = writer.cast::<IMFSinkWriterEx>() {
                for i in 0..8 {
                    let mut transform = None;
                    let mut category = GUID::zeroed();
                    if ex
                        .GetTransformForStream(index, i, Some(&mut category), &mut transform)
                        .is_err()
                    {
                        break;
                    }
                    if let Some(t) = transform {
                        if let Ok(codec) = t.cast::<ICodecAPI>() {
                            let value = windows::Win32::System::Variant::VARIANT::from(60u32);
                            if let Err(e) = codec.SetValue(&CODECAPI_AVEncMPVGOPSize, &value) {
                                crate::logf!("record GOP request: {e}");
                            }
                            let none = windows::Win32::System::Variant::VARIANT::from(0u32);
                            if let Err(e) =
                                codec.SetValue(&CODECAPI_AVEncMPVDefaultBPictureCount, &none)
                            {
                                crate::logf!("record B-frame request: {e}");
                            }
                        }
                        if category == MFT_CATEGORY_VIDEO_ENCODER {
                            let a = t.GetAttributes()?;
                            let mut name = [0u16; 512];
                            let mut len = 0;
                            let friendly = if a
                                .GetString(&MFT_FRIENDLY_NAME_Attribute, &mut name, Some(&mut len))
                                .is_ok()
                            {
                                String::from_utf16_lossy(&name[..len as usize])
                            } else {
                                "unnamed encoder".into()
                            };
                            let mut hw = [0u16; 512];
                            let actual_hw = a
                                .GetString(&MFT_ENUM_HARDWARE_URL_Attribute, &mut hw, None)
                                .is_ok();
                            backend =
                                format!("{friendly}; hardware={actual_hw}; requested={hardware}");
                        }
                    }
                }
            }
            writer.BeginWriting()?;
            Ok(Writer {
                media: self,
                writer,
                stream,
                index,
                frames: 0,
                backend,
            })
        }
    }
    pub fn reader(&self, path: &Path) -> Result<Reader> {
        let create = entry!(
            self.rw,
            "MFCreateSourceReaderFromURL",
            unsafe extern "system" fn(PCWSTR, *mut c_void, *mut *mut c_void) -> HRESULT
        );
        let name = wide(path);
        let mut p = std::ptr::null_mut();
        unsafe {
            create(PCWSTR(name.as_ptr()), std::ptr::null_mut(), &mut p).ok()?;
            let reader = IMFSourceReader::from_abi(p)?;
            reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
            reader.SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, true)?;
            let mt = self.media_type()?;
            mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            reader.SetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, None, &mt)?;
            let geometry = Geometry::of(
                &reader.GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)?,
            )?;
            let (w, h) = (geometry.w, geometry.h);
            Ok(Reader {
                reader,
                w,
                h,
                geometry,
                last: None,
            })
        }
    }
}
pub struct Writer<'a> {
    media: &'a Media,
    writer: IMFSinkWriter,
    stream: IMFByteStream,
    index: u32,
    pub frames: u64,
    pub backend: String,
}
impl Writer<'_> {
    pub fn write(&mut self, nv12: &[u8]) -> Result<()> {
        let s = self.media.sample(nv12, self.frames)?;
        unsafe {
            self.writer.WriteSample(self.index, &s)?;
        }
        self.frames += 1;
        Ok(())
    }
    pub fn bytes(&self) -> u64 {
        unsafe { self.stream.GetLength().unwrap_or(0) }
    }
    pub fn finish(self) -> Result<()> {
        // Covers qualification and selftests too. A blocked Finalize/drop keeps only .part.
        let (done, watch) = std::sync::mpsc::channel();
        let watchdog = std::thread::spawn(move || {
            if watch
                .recv_timeout(std::time::Duration::from_millis(record::FINALIZE_MS))
                .is_err()
            {
                std::process::exit(74);
            }
        });
        let result = (move || {
            if self.frames == 0 {
                return Err(error("recording stopped before first frame"));
            }
            unsafe {
                self.writer.Finalize().map_err(|e| error(format!("Finalize: {e}")))?;
            }
            drop(self.writer);
            // The MPEG-4 sink flushes and closes its byte stream inside Finalize, so a second
            // Flush fails with E_INVALIDARG (measured on Windows 2026-11-17). Close is harmless
            // when already closed; publish() syncs the file to disk before the rename.
            unsafe {
                let _ = self.stream.Close();
            }
            drop(self.stream);
            Ok(())
        })();
        let _ = done.send(());
        let _ = watchdog.join();
        result
    }
}
pub struct Decoded {
    pub pixels: Vec<u8>,
    pub time: i64,
    pub duration: i64,
}
/// Decoder surface layout. H.264 decoders announce their real surface (16-aligned height and
/// a display aperture) only after the first sample, via a current-media-type change.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Geometry {
    /// Surface size and default row pitch.
    fw: usize,
    fh: usize,
    stride: usize,
    /// Display aperture inside the surface.
    x: usize,
    y: usize,
    w: u32,
    h: u32,
}
impl Geometry {
    fn of(t: &IMFMediaType) -> Result<Self> {
        unsafe {
            let dim = t.GetUINT64(&MF_MT_FRAME_SIZE)?;
            let (fw, fh) = ((dim >> 32) as usize, (dim as u32) as usize);
            let stride = t
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .map(|s| s as i32)
                .ok()
                .filter(|s| *s >= fw as i32)
                .map_or(fw, |s| s as usize);
            let mut area = [0u8; std::mem::size_of::<MFVideoArea>()];
            let (mut x, mut y, mut w, mut h) = (0usize, 0usize, fw, fh);
            if t
                .GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, &mut area, None)
                .is_ok()
            {
                let i16_at = |o: usize| i16::from_le_bytes([area[o], area[o + 1]]);
                let i32_at = |o: usize| i32::from_le_bytes(area[o..o + 4].try_into().unwrap());
                let (ax, ay, aw, ah) = (i16_at(2), i16_at(6), i32_at(8), i32_at(12));
                if ax >= 0 && ay >= 0 && aw > 0 && ah > 0 {
                    (x, y, w, h) = (ax as usize, ay as usize, aw as usize, ah as usize);
                }
            }
            if x % 2 != 0 || y % 2 != 0 || w % 2 != 0 || h % 2 != 0 || x + w > fw || y + h > fh {
                return Err(error(format!(
                    "decoder aperture {x},{y} {w}x{h} outside surface {fw}x{fh}"
                )));
            }
            Ok(Self { fw, fh, stride, x, y, w: w as u32, h: h as u32 })
        }
    }
    /// Copies the display aperture of an NV12 surface into a tight w*h*3/2 frame.
    fn copy(&self, base: *const u8, pitch: usize, len: usize) -> Result<Vec<u8>> {
        let (w, h) = (self.w as usize, self.h as usize);
        if pitch < self.fw || len < pitch * self.fh * 3 / 2 {
            return Err(error("decoder NV12 buffer is too short"));
        }
        let mut pixels = vec![0; w * h * 3 / 2];
        unsafe {
            for row in 0..h {
                std::ptr::copy_nonoverlapping(
                    base.add((self.y + row) * pitch + self.x),
                    pixels.as_mut_ptr().add(row * w),
                    w,
                );
            }
            let uv = base.add(pitch * self.fh);
            for row in 0..h / 2 {
                std::ptr::copy_nonoverlapping(
                    uv.add((self.y / 2 + row) * pitch + self.x),
                    pixels.as_mut_ptr().add(w * h + row * w),
                    w,
                );
            }
        }
        Ok(pixels)
    }
}
pub struct Reader {
    reader: IMFSourceReader,
    pub w: u32,
    pub h: u32,
    geometry: Geometry,
    last: Option<i64>,
}
impl Reader {
    pub fn next_frame(&mut self) -> Result<Option<Decoded>> {
        loop {
            unsafe {
                let mut flags = 0;
                let mut time = 0;
                let mut sample = None;
                self.reader.ReadSample(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut time),
                    Some(&mut sample),
                )?;
                if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                    return Err(error("Media Foundation decoder reported a stream error"));
                }
                if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
                    // Expected once at the start: adopt the real surface, but the picture
                    // itself must not change size mid-stream.
                    let g = Geometry::of(
                        &self
                            .reader
                            .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)?,
                    )?;
                    if (g.w, g.h) != (self.w, self.h) {
                        return Err(error(format!(
                            "decoder picture changed from {}x{} to {}x{}",
                            self.w, self.h, g.w, g.h
                        )));
                    }
                    self.geometry = g;
                }
                if let Some(s) = sample {
                    if self.last.is_some_and(|t| time <= t) {
                        return Err(error("non-increasing decoded timestamps"));
                    }
                    self.last = Some(time);
                    let duration = s.GetSampleDuration().unwrap_or(333_333);
                    let b = s.ConvertToContiguousBuffer()?;
                    let pixels = if let Ok(two_d) = b.cast::<IMF2DBuffer2>() {
                        let (mut scan, mut pitch, mut start, mut len) =
                            (std::ptr::null_mut(), 0i32, std::ptr::null_mut(), 0u32);
                        two_d.Lock2DSize(
                            MF2DBuffer_LockFlags_Read,
                            &mut scan,
                            &mut pitch,
                            &mut start,
                            &mut len,
                        )?;
                        let available = (start as usize + len as usize).saturating_sub(scan as usize);
                        let r = if pitch <= 0 {
                            Err(error("bottom-up decoder surface"))
                        } else {
                            self.geometry.copy(scan, pitch as usize, available)
                        };
                        two_d.Unlock2D()?;
                        r?
                    } else {
                        let mut src = std::ptr::null_mut();
                        let mut len = 0;
                        b.Lock(&mut src, None, Some(&mut len))?;
                        let r = self.geometry.copy(src, self.geometry.stride, len as usize);
                        b.Unlock()?;
                        r?
                    };
                    return Ok(Some(Decoded {
                        pixels,
                        time,
                        duration,
                    }));
                }
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    return Ok(None);
                }
            }
        }
    }
}
/// Retry the entire decode once at the smaller bound; failures retain the finalized MP4.
pub fn export_gif(
    media: &Media,
    output: &record::Output,
) -> std::result::Result<(u64, u32), String> {
    let mut last = String::new();
    for (mw, mh) in [(960, 540), (640, 360)] {
        let attempt = (|| -> std::result::Result<(u64, u32), String> {
            let mut r = media.reader(&output.mp4).map_err(|e| e.to_string())?;
            let size = record::fit(r.w, r.h, mw, mh, false)?;
            let file = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&output.gif_part)
                .map_err(|e| e.to_string())?;
            let mut g =
                record::GifTimeline::new(record::GifStream::new(file, size, record::GIF_CAP)?);
            while let Some(frame) = r.next_frame().map_err(|e| e.to_string())? {
                let bgra = record::nv12_bgra(&frame.pixels, r.w, r.h)?;
                let px = record::crop_scale(
                    &bgra,
                    r.w,
                    r.h,
                    r.w as usize * 4,
                    crate::geometry::Rect::from_xywh(0, 0, r.w as i32, r.h as i32),
                    size,
                )?;
                g.push(frame.time, frame.duration, px)?;
            }
            let (file, _, frames) = g.finish()?;
            file.sync_all().map_err(|e| e.to_string())?;
            drop(file);
            let bytes = record::publish(&output.gif_part, &output.gif, record::GIF_CAP)
                .map_err(|e| e.to_string())?;
            Ok((bytes, frames))
        })();
        match attempt {
            Ok(v) => return Ok(v),
            Err(e) => {
                let retry = e.contains("byte cap");
                last = e;
                if !retry {
                    break;
                }
            }
        }
    }
    Err(last)
}

/// Optional-component failure injection without changing the host's installed components.
pub fn test_missing_components() -> std::result::Result<(), String> {
    unsafe {
        if LoadLibraryExW(
            windows::core::w!("snip-record-selftest-absent.dll"),
            None,
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
        .is_ok()
        {
            return Err("missing DLL unexpectedly loaded".into());
        }
        let kernel = LoadLibraryExW(
            windows::core::w!("kernel32.dll"),
            None,
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
        .map_err(|e| e.to_string())?;
        if GetProcAddress(kernel, windows::core::s!("SnipRecordSelftestAbsentEntry")).is_some() {
            return Err("missing symbol unexpectedly resolved".into());
        }
    }
    Ok(())
}
