//! Extracts an executable's icon on a dedicated STA thread and returns it as a PNG data URL.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows_sys::Win32::UI::Shell::{SHDefExtractIconW, SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

use crate::procinfo::wide;

const MAX_CACHE: usize = 512;
const FAIL_TTL: Duration = Duration::from_secs(300);

enum Entry {
    Ok(String),
    Failed(Instant),
}

#[derive(Default)]
struct IconCache {
    map: HashMap<String, Entry>,
    order: VecDeque<String>,
    logged: HashSet<String>,
}

impl IconCache {
    fn get(&mut self, key: &str, now: Instant) -> Option<Option<String>> {
        match self.map.get(key) {
            Some(Entry::Ok(url)) => Some(Some(url.clone())),
            Some(Entry::Failed(until)) if now < *until => Some(None),
            Some(Entry::Failed(_)) => {
                self.map.remove(key);
                None
            }
            None => None,
        }
    }

    fn insert(&mut self, key: String, val: Entry, now: Instant) {
        if self.map.contains_key(&key) {
            self.map.insert(key, val);
            return;
        }
        if self.map.len() >= MAX_CACHE {
            // Evict expired failures first
            self.map.retain(|_, v| !matches!(v, Entry::Failed(t) if now >= *t));
            self.order.retain(|k| self.map.contains_key(k));
        }
        while self.map.len() >= MAX_CACHE {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            } else {
                self.map.clear();
                break;
            }
        }
        self.order.push_back(key.clone());
        self.map.insert(key, val);
    }
}

fn cache() -> &'static Mutex<IconCache> {
    static C: OnceLock<Mutex<IconCache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(IconCache::default()))
}

type Req = (String, SyncSender<Option<String>>);

fn sta_worker() -> &'static SyncSender<Req> {
    static TX: OnceLock<SyncSender<Req>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = sync_channel::<Req>(64);
        let _ = std::thread::Builder::new().name("icon-sta".into()).spawn(move || unsafe {
            use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
            while let Ok((path, reply)) = rx.recv() {
                let _ = reply.send(extract(&path));
            }
        });
        tx
    })
}

pub fn icon_data_url(path: &str) -> Option<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }
    let key = trimmed.to_lowercase();
    let now = Instant::now();
    if let Some(cached) = cache().lock().unwrap().get(&key, now) {
        return cached;
    }
    let (reply_tx, reply_rx) = sync_channel(1);
    if sta_worker().send((trimmed.to_string(), reply_tx)).is_err() {
        return None;
    }
    let v = reply_rx.recv().ok().flatten();
    let now = Instant::now();
    let mut c = cache().lock().unwrap();
    match &v {
        Some(url) => {
            c.logged.remove(&key);
            c.insert(key, Entry::Ok(url.clone()), now);
        }
        None => {
            if c.logged.len() >= 2048 {
                c.logged.clear();
            }
            let first_fail = c.logged.insert(key.clone());
            c.insert(key, Entry::Failed(now + FAIL_TTL), now);
            drop(c);
            if first_fail {
                crate::log(format!("icon extraction failed: {trimmed}"));
            }
        }
    }
    v
}

fn extract(path: &str) -> Option<String> {
    unsafe {
        let w = wide(path);
        let mut large: HICON = std::ptr::null_mut();
        // LOWORD = large size, HIWORD = small size.
        let hr = SHDefExtractIconW(w.as_ptr(), 0, 0, &mut large, std::ptr::null_mut(), 48 | (16 << 16));
        if hr != 0 || large.is_null() {
            let mut info: SHFILEINFOW = std::mem::zeroed();
            let r = SHGetFileInfoW(
                w.as_ptr(),
                0,
                &mut info,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON | SHGFI_LARGEICON,
            );
            if r == 0 || info.hIcon.is_null() {
                return None;
            }
            large = info.hIcon;
        }
        let out = hicon_to_png(large);
        DestroyIcon(large);
        out.map(|png| format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)))
    }
}

unsafe fn bitmap_bits(hdc: *mut core::ffi::c_void, hbm: *mut core::ffi::c_void, w: i32, h: i32) -> Option<Vec<u8>> {
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h; // top-down
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let lines = GetDIBits(hdc, hbm, 0, h as u32, buf.as_mut_ptr() as *mut _, &mut bi, DIB_RGB_COLORS);
    if lines == 0 {
        None
    } else {
        Some(buf)
    }
}

unsafe fn hicon_to_png(icon: HICON) -> Option<Vec<u8>> {
    let mut ii: ICONINFO = std::mem::zeroed();
    if GetIconInfo(icon, &mut ii) == 0 {
        return None;
    }
    let color = ii.hbmColor;
    let mask = ii.hbmMask;
    let cleanup = || {
        if !color.is_null() {
            DeleteObject(color);
        }
        if !mask.is_null() {
            DeleteObject(mask);
        }
    };
    if color.is_null() {
        cleanup();
        return None;
    }
    let mut bm: BITMAP = std::mem::zeroed();
    if GetObjectW(color, std::mem::size_of::<BITMAP>() as i32, &mut bm as *mut _ as *mut _) == 0 {
        cleanup();
        return None;
    }
    let (w, h) = (bm.bmWidth, bm.bmHeight);
    if w <= 0 || h <= 0 || w > 256 || h > 256 {
        cleanup();
        return None;
    }
    let hdc = CreateCompatibleDC(std::ptr::null_mut());
    if hdc.is_null() {
        cleanup();
        return None;
    }
    let bits = bitmap_bits(hdc, color, w, h);
    let mut rgba = match bits {
        Some(b) => b,
        None => {
            DeleteDC(hdc);
            cleanup();
            return None;
        }
    };
    // BGRA -> RGBA
    let mut any_alpha = false;
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
        if px[3] != 0 {
            any_alpha = true;
        }
    }
    if !any_alpha {
        // Old-style icon without alpha: use the AND mask (0 = opaque).
        let masked = if !mask.is_null() {
            if let Some(m) = bitmap_bits(hdc, mask, w, h) {
                for (px, mp) in rgba.chunks_exact_mut(4).zip(m.chunks_exact(4)) {
                    px[3] = if mp[0] == 0 { 255 } else { 0 };
                }
                true
            } else {
                false
            }
        } else {
            false
        };
        if !masked {
            for px in rgba.chunks_exact_mut(4) {
                px[3] = 255;
            }
        }
    }
    DeleteDC(hdc);
    cleanup();

    let mut png_buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png_buf, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().ok()?;
        writer.write_image_data(&rgba).ok()?;
    }
    Some(png_buf)
}
