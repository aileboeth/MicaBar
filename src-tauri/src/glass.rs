//! Live frosted-glass backdrop: captures the screen area behind the island window with DXGI
//! Desktop Duplication (our own window excluded), downsizes it on the GPU (mip level 2 = 1/4),
//! blurs it on the CPU and sends it to the page, which draws it under the island's glass tint.
//! Frames are only produced when something behind the island changed (max ~30 fps); capture is
//! released while a fullscreen app is in front or the window is hidden.

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::ipc::{Channel, InvokeResponseBody};
use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::*;

use crate::log;
use crate::window_ctl::WinState;

pub type FrameChannel = Arc<Mutex<Option<Channel<InvokeResponseBody>>>>;

/// Minimum time between frames, by resource profile (saver 10 fps, balanced 30, smooth 60).
fn frame_interval() -> Duration {
    Duration::from_millis(match crate::settings::str_at("general.profile", "balanced").as_str() {
        "saver" => 100,
        "smooth" => 16,
        _ => 33,
    })
}
const BLUR_RADIUS: usize = 7; // in quarter-resolution pixels, 3 box passes ≈ gaussian

/// Excludes a window from Desktop Duplication only (screenshots still include it).
/// Uses the undocumented user32!SetWindowCompositionAttribute(WCA_EXCLUDED_FROM_DDA = 24).
pub fn exclude_from_dda(hwnd: isize) -> bool {
    #[repr(C)]
    struct Data {
        attrib: u32,
        data: *mut c_void,
        size: usize,
    }
    type SetWca = unsafe extern "system" fn(isize, *mut Data) -> i32;
    unsafe {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        let user32 = GetModuleHandleW(crate::procinfo::wide("user32.dll").as_ptr());
        if user32.is_null() {
            return false;
        }
        let Some(f) = GetProcAddress(user32, b"SetWindowCompositionAttribute\0".as_ptr()) else { return false };
        let f: SetWca = std::mem::transmute(f);
        let mut on: i32 = 1;
        let mut d = Data { attrib: 24, data: &mut on as *mut i32 as *mut c_void, size: 4 };
        f(hwnd, &mut d) != 0
    }
}

struct Scaler {
    w: u32,
    h: u32,
    mip: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    staging: ID3D11Texture2D,
}

struct Capture {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    out: RECT,
    scaler: Option<Scaler>,
}

fn intersects(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

impl Capture {
    fn open(px: i32, py: i32) -> windows::core::Result<Capture> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut i = 0;
            while let Ok(adapter) = factory.EnumAdapters1(i) {
                i += 1;
                let mut j = 0;
                while let Ok(output) = adapter.EnumOutputs(j) {
                    j += 1;
                    let desc = output.GetDesc()?;
                    let r = desc.DesktopCoordinates;
                    if px < r.left || px >= r.right || py < r.top || py >= r.bottom {
                        continue;
                    }
                    let mut device = None;
                    let mut ctx = None;
                    D3D11CreateDevice(
                        &adapter,
                        D3D_DRIVER_TYPE_UNKNOWN,
                        HMODULE::default(),
                        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                        None,
                        D3D11_SDK_VERSION,
                        Some(&mut device),
                        None,
                        Some(&mut ctx),
                    )?;
                    let device = device.ok_or_else(|| windows::core::Error::from_hresult(windows::core::HRESULT(-1)))?;
                    let ctx = ctx.ok_or_else(|| windows::core::Error::from_hresult(windows::core::HRESULT(-1)))?;
                    let out1: IDXGIOutput1 = output.cast()?;
                    let dup = out1.DuplicateOutput(&device)?;
                    return Ok(Capture { device, ctx, dup, out: r, scaler: None });
                }
            }
            Err(windows::core::Error::from_hresult(DXGI_ERROR_NOT_FOUND))
        }
    }

    fn scaler(&mut self, w: u32, h: u32) -> windows::core::Result<&Scaler> {
        if self.scaler.as_ref().map(|s| s.w != w || s.h != h).unwrap_or(true) {
            unsafe {
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: w,
                    Height: h,
                    MipLevels: 3,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32,
                };
                let mut mip = None;
                self.device.CreateTexture2D(&desc, None, Some(&mut mip))?;
                let mip = mip.unwrap();
                let mut srv = None;
                self.device.CreateShaderResourceView(&mip, None, Some(&mut srv))?;
                let sdesc = D3D11_TEXTURE2D_DESC {
                    Width: (w >> 2).max(1),
                    Height: (h >> 2).max(1),
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut staging = None;
                self.device.CreateTexture2D(&sdesc, None, Some(&mut staging))?;
                self.scaler = Some(Scaler { w, h, mip, srv: srv.unwrap(), staging: staging.unwrap() });
            }
        }
        Ok(self.scaler.as_ref().unwrap())
    }

    /// Returns a quarter-size BGRA image of `region` (desktop coords) when the area `watch`
    /// changed since the last frame (or `force`).
    fn frame(&mut self, region: RECT, watch: RECT, force: bool) -> windows::core::Result<Option<(u32, u32, Vec<u8>)>> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            match self.dup.AcquireNextFrame(50, &mut info, &mut res) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
                Err(e) => return Err(e),
            }
            let out = self.read(&info, res, region, watch, force);
            let _ = self.dup.ReleaseFrame();
            out
        }
    }

    unsafe fn changed(&self, info: &DXGI_OUTDUPL_FRAME_INFO, watch: &RECT) -> bool {
        if info.LastPresentTime == 0 {
            return false; // pointer-only update
        }
        if info.TotalMetadataBufferSize == 0 {
            return true;
        }
        let o = &self.out;
        let local = RECT { left: watch.left - o.left, top: watch.top - o.top, right: watch.right - o.left, bottom: watch.bottom - o.top };
        let cap = info.TotalMetadataBufferSize as usize;
        let mut buf: Vec<u8> = vec![0; cap];
        let mut used = 0u32;
        if self.dup.GetFrameMoveRects(cap as u32, buf.as_mut_ptr() as *mut DXGI_OUTDUPL_MOVE_RECT, &mut used).is_ok() {
            let n = used as usize / std::mem::size_of::<DXGI_OUTDUPL_MOVE_RECT>();
            let mv = std::slice::from_raw_parts(buf.as_ptr() as *const DXGI_OUTDUPL_MOVE_RECT, n);
            if mv.iter().any(|m| intersects(&m.DestinationRect, &local)) {
                return true;
            }
        }
        let mut used = 0u32;
        if self.dup.GetFrameDirtyRects(cap as u32, buf.as_mut_ptr() as *mut RECT, &mut used).is_ok() {
            let n = used as usize / std::mem::size_of::<RECT>();
            let d = std::slice::from_raw_parts(buf.as_ptr() as *const RECT, n);
            return d.iter().any(|r| intersects(r, &local));
        }
        true
    }

    unsafe fn read(
        &mut self,
        info: &DXGI_OUTDUPL_FRAME_INFO,
        res: Option<IDXGIResource>,
        region: RECT,
        watch: RECT,
        force: bool,
    ) -> windows::core::Result<Option<(u32, u32, Vec<u8>)>> {
        if !(force && info.LastPresentTime != 0) && !self.changed(info, &watch) {
            return Ok(None);
        }
        let Some(res) = res else { return Ok(None) };
        let tex: ID3D11Texture2D = res.cast()?;
        let o = self.out;
        let l = region.left.max(o.left) - o.left;
        let t = region.top.max(o.top) - o.top;
        let r = region.right.min(o.right) - o.left;
        let b = region.bottom.min(o.bottom) - o.top;
        if r - l < 8 || b - t < 8 {
            return Ok(None);
        }
        let (w, h) = ((r - l) as u32, (b - t) as u32);
        let ctx = self.ctx.clone();
        let sc = self.scaler(w, h)?;
        let bx = D3D11_BOX { left: l as u32, top: t as u32, front: 0, right: r as u32, bottom: b as u32, back: 1 };
        ctx.CopySubresourceRegion(&sc.mip, 0, 0, 0, 0, &tex, 0, Some(&bx));
        ctx.GenerateMips(&sc.srv);
        ctx.CopySubresourceRegion(&sc.staging, 0, 0, 0, 0, &sc.mip, 2, None);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&sc.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let (qw, qh) = ((w >> 2).max(1) as usize, (h >> 2).max(1) as usize);
        let mut px = vec![0u8; qw * qh * 4];
        for y in 0..qh {
            let src = std::slice::from_raw_parts((mapped.pData as *const u8).add(y * mapped.RowPitch as usize), qw * 4);
            px[y * qw * 4..(y + 1) * qw * 4].copy_from_slice(src);
        }
        ctx.Unmap(&sc.staging, 0);
        Ok(Some((qw as u32, qh as u32, px)))
    }
}

/// Three box-blur passes over an interleaved 4-channel image (horizontal then vertical).
fn blur(px: &mut [u8], w: usize, h: usize, r: usize) {
    let mut tmp = vec![0u8; px.len()];
    for _ in 0..3 {
        box_pass(px, &mut tmp, w, h, r, true);
        box_pass(&tmp, px, w, h, r, false);
    }
}

fn box_pass(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, horizontal: bool) {
    let (lines, len) = if horizontal { (h, w) } else { (w, h) };
    let idx = |line: usize, i: usize| if horizontal { (line * w + i) * 4 } else { (i * w + line) * 4 };
    let span = (2 * r + 1) as u32;
    for line in 0..lines {
        for c in 0..3 {
            let mut acc: u32 = 0;
            for k in 0..=2 * r {
                let i = k.saturating_sub(r).min(len - 1);
                acc += src[idx(line, i) + c] as u32;
            }
            for i in 0..len {
                dst[idx(line, i) + c] = (acc / span) as u8;
                let out_i = i.saturating_sub(r).min(len - 1);
                let in_i = (i + r + 1).min(len - 1);
                acc = acc + src[idx(line, in_i) + c] as u32 - src[idx(line, out_i) + c] as u32;
            }
        }
    }
}

/// BGRA -> RGBA with a mild saturation boost, alpha forced opaque.
fn finish(px: &mut [u8]) {
    for p in px.chunks_exact_mut(4) {
        let (b, g, r) = (p[0] as f32, p[1] as f32, p[2] as f32);
        let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let s = 1.18;
        p[0] = (l + (r - l) * s).clamp(0.0, 255.0) as u8;
        p[1] = (l + (g - l) * s).clamp(0.0, 255.0) as u8;
        p[2] = (l + (b - l) * s).clamp(0.0, 255.0) as u8;
        p[3] = 255;
    }
}

pub fn glass_loop(ws: Arc<WinState>, chan: FrameChannel) {
    let mut cap: Option<Capture> = None;
    let mut last_sent = Instant::now() - frame_interval();
    let mut last_region = RECT::default();
    let mut fail_until: Option<Instant> = None;
    let mut logged_err = false;
    loop {
        let paused = ws.game.load(Ordering::SeqCst) || ws.hidden.load(Ordering::SeqCst) || chan.lock().unwrap().is_none();
        let hwnd = ws.hwnd.load(Ordering::SeqCst);
        if paused || hwnd == 0 {
            cap = None;
            std::thread::sleep(Duration::from_millis(300));
            continue;
        }
        if let Some(t) = fail_until {
            if Instant::now() < t {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            fail_until = None;
        }
        let mut wr = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd as _, &mut wr) };
        let region = RECT { left: wr.left, top: wr.top, right: wr.right, bottom: wr.bottom };
        let expanded = ws.expanded.load(Ordering::SeqCst);
        let hr = if expanded { *ws.panel.lock().unwrap() } else { *ws.pill.lock().unwrap() };
        let watch = RECT {
            left: wr.left + hr.x as i32 - 8,
            top: wr.top + hr.y as i32 - 8,
            right: wr.left + (hr.x + hr.w) as i32 + 8,
            bottom: wr.top + (hr.y + hr.h) as i32 + 8,
        };
        if cap.is_none() {
            match Capture::open((wr.left + wr.right) / 2, wr.top.max(0) + 4) {
                Ok(c) => {
                    log("glass: desktop duplication started");
                    logged_err = false;
                    cap = Some(c);
                    last_region = RECT::default();
                }
                Err(e) => {
                    if !logged_err {
                        log(format!("glass: duplication unavailable: {e}"));
                        logged_err = true;
                    }
                    fail_until = Some(Instant::now() + Duration::from_secs(3));
                    continue;
                }
            }
        }
        let force = region.left != last_region.left || region.top != last_region.top || region.right != last_region.right || region.bottom != last_region.bottom;
        if force && last_region.right != 0 {
            // window moved/resized: a new duplication delivers a full frame immediately
            cap = None;
            continue;
        }
        let c = cap.as_mut().unwrap();
        match c.frame(region, watch, force) {
            Ok(Some((w, h, mut px))) => {
                last_region = region;
                blur(&mut px, w as usize, h as usize, BLUR_RADIUS);
                finish(&mut px);
                let wait = frame_interval().saturating_sub(last_sent.elapsed());
                if !wait.is_zero() {
                    std::thread::sleep(wait);
                }
                let mut msg = Vec::with_capacity(8 + px.len());
                msg.extend_from_slice(&w.to_le_bytes());
                msg.extend_from_slice(&h.to_le_bytes());
                msg.extend_from_slice(&px);
                if let Some(ch) = chan.lock().unwrap().as_ref() {
                    let _ = ch.send(InvokeResponseBody::Raw(msg));
                }
                last_sent = Instant::now();
            }
            Ok(None) => {}
            Err(e) => {
                // DXGI_ERROR_ACCESS_LOST (mode change, UAC desktop, lock screen) and friends
                log(format!("glass: frame error {e}; reopening"));
                cap = None;
                fail_until = Some(Instant::now() + Duration::from_millis(800));
            }
        }
    }
}
