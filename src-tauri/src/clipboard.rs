//! Clipboard history: text and pictures copied anywhere, newest first, in the island's
//! "剪贴板" page. Clicking an entry only puts it back on the clipboard.
//!
//! - A message-only window gets WM_CLIPBOARDUPDATE and signals a background worker immediately;
//!   all reading, encoding and DPAPI encryption run off the window thread.
//! - Content a password manager marks as private is skipped (ExcludeClipboardContentFromMonitor-
//!   Processing / CanIncludeInClipboardHistory != 1 / Clipboard Viewer Ignore / ConcealedType…),
//!   fail-closed and switchable.
//! - Stored on disk encrypted for the current Windows user (DPAPI) under
//!   %LOCALAPPDATA%\MicaBar\clipboard (`index.dat`, `<id>.dat`, `<id>.thumb`), or only in memory.
//!   Never logged.
//! - Kept for `clip.days` days, at most `clip.max` entries, and bounded total byte size; pinned
//!   entries are never removed.
//! - The same content copied again moves to the top instead of being added twice.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use windows_sys::Win32::Foundation::HWND;

use crate::{log, now_ms, settings};

const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
const CF_DIBV5: u32 = 17;

/// text up to this many chars lives in the index; longer text goes to its own encrypted file
const INLINE_CHARS: usize = 4000;
const MAX_TEXT_CHARS: usize = 2_000_000;
const MAX_RAW_TEXT_BYTES: usize = 8 * 1024 * 1024;
const MAX_PIXELS: usize = 40_000_000;
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_RAW_DIB_BYTES: usize = MAX_PIXELS * 4 + 4096;
const MAX_DISK_BYTES: usize = 250 * 1024 * 1024;
const MAX_MEM_BYTES: usize = 100 * 1024 * 1024;
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
    /// legacy inline thumbnail (migrated to `<id>.thumb` on disk; kept empty in `index.dat`)
    #[serde(skip_serializing_if = "String::is_empty")]
    pub thumb: String,
    /// last copied (ms)
    pub at: u64,
    pub pinned: bool,
    /// the program that put it on the clipboard (exe name / full path), when known
    pub app: String,
    pub app_path: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub hash: u64,
    /// UTF-8 byte length for text items (used for fast dedup & byte cap accounting)
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub bytes: usize,
    /// the full text when short (never sent to the page: see View)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

fn is_zero_usize(v: &usize) -> bool {
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
    /// cached `data:image/png;base64,...` thumbnails (persisted separately in `<id>.thumb` on disk)
    thumbs: HashMap<u64, String>,
    /// where the history lives now: true = disk
    disk: bool,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);
static HWND_CLIP: AtomicIsize = AtomicIsize::new(0);
/// hash of what MicaBar itself just put on the clipboard (moves that entry up, nothing else)
static OWN_SET: AtomicU64 = AtomicU64::new(0);
static APP: Mutex<Option<AppHandle>> = Mutex::new(None);
static SIGNAL: OnceLock<(Mutex<bool>, Condvar)> = OnceLock::new();

fn signal() -> &'static (Mutex<bool>, Condvar) {
    SIGNAL.get_or_init(|| (Mutex::new(false), Condvar::new()))
}

fn dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("MicaBar").join("clipboard")
}

fn blob_path(id: u64) -> PathBuf {
    dir().join(format!("{id}.dat"))
}

fn thumb_path(id: u64) -> PathBuf {
    dir().join(format!("{id}.thumb"))
}

fn want_disk() -> bool {
    settings::str_at("clip.store", "disk") != "memory"
}

// ---------- Cached clipboard format IDs ----------

struct ClipFormats {
    exclude_monitor: u32,
    viewer_ignore_spaced: u32,
    viewer_ignore: u32,
    can_include_history: u32,
    concealed: u32,
    transient: u32,
    auto_generated: u32,
    png: u32,
}

fn register_fmt(name: &str) -> u32 {
    let w: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    unsafe { windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW(w.as_ptr()) }
}

fn clip_formats() -> &'static ClipFormats {
    static F: OnceLock<ClipFormats> = OnceLock::new();
    F.get_or_init(|| ClipFormats {
        exclude_monitor: register_fmt("ExcludeClipboardContentFromMonitorProcessing"),
        viewer_ignore_spaced: register_fmt("Clipboard Viewer Ignore"),
        viewer_ignore: register_fmt("ClipboardViewerIgnore"),
        can_include_history: register_fmt("CanIncludeInClipboardHistory"),
        concealed: register_fmt("org.nspasteboard.ConcealedType"),
        transient: register_fmt("org.nspasteboard.TransientType"),
        auto_generated: register_fmt("org.nspasteboard.AutoGeneratedType"),
        png: register_fmt("PNG"),
    })
}

// ---------- Deterministic 64-bit FNV-1a ----------

struct Fnv1a(u64);

impl Fnv1a {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
    fn finish(self) -> u64 {
        self.0.max(1)
    }
}

fn hash_text(t: &str) -> u64 {
    let mut h = Fnv1a::new();
    h.write(&[0u8]);
    h.write(&(t.len() as u64).to_le_bytes());
    h.write(t.as_bytes());
    h.finish()
}

fn hash_image(rgba: &[u8], w: u32, hh: u32) -> u64 {
    let mut h = Fnv1a::new();
    h.write(&[1u8]);
    h.write(&w.to_le_bytes());
    h.write(&hh.to_le_bytes());
    h.write(&(rgba.len() as u64).to_le_bytes());
    h.write(rgba);
    h.finish()
}

// ---------- DPAPI ----------

fn protect(data: &[u8], decrypt: bool) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(data.len()).ok()?,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // CRYPTPROTECT_UI_FORBIDDEN = 1
        let ok = if decrypt {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                &mut out,
            )
        } else {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                &mut out,
            )
        };
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as _);
        Some(v)
    }
}

fn write_cipher_file(path: &Path, enc: &[u8]) -> bool {
    let _ = std::fs::create_dir_all(dir());
    settings::atomic_write(path, enc).is_ok()
}

fn write_enc(path: &Path, data: &[u8]) -> bool {
    let Some(enc) = protect(data, false) else { return false };
    write_cipher_file(path, &enc)
}

fn read_enc(path: &Path) -> Option<Vec<u8>> {
    protect(&std::fs::read(path).ok()?, true)
}

// ---------- store ----------

fn scan_max_file_id(folder: &Path) -> u64 {
    let mut max_id = 0u64;
    if let Ok(rd) = std::fs::read_dir(folder) {
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if let Some(stem) = name.strip_suffix(".dat").or_else(|| name.strip_suffix(".thumb")) {
                if let Ok(id) = stem.parse::<u64>() {
                    max_id = max_id.max(id);
                }
            }
        }
    }
    max_id
}

fn startup_gc(folder: &Path, items: &[Item]) {
    let Ok(rd) = std::fs::read_dir(folder) else { return };
    let blob_ids: HashSet<u64> = items.iter().filter(|i| has_blob(i)).map(|i| i.id).collect();
    let img_ids: HashSet<u64> = items.iter().filter(|i| i.kind == "image").map(|i| i.id).collect();
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name == "index.dat" || name == "index.dat.bak" {
            continue;
        }
        if name.ends_with(".tmp") {
            let _ = std::fs::remove_file(&p);
            continue;
        }
        if let Some(stem) = name.strip_suffix(".dat") {
            if let Ok(id) = stem.parse::<u64>() {
                if !blob_ids.contains(&id) {
                    let _ = std::fs::remove_file(&p);
                }
            }
        } else if let Some(stem) = name.strip_suffix(".thumb") {
            if let Ok(id) = stem.parse::<u64>() {
                if !img_ids.contains(&id) {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
    }
}

fn load() -> Store {
    let disk = want_disk();
    let mut s = Store {
        next: 1,
        items: Vec::new(),
        mem: HashMap::new(),
        thumbs: HashMap::new(),
        disk,
    };
    let folder = dir();
    if disk {
        let ix_path = folder.join("index.dat");
        let non_empty = std::fs::metadata(&ix_path).map(|m| m.len() > 0).unwrap_or(false);
        if non_empty {
            match read_enc(&ix_path).and_then(|b| serde_json::from_slice::<Index>(&b).ok()) {
                Some(ix) => {
                    let max_item = ix.items.iter().map(|i| i.id).max().unwrap_or(0);
                    s.next = ix.next.max(max_item.saturating_add(1)).max(1);
                    s.items = ix.items;
                    let mut migrated = false;
                    for it in &mut s.items {
                        if it.kind == "image" {
                            if !it.thumb.is_empty() {
                                // Smoothly migrate legacy inline Base64 thumb out of index.dat into <id>.thumb
                                let t = std::mem::take(&mut it.thumb);
                                if write_enc(&thumb_path(it.id), t.as_bytes()) {
                                    s.thumbs.insert(it.id, t);
                                    migrated = true;
                                } else {
                                    it.thumb = t.clone();
                                    s.thumbs.insert(it.id, t);
                                }
                            } else if let Some(tb) = read_enc(&thumb_path(it.id)).and_then(|b| String::from_utf8(b).ok()) {
                                s.thumbs.insert(it.id, tb);
                            }
                        }
                    }
                    startup_gc(&folder, &s.items);
                    if migrated {
                        save_index(&s);
                    }
                }
                None => {
                    // Never silently overwrite existing .dat/.thumb files from next = 1!
                    let bak = folder.join("index.dat.bak");
                    let _ = std::fs::copy(&ix_path, &bak);
                    let max_file = scan_max_file_id(&folder);
                    s.next = max_file.saturating_add(1).max(1);
                    log(format!(
                        "clip: index.dat unreadable, backed up to index.dat.bak (continuing from id {})",
                        s.next
                    ));
                }
            }
        } else {
            startup_gc(&folder, &[]);
        }
    } else {
        // only in memory: whatever an earlier run left on disk goes
        let _ = std::fs::remove_dir_all(folder);
    }
    s
}

fn save_index(s: &Store) {
    if !s.disk {
        return;
    }
    let ix = Index {
        v: 2,
        next: s.next,
        items: s.items.clone(),
    };
    if let Ok(b) = serde_json::to_vec(&ix) {
        if !write_enc(&dir().join("index.dat"), &b) {
            log("clip: could not save the index");
        }
    }
}

fn drop_blob(s: &mut Store, id: u64) {
    s.mem.remove(&id);
    s.thumbs.remove(&id);
    if s.disk {
        let _ = std::fs::remove_file(blob_path(id));
        let _ = std::fs::remove_file(thumb_path(id));
    }
}

fn has_blob(it: &Item) -> bool {
    it.kind == "image" || it.text.is_none()
}

fn item_storage_bytes(s: &Store, it: &Item) -> usize {
    if it.kind == "image" {
        let tb = s.thumbs.get(&it.id).map(|t| t.len()).unwrap_or(it.thumb.len());
        it.len.saturating_add(tb)
    } else if it.bytes > 0 {
        it.bytes
    } else if let Some(ref t) = it.text {
        t.len()
    } else if let Some(b) = s.mem.get(&it.id) {
        b.len()
    } else {
        it.len.saturating_mul(2)
    }
}

/// Days / count / byte limits (pinned entries stay) and the disk <-> memory switch.
fn prune(s: &mut Store) -> bool {
    let mut changed = false;
    let disk = want_disk();
    if disk != s.disk {
        if disk {
            // memory -> disk: verify writes succeed before dropping in-memory blobs
            let mut ok = true;
            for (&id, data) in &s.mem {
                if !write_enc(&blob_path(id), data) {
                    ok = false;
                    break;
                }
            }
            if ok {
                for it in s.items.iter().filter(|i| i.kind == "image") {
                    if let Some(tb) = s.thumbs.get(&it.id) {
                        if !write_enc(&thumb_path(it.id), tb.as_bytes()) {
                            ok = false;
                            break;
                        }
                    }
                }
            }
            if ok {
                s.disk = true;
                s.mem.clear();
                log("clip: history now kept on disk (encrypted)");
                changed = true;
            } else {
                log("clip: switching history to disk failed; keeping in memory");
            }
        } else {
            // disk -> memory: read all payloads & thumbnails in first; only remove_dir_all if none failed
            let mut loaded_mem = HashMap::new();
            let mut ok = true;
            for it in &s.items {
                if has_blob(it) {
                    let bp = blob_path(it.id);
                    if bp.exists() {
                        match read_enc(&bp) {
                            Some(d) => {
                                loaded_mem.insert(it.id, d);
                            }
                            None => {
                                ok = false;
                                break;
                            }
                        }
                    }
                }
                if it.kind == "image" && !s.thumbs.contains_key(&it.id) {
                    let tp = thumb_path(it.id);
                    if tp.exists() {
                        match read_enc(&tp).and_then(|b| String::from_utf8(b).ok()) {
                            Some(tb) => {
                                s.thumbs.insert(it.id, tb);
                            }
                            None => {
                                ok = false;
                                break;
                            }
                        }
                    }
                }
            }
            if ok {
                s.mem.extend(loaded_mem);
                s.disk = false;
                let _ = std::fs::remove_dir_all(dir());
                log("clip: history now kept in memory only");
                changed = true;
            } else {
                log("clip: switching history to memory aborted because encrypted files could not be read");
            }
        }
    }
    let days = settings::num_at("clip.days", 7.0).max(0.0);
    let max = settings::num_at("clip.max", 200.0).clamp(1.0, 5000.0) as usize;
    let now = now_ms();
    let mut gone: HashSet<u64> = HashSet::new();
    if days > 0.0 {
        let cut = now.saturating_sub((days * 86_400_000.0) as u64);
        for it in s.items.iter().filter(|i| !i.pinned && i.at < cut) {
            gone.insert(it.id);
        }
    }
    let loose: Vec<u64> = s
        .items
        .iter()
        .filter(|i| !i.pinned && !gone.contains(&i.id))
        .map(|i| i.id)
        .collect();
    if loose.len() > max {
        // items are newest first: the tail goes
        gone.extend(loose[max..].iter().copied());
    }
    if !gone.is_empty() {
        for &id in &gone {
            drop_blob(s, id);
        }
        s.items.retain(|i| !gone.contains(&i.id));
        changed = true;
    }
    // Enforce total storage byte cap (250 MB on disk, 100 MB in memory), evicting oldest unpinned first
    let byte_cap = if s.disk { MAX_DISK_BYTES } else { MAX_MEM_BYTES };
    let mut total_bytes: usize = s.items.iter().map(|i| item_storage_bytes(s, i)).sum();
    while total_bytes > byte_cap {
        let Some(pos) = s.items.iter().rposition(|i| !i.pinned) else { break };
        let used = item_storage_bytes(s, &s.items[pos]);
        let id = s.items[pos].id;
        drop_blob(s, id);
        s.items.remove(pos);
        total_bytes = total_bytes.saturating_sub(used);
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
    for _ in 0..8 {
        if unsafe { OpenClipboard(hwnd) } != 0 {
            return Some(Guard);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    None
}

unsafe fn global_bytes_capped(h: *mut core::ffi::c_void, max_bytes: usize) -> Option<Vec<u8>> {
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    if h.is_null() {
        return None;
    }
    let n = GlobalSize(h);
    if n == 0 || n > max_bytes {
        return None;
    }
    let p = GlobalLock(h) as *const u8;
    if p.is_null() {
        return None;
    }
    let v = std::slice::from_raw_parts(p, n).to_vec();
    GlobalUnlock(h);
    Some(v)
}

unsafe fn read_raw_utf16_capped() -> Option<Result<Vec<u8>, ()>> {
    use windows_sys::Win32::System::DataExchange::{GetClipboardData, IsClipboardFormatAvailable};
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
        return None;
    }
    let h = GetClipboardData(CF_UNICODETEXT) as *mut core::ffi::c_void;
    if h.is_null() {
        return None;
    }
    let total = GlobalSize(h) & !1;
    if total == 0 {
        return None;
    }
    let p = GlobalLock(h) as *const u8;
    if p.is_null() {
        return None;
    }
    let scan_limit = total.min(MAX_RAW_TEXT_BYTES);
    let mut nul_off = None;
    let mut i = 0usize;
    while i + 1 < scan_limit {
        if *p.add(i) == 0 && *p.add(i + 1) == 0 {
            nul_off = Some(i);
            break;
        }
        i += 2;
    }
    let res = match nul_off {
        Some(0) => None,
        Some(len) => Some(Ok(std::slice::from_raw_parts(p, len).to_vec())),
        None if total > MAX_RAW_TEXT_BYTES => Some(Err(())),
        None => Some(Ok(std::slice::from_raw_parts(p, scan_limit).to_vec())),
    };
    GlobalUnlock(h);
    res
}

/// Marked private by the program that copied it (password managers do this). Fail-closed!
fn private_marked() -> bool {
    use windows_sys::Win32::System::DataExchange::{GetClipboardData, IsClipboardFormatAvailable};
    let f = clip_formats();
    unsafe {
        for &id in &[
            f.exclude_monitor,
            f.viewer_ignore_spaced,
            f.viewer_ignore,
            f.concealed,
            f.transient,
            f.auto_generated,
        ] {
            if id != 0 && IsClipboardFormatAvailable(id) != 0 {
                return true;
            }
        }
        if f.can_include_history != 0 && IsClipboardFormatAvailable(f.can_include_history) != 0 {
            // Fail-closed: unless explicitly >= 4 bytes with non-zero DWORD, treat as private!
            let allow = global_bytes_capped(GetClipboardData(f.can_include_history) as _, 64)
                .map(|b| b.len() >= 4 && u32::from_le_bytes([b[0], b[1], b[2], b[3]]) != 0)
                .unwrap_or(false);
            if !allow {
                return true;
            }
        }
        false
    }
}

/// Resolves the app name and exe path of the clipboard owner window (unwrapping UWP
/// `ApplicationFrameHost.exe` to its hosted `CoreWindow` process). Called AFTER `CloseClipboard()`.
fn owner_app_from_hwnd(hwnd: isize) -> (String, String) {
    use windows_sys::Win32::Foundation::{BOOL, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumChildWindows, GetWindowThreadProcessId};
    if hwnd == 0 {
        return Default::default();
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd as HWND, &mut pid) };
    if pid == 0 {
        return Default::default();
    }
    let mut path = crate::procinfo::image_path(pid).unwrap_or_default();
    if path
        .rsplit(['\\', '/'])
        .next()
        .map(|n| n.eq_ignore_ascii_case("applicationframehost.exe"))
        .unwrap_or(false)
    {
        struct FindChild {
            host: u32,
            child: u32,
        }
        unsafe extern "system" fn cb(ch: HWND, lp: LPARAM) -> BOOL {
            let f = &mut *(lp as *mut FindChild);
            let mut p = 0u32;
            windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(ch, &mut p);
            if p != 0 && p != f.host {
                f.child = p;
                return 0;
            }
            1
        }
        let mut fc = FindChild { host: pid, child: 0 };
        unsafe { EnumChildWindows(hwnd as HWND, Some(cb), &mut fc as *mut FindChild as LPARAM) };
        if fc.child != 0 {
            if let Some(real) = crate::procinfo::image_path(fc.child) {
                path = real;
            }
        }
    }
    if path.is_empty() {
        return Default::default();
    }
    let name = path.rsplit(['\\', '/']).next().unwrap_or("").to_string();
    (name, path)
}

enum RawClip {
    Utf16(Vec<u8>),
    TextTooLong,
    Png { png: Vec<u8>, dib_fallback: Option<Vec<u8>> },
    Dib(Vec<u8>),
    ImageTooBig,
}

/// Copies only raw bytes out while holding the clipboard open (< 1 ms), then closes immediately.
fn capture_raw() -> Option<(RawClip, isize)> {
    use windows_sys::Win32::System::DataExchange::{GetClipboardData, GetClipboardOwner, IsClipboardFormatAvailable};
    use windows_sys::Win32::System::Memory::GlobalSize;
    let owner = unsafe { GetClipboardOwner() } as isize;
    let _g = open(std::ptr::null_mut())?;
    if settings::bool_at("clip.skipPrivate", true) && private_marked() {
        return None;
    }
    unsafe {
        if let Some(res) = read_raw_utf16_capped() {
            match res {
                Ok(u16_bytes) => {
                    // Check that it isn't purely whitespace; if purely whitespace, allow image fallback
                    let non_ws = u16_bytes
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .any(|u| !char::from_u32(u as u32).map(char::is_whitespace).unwrap_or(false));
                    if non_ws {
                        return Some((RawClip::Utf16(u16_bytes), owner));
                    }
                }
                Err(()) => return Some((RawClip::TextTooLong, owner)),
            }
        }
        if !settings::bool_at("clip.images", true) {
            return None;
        }
        let f = clip_formats();
        if f.png != 0 && IsClipboardFormatAvailable(f.png) != 0 {
            let h = GetClipboardData(f.png) as *mut core::ffi::c_void;
            if !h.is_null() && GlobalSize(h) > MAX_IMAGE_BYTES {
                return Some((RawClip::ImageTooBig, owner));
            }
            if let Some(png_bytes) = global_bytes_capped(h, MAX_IMAGE_BYTES) {
                if png_bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                    let dib_fb = if IsClipboardFormatAvailable(CF_DIBV5) != 0 {
                        global_bytes_capped(GetClipboardData(CF_DIBV5) as _, MAX_RAW_DIB_BYTES)
                    } else if IsClipboardFormatAvailable(CF_DIB) != 0 {
                        global_bytes_capped(GetClipboardData(CF_DIB) as _, MAX_RAW_DIB_BYTES)
                    } else {
                        None
                    };
                    return Some((RawClip::Png { png: png_bytes, dib_fallback: dib_fb }, owner));
                }
            }
        }
        for fmt_id in [CF_DIBV5, CF_DIB] {
            if IsClipboardFormatAvailable(fmt_id) != 0 {
                let h = GetClipboardData(fmt_id) as *mut core::ffi::c_void;
                if !h.is_null() && GlobalSize(h) > MAX_RAW_DIB_BYTES {
                    return Some((RawClip::ImageTooBig, owner));
                }
                if let Some(dib_bytes) = global_bytes_capped(h, MAX_RAW_DIB_BYTES) {
                    return Some((RawClip::Dib(dib_bytes), owner));
                }
            }
        }
    }
    None
}

fn extract_channel(val: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let m = mask >> shift;
    let raw = (val >> shift) & m;
    if m == 0xFF {
        raw as u8
    } else {
        ((raw as u64 * 255 + (m as u64 / 2)) / m as u64).min(255) as u8
    }
}

/// Decodes CF_DIB / CF_DIBV5 (`BI_RGB = 0`, `BI_BITFIELDS = 3`, `BI_ALPHABITFIELDS = 6`) into RGBA.
fn decode_dib(b: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    if b.len() < 40 {
        return None;
    }
    let rd32 = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let hsize = rd32(0) as usize;
    if hsize < 40 || hsize > b.len() {
        return None;
    }
    let w = rd32(4) as i32;
    let h = rd32(8) as i32;
    let bpp = u16::from_le_bytes([b[14], b[15]]);
    let comp = rd32(16);
    let used = rd32(32) as usize;
    if w <= 0 || h == 0 || !matches!(bpp, 16 | 24 | 32) || !matches!(comp, 0 | 3 | 6) {
        return None;
    }
    let (w, top_down, h) = (w as usize, h < 0, h.unsigned_abs() as usize);
    if w.saturating_mul(h) > MAX_PIXELS {
        log(format!("clip: picture {w}x{h} too big, skipped"));
        return None;
    }
    let stride = (w * bpp as usize).div_ceil(32) * 4;
    let need = stride.checked_mul(h)?;
    let (r_mask, g_mask, b_mask, a_mask, has_explicit_alpha_mask) = match comp {
        3 | 6 => {
            let need_hdr = if comp == 6 { 56 } else { 52 };
            if b.len() < need_hdr {
                return None;
            }
            let rm = rd32(40);
            let gm = rd32(44);
            let bm = rd32(48);
            let (am, explicit_a) = if comp == 6 || hsize >= 56 {
                let m = rd32(52);
                (m, m != 0)
            } else {
                (0, false)
            };
            if rm == 0 && gm == 0 && bm == 0 {
                return None;
            }
            (rm, gm, bm, am, explicit_a)
        }
        _ => {
            if bpp == 16 {
                (0x7C00, 0x03E0, 0x001F, 0, false)
            } else {
                (0x00FF_0000, 0x0000_FF00, 0x0000_00FF, if bpp == 32 { 0xFF00_0000 } else { 0 }, false)
            }
        }
    };
    let extra_masks = match (comp, hsize) {
        (3, 40) => 12,
        (6, 40) => 16,
        _ => 0,
    };
    let color_table = used.checked_mul(4)?;
    let mut off = hsize.checked_add(extra_masks)?.checked_add(color_table)?;
    if b.len() < off.checked_add(need)? {
        return None;
    }
    // Handle rare apps that append 12 redundant mask bytes after BITMAPV4/V5HEADER
    if extra_masks == 0 && comp == 3 && hsize >= 108 && b.len() == off + 12 + need {
        off += 12;
    }
    let mut out = vec![0u8; w * h * 4];
    let mut any_alpha = false;
    let bytes_per_px = (bpp as usize) / 8;
    for y in 0..h {
        let src = off + stride * if top_down { y } else { h - 1 - y };
        for x in 0..w {
            let p = src + x * bytes_per_px;
            let o = (y * w + x) * 4;
            if comp == 0 && (bpp == 24 || bpp == 32) {
                out[o] = b[p + 2];
                out[o + 1] = b[p + 1];
                out[o + 2] = b[p];
                let a = if bpp == 32 { b[p + 3] } else { 255 };
                out[o + 3] = a;
                any_alpha |= bpp == 32 && a != 0;
            } else {
                let val = match bpp {
                    16 => u16::from_le_bytes([b[p], b[p + 1]]) as u32,
                    24 => (b[p] as u32) | ((b[p + 1] as u32) << 8) | ((b[p + 2] as u32) << 16),
                    _ => u32::from_le_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]]),
                };
                out[o] = extract_channel(val, r_mask);
                out[o + 1] = extract_channel(val, g_mask);
                out[o + 2] = extract_channel(val, b_mask);
                if a_mask != 0 {
                    let a = extract_channel(val, a_mask);
                    out[o + 3] = a;
                    any_alpha |= a != 0;
                } else {
                    out[o + 3] = 255;
                    any_alpha = true;
                }
            }
        }
    }
    // Classic Win32 32-bit GDI bitmaps without an explicit non-zero alpha channel leave A = 0 everywhere
    if bpp == 32 && !any_alpha && (!has_explicit_alpha_mask || a_mask != 0) {
        for a in out.iter_mut().skip(3).step_by(4) {
            *a = 255;
        }
    }
    Some((out, w as u32, h as u32))
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
    let (w, h) = (r.info().width, r.info().height);
    if w == 0 || h == 0 || (w as usize).saturating_mul(h as usize) > MAX_PIXELS {
        return None;
    }
    let buf_size = r.output_buffer_size();
    if buf_size > MAX_PIXELS * 4 {
        return None;
    }
    let mut buf = vec![0; buf_size];
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
    png_encode(&out, tw, th)
        .map(|p| format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(p)))
        .unwrap_or_default()
}

/// Single-pass O(PREVIEW_CHARS) folded preview.
fn preview_of(t: &str) -> String {
    let mut s = String::with_capacity(PREVIEW_CHARS.min(t.len()));
    let mut count = 0usize;
    let mut space = false;
    for c in t.chars() {
        if c.is_whitespace() {
            if !space && !s.is_empty() {
                s.push(' ');
                count += 1;
                if count >= PREVIEW_CHARS {
                    break;
                }
            }
            space = true;
        } else {
            s.push(c);
            count += 1;
            space = false;
            if count >= PREVIEW_CHARS {
                break;
            }
        }
    }
    s.trim_end().to_string()
}

enum Decoded {
    Text {
        text: String,
        char_len: usize,
        byte_len: usize,
        hash: u64,
    },
    Image {
        rgba: Vec<u8>,
        raw_png: Option<Vec<u8>>,
        w: u32,
        h: u32,
        hash: u64,
    },
}

fn process_update() {
    crate::perf::tick("clipboard");
    if !settings::bool_at("clip.on", true) {
        return;
    }
    let Some((raw, owner_hwnd)) = capture_raw() else { return };
    // Clipboard is ALREADY closed here! Decode, resolve owner, encode PNG/thumb & DPAPI outside locks.
    let decoded = match raw {
        RawClip::TextTooLong => {
            log("clip: text too long, skipped");
            return;
        }
        RawClip::ImageTooBig => {
            log("clip: picture > 64 MB, skipped");
            return;
        }
        RawClip::Utf16(u16_bytes) => {
            let units: Vec<u16> = u16_bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|&c| c != 0)
                .collect();
            let t = String::from_utf16_lossy(&units);
            if t.trim().is_empty() {
                return;
            }
            let char_len = t.chars().count();
            if char_len > MAX_TEXT_CHARS {
                log("clip: text too long, skipped");
                return;
            }
            let byte_len = t.len();
            let hash = hash_text(&t);
            Decoded::Text { text: t, char_len, byte_len, hash }
        }
        RawClip::Png { png, dib_fallback } => {
            if let Some((rgba, w, h)) = png_decode(&png) {
                let hash = hash_image(&rgba, w, h);
                Decoded::Image {
                    rgba,
                    raw_png: Some(png),
                    w,
                    h,
                    hash,
                }
            } else if let Some(dib) = dib_fallback.and_then(|d| decode_dib(&d)) {
                let (rgba, w, h) = dib;
                let hash = hash_image(&rgba, w, h);
                Decoded::Image {
                    rgba,
                    raw_png: None,
                    w,
                    h,
                    hash,
                }
            } else {
                return;
            }
        }
        RawClip::Dib(dib_bytes) => {
            let Some((rgba, w, h)) = decode_dib(&dib_bytes) else { return };
            let hash = hash_image(&rgba, w, h);
            Decoded::Image {
                rgba,
                raw_png: None,
                w,
                h,
                hash,
            }
        }
    };

    let (app, path) = owner_app_from_hwnd(owner_hwnd);
    let own = !path.is_empty()
        && std::env::current_exe()
            .map(|e| e.to_string_lossy().eq_ignore_ascii_case(&path))
            .unwrap_or(false);
    let hash = match &decoded {
        Decoded::Text { hash, .. } | Decoded::Image { hash, .. } => *hash,
    };
    let back = OWN_SET.swap(0, Ordering::SeqCst) == hash;
    let now = now_ms();

    // Fast path: check deduplication before running png_encode / thumb / DPAPI encryption!
    let (deduped, disk_mode, reserved_id) = with_store(|s| {
        let dup_pos = s.items.iter().position(|i| {
            if i.hash == 0 || i.hash != hash {
                return false;
            }
            match &decoded {
                Decoded::Text { char_len, byte_len, .. } => {
                    i.kind == "text" && i.len == *char_len && (i.bytes == 0 || i.bytes == *byte_len)
                }
                Decoded::Image { w, h, .. } => i.kind == "image" && i.w == *w && i.h == *h,
            }
        });
        if let Some(pos) = dup_pos {
            let mut it = s.items.remove(pos);
            it.at = now;
            if !app.is_empty() && !own {
                it.app = app.clone();
                it.app_path = path.clone();
            }
            s.items.insert(0, it);
            save_index(s);
            return (Some(false), s.disk, 0);
        }
        if back {
            return (Some(true), s.disk, 0); // suppress self-copy if item was already deleted
        }
        let id = s.next;
        s.next += 1;
        (None, s.disk, id)
    });

    if let Some(suppressed_or_moved) = deduped {
        if !suppressed_or_moved {
            changed(false);
        }
        return;
    }

    // Heavy encoding & DPAPI encryption completely OUTSIDE STORE.lock()!
    let mut it = Item {
        id: reserved_id,
        at: now,
        app,
        app_path: path,
        hash,
        ..Default::default()
    };
    let mut mem_blob: Option<Vec<u8>> = None;
    let mut mem_thumb: Option<String> = None;

    match decoded {
        Decoded::Text { text, char_len, byte_len, .. } => {
            it.kind = "text".into();
            it.preview = preview_of(&text);
            it.len = char_len;
            it.bytes = byte_len;
            it.more = char_len > PREVIEW_CHARS;
            if char_len <= INLINE_CHARS {
                it.text = Some(text);
            } else if disk_mode {
                let Some(enc) = protect(text.as_bytes(), false) else { return };
                if !write_cipher_file(&blob_path(reserved_id), &enc) {
                    return;
                }
            } else {
                mem_blob = Some(text.into_bytes());
            }
        }
        Decoded::Image { rgba, raw_png, w, h, .. } => {
            let png_bytes = match raw_png {
                Some(p) => p,
                None => {
                    let Some(p) = png_encode(&rgba, w, h) else { return };
                    p
                }
            };
            if png_bytes.len() > MAX_IMAGE_BYTES {
                log("clip: encoded picture > 64 MB, skipped");
                return;
            }
            let th = thumb(&rgba, w, h);
            it.kind = "image".into();
            it.w = w;
            it.h = h;
            it.len = png_bytes.len();
            it.bytes = png_bytes.len();
            if disk_mode {
                let Some(enc_blob) = protect(&png_bytes, false) else { return };
                let Some(enc_thumb) = protect(th.as_bytes(), false) else { return };
                if !write_cipher_file(&blob_path(reserved_id), &enc_blob) {
                    return;
                }
                if !write_cipher_file(&thumb_path(reserved_id), &enc_thumb) {
                    let _ = std::fs::remove_file(blob_path(reserved_id));
                    return;
                }
            } else {
                mem_blob = Some(png_bytes);
            }
            mem_thumb = Some(th);
        }
    }

    with_store(|s| {
        // Handle edge case if user toggled disk <-> memory mid-encode
        if s.disk != disk_mode {
            if s.disk {
                if let Some(ref b) = mem_blob {
                    if !write_enc(&blob_path(reserved_id), b) {
                        return;
                    }
                }
                if let Some(ref tb) = mem_thumb {
                    let _ = write_enc(&thumb_path(reserved_id), tb.as_bytes());
                }
            } else if let Some(b) = read_enc(&blob_path(reserved_id)) {
                s.mem.insert(reserved_id, b);
                let _ = std::fs::remove_file(blob_path(reserved_id));
                let _ = std::fs::remove_file(thumb_path(reserved_id));
            }
        } else if let Some(b) = mem_blob {
            s.mem.insert(reserved_id, b);
        }
        if let Some(tb) = mem_thumb {
            s.thumbs.insert(reserved_id, tb);
        }
        s.items.insert(0, it);
        prune(s);
        save_index(s);
    });
    changed(true);
}

fn capture_worker() {
    let (lock, cvar) = signal();
    loop {
        let mut dirty = lock.lock().unwrap();
        while !*dirty {
            dirty = cvar.wait(dirty).unwrap();
        }
        // Coalesce format bursts (~120 ms) while apps finish putting multiple formats on the clipboard
        let started = Instant::now();
        let mut deadline = started + Duration::from_millis(120);
        loop {
            *dirty = false;
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let (g, _) = cvar.wait_timeout(dirty, deadline - now).unwrap();
            dirty = g;
            if *dirty {
                // Another WM_CLIPBOARDUPDATE arrived; extend slightly (capped at 300 ms from first trigger)
                deadline = (Instant::now() + Duration::from_millis(60)).min(started + Duration::from_millis(300));
            } else {
                break;
            }
        }
        drop(dirty);
        process_update();
    }
}

/// 100% non-blocking window proc: just wakes the background capture worker and returns immediately.
unsafe extern "system" fn wnd_proc(h: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{DefWindowProcW, WM_CLIPBOARDUPDATE};
    if msg == WM_CLIPBOARDUPDATE {
        let (lock, cvar) = signal();
        if let Ok(mut dirty) = lock.lock() {
            *dirty = true;
            cvar.notify_one();
        }
        return 0;
    }
    DefWindowProcW(h, msg, wp, lp)
}

/// Starts the listener, background capture worker, and clean-up loop.
pub fn start(app: AppHandle) {
    *APP.lock().unwrap() = Some(app);
    let _ = clip_formats();
    std::thread::spawn(capture_worker);
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
        let hwnd = CreateWindowExW(
            0,
            cls.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
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

fn item_to_view(thumbs: &mut HashMap<u64, String>, disk: bool, i: &Item) -> View {
    let thumb_str = if i.kind == "image" {
        if let Some(t) = thumbs.get(&i.id) {
            t.clone()
        } else if !i.thumb.is_empty() {
            thumbs.insert(i.id, i.thumb.clone());
            i.thumb.clone()
        } else if disk {
            let loaded = read_enc(&thumb_path(i.id))
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_default();
            if !loaded.is_empty() {
                thumbs.insert(i.id, loaded.clone());
            }
            loaded
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    View {
        id: i.id,
        kind: i.kind.clone(),
        preview: i.preview.clone(),
        len: i.len,
        more: i.more,
        w: i.w,
        h: i.h,
        thumb: thumb_str,
        at: i.at,
        pinned: i.pinned,
        app: i.app.clone(),
        app_path: i.app_path.clone(),
    }
}

#[allow(dead_code)]
pub fn list() -> Vec<View> {
    list_filtered(None)
}

/// Returns clipboard entries, optionally filtered case-insensitively across `app`, `preview`,
/// and the full text payload (including `<id>.dat` / `mem` blobs for text > 400 chars).
pub fn list_filtered(query: Option<&str>) -> Vec<View> {
    let needle = query.map(str::trim).filter(|q| !q.is_empty()).map(|q| q.to_lowercase());
    let Some(needle) = needle else {
        return with_store(|s| {
            let Store { items, thumbs, disk, .. } = s;
            items.iter().map(|i| item_to_view(thumbs, *disk, i)).collect()
        });
    };

    // Pass 1 under lock: match app / preview / inline text / memory blob without cloning `s.items`
    enum Candidate {
        Matched(View),
        NeedDiskBlob(View),
    }
    let candidates: Vec<Candidate> = with_store(|s| {
        let Store { items, mem, thumbs, disk, .. } = s;
        let mut out = Vec::with_capacity(items.len());
        for i in items.iter() {
            let quick_hit = i.app.to_lowercase().contains(&needle)
                || i.preview.to_lowercase().contains(&needle)
                || i.text.as_ref().map(|t| t.to_lowercase().contains(&needle)).unwrap_or(false);
            if quick_hit {
                out.push(Candidate::Matched(item_to_view(thumbs, *disk, i)));
            } else if i.kind == "text" && i.text.is_none() {
                if *disk {
                    out.push(Candidate::NeedDiskBlob(item_to_view(thumbs, *disk, i)));
                } else if let Some(b) = mem.get(&i.id) {
                    if String::from_utf8_lossy(b).to_lowercase().contains(&needle) {
                        out.push(Candidate::Matched(item_to_view(thumbs, *disk, i)));
                    }
                }
            }
        }
        out
    });

    // Pass 2 outside lock: decrypt any remaining >4000-char text blobs from disk
    let mut views = Vec::with_capacity(candidates.len());
    for c in candidates {
        match c {
            Candidate::Matched(v) => views.push(v),
            Candidate::NeedDiskBlob(v) => {
                if let Some(bytes) = read_enc(&blob_path(v.id)) {
                    if String::from_utf8_lossy(&bytes).to_lowercase().contains(&needle) {
                        views.push(v);
                    }
                }
            }
        }
    }
    views
}

fn build_dib_v1(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
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
    dib
}

fn build_dib_v5(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut dib = Vec::with_capacity(124 + rgba.len());
    dib.extend_from_slice(&124u32.to_le_bytes()); // bV5Size
    dib.extend_from_slice(&(w as i32).to_le_bytes()); // bV5Width
    dib.extend_from_slice(&(h as i32).to_le_bytes()); // bV5Height (bottom-up)
    dib.extend_from_slice(&1u16.to_le_bytes()); // bV5Planes
    dib.extend_from_slice(&32u16.to_le_bytes()); // bV5BitCount
    dib.extend_from_slice(&3u32.to_le_bytes()); // bV5Compression = BI_BITFIELDS
    dib.extend_from_slice(&((w * h * 4) as u32).to_le_bytes()); // bV5SizeImage
    dib.extend_from_slice(&[0u8; 16]); // XPelsPerMeter, YPelsPerMeter, ClrUsed, ClrImportant
    dib.extend_from_slice(&0x00FF_0000u32.to_le_bytes()); // bV5RedMask
    dib.extend_from_slice(&0x0000_FF00u32.to_le_bytes()); // bV5GreenMask
    dib.extend_from_slice(&0x0000_00FFu32.to_le_bytes()); // bV5BlueMask
    dib.extend_from_slice(&0xFF00_0000u32.to_le_bytes()); // bV5AlphaMask
    dib.extend_from_slice(&0x7352_4742u32.to_le_bytes()); // bV5CSType = LCS_sRGB
    dib.extend_from_slice(&[0u8; 36]); // bV5Endpoints
    dib.extend_from_slice(&[0u8; 12]); // GammaRed, GammaGreen, GammaBlue
    dib.extend_from_slice(&4u32.to_le_bytes()); // bV5Intent = LCS_GM_IMAGES
    dib.extend_from_slice(&[0u8; 12]); // ProfileData, ProfileSize, Reserved
    for y in (0..h as usize).rev() {
        for x in 0..w as usize {
            let o = (y * w as usize + x) * 4;
            dib.extend_from_slice(&[rgba[o + 2], rgba[o + 1], rgba[o], rgba[o + 3]]);
        }
    }
    dib
}

/// Puts an entry back on the clipboard (text, or a picture as PNG + CF_DIBV5 + CF_DIB).
pub fn copy(id: u64) -> Result<(), String> {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{EmptyClipboard, SetClipboardData};
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    let (kind, inline_text, disk_mode, mem_payload, hash) = with_store(|s| {
        let it = s
            .items
            .iter()
            .find(|i| i.id == id)
            .ok_or_else(|| crate::i18n::t("这条记录已经不在了"))?;
        Ok::<_, String>((
            it.kind.clone(),
            it.text.clone(),
            s.disk,
            if s.disk { None } else { s.mem.get(&id).cloned() },
            it.hash,
        ))
    })?;

    let payload = match inline_text {
        Some(t) => t.into_bytes(),
        None if disk_mode => read_enc(&blob_path(id)).ok_or_else(|| crate::i18n::t("读不到这条记录的内容"))?,
        None => mem_payload.ok_or_else(|| crate::i18n::t("读不到这条记录的内容"))?,
    };

    // formats: (id, bytes), encoded before opening clipboard
    let mut formats: Vec<(u32, Vec<u8>)> = Vec::new();
    if kind == "image" {
        let (rgba, w, h) = png_decode(&payload).ok_or_else(|| crate::i18n::t("读不到这条记录的内容"))?;
        let dib_v5 = build_dib_v5(&rgba, w, h);
        let dib_v1 = build_dib_v1(&rgba, w, h);
        let png_fmt = clip_formats().png;
        if png_fmt != 0 {
            formats.push((png_fmt, payload));
        }
        formats.push((CF_DIBV5, dib_v5));
        formats.push((CF_DIB, dib_v1));
    } else {
        let t = String::from_utf8(payload).map_err(|e| e.to_string())?;
        let mut u: Vec<u8> = t.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        u.extend_from_slice(&[0, 0]);
        formats.push((CF_UNICODETEXT, u));
    }

    // Pass NULL (not HWND_CLIP!) so other apps calling EmptyClipboard() never block on our window thread,
    // and only set OWN_SET AFTER open() succeeds!
    let _g = open(std::ptr::null_mut()).ok_or_else(|| crate::i18n::t("剪贴板正被别的程序占用，稍后再试"))?;
    OWN_SET.store(hash, Ordering::SeqCst);
    unsafe {
        if EmptyClipboard() == 0 {
            OWN_SET.store(0, Ordering::SeqCst);
            return Err(crate::i18n::t("没能放进剪贴板"));
        }
        for (f, data) in formats {
            let hm = GlobalAlloc(GMEM_MOVEABLE, data.len());
            if hm.is_null() {
                OWN_SET.store(0, Ordering::SeqCst);
                return Err("GlobalAlloc".into());
            }
            let p = GlobalLock(hm) as *mut u8;
            if p.is_null() {
                GlobalFree(hm);
                OWN_SET.store(0, Ordering::SeqCst);
                return Err("GlobalLock".into());
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
            GlobalUnlock(hm);
            if SetClipboardData(f, hm as _).is_null() {
                GlobalFree(hm);
                OWN_SET.store(0, Ordering::SeqCst);
                return Err(crate::i18n::t("没能放进剪贴板"));
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
