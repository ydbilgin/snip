//! PNG through WIC (built into Windows, no crate). 24-bit BGR, like a screenshot needs.
//! Call from a thread that has COM initialised (the save worker does it).

use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::StructuredStorage::{CreateStreamOnHGlobal, GetHGlobalFromStream};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IStream, STREAM_SEEK_END, STREAM_SEEK_SET};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::core::Result;

fn factory() -> Result<IWICImagingFactory> {
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
}

fn stream_bytes(stream: &IStream) -> Result<Vec<u8>> {
    unsafe {
        let mut size = 0u64;
        stream.Seek(0, STREAM_SEEK_END, Some(&mut size))?;
        let hg = GetHGlobalFromStream(stream)?;
        let p = GlobalLock(hg) as *const u8;
        if p.is_null() {
            return Err(windows::core::Error::from_thread());
        }
        let v = std::slice::from_raw_parts(p, size as usize).to_vec();
        let _ = GlobalUnlock(hg);
        Ok(v)
    }
}

/// Encode top-down BGRA (alpha ignored) as a 24-bit PNG.
pub fn encode_png(width: u32, height: u32, bgra: &[u8]) -> Result<Vec<u8>> {
    let bgr = crate::dib::bgra_to_bgr(bgra);
    unsafe {
        let f = factory()?;
        let stream = CreateStreamOnHGlobal(HGLOBAL::default(), true)?;
        let enc = f.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        let mut props = None;
        enc.CreateNewFrame(&mut frame, &mut props)?;
        let frame = frame.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
        frame.Initialize(props.as_ref())?;
        frame.SetSize(width, height)?;
        let mut fmt = GUID_WICPixelFormat24bppBGR;
        frame.SetPixelFormat(&mut fmt)?;
        if fmt != GUID_WICPixelFormat24bppBGR {
            return Err(windows::core::Error::new(windows::Win32::Foundation::E_FAIL, "PNG encoder refused 24bpp BGR"));
        }
        frame.WritePixels(height, width * 3, &bgr)?;
        frame.Commit()?;
        enc.Commit()?;
        stream_bytes(&stream)
    }
}

/// Encode top-down BGRA (alpha ignored) as a JPEG with full-resolution colour (4:4:4, no chroma
/// subsampling, so coloured text keeps its edges). `quality` 0.0 to 1.0; 0.95 measured visually
/// lossless on the user's game captures (PSNR >= 44 dB, SSIM >= 0.995) at a third of the PNG's size.
pub fn encode_jpeg(width: u32, height: u32, bgra: &[u8], quality: f32) -> Result<Vec<u8>> {
    use std::mem::ManuallyDrop;
    use windows::Win32::System::Com::StructuredStorage::PROPBAG2;
    use windows::Win32::System::Variant::{VARENUM, VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_R4, VT_UI1};
    use windows::core::{PWSTR, w};
    let variant = |vt: VARENUM, value: VARIANT_0_0_0| VARIANT { Anonymous: VARIANT_0 { Anonymous: ManuallyDrop::new(VARIANT_0_0 { vt, wReserved1: 0, wReserved2: 0, wReserved3: 0, Anonymous: value }) } };
    let bgr = crate::dib::bgra_to_bgr(bgra);
    unsafe {
        let f = factory()?;
        let stream = CreateStreamOnHGlobal(HGLOBAL::default(), true)?;
        let enc = f.CreateEncoder(&GUID_ContainerFormatJpeg, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        let mut props = None;
        enc.CreateNewFrame(&mut frame, &mut props)?;
        let frame = frame.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
        if let Some(bag) = props.as_ref() {
            let names = [w!("ImageQuality"), w!("JpegYCrCbSubsampling")];
            let opts = names.map(|n| PROPBAG2 { pstrName: PWSTR(n.as_ptr() as *mut _), ..Default::default() });
            // WICJpegYCrCbSubsampling444 = 3, a VT_UI1.
            let vals = [variant(VT_R4, VARIANT_0_0_0 { fltVal: quality.clamp(0.0, 1.0) }), variant(VT_UI1, VARIANT_0_0_0 { bVal: 3 })];
            bag.Write(2, opts.as_ptr(), vals.as_ptr())?;
        }
        frame.Initialize(props.as_ref())?;
        frame.SetSize(width, height)?;
        let mut fmt = GUID_WICPixelFormat24bppBGR;
        frame.SetPixelFormat(&mut fmt)?;
        if fmt != GUID_WICPixelFormat24bppBGR {
            return Err(windows::core::Error::new(windows::Win32::Foundation::E_FAIL, "JPEG encoder refused 24bpp BGR"));
        }
        frame.WritePixels(height, width * 3, &bgr)?;
        frame.Commit()?;
        enc.Commit()?;
        stream_bytes(&stream)
    }
}

/// Decode a PNG to top-down BGRA (for the self-test).
pub fn decode_png(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    unsafe {
        let f = factory()?;
        let stream = CreateStreamOnHGlobal(HGLOBAL::default(), true)?;
        let mut written = 0u32;
        stream.Write(bytes.as_ptr() as *const _, bytes.len() as u32, Some(&mut written)).ok()?;
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        let dec = f.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
        let frame = dec.GetFrame(0)?;
        let conv = WICConvertBitmapSource(&GUID_WICPixelFormat32bppBGRA, &frame)?;
        let (mut w, mut h) = (0u32, 0u32);
        conv.GetSize(&mut w, &mut h)?;
        // A damaged or hostile file can claim any size: refuse what does not fit in memory sanely.
        let stride = w.checked_mul(4).ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_INVALIDARG))?;
        let size = stride.checked_mul(h).filter(|&n| n <= 1 << 30).ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_INVALIDARG))?;
        let mut buf = vec![0u8; size as usize];
        conv.CopyPixels(std::ptr::null(), stride, &mut buf)?;
        Ok((w, h, buf))
    }
}
