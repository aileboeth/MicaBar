//! Clipboard history: text and pictures copied anywhere, newest first, in the island's
//! "剪贴板" page. Clicking an entry only puts it back on the clipboard.
//!
//! - A message-only window gets WM_CLIPBOARDUPDATE; the content is read a moment later.
//! - Content a password manager marks as private is skipped (ExcludeClipboardContentFromMonitor-
//!   Processing / CanIncludeInClipboardHistory = 0 / Clipboard Viewer Ignore), switchable.
//! - Stored on disk encrypted for the current Windows user (DPAPI) under
//!   %LOCALAPPDATA%\MicaBar\clipboard, or only in memory. Never logged.
//! - Kept for `clip.days` days and at most `clip.max` entries; pinned entries are never removed.
//! - The same content copied again moves to the top instead of being added twice.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use windows_sys::Win32::Foundation::HWND;

use crate::{log, now_ms, settings};

const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
/// text up to this many chars lives in the index; longer text goes to its own encrypted file
const INLINE_CHARS: usize = 4000;
const MAX_TEXT_CHARS: usize = 2_000_000;
const MAX_PIXELS: usize = 40_000_000;
const PREVIEW_CHARS: usize = 400;

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Item {
    pub id: u64,
    /// text | image
    pub kind: String,
    /// text: the start (whitespace runs folded); image: ""
    pub preview: String,
    /// text: characters; image: bytes of the stored PNG
    pub len: usize,
    /// text: the preview is only the start
    pub more: bool,
    pub w: u32,
    pub h: u32,
    /// image: a small PNG as a data URL
    pub thumb: String,
    /// last copied (ms)
    pub at: u64,
    pub pinned: bool,
    /// the program that put it on the clipboard (exe name / full path), when known
    pub app: String,
    pub app_path: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub hash: u64,
    /// the full text when short (never sent to the page: see View)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// What the page gets: everything but the full text and the hash.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub id: u64,
    pub kind: String,
    pub preview: String,
    pub len: usize,
    pub more: bool,
    pub w: u32,
    pub h: u32,
    pub thumb: String,
    pub at: u64,
    pub pinned: bool,
    pub app: String,
    pub app_path: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Index {
    v: u32,
    next: u64,
    items: Vec<Item>,
}

struct Store {
    next: u64,
    items: Vec<Item>,
    /// payloads (long text as UTF-8, images as PNG) while only kept in memory
    mem: HashMap<u64, Vec<u8>>,
    /// where the history lives now: true = disk
    disk: bool,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);
static HWND_CLIP: AtomicIsize = AtomicIsize::new(0);
/// hash of what MicaBar itself just put on the clipboard (moves that entry up, nothing else)
static OWN_SET: AtomicU64 = AtomicU64::new(0);
static APP: Mutex<Option<AppHandle>> = Mutex::new(None);

fn dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("MicaBar").join("clipboard")
}

fn blob_path(id: u64) -> PathBuf {
    dir().join(format!("{id}.dat"))
}

fn want_disk() -> bool {
    settings::str_at("clip.store", "disk") != "memory"
}

// ---------- DPAPI ----------

fn protect(data: &[u8], decrypt: bool) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};
    unsafe {
        let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // CRYPTPROTECT_UI_FORBIDDEN
        let ok = if decrypt {
            CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 1, &mut out)
        } else {
            CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 1, &mut out)
        };
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as _);
        Some(v)
    }
}

fn write_enc(path: &PathBuf, data: &[u8]) -> bool {
    let Some(enc) = protect(data, false) else { return false };
    let _ = std::fs::create_dir_all(dir());
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, enc).is_ok() && std::fs::rename(&tmp, path).is_ok()
}

fn read_enc(path: &PathBuf) -> Option<Vec<u8>> {
    protect(&std::fs::read(path).ok()?, true)
}

// ---------- store ----------

fn load() -> Store {
    let disk = want_disk();
    let mut s = Store { next: 1, items: Vec::new(), mem: HashMap::new(), disk };
    if disk {
        if let Some(ix) = read_enc(&dir().join("index.dat")).and_then(|b| serde_json::from_slice::<Index>(&b).ok()) {
            s.next = ix.next.max(1);
            s.items = ix.items;
        }
    } else {
        // only in memory: whatever an earlier run left on disk goes
        let _ = std::fs::remove_dir_all(dir());
    }
    s
}

fn save_index(s: &Store) {
    if !s.disk {
        return;
    }
    let ix = Index { v: 1, next: s.next, items: s.items.clone() };
    if let Ok(b) = serde_json::to_vec(&ix) {
        if !write_enc(&dir().join("index.dat"), &b) {
            log("clip: could not save the index");
        }
    }
}

fn put_blob(s: &mut Store, id: u64, data: Vec<u8>) -> bool {
    if s.disk {
        write_enc(&blob_path(id), &data)
    } else {
        s.mem.insert(id, data);
        true
    }
}

fn get_blob(s: &Store, id: u64) -> Option<Vec<u8>> {
    if s.disk {
        read_enc(&blob_path(id))
    } else {
        s.mem.get(&id).cloned()
    }
}

fn drop_blob(s: &mut Store, id: u64) {
    s.mem.remove(&id);
    if s.disk {
        let _ = std::fs::remove_file(blob_path(id));
    }
}

fn has_blob(it: &Item) -> bool {
    it.kind == "image" || it.text.is_none()
}

/// Days / count limits (pinned entries stay) and the disk <-> memory switch.
fn prune(s: &mut Store) -> bool {
    let mut changed = false;
    let disk = want_disk();
    if disk != s.disk {
        if disk {
            // memory -> disk: write what there is
            s.disk = true;
            let mem = std::mem::take(&mut s.mem);
            for (id, data) in mem {
                write_enc(&blob_path(id), &data);
            }
        } else {
            // disk -> memory: read the payloads in, then remove the files
            let ids: Vec<u64> = s.items.iter().filter(|i| has_blob(i)).map(|i| i.id).collect();
            for id in ids {
                if let Some(d) = read_enc(&blob_path(id)) {
                    s.mem.insert(id, d);
                }
            }
            s.disk = false;
            let _ = std::fs::remove_dir_all(dir());
        }
        log(format!("clip: history now kept {}", if disk { "on disk (encrypted)" } else { "in memory only" }));
        changed = true;
    }
    let days = settings::num_at("clip.days", 7.0).max(0.0);
    let max = settings::num_at("clip.max", 200.0).clamp(1.0, 5000.0) as usize;
    let now = now_ms();
    let mut gone = Vec::new();
    if days > 0.0 {
        let cut = now.saturating_sub((days * 86_400_000.0) as u64);
        for it in s.items.iter().filter(|i| !i.pinned && i.at < cut) {
            gone.push(it.id);
        }
    }
    let loose: Vec<u64> = s.items.iter().filter(|i| !i.pinned && !gone.contains(&i.id)).map(|i| i.id).collect();
    if loose.len() > max {
        // items are newest first: the tail goes
        gone.extend(loose[max..].iter().copied());
    }
    if !gone.is_empty() {
        for id in &gone {
            drop_blob(s, *id);
        }
        s.items.retain(|i| !gone.contains(&i.id));
        changed = true;
    }
    changed
}

fn with_store<T>(f: impl FnOnce(&mut Store) -> T) -> T {
    let mut g = STORE.lock().unwrap();
    let s = g.get_or_insert_with(load);
    f(s)
}

fn changed(new_item: bool) {
    if let Some(app) = APP.lock().unwrap().clone() {
        let _ = app.emit("clip", new_item);
    }
}

// ---------- reading the clipboard ----------

struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::System::DataExchange::CloseClipboard() };
    }
}

fn open(hwnd: HWND) -> Option<Guard> {
    use windows_sys::Win32::System::DataExchange::OpenClipboard;
    // another program may hold it for a moment
    for _ in 0..10 {
        if unsafe { OpenClipboard(hwnd) } != 0 {
            return Some(Guard);
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    None
}

fn fmt(name: &str) -> u32 {
    let w: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    unsafe { windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW(w.as_ptr()) }
}

unsafe fn global_bytes(h: *mut core::ffi::c_void) -> Option<Vec<u8>> {
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    if h.is_null() {
        return None;
    }
    let p = GlobalLock(h) as *const u8;
    if p.is_null() {
        return None;
    }
    let n = GlobalSize(h);
    let v = std::slice::from_raw_parts(p, n).to_vec();
    GlobalUnlock(h);
    Some(v)
}

/// Marked private by the program that copied it (password managers do this).
fn private_marked() -> bool {
    use windows_sys::Win32::System::DataExchange::{GetClipboardData, IsClipboardFormatAvailable};
    unsafe {
        if IsClipboardFormatAvailable(fmt("ExcludeClipboardContentFromMonitorProcessing")) != 0 || IsClipboardFormatAvailable(fmt("Clipboard Viewer Ignore")) != 0 {
            return true;
        }
        let f = fmt("CanIncludeInClipboardHistory");
        if IsClipboardFormatAvailable(f) != 0 {
            if let Some(b) = global_bytes(GetClipboardData(f) as _) {
                if b.len() >= 4 && u32::from_le_bytes([b[0], b[1], b[2], b[3]]) == 0 {
                    return true;
                }
            }
        }
        false
    }
}

fn owner_app() -> (String, String) {
    use windows_sys::Win32::System::DataExchange::GetClipboardOwner;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    unsafe {
        let w = GetClipboardOwner();
        if w.is_null() {
            return Default::default();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(w, &mut pid);
        match crate::procinfo::image_path(pid) {
            Some(p) => (p.rsplit('\\').next().unwrap_or("").to_string(), p),
            None => Default::default(),
        }
    }
}

enum Content {
    Text(String),
    /// RGBA, width, height
    Image(Vec<u8>, u32, u32),
}

fn read_text() -> Option<String> {
    use windows_sys::Win32::System::DataExchange::{GetClipboardData, IsClipboardFormatAvailable};
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
            return None;
        }
        let b = global_bytes(GetClipboardData(CF_UNICODETEXT) as _)?;
        let u: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
        let s = String::from_utf16_lossy(&u);
        (!s.trim().is_empty()).then_some(s)
    }
}

/// CF_DIB -> RGBA (24 / 32 bit; others are skipped).
fn read_dib() -> Option<(Vec<u8>, u32, u32)> {
    use windows_sys::Win32::System::DataExchange::{GetClipboardData, IsClipboardFormatAvailable};
    let b = unsafe {
        if IsClipboardFormatAvailable(CF_DIB) == 0 {
            return None;
        }
        global_bytes(GetClipboardData(CF_DIB) as _)?
    };
    if b.len() < 40 {
        return None;
    }
    let rd32 = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let hsize = rd32(0) as usize;
    let w = rd32(4) as i32;
    let h = rd32(8) as i32;
    let bpp = u16::from_le_bytes([b[14], b[15]]);
    let comp = rd32(16);
    let used = rd32(32) as usize;
    if w <= 0 || h == 0 || !(bpp == 24 || bpp == 32) || !(comp == 0 || comp == 3) {
        return None;
    }
    let (w, top_down, h) = (w as usize, h < 0, h.unsigned_abs() as usize);
    if w * h > MAX_PIXELS {
        log(format!("clip: picture {w}x{h} too big, skipped"));
        return None;
    }
    let masks = if comp == 3 && hsize == 40 { 12 } else { 0 };
    let off = hsize + masks + used * 4;
    let stride = (w * bpp as usize).div_ceil(32) * 4;
    if b.len() < off + stride * h {
        return None;
    }
    let mut out = vec![0u8; w * h * 4];
    let mut any_alpha = false;
    for y in 0..h {
        let src = off + stride * if top_down { y } else { h - 1 - y };
        for x in 0..w {
            let p = src + x * (bpp as usize / 8);
            let o = (y * w + x) * 4;
            out[o] = b[p + 2];
            out[o + 1] = b[p + 1];
            out[o + 2] = b[p];
            out[o + 3] = if bpp == 32 { b[p + 3] } else { 255 };
            any_alpha |= bpp == 32 && b[p + 3] != 0;
        }
    }
    // most programs leave the fourth byte at 0: that means opaque, not invisible
    if bpp == 32 && !any_alpha {
        for a in out.iter_mut().skip(3).step_by(4) {
            *a = 255;
        }
    }
    Some((out, w as u32, h as u32))
}

fn read_content() -> Option<Content> {
    if let Some(t) = read_text() {
        if t.chars().count() > MAX_TEXT_CHARS {
            log("clip: text too long, skipped");
            return None;
        }
        return Some(Content::Text(t));
    }
    if !settings::bool_at("clip.images", true) {
        return None;
    }
    read_dib().map(|(px, w, h)| Content::Image(px, w, h))
}

fn hash_of(c: &Content) -> u64 {
    let mut h = DefaultHasher::new();
    match c {
        Content::Text(t) => {
            0u8.hash(&mut h);
            t.hash(&mut h);
        }
        Content::Image(px, w, hh) => {
            1u8.hash(&mut h);
            (w, hh).hash(&mut h);
            px.hash(&mut h);
        }
    }
    h.finish().max(1)
}

fn png_encode(rgba: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut wr = enc.write_header().ok()?;
        wr.write_image_data(rgba).ok()?;
    }
    Some(buf)
}

fn png_decode(data: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(data));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        _ => return None,
    };
    Some((rgba, w, h))
}

/// A small copy (fits 320 x 200) as a PNG data URL, box-averaged.
fn thumb(rgba: &[u8], w: u32, h: u32) -> String {
    use base64::Engine;
    let scale = (320.0 / w as f64).min(200.0 / h as f64).min(1.0);
    let tw = ((w as f64 * scale).round() as u32).max(1);
    let th = ((h as f64 * scale).round() as u32).max(1);
    let mut out = vec![0u8; (tw * th * 4) as usize];
    for ty in 0..th {
        let y0 = (ty as u64 * h as u64 / th as u64) as u32;
        let y1 = (((ty + 1) as u64 * h as u64 / th as u64) as u32).max(y0 + 1).min(h);
        for tx in 0..tw {
            let x0 = (tx as u64 * w as u64 / tw as u64) as u32;
            let x1 = (((tx + 1) as u64 * w as u64 / tw as u64) as u32).max(x0 + 1).min(w);
            let mut acc = [0u64; 4];
            let mut n = 0u64;
            // sample at most 8 x 8 points per box (big screenshots)
            let sy = ((y1 - y0) / 8).max(1);
            let sx = ((x1 - x0) / 8).max(1);
            let mut y = y0;
            while y < y1 {
                let mut x = x0;
                while x < x1 {
                    let o = ((y * w + x) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += rgba[o + c] as u64;
                    }
                    n += 1;
                    x += sx;
                }
                y += sy;
            }
            let o = ((ty * tw + tx) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    png_encode(&out, tw, th).map(|p| format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(p))).unwrap_or_default()
}

fn preview_of(t: &str) -> String {
    let mut s = String::new();
    let mut space = false;
    for c in t.chars() {
        if c.is_whitespace() {
            if !space && !s.is_empty() {
                s.push(' ');
            }
            space = true;
        } else {
            s.push(c);
            space = false;
        }
        if s.chars().count() >= PREVIEW_CHARS {
            break;
        }
    }
    s.trim_end().to_string()
}

fn on_update(hwnd: HWND) {
    if !settings::bool_at("clip.on", true) {
        return;
    }
    let (content, app, path) = {
        let Some(_g) = open(hwnd) else { return };
        if settings::bool_at("clip.skipPrivate", true) && private_marked() {
            return;
        }
        let Some(c) = read_content() else { return };
        let (app, path) = owner_app();
        (c, app, path)
    };
    // an entry MicaBar just put back counts as that entry (moved up), never as a new one; the
    // owner is then MicaBar's own window, which shouldn't replace the program it came from
    let own = std::env::current_exe().map(|e| e.to_string_lossy().eq_ignore_ascii_case(&path)).unwrap_or(false);
    let hash = hash_of(&content);
    let back = OWN_SET.swap(0, Ordering::SeqCst) == hash;
    let now = now_ms();
    let added = with_store(|s| {
        if let Some(pos) = s.items.iter().position(|i| i.hash == hash) {
            // copied again: to the top
            let mut it = s.items.remove(pos);
            it.at = now;
            if !app.is_empty() && !own {
                it.app = app.clone();
                it.app_path = path.clone();
            }
            s.items.insert(0, it);
            save_index(s);
            return Some(false);
        }
        if back {
            return None;
        }
        let id = s.next;
        s.next += 1;
        let mut it = Item { id, at: now, app, app_path: path, hash, ..Default::default() };
        match content {
            Content::Text(t) => {
                it.kind = "text".into();
                it.preview = preview_of(&t);
                it.len = t.chars().count();
                it.more = it.len > PREVIEW_CHARS;
                if it.len <= INLINE_CHARS {
                    it.text = Some(t);
                } else if !put_blob(s, id, t.into_bytes()) {
                    return None;
                }
            }
            Content::Image(px, w, h) => {
                let png = png_encode(&px, w, h)?;
                it.kind = "image".into();
                it.w = w;
                it.h = h;
                it.len = png.len();
                it.thumb = thumb(&px, w, h);
                if !put_blob(s, id, png) {
                    return None;
                }
            }
        }
        s.items.insert(0, it);
        prune(s);
        save_index(s);
        Some(true)
    });
    if let Some(new) = added {
        changed(new);
    }
}

unsafe extern "system" fn wnd_proc(h: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{DefWindowProcW, KillTimer, SetTimer, WM_CLIPBOARDUPDATE, WM_TIMER};
    match msg {
        WM_CLIPBOARDUPDATE => {
            // read a moment later: programs often put several formats in a row
            SetTimer(h, 1, 120, None);
            0
        }
        WM_TIMER if wp == 1 => {
            KillTimer(h, 1);
            on_update(h);
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

/// Starts the listener and the hourly clean-up.
pub fn start(app: AppHandle) {
    *APP.lock().unwrap() = Some(app);
    std::thread::spawn(|| unsafe {
        use windows_sys::Win32::System::DataExchange::AddClipboardFormatListener;
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        // load (and apply the limits) before the first copy arrives
        with_store(|s| {
            if prune(s) {
                save_index(s);
            }
        });
        let hinst = GetModuleHandleW(std::ptr::null());
        let cls: Vec<u16> = "MicaBarClipboard".encode_utf16().chain(Some(0)).collect();
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wnd_proc);
        wc.hInstance = hinst;
        wc.lpszClassName = cls.as_ptr();
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(0, cls.as_ptr(), std::ptr::null(), 0, 0, 0, 0, 0, HWND_MESSAGE, std::ptr::null_mut(), hinst, std::ptr::null());
        HWND_CLIP.store(hwnd as isize, Ordering::SeqCst);
        // MicaBar runs elevated: let the update message through
        ChangeWindowMessageFilterEx(hwnd, WM_CLIPBOARDUPDATE, MSGFLT_ALLOW, std::ptr::null_mut());
        let ok = AddClipboardFormatListener(hwnd);
        log(format!("clip: listener {}", if ok != 0 { "on" } else { "failed" }));
        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    });
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_secs(60));
        let ch = with_store(|s| {
            let c = prune(s);
            if c {
                save_index(s);
            }
            c
        });
        if ch {
            changed(false);
        }
    });
}

pub fn list() -> Vec<View> {
    with_store(|s| {
        s.items
            .iter()
            .map(|i| View {
                id: i.id,
                kind: i.kind.clone(),
                preview: i.preview.clone(),
                len: i.len,
                more: i.more,
                w: i.w,
                h: i.h,
                thumb: i.thumb.clone(),
                at: i.at,
                pinned: i.pinned,
                app: i.app.clone(),
                app_path: i.app_path.clone(),
            })
            .collect()
    })
}

/// Puts an entry back on the clipboard (text, or a picture as DIB + PNG).
pub fn copy(id: u64) -> Result<(), String> {
    use windows_sys::Win32::System::DataExchange::{EmptyClipboard, SetClipboardData};
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    let (kind, payload, hash) = with_store(|s| {
        let it = s.items.iter().find(|i| i.id == id).ok_or_else(|| crate::i18n::t("这条记录已经不在了").to_string())?;
        let payload = match &it.text {
            Some(t) => t.clone().into_bytes(),
            None => get_blob(s, id).ok_or_else(|| crate::i18n::t("读不到这条记录的内容").to_string())?,
        };
        Ok::<_, String>((it.kind.clone(), payload, it.hash))
    })?;
    // formats: (id, bytes)
    let mut formats: Vec<(u32, Vec<u8>)> = Vec::new();
    if kind == "image" {
        let (rgba, w, h) = png_decode(&payload).ok_or_else(|| crate::i18n::t("读不到这条记录的内容").to_string())?;
        let mut dib = Vec::with_capacity(40 + rgba.len());
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&(w as i32).to_le_bytes());
        dib.extend_from_slice(&(h as i32).to_le_bytes()); // bottom-up
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        dib.extend_from_slice(&((w * h * 4) as u32).to_le_bytes());
        dib.extend_from_slice(&[0u8; 16]);
        for y in (0..h as usize).rev() {
            for x in 0..w as usize {
                let o = (y * w as usize + x) * 4;
                dib.extend_from_slice(&[rgba[o + 2], rgba[o + 1], rgba[o], rgba[o + 3]]);
            }
        }
        formats.push((CF_DIB, dib));
        formats.push((fmt("PNG"), payload));
    } else {
        let t = String::from_utf8(payload).map_err(|e| e.to_string())?;
        let mut u: Vec<u8> = t.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        u.extend_from_slice(&[0, 0]);
        formats.push((CF_UNICODETEXT, u));
    }
    let hwnd = HWND_CLIP.load(Ordering::SeqCst) as HWND;
    OWN_SET.store(hash, Ordering::SeqCst);
    let _g = open(hwnd).ok_or_else(|| crate::i18n::t("剪贴板正被别的程序占用，稍后再试").to_string())?;
    unsafe {
        EmptyClipboard();
        for (f, data) in formats {
            let hm = GlobalAlloc(GMEM_MOVEABLE, data.len());
            if hm.is_null() {
                return Err("GlobalAlloc".into());
            }
            let p = GlobalLock(hm) as *mut u8;
            if p.is_null() {
                GlobalFree(hm);
                return Err("GlobalLock".into());
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
            GlobalUnlock(hm);
            if SetClipboardData(f, hm as _).is_null() {
                GlobalFree(hm);
                return Err(crate::i18n::t("没能放进剪贴板").into());
            }
        }
    }
    Ok(())
}

pub fn pin(id: u64, on: bool) {
    with_store(|s| {
        if let Some(it) = s.items.iter_mut().find(|i| i.id == id) {
            it.pinned = on;
        }
        save_index(s);
    });
    changed(false);
}

pub fn delete(id: u64) {
    with_store(|s| {
        drop_blob(s, id);
        s.items.retain(|i| i.id != id);
        save_index(s);
    });
    changed(false);
}

/// Removes everything except pinned entries (`all` = pinned too).
pub fn clear(all: bool) {
    with_store(|s| {
        let gone: Vec<u64> = s.items.iter().filter(|i| all || !i.pinned).map(|i| i.id).collect();
        for id in &gone {
            drop_blob(s, *id);
        }
        s.items.retain(|i| !gone.contains(&i.id));
        save_index(s);
    });
    log(format!("clip: history cleared ({})", if all { "all" } else { "pinned kept" }));
    changed(false);
}

/// Applies changed limits / storage at once (settings page).
pub fn apply_settings() {
    let ch = with_store(|s| {
        let c = prune(s);
        if c {
            save_index(s);
        }
        c
    });
    if ch {
        changed(false);
    }
}
