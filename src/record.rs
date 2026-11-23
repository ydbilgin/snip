//! OS-free recording contract: physical crop, SDR conversion, timeline, bounded GIF and files.
use crate::geometry::{Point, Rect};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const FPS: u64 = 30;
pub const STOP_BYTES: u64 = 60 * 1024 * 1024;
pub const MP4_CAP: u64 = 64 * 1024 * 1024;
pub const GIF_CAP: u64 = 25 * 1024 * 1024;
pub const FIRST_FRAME_MS: u64 = 2000;
pub const FINALIZE_MS: u64 = 15_000;
pub fn timestamp(n: u64) -> i64 {
    (n * 10_000_000 / FPS) as i64
}
pub fn duration(n: u64) -> i64 {
    timestamp(n + 1) - timestamp(n)
}
pub fn bitrate(w: u32, h: u32) -> u32 {
    ((w as u64 * h as u64 * 3).clamp(2_000_000, 8_000_000)) as u32
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    #[default]
    Ready,
    Selecting,
    Starting,
    Recording,
    Finalizing,
    Error,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub phase: Phase,
    pub pid: u32,
    pub started_qpc: i64,
    pub elapsed_ms: u64,
    pub frames: u64,
    pub repeats: u64,
    pub bytes: u64,
    pub backend: String,
    pub output: String,
    pub error: String,
}
#[derive(Default, Debug)]
pub struct Control {
    pub phase: Phase,
    pub stop: bool,
}
impl Control {
    pub fn select(&mut self) -> bool {
        if !matches!(self.phase, Phase::Ready | Phase::Error) {
            return false;
        }
        self.phase = Phase::Selecting;
        self.stop = false;
        true
    }
    pub fn selected(&mut self) -> bool {
        if self.phase != Phase::Selecting || self.stop {
            return false;
        }
        self.phase = Phase::Starting;
        true
    }
    pub fn first_frame(&mut self) -> bool {
        if self.phase != Phase::Starting || self.stop {
            return false;
        }
        self.phase = Phase::Recording;
        true
    }
    pub fn request_stop(&mut self) {
        self.stop = true;
    }
    pub fn finalize(&mut self) {
        self.phase = Phase::Finalizing;
    }
    pub fn finish(&mut self, ok: bool) {
        self.phase = if ok { Phase::Ready } else { Phase::Error };
    }
}

/// The anchor monitor owns the entire selection, including window-click intersections.
pub fn selection(rect: Rect, anchor: Point, monitors: &[Rect]) -> Option<Rect> {
    let r = rect.intersect(monitors.iter().find(|m| m.contains(anchor))?)?;
    (r.width() >= 2 && r.height() >= 2).then_some(r)
}
/// All bars and the optional control are outside the captured pixels, clipped to the monitor.
pub fn indicator(rect: Rect, monitor: Rect) -> (Vec<Rect>, Option<Rect>) {
    let bars = [
        Rect::new(rect.left - 2, rect.top - 2, rect.right + 2, rect.top),
        Rect::new(rect.left - 2, rect.bottom, rect.right + 2, rect.bottom + 2),
        Rect::new(rect.left - 2, rect.top, rect.left, rect.bottom),
        Rect::new(rect.right, rect.top, rect.right + 2, rect.bottom),
    ]
    .iter()
    .filter_map(|r| r.intersect(&monitor))
    .collect();
    let panel = [
        Rect::from_xywh(rect.left, rect.top - 32, 180, 30),
        Rect::from_xywh(rect.left, rect.bottom + 2, 180, 30),
    ]
    .into_iter()
    .find(|p| p.intersect(&monitor) == Some(*p) && p.intersect(&rect).is_none());
    (bars, panel)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub w: u32,
    pub h: u32,
    pub content_w: u32,
    pub content_h: u32,
}
pub fn fit(w: u32, h: u32, max_w: u32, max_h: u32, even: bool) -> Result<Size, String> {
    if w < 2 || h < 2 || max_w < 2 || max_h < 2 {
        return Err("recording area must be at least 2x2".into());
    }
    let scale = (max_w as f64 / w as f64)
        .min(max_h as f64 / h as f64)
        .min(1.0);
    let cw = ((w as f64 * scale).floor() as u32).max(1);
    let ch = ((h as f64 * scale).floor() as u32).max(1);
    Ok(Size {
        w: if even { (cw + 1) & !1 } else { cw },
        h: if even { (ch + 1) & !1 } else { ch },
        content_w: cw,
        content_h: ch,
    })
}
/// BGRA source with arbitrary positive stride, crop in source coordinates, nearest pixel scaling.
/// Padding repeats the last selected edge; it never exposes neighbouring screen pixels.
pub fn crop_scale(
    src: &[u8],
    w: u32,
    h: u32,
    stride: usize,
    crop: Rect,
    size: Size,
) -> Result<Vec<u8>, String> {
    if stride < w as usize * 4
        || src.len() < stride.checked_mul(h as usize).ok_or("frame overflow")?
        || crop.is_empty()
        || crop.left < 0
        || crop.top < 0
        || crop.right > w as i32
        || crop.bottom > h as i32
        || size.content_w == 0
        || size.content_h == 0
        || size.w < size.content_w
        || size.h < size.content_h
    {
        return Err("invalid recording frame/crop/stride".into());
    }
    let len = (size.w as usize)
        .checked_mul(size.h as usize)
        .and_then(|v| v.checked_mul(4))
        .ok_or("frame overflow")?;
    let mut out = vec![0; len];
    for y in 0..size.h {
        let sy = crop.top as usize
            + (y.min(size.content_h - 1) as u64 * crop.height() as u64 / size.content_h as u64)
                as usize;
        for x in 0..size.w {
            let sx = crop.left as usize
                + (x.min(size.content_w - 1) as u64 * crop.width() as u64 / size.content_w as u64)
                    as usize;
            let at = sy * stride + sx * 4;
            let to = (y as usize * size.w as usize + x as usize) * 4;
            out[to..to + 4].copy_from_slice(&src[at..at + 4]);
        }
    }
    Ok(out)
}
fn clamp(v: f64) -> u8 {
    v.round().clamp(0., 255.) as u8
}
/// BT.709 limited-range NV12, top down, 2x2 chroma average. All arithmetic is explicit.
pub fn nv12(bgra: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    if w == 0
        || h == 0
        || !w.is_multiple_of(2)
        || !h.is_multiple_of(2)
        || bgra.len() != w as usize * h as usize * 4
    {
        return Err("NV12 requires even BGRA dimensions".into());
    }
    let n = w as usize * h as usize;
    let mut out = vec![0; n + n / 2];
    for (i, p) in bgra.as_chunks::<4>().0.iter().enumerate() {
        out[i] =
            clamp(16. + 0.182586 * p[2] as f64 + 0.614231 * p[1] as f64 + 0.062007 * p[0] as f64);
    }
    for y in (0..h as usize).step_by(2) {
        for x in (0..w as usize).step_by(2) {
            let (mut b, mut g, mut r) = (0., 0., 0.);
            for dy in 0..2 {
                for dx in 0..2 {
                    let p = &bgra[((y + dy) * w as usize + x + dx) * 4..];
                    b += p[0] as f64 / 4.;
                    g += p[1] as f64 / 4.;
                    r += p[2] as f64 / 4.;
                }
            }
            let at = n + (y / 2) * w as usize + x;
            out[at] = clamp(128. - 0.100644 * r - 0.338572 * g + 0.439216 * b);
            out[at + 1] = clamp(128. + 0.439216 * r - 0.398942 * g - 0.040274 * b);
        }
    }
    Ok(out)
}
pub fn nv12_bgra(src: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let n = w as usize * h as usize;
    if w == 0 || h == 0 || !w.is_multiple_of(2) || !h.is_multiple_of(2) || src.len() != n + n / 2 {
        return Err("invalid NV12 frame".into());
    }
    let mut out = vec![0; n * 4];
    for y in 0..h as usize {
        for x in 0..w as usize {
            let yy = (src[y * w as usize + x] as f64 - 16.) * 255. / 219.;
            let uv = n + (y / 2) * w as usize + (x & !1);
            let u = (src[uv] as f64 - 128.) * 255. / 224.;
            let v = (src[uv + 1] as f64 - 128.) * 255. / 224.;
            let at = (y * w as usize + x) * 4;
            out[at..at + 4].copy_from_slice(&[
                clamp(yy + 1.8556 * u),
                clamp(yy - 0.187324 * u - 0.468124 * v),
                clamp(yy + 1.5748 * v),
                255,
            ]);
        }
    }
    Ok(out)
}

/// Fails before an over-cap write, including GIF trailer/flush; no silent truncation.
pub struct Limited<W> {
    pub inner: W,
    pub bytes: u64,
    pub cap: u64,
}
impl<W: Write> Write for Limited<W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if b.len() as u64 > self.cap.saturating_sub(self.bytes) {
            return Err(io::Error::other("recording byte cap exceeded"));
        }
        let n = self.inner.write(b)?;
        self.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
/// One pending RGB image: identical frames extend its delay, never its memory footprint.
pub struct GifStream<W: Write> {
    encoder: gif::Encoder<Limited<W>>,
    size: Size,
    pending: Option<(Vec<u8>, u16)>,
    pub frames: u32,
}
impl<W: Write> GifStream<W> {
    pub fn new(out: W, size: Size, cap: u64) -> Result<Self, String> {
        if size.w > u16::MAX as u32 || size.h > u16::MAX as u32 {
            return Err("GIF dimensions overflow".into());
        }
        let mut encoder = gif::Encoder::new(
            Limited {
                inner: out,
                bytes: 0,
                cap,
            },
            size.w as u16,
            size.h as u16,
            &[],
        )
        .map_err(|e| e.to_string())?;
        encoder
            .set_repeat(gif::Repeat::Infinite)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            encoder,
            size,
            pending: None,
            frames: 0,
        })
    }
    pub fn push(&mut self, bgra: &[u8], cs: u16) -> Result<(), String> {
        if bgra.len() != self.size.w as usize * self.size.h as usize * 4 || cs == 0 {
            return Err("invalid GIF frame/delay".into());
        }
        let rgb: Vec<u8> = bgra
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0]])
            .collect();
        if let Some((p, d)) = &mut self.pending
            && same_picture(p, &rgb)
            && let Some(delay) = d.checked_add(cs)
        {
            *d = delay;
            return Ok(());
        }
        self.emit()?;
        self.pending = Some((rgb, cs));
        Ok(())
    }
    fn emit(&mut self) -> Result<(), String> {
        if let Some((rgb, delay)) = self.pending.take() {
            let mut frame =
                gif::Frame::from_rgb_speed(self.size.w as u16, self.size.h as u16, &rgb, 10);
            frame.delay = delay;
            frame.dispose = gif::DisposalMethod::Keep;
            self.encoder
                .write_frame(&frame)
                .map_err(|e| e.to_string())?;
            self.frames += 1;
        }
        Ok(())
    }
    pub fn finish(mut self) -> Result<(W, u64, u32), String> {
        self.emit()?;
        let mut out = self.encoder.into_inner().map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
        Ok((out.inner, out.bytes, self.frames))
    }
}

/// Sample-and-hold on presentation timestamps. One pending output lets the final delay be rounded
/// once (to 1 cs) at EOF, even when duration is not a multiple of 100 ms.
/// Decoded H.264 frames of an unchanged screen differ by a few levels (measured up to 2/255 on
/// Windows 2026-11-17, at keyframes); a real change moves some channel by much more.
pub const GIF_SAME_TOLERANCE: u8 = 8;
fn same_picture(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.abs_diff(*y) <= GIF_SAME_TOLERANCE)
}

pub struct GifTimeline<W: Write> {
    stream: GifStream<W>,
    last: Option<(Vec<u8>, i64)>,
    held: Option<(Vec<u8>, i64)>,
    next: i64,
    end: i64,
}
impl<W: Write> GifTimeline<W> {
    pub fn new(stream: GifStream<W>) -> Self {
        Self {
            stream,
            last: None,
            held: None,
            next: 0,
            end: 0,
        }
    }
    fn sample(&mut self, pixels: &[u8]) -> Result<(), String> {
        if let Some((held, at)) = self.held.take() {
            self.stream
                .push(&held, ((self.next - at) / 100_000) as u16)?;
        }
        self.held = Some((pixels.to_vec(), self.next));
        self.next += 1_000_000;
        Ok(())
    }
    pub fn push(&mut self, time: i64, duration: i64, pixels: Vec<u8>) -> Result<(), String> {
        if time < 0 || duration <= 0 || self.last.as_ref().is_some_and(|(_, t)| time <= *t) {
            return Err("invalid GIF presentation timeline".into());
        }
        if let Some((previous, t)) = self.last.take() {
            while self.next < time {
                self.sample(&previous)?;
            }
            self.last = Some((previous, t));
        }
        self.last = Some((pixels, time));
        self.end = time + duration;
        Ok(())
    }
    pub fn finish(mut self) -> Result<(W, u64, u32), String> {
        let (last, _) = self.last.take().ok_or("empty GIF timeline")?;
        while self.next < self.end {
            self.sample(&last)?;
        }
        if let Some((held, at)) = self.held.take() {
            self.stream
                .push(&held, ((self.end - at + 50_000) / 100_000).max(1) as u16)?;
        }
        self.stream.finish()
    }
}

pub struct Output {
    pub mp4: PathBuf,
    pub gif: PathBuf,
    pub part: PathBuf,
    pub gif_part: PathBuf,
}
impl Output {
    /// Reserve both extensions before recording. Failed artifacts remain available for diagnosis.
    pub fn reserve(dir: &Path, stamp: crate::naming::Stamp) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        for i in 0..10_000 {
            let suffix = if i == 0 {
                String::new()
            } else {
                format!(" ({})", i + 1)
            };
            let base = format!(
                "Ekran kaydı {:04}-{:02}-{:02} {:02}{:02}{:02}{suffix}",
                stamp.year, stamp.month, stamp.day, stamp.hour, stamp.minute, stamp.second
            );
            let mp4 = dir.join(format!("{base}.mp4"));
            let gif = dir.join(format!("{base}.gif"));
            let part = dir.join(format!("{base}.mp4.part"));
            let gif_part = dir.join(format!("{base}.gif.part"));
            if mp4.exists() || gif.exists() || gif_part.exists() {
                continue;
            }
            match OpenOptions::new().write(true).create_new(true).open(&part) {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&gif_part)
            {
                Ok(_) => {
                    return Ok(Self {
                        mp4,
                        gif,
                        part,
                        gif_part,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("recording filename space exhausted"))
    }
}
/// Never replaces another output. Only call after the media writer successfully finalized.
pub fn publish(part: &Path, dest: &Path, cap: u64) -> io::Result<u64> {
    let f = OpenOptions::new().read(true).write(true).open(part)?;
    let bytes = f.metadata()?.len();
    if bytes == 0 || bytes > cap {
        return Err(io::Error::other(
            "final recording is empty or exceeds byte cap",
        ));
    }
    f.sync_all()?;
    drop(f);
    crate::naming::rename_new(part, dest)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gif_coalesces_decode_noise_but_not_changes() {
        let a = vec![0u8; 12];
        let mut b = a.clone();
        b[5] = GIF_SAME_TOLERANCE;
        assert!(same_picture(&a, &b));
        b[5] = GIF_SAME_TOLERANCE + 1;
        assert!(!same_picture(&a, &b));
        assert!(!same_picture(&a, &a[..9]));
    }
    #[test]
    fn timeline_never_accumulates_rounding() {
        assert_eq!(timestamp(90), 30_000_000);
        assert_eq!((0..9000).map(duration).sum::<i64>(), timestamp(9000));
        assert!(duration(1) > 0);
    }
    #[test]
    fn sizes_limits_and_portrait() {
        assert_eq!(
            fit(641, 359, 1920, 1080, true).unwrap(),
            Size {
                w: 642,
                h: 360,
                content_w: 641,
                content_h: 359
            }
        );
        for (w, h) in [(7680, 4320), (1080, 1920), (2, 9000), (1921, 1080)] {
            let s = fit(w, h, 1920, 1080, true).unwrap();
            assert!(s.w <= 1920 && s.h <= 1080 && s.w.is_multiple_of(2) && s.h.is_multiple_of(2));
        }
        assert!(fit(1, 5, 1920, 1080, true).is_err());
        assert_eq!(bitrate(1920, 1080), 6_220_800);
    }
    #[test]
    fn padded_stride_crop_and_odd_edges() {
        let src = [
            10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 0, 0, 0, 0, 11, 21, 31, 255, 41, 51,
            61, 255, 71, 81, 91, 255, 0, 0, 0, 0,
        ];
        let s = Size {
            w: 4,
            h: 2,
            content_w: 3,
            content_h: 2,
        };
        let out = crop_scale(&src, 3, 2, 16, Rect::from_xywh(0, 0, 3, 2), s).unwrap();
        assert_eq!(&out[8..12], &out[12..16]);
        assert_eq!(&out[24..28], &out[28..32]);
        assert_eq!(out[16], 11);
        assert!(crop_scale(&src, 3, 2, 11, Rect::from_xywh(0, 0, 3, 2), s).is_err());
    }
    #[test]
    fn nv12_vectors_orientation_and_rgb_order() {
        for (p, y, u, v) in [
            ([0, 0, 0, 255], 16, 128, 128),
            ([255, 255, 255, 255], 235, 128, 128),
            ([0, 0, 255, 255], 63, 102, 240),
            ([255, 0, 0, 255], 32, 240, 118),
        ] {
            let buf = p.repeat(4);
            let n = nv12(&buf, 2, 2).unwrap();
            assert_eq!(&n[..4], &[y; 4]);
            assert_eq!(&n[4..], &[u, v]);
            let b = nv12_bgra(&n, 2, 2).unwrap();
            for i in 0..3 {
                assert!((b[i] as i32 - p[i] as i32).abs() <= 2);
            }
        }
        let mixed = [[0, 0, 0, 255]; 4]
            .concat()
            .into_iter()
            .chain([[255, 255, 255, 255]; 4].concat())
            .collect::<Vec<_>>();
        let n = nv12(&mixed, 2, 4).unwrap();
        assert_eq!(&n[..4], &[16; 4]);
        assert_eq!(&n[4..8], &[235; 4]);
        let averaged = nv12(
            &[
                [0, 0, 255, 255],
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [255, 255, 255, 255],
            ]
            .concat(),
            2,
            2,
        )
        .unwrap();
        assert_eq!(&averaged[4..], &[128, 128]);
    }
    #[test]
    fn selection_and_indicator_across_negative_monitors() {
        let monitors = [
            Rect::from_xywh(-1920, 218, 1920, 1080),
            Rect::from_xywh(0, 0, 3840, 2160),
            Rect::from_xywh(-320, -1440, 2560, 1440),
        ];
        assert_eq!(
            selection(
                Rect::from_xywh(-100, 500, 500, 300),
                Point::new(-50, 600),
                &monitors
            ),
            Some(Rect::from_xywh(-100, 500, 100, 300))
        );
        for m in monitors {
            for r in [m, Rect::from_xywh(m.left + 50, m.top + 50, 641, 359)] {
                let (bars, p) = indicator(r, m);
                for b in bars.into_iter().chain(p) {
                    assert!(b.intersect(&r).is_none());
                    assert_eq!(b.intersect(&m), Some(b));
                }
            }
        }
    }
    #[test]
    fn old_recorder_state_defaults_new_fields() {
        let s: State = serde_json::from_str(r#"{"phase":"recording","pid":42}"#).unwrap();
        assert_eq!(s.phase, Phase::Recording);
        assert_eq!(s.frames, 0);
        assert_eq!(s.started_qpc, 0);
    }
    #[test]
    fn stop_before_ready_and_duplicates() {
        let mut c = Control::default();
        assert!(c.select());
        assert!(!c.select());
        c.request_stop();
        assert!(!c.selected());
        c.finish(true);
        assert!(c.select());
        assert!(c.selected());
        assert!(c.first_frame());
        c.request_stop();
        c.finalize();
        c.request_stop();
        assert_eq!(c.phase, Phase::Finalizing);
        assert!(!c.select());
    }
    #[test]
    fn gif_roundtrip_coalesces_and_preserves_duration() {
        let s = fit(8, 6, 960, 540, false).unwrap();
        let mut g = GifStream::new(Vec::new(), s, 1_000_000).unwrap();
        for i in 0..30 {
            g.push(&[if i < 15 { 0 } else { 255 }, 0, 0, 255].repeat(48), 10)
                .unwrap();
        }
        let (bytes, _, count) = g.finish().unwrap();
        assert_eq!(count, 2);
        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::RGBA);
        let mut r = opts.read_info(bytes.as_slice()).unwrap();
        assert_eq!(r.repeat(), gif::Repeat::Infinite);
        assert_eq!((r.width(), r.height()), (8, 6));
        let mut delays = 0;
        let mut frames = 0;
        while let Some(f) = r.read_next_frame().unwrap() {
            assert_eq!(f.dispose, gif::DisposalMethod::Keep);
            delays += f.delay as u32;
            frames += 1;
        }
        assert_eq!((frames, delays), (2, 300));
    }
    #[test]
    fn gif_timeline_is_10hz_and_quantizes_only_the_tail() {
        for end in [30_000_000, 30_140_000] {
            let size = fit(4, 4, 960, 540, false).unwrap();
            let mut g = GifTimeline::new(GifStream::new(Vec::new(), size, 1_000_000).unwrap());
            for n in 0..90 {
                g.push(
                    timestamp(n),
                    if n == 89 {
                        end - timestamp(n)
                    } else {
                        duration(n)
                    },
                    // Steps larger than GIF_SAME_TOLERANCE: every 10 Hz sample is a real change.
                    [(n as u8).wrapping_mul(10), 0, 0, 255].repeat(16),
                )
                .unwrap();
            }
            let (b, _, _) = g.finish().unwrap();
            let mut d = gif::DecodeOptions::new().read_info(b.as_slice()).unwrap();
            let mut count = 0;
            let mut total = 0;
            while let Some(f) = d.read_next_frame().unwrap() {
                total += f.delay as i64;
                count += 1;
            }
            assert_eq!(count, if end == 30_000_000 { 30 } else { 31 });
            assert_eq!(total, (end + 50_000) / 100_000);
        }
    }
    #[test]
    fn gif_caps_and_trailer_errors_propagate() {
        let s = fit(2, 2, 960, 540, false).unwrap();
        assert!(GifStream::new(Vec::new(), s, 1).is_err());
        let mut g = GifStream::new(Vec::new(), s, 40).unwrap();
        g.push(&[0, 0, 255, 255].repeat(4), 10).unwrap();
        assert!(g.finish().is_err());
    }
    #[test]
    fn publication_never_replaces_and_rejects_over_cap() {
        let dir = std::env::temp_dir().join(format!("snip-record-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("owned.part");
        let d = dir.join("owned.mp4");
        std::fs::write(&p, b"movie").unwrap();
        std::fs::write(&d, b"previous").unwrap();
        assert!(publish(&p, &d, 64).is_err());
        assert_eq!(std::fs::read(&d).unwrap(), b"previous");
        assert!(publish(&p, &dir.join("cap.mp4"), 4).is_err());
        std::fs::remove_file(&d).unwrap();
        assert_eq!(publish(&p, &d, 64).unwrap(), 5);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
