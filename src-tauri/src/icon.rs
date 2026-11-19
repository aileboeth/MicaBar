//! Extracts an executable's icon and returns it as a PNG data URL.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use base64::Engine;
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows_sys::Win32::UI::Shell::{SHDefExtractIconW, SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

use crate::procinfo::wide;

static CACHE: LazyLock<Mutex<HashMap<String, Option<String>>>> = LazyLock::new(Default::default);

/// Shell icon APIs want COM initialised on the calling thread and are not happy being hit from
/// many pool threads at once, so extraction is serialised. Failures are not cached (the page
/// retries later).
static EXTRACT: Mutex<()> = Mutex::new(());

pub fn icon_data_url(path: &str) -> Option<String> {
    if let Some(v) = CACHE.lock().unwrap().get(path) {
        return v.clone();
    }
    let v = {
        let _g = EXTRACT.lock().unwrap();
        unsafe {
            use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            // S_FALSE / RPC_E_CHANGED_MODE are fine: COM is usable either way
            CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        }
        extract(path)
    };
    if v.is_some() {
        CACHE.lock().unwrap().insert(path.to_string(), v.clone());
    } else {
        crate::log(format!("icon extraction failed: {path}"));
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
        if let Some(m) = bitmap_bits(hdc, mask, w, h) {
            for (px, mp) in rgba.chunks_exact_mut(4).zip(m.chunks_exact(4)) {
                px[3] = if mp[0] == 0 { 255 } else { 0 };
            }
        } else {
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
