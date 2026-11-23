//! Persistent cursor-enabled, bordered WGC monitor capture, exclusively for recording.
use crate::geometry::Rect;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{E_FAIL, E_POINTER, HMODULE};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{Interface, Result};
fn error(s: &str) -> windows::core::Error {
    windows::core::Error::new(E_FAIL, s)
}
pub struct Capture {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    item: GraphicsCaptureItem,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    staging: Option<ID3D11Texture2D>,
    monitor: Rect,
    crop: Rect,
    closed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    closed_token: i64,
}
impl Capture {
    pub fn new(monitor: Rect, crop: Rect) -> Result<Self> {
        unsafe {
            let h = crate::capture::monitors()
                .into_iter()
                .find(|m| m.rect == monitor)
                .ok_or_else(|| error("Recording monitor disconnected"))?
                .hmon;
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut adapter = None;
            'adapters: for i in 0.. {
                let Ok(a) = factory.EnumAdapters1(i) else {
                    break;
                };
                for j in 0.. {
                    let Ok(o) = a.EnumOutputs(j) else {
                        break;
                    };
                    let d = o.GetDesc()?;
                    if d.Monitor == h {
                        let desc = o.cast::<IDXGIOutput6>()?.GetDesc1()?;
                        if desc.ColorSpace != DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709
                            && desc.ColorSpace != DXGI_COLOR_SPACE_RGB_STUDIO_G22_NONE_P709
                        {
                            return Err(error(
                                "HDR recording is unavailable. Select an SDR monitor / HDR kaydı desteklenmiyor",
                            ));
                        }
                        adapter = Some(a);
                        break 'adapters;
                    }
                }
            }
            let adapter =
                adapter.ok_or_else(|| error("Cannot identify recording monitor/HDR state"))?;
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.ok_or_else(|| error("No D3D11 device"))?;
            let context = context.ok_or_else(|| error("No D3D11 context"))?;
            let dxgi: IDXGIDevice = device.cast()?;
            let d3d: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;
            let interop =
                windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
            let item: GraphicsCaptureItem = interop.CreateForMonitor(HMONITOR(h.0))?;
            let closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = closed.clone();
            let closed_token =
                item.Closed(&windows::Foundation::TypedEventHandler::new(move |_, _| {
                    flag.store(true, std::sync::atomic::Ordering::Release);
                    Ok(())
                }))?;
            let size = item.Size()?;
            if size.Width != monitor.width() || size.Height != monitor.height() {
                return Err(error("Recording monitor size changed"));
            }
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                &d3d,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                size,
            )?;
            let session = pool.CreateCaptureSession(&item)?;
            // Keep the OS border on. Never request borderless access.
            session.SetIsCursorCaptureEnabled(true)?;
            session.StartCapture()?;
            Ok(Self {
                pool,
                session,
                item,
                device,
                context,
                staging: None,
                monitor,
                crop: crop.offset(-monitor.left, -monitor.top),
                closed,
                closed_token,
            })
        }
    }
    pub fn frame(&mut self) -> Result<Option<Vec<u8>>> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(error("Recording stopped: capture item closed"));
        }
        let frame = match self.pool.TryGetNextFrame() {
            Ok(frame) => frame,
            Err(e) if e.code() == E_POINTER => {
                self.item.Size()?;
                unsafe {
                    self.device.GetDeviceRemovedReason()?;
                }
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let result = (|| unsafe {
            let size = frame.ContentSize()?;
            if size.Width != self.monitor.width() || size.Height != self.monitor.height() {
                return Err(error("Recording stopped: display size changed"));
            }
            let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
            let texture: ID3D11Texture2D = access.GetInterface()?;
            read_crop(
                &self.device,
                &self.context,
                &texture,
                self.crop,
                &mut self.staging,
            )
            .map(Some)
        })();
        let close = frame.Close();
        match result {
            Ok(v) => {
                close?;
                Ok(v)
            }
            Err(e) => Err(e),
        }
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.item.RemoveClosed(self.closed_token);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
/// Also used by the synthetic GPU acceptance test: the actual crop/readback boundary.
pub fn read_crop(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    texture: &ID3D11Texture2D,
    crop: Rect,
    staging: &mut Option<ID3D11Texture2D>,
) -> Result<Vec<u8>> {
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        if crop.left < 0
            || crop.top < 0
            || crop.right > desc.Width as i32
            || crop.bottom > desc.Height as i32
            || crop.is_empty()
        {
            return Err(error("Invalid GPU recording crop"));
        }
        if staging.is_none() {
            desc.Width = crop.width() as u32;
            desc.Height = crop.height() as u32;
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            desc.MipLevels = 1;
            desc.ArraySize = 1;
            device.CreateTexture2D(&desc, None, Some(staging))?;
        }
        let st = staging
            .as_ref()
            .ok_or_else(|| error("No staging texture"))?;
        let b = D3D11_BOX {
            left: crop.left as u32,
            top: crop.top as u32,
            front: 0,
            right: crop.right as u32,
            bottom: crop.bottom as u32,
            back: 1,
        };
        context.CopySubresourceRegion(st, 0, 0, 0, 0, texture, 0, Some(&b));
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(st, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let stride = crop.width() as usize * 4;
        let mut pixels = vec![0; stride * crop.height() as usize];
        for row in 0..crop.height() as usize {
            std::ptr::copy_nonoverlapping(
                (mapped.pData as *const u8).add(row * mapped.RowPitch as usize),
                pixels.as_mut_ptr().add(row * stride),
                stride,
            );
        }
        context.Unmap(st, 0);
        Ok(pixels)
    }
}
