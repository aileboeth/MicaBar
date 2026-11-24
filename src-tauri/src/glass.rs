//! Live frosted-glass backdrop: captures the screen area behind the island window with DXGI
//! Desktop Duplication (our own window excluded), downsizes it on the GPU (mip level 2 = 1/4),
//! blurs it on the CPU and sends it to the page, which draws it under the island's glass tint.
//! Frames are only produced when something behind the island changed (max ~30 fps); capture is
//! released while a fullscreen app is in front or the window is hidden.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::ipc::{Channel, InvokeResponseBody};
use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::*;

use crate::log;
use crate::window_ctl::WinState;

pub type FrameChannel = Arc<Mutex<Option<Channel<InvokeResponseBody>>>>;

static FORCE_FRAME: AtomicBool = AtomicBool::new(false);
static REOPEN_NOW: AtomicBool = AtomicBool::new(false);

/// Pulses an immediate backdrop frame (e.g. when a new WebView channel connects or settings change).
pub fn request_frame() {
    FORCE_FRAME.store(true, Ordering::SeqCst);
}

/// Clears any retry backoff and forces a fresh duplication frame (sleep resume, display/DPI change).
pub fn notify_display_or_resume() {
    REOPEN_NOW.store(true, Ordering::SeqCst);
    FORCE_FRAME.store(true, Ordering::SeqCst);
}

#[allow(dead_code)]
pub fn notify_channel_ready() {
    request_frame();
}

pub fn set_channel(slot: &FrameChannel, ch: Option<Channel<InvokeResponseBody>>) {
    let has = ch.is_some();
    *slot.lock().unwrap() = ch;
    if has {
        request_frame();
    }
}

/// Minimum time between frames, by resource profile (saver 5 fps, balanced 15, smooth 30): the
/// backdrop is heavily blurred, more frames only cost CPU / GPU (measured 2026-11-22: 30 fps of a
/// window-sized frame took ~60% of a core across MicaBar and WebView2).
fn frame_interval() -> Duration {
    Duration::from_millis(match crate::settings::hot().profile {
        0 => 200,
        2 => 33,
        _ => 66,
    })
}

/// Physical px around the island that are captured too: the blur (3 box passes of BLUR_RADIUS
/// quarter-px) reaches that far, so the visible part has no clamped edge.
const CROP_MARGIN: i32 = 3 * BLUR_RADIUS as i32 * 4;
/// The captured area snaps to this grid (window-relative), so small size changes of the pill
/// don't restart the capture.
const CROP_GRID: i32 = 128;

/// The part of the window worth capturing: the island (pill, card or panel) plus the blur margin,
/// on the grid, inside the window and the monitor.
fn crop_rect(win: &RECT, island: &crate::window_ctl::Rect, out: &RECT) -> RECT {
    let lo = |v: i32| v.div_euclid(CROP_GRID) * CROP_GRID;
    let hi = |v: i32| (v + CROP_GRID - 1).div_euclid(CROP_GRID) * CROP_GRID;
    let l = lo(island.x as i32 - CROP_MARGIN).max(0);
    let t = lo(island.y as i32 - CROP_MARGIN).max(0);
    let r = hi((island.x + island.w) as i32 + CROP_MARGIN);
    let b = hi((island.y + island.h) as i32 + CROP_MARGIN);
    RECT {
        left: (win.left + l).max(out.left),
        top: (win.top + t).max(out.top),
        right: (win.left + r).min(win.right).min(out.right),
        bottom: (win.top + b).min(win.bottom).min(out.bottom),
    }
}

/// Mean difference per channel x100 between the new frame and the last one sent, over the part
/// the island covers (`area`: x0, y0, x1, y1 in quarter px of the piece; the blur margin around
/// it doesn't count), every other pixel. Under 100 (one level) the frame is skipped: a blinking
/// caret or a ticking clock behind the blur isn't worth a frame.
fn diff100(a: &[u8], b: &[u8], w: usize, area: (usize, usize, usize, usize)) -> u64 {
    if a.len() != b.len() || a.is_empty() || w == 0 {
        return u64::MAX;
    }
    let h = a.len() / 4 / w;
    let (x0, y0) = (area.0.min(w), area.1.min(h));
    let (x1, y1) = (area.2.min(w).max(x0), area.3.min(h).max(y0));
    let (mut sum, mut n) = (0u64, 0u64);
    for y in (y0..y1).step_by(2) {
        for x in (x0..x1).step_by(2) {
            let i = (y * w + x) * 4;
            for c in 0..3 {
                sum += (a[i + c] as i32 - b[i + c] as i32).unsigned_abs() as u64;
            }
            n += 3;
        }
    }
    if n == 0 {
        return 0;
    }
    sum * 100 / n
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

struct CachedDesktop {
    w: u32,
    h: u32,
    fmt: DXGI_FORMAT,
    tex: ID3D11Texture2D,
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
    cached: Option<CachedDesktop>,
    scaler: Option<Scaler>,
    meta_buf: Vec<u8>,
}

fn intersects(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

fn same_rect(a: &RECT, b: &RECT) -> bool {
    a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
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
                    return Ok(Capture {
                        device,
                        ctx,
                        dup,
                        out: r,
                        cached: None,
                        scaler: None,
                        meta_buf: Vec::new(),
                    });
                }
            }
            Err(windows::core::Error::from_hresult(DXGI_ERROR_NOT_FOUND))
        }
    }

    fn contains_point(&self, px: i32, py: i32) -> bool {
        px >= self.out.left && px < self.out.right && py >= self.out.top && py < self.out.bottom
    }

    unsafe fn update_cache(&mut self, src: &ID3D11Texture2D) -> windows::core::Result<()> {
        let mut td = D3D11_TEXTURE2D_DESC::default();
        src.GetDesc(&mut td);
        let recreate = self
            .cached
            .as_ref()
            .map(|c| c.w != td.Width || c.h != td.Height || c.fmt != td.Format)
            .unwrap_or(true);
        if recreate {
            let cdesc = D3D11_TEXTURE2D_DESC {
                Width: td.Width,
                Height: td.Height,
                MipLevels: 1,
                ArraySize: 1,
                Format: td.Format,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: 0,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut tex = None;
            self.device.CreateTexture2D(&cdesc, None, Some(&mut tex))?;
            self.cached = Some(CachedDesktop { w: td.Width, h: td.Height, fmt: td.Format, tex: tex.unwrap() });
        }
        let dst = &self.cached.as_ref().unwrap().tex;
        self.ctx.CopySubresourceRegion(dst, 0, 0, 0, 0, src, 0, None);
        Ok(())
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

    unsafe fn changed(&mut self, info: &DXGI_OUTDUPL_FRAME_INFO, watch: &RECT) -> bool {
        if info.LastPresentTime == 0 {
            return false; // pointer-only update
        }
        if info.TotalMetadataBufferSize == 0 {
            return true;
        }
        let o = self.out;
        let local = RECT { left: watch.left - o.left, top: watch.top - o.top, right: watch.right - o.left, bottom: watch.bottom - o.top };
        let cap = info.TotalMetadataBufferSize as usize;
        if self.meta_buf.len() < cap {
            self.meta_buf.resize(cap, 0);
        }
        let mut used = 0u32;
        if self
            .dup
            .GetFrameMoveRects(cap as u32, self.meta_buf.as_mut_ptr() as *mut DXGI_OUTDUPL_MOVE_RECT, &mut used)
            .is_ok()
        {
            let n = used as usize / std::mem::size_of::<DXGI_OUTDUPL_MOVE_RECT>();
            let mv = std::slice::from_raw_parts(self.meta_buf.as_ptr() as *const DXGI_OUTDUPL_MOVE_RECT, n);
            if mv.iter().any(|m| intersects(&m.DestinationRect, &local)) {
                return true;
            }
        }
        let mut used = 0u32;
        if self
            .dup
            .GetFrameDirtyRects(cap as u32, self.meta_buf.as_mut_ptr() as *mut RECT, &mut used)
            .is_ok()
        {
            let n = used as usize / std::mem::size_of::<RECT>();
            let d = std::slice::from_raw_parts(self.meta_buf.as_ptr() as *const RECT, n);
            return d.iter().any(|r| intersects(r, &local));
        }
        true
    }

    unsafe fn read_from_cached(&mut self, region: RECT, out_px: &mut Vec<u8>) -> windows::core::Result<Option<(u32, u32)>> {
        let Some((cached_tex, cw, ch)) = self.cached.as_ref().map(|c| (c.tex.clone(), c.w as i32, c.h as i32)) else {
            return Ok(None);
        };
        let o = self.out;
        let l = (region.left.max(o.left) - o.left).clamp(0, cw);
        let t = (region.top.max(o.top) - o.top).clamp(0, ch);
        let r = (region.right.min(o.right) - o.left).clamp(0, cw);
        let b = (region.bottom.min(o.bottom) - o.top).clamp(0, ch);
        if r - l < 8 || b - t < 8 {
            return Ok(None);
        }
        let (w, h) = ((r - l) as u32, (b - t) as u32);
        let ctx = self.ctx.clone();
        let sc = self.scaler(w, h)?;
        let bx = D3D11_BOX { left: l as u32, top: t as u32, front: 0, right: r as u32, bottom: b as u32, back: 1 };
        ctx.CopySubresourceRegion(&sc.mip, 0, 0, 0, 0, &cached_tex, 0, Some(&bx));
        ctx.GenerateMips(&sc.srv);
        ctx.CopySubresourceRegion(&sc.staging, 0, 0, 0, 0, &sc.mip, 2, None);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&sc.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let (qw, qh) = ((w >> 2).max(1) as usize, (h >> 2).max(1) as usize);
        out_px.resize(qw * qh * 4, 0);
        for y in 0..qh {
            let src = std::slice::from_raw_parts((mapped.pData as *const u8).add(y * mapped.RowPitch as usize), qw * 4);
            out_px[y * qw * 4..(y + 1) * qw * 4].copy_from_slice(src);
        }
        ctx.Unmap(&sc.staging, 0);
        Ok(Some((qw as u32, qh as u32)))
    }

    /// Writes a quarter-size BGRA image of `region` (desktop coords) into `out_px` when `watch`
    /// changed since the last frame, or immediately from `cached_tex` when `immediate` is set.
    fn frame(&mut self, region: RECT, watch: RECT, immediate: bool, out_px: &mut Vec<u8>) -> windows::core::Result<Option<(u32, u32)>> {
        unsafe {
            let timeout_ms = if immediate && self.cached.is_some() { 0 } else { 50 };
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            let mut watch_dirty = false;
            match self.dup.AcquireNextFrame(timeout_ms, &mut info, &mut res) {
                Ok(()) => {
                    let step = (|| -> windows::core::Result<()> {
                        if info.LastPresentTime != 0 || self.cached.is_none() {
                            if let Some(r) = res.as_ref() {
                                let tex: ID3D11Texture2D = r.cast()?;
                                self.update_cache(&tex)?;
                                if !immediate && info.LastPresentTime != 0 {
                                    watch_dirty = self.changed(&info, &watch);
                                }
                            }
                        }
                        Ok(())
                    })();
                    let _ = self.dup.ReleaseFrame();
                    step?;
                }
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                    if !immediate {
                        return Ok(None);
                    }
                }
                Err(e) => return Err(e),
            }
            if immediate || watch_dirty {
                self.read_from_cached(region, out_px)
            } else {
                Ok(None)
            }
        }
    }
}

const SPAN: u32 = (2 * BLUR_RADIUS + 1) as u32;

/// Three box-blur passes over an interleaved 4-channel image (horizontal then row-major vertical).
fn blur(px: &mut [u8], w: usize, h: usize, tmp: &mut Vec<u8>, col_sums: &mut Vec<u32>) {
    if w == 0 || h == 0 {
        return;
    }
    tmp.resize(w * h * 4, 0);
    for _ in 0..3 {
        horiz_pass(px, tmp, w, h);
        vert_pass(tmp, px, w, h, col_sums);
    }
}

fn horiz_pass(src: &[u8], dst: &mut [u8], w: usize, h: usize) {
    for y in 0..h {
        let off = y * w * 4;
        let srow = &src[off..off + w * 4];
        let drow = &mut dst[off..off + w * 4];
        let mut acc = [0u32; 3];
        for k in 0..=2 * BLUR_RADIUS {
            let ix = k.saturating_sub(BLUR_RADIUS).min(w - 1) * 4;
            acc[0] += srow[ix] as u32;
            acc[1] += srow[ix + 1] as u32;
            acc[2] += srow[ix + 2] as u32;
        }
        for x in 0..w {
            let o = x * 4;
            drow[o] = (acc[0] / SPAN) as u8;
            drow[o + 1] = (acc[1] / SPAN) as u8;
            drow[o + 2] = (acc[2] / SPAN) as u8;
            let out_ix = x.saturating_sub(BLUR_RADIUS) * 4;
            let in_ix = (x + BLUR_RADIUS + 1).min(w - 1) * 4;
            acc[0] = acc[0] + srow[in_ix] as u32 - srow[out_ix] as u32;
            acc[1] = acc[1] + srow[in_ix + 1] as u32 - srow[out_ix + 1] as u32;
            acc[2] = acc[2] + srow[in_ix + 2] as u32 - srow[out_ix + 2] as u32;
        }
    }
}

fn vert_pass(src: &[u8], dst: &mut [u8], w: usize, h: usize, col_sums: &mut Vec<u32>) {
    col_sums.clear();
    col_sums.resize(w * 3, 0);
    let first = &src[..w * 4];
    let init_reps = (BLUR_RADIUS + 1) as u32;
    for x in 0..w {
        col_sums[x * 3] = first[x * 4] as u32 * init_reps;
        col_sums[x * 3 + 1] = first[x * 4 + 1] as u32 * init_reps;
        col_sums[x * 3 + 2] = first[x * 4 + 2] as u32 * init_reps;
    }
    for k in 1..=BLUR_RADIUS {
        let sy = k.min(h - 1);
        let srow = &src[sy * w * 4..(sy + 1) * w * 4];
        for x in 0..w {
            col_sums[x * 3] += srow[x * 4] as u32;
            col_sums[x * 3 + 1] += srow[x * 4 + 1] as u32;
            col_sums[x * 3 + 2] += srow[x * 4 + 2] as u32;
        }
    }
    for y in 0..h {
        let drow = &mut dst[y * w * 4..(y + 1) * w * 4];
        for x in 0..w {
            drow[x * 4] = (col_sums[x * 3] / SPAN) as u8;
            drow[x * 4 + 1] = (col_sums[x * 3 + 1] / SPAN) as u8;
            drow[x * 4 + 2] = (col_sums[x * 3 + 2] / SPAN) as u8;
        }
        if y + 1 < h {
            let out_y = y.saturating_sub(BLUR_RADIUS);
            let in_y = (y + BLUR_RADIUS + 1).min(h - 1);
            let out_row = &src[out_y * w * 4..(out_y + 1) * w * 4];
            let in_row = &src[in_y * w * 4..(in_y + 1) * w * 4];
            for x in 0..w {
                col_sums[x * 3] = col_sums[x * 3] + in_row[x * 4] as u32 - out_row[x * 4] as u32;
                col_sums[x * 3 + 1] = col_sums[x * 3 + 1] + in_row[x * 4 + 1] as u32 - out_row[x * 4 + 1] as u32;
                col_sums[x * 3 + 2] = col_sums[x * 3 + 2] + in_row[x * 4 + 2] as u32 - out_row[x * 4 + 2] as u32;
            }
        }
    }
}

/// BGRA -> RGBA with a mild saturation boost (s = 1.18 in Q8 fixed-point = 302/256), alpha opaque.
fn finish(px: &mut [u8]) {
    for p in px.chunks_exact_mut(4) {
        let (b, g, r) = (p[0] as i32, p[1] as i32, p[2] as i32);
        let l = (54 * r + 183 * g + 19 * b) >> 8;
        p[0] = (l + (((r - l) * 302 + 128) >> 8)).clamp(0, 255) as u8;
        p[1] = (l + (((g - l) * 302 + 128) >> 8)).clamp(0, 255) as u8;
        p[2] = (l + (((b - l) * 302 + 128) >> 8)).clamp(0, 255) as u8;
        p[3] = 255;
    }
}

pub fn glass_loop(ws: Arc<WinState>, chan: FrameChannel) {
    let mut cap: Option<Capture> = None;
    let mut last_sent: Option<Instant> = None;
    let mut last_checked: Option<Instant> = None;
    let mut last_region: Option<RECT> = None;
    let mut last_crop: Option<RECT> = None;
    let mut pending_force = false;
    let mut had_chan = false;
    // reused buffers across iterations
    let mut px_buf: Vec<u8> = Vec::new();
    let mut last_raw: Vec<u8> = Vec::new();
    let mut blur_tmp: Vec<u8> = Vec::new();
    let mut col_sums: Vec<u32> = Vec::new();
    let mut fail_until: Option<Instant> = None;
    let mut logged_err = false;
    let mut logged_frame_err = false;
    let mut was_paused = false;
    loop {
        crate::perf::tick("glass");
        let has_chan = chan.lock().unwrap().is_some();
        if has_chan && !had_chan {
            pending_force = true;
            fail_until = None;
        }
        had_chan = has_chan;
        pending_force |= FORCE_FRAME.swap(false, Ordering::SeqCst);
        if REOPEN_NOW.swap(false, Ordering::SeqCst) {
            cap = None;
            last_region = None;
            last_crop = None;
            last_checked = None;
            last_raw.clear();
            fail_until = None;
            pending_force = true;
        }

        let paused = ws.game.load(Ordering::SeqCst)
            || ws.hidden.load(Ordering::SeqCst)
            || ws.free.load(Ordering::SeqCst)
            || !has_chan;
        let hwnd = ws.hwnd.load(Ordering::SeqCst);
        if paused || hwnd == 0 {
            cap = None;
            last_region = None;
            last_crop = None;
            last_checked = None;
            if !was_paused {
                was_paused = true;
                px_buf = Vec::new();
                last_raw = Vec::new();
                blur_tmp = Vec::new();
                col_sums = Vec::new();
            } else {
                last_raw.clear();
            }
            std::thread::sleep(Duration::from_millis(300));
            continue;
        }
        was_paused = false;
        if let Some(t) = fail_until {
            if Instant::now() < t && !REOPEN_NOW.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(120));
                continue;
            }
            fail_until = None;
        }
        let mut wr = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd as _, &mut wr) } == 0 {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        let region = RECT { left: wr.left, top: wr.top, right: wr.right, bottom: wr.bottom };
        let (cx, cy) = ((wr.left + wr.right) / 2, (wr.top + wr.bottom) / 2);

        // Only drop duplication when the window moves outside the captured monitor output.
        if cap.as_ref().map(|c| !c.contains_point(cx, cy)).unwrap_or(false) {
            cap = None;
            last_region = None;
            last_crop = None;
            last_checked = None;
            last_raw.clear();
        }

        if cap.is_none() {
            match Capture::open(cx, cy) {
                Ok(c) => {
                    let _ = exclude_from_dda(hwnd);
                    log("glass: desktop duplication started");
                    logged_err = false;
                    logged_frame_err = false;
                    cap = Some(c);
                    last_region = None;
                    last_crop = None;
                    last_checked = None;
                    last_raw.clear();
                }
                Err(e) => {
                    if !logged_err {
                        log(format!("glass: duplication unavailable: {e}"));
                        logged_err = true;
                    }
                    // Software / RDP display drivers return DXGI_ERROR_UNSUPPORTED: avoid creating &
                    // dropping a D3D11 device every 3s unless a display/resume event arrives.
                    let wait = if e.code() == DXGI_ERROR_UNSUPPORTED {
                        Duration::from_secs(30)
                    } else {
                        Duration::from_millis(1200)
                    };
                    fail_until = Some(Instant::now() + wait);
                    continue;
                }
            }
        }

        let expanded = ws.expanded.load(Ordering::SeqCst);
        let hr = if expanded { *ws.panel.lock().unwrap() } else { *ws.pill.lock().unwrap() };
        let watch = RECT {
            left: wr.left + hr.x as i32 - 8,
            top: wr.top + hr.y as i32 - 8,
            right: wr.left + (hr.x + hr.w) as i32 + 8,
            bottom: wr.top + (hr.y + hr.h) as i32 + 8,
        };
        let c = cap.as_mut().unwrap();
        let crop = crop_rect(&region, &hr, &c.out);
        let force = last_region.as_ref().map(|r| !same_rect(r, &region)).unwrap_or(true);
        let same_crop = last_crop.as_ref().map(|l| same_rect(l, &crop)).unwrap_or(false);
        let immediate = force || !same_crop || pending_force;

        // Pace BEFORE AcquireNextFrame in ~15ms slices so expand/collapse/force stay instant (<15ms)
        // while 60–144Hz low-diff animations never cause 60–144Hz GPU readbacks.
        if !immediate && c.cached.is_some() {
            let interval = frame_interval();
            let anchor = match (last_checked, last_sent) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            if let Some(t) = anchor {
                let wait = interval.saturating_sub(t.elapsed());
                if !wait.is_zero() {
                    std::thread::sleep(wait.min(Duration::from_millis(15)));
                    continue;
                }
            }
        }

        last_checked = Some(Instant::now());
        match c.frame(crop, watch, immediate, &mut px_buf) {
            Ok(Some((w, h))) => {
                let area = (
                    ((watch.left - crop.left).max(0) >> 2) as usize,
                    ((watch.top - crop.top).max(0) >> 2) as usize,
                    ((watch.right - crop.left).max(0) >> 2) as usize,
                    ((watch.bottom - crop.top).max(0) >> 2) as usize,
                );
                let d = if same_crop && !force && !pending_force {
                    diff100(&px_buf, &last_raw, w as usize, area)
                } else {
                    u64::MAX
                };
                last_region = Some(region);
                if d < 100 {
                    crate::perf::count("glass same", 1);
                    continue;
                }
                if d != u64::MAX {
                    crate::perf::count("glass diff_sum", d);
                    crate::perf::count("glass diff_n", 1);
                }
                last_crop = Some(crop);
                pending_force = false;
                last_raw.clear();
                last_raw.extend_from_slice(&px_buf);
                blur(&mut px_buf, w as usize, h as usize, &mut blur_tmp, &mut col_sums);
                finish(&mut px_buf);
                // header: piece size, its offset and the whole window (all in quarter px)
                let ox = ((crop.left - region.left) >> 2) as u32;
                let oy = ((crop.top - region.top) >> 2) as u32;
                let ww = ((region.right - region.left) >> 2) as u32;
                let wh = ((region.bottom - region.top) >> 2) as u32;
                let mut msg = Vec::with_capacity(24 + px_buf.len());
                for v in [w, h, ox, oy, ww, wh] {
                    msg.extend_from_slice(&v.to_le_bytes());
                }
                msg.extend_from_slice(&px_buf);
                if let Some(ch) = chan.lock().unwrap().as_ref() {
                    let _ = ch.send(InvokeResponseBody::Raw(msg));
                    crate::perf::count("glass frames", 1);
                }
                last_sent = Some(Instant::now());
            }
            Ok(None) => {
                // When waiting on the initial desktop frame before cache exists, avoid busy spinning.
                if c.cached.is_none() {
                    std::thread::sleep(Duration::from_millis(15));
                }
            }
            Err(e) => {
                // DXGI_ERROR_ACCESS_LOST (mode change, UAC desktop, lock screen, sleep) and friends
                if !logged_frame_err {
                    log(format!("glass: frame error {e}; reopening"));
                    logged_frame_err = true;
                }
                cap = None;
                last_region = None;
                last_crop = None;
                last_checked = None;
                last_raw.clear();
                fail_until = Some(Instant::now() + Duration::from_millis(500));
            }
        }
    }
}
