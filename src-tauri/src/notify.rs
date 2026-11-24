//! Notifications in the island. Three sources:
//! - Windows toasts, read from the system's notification database (Telegram, mail, browsers…);
//! - taskbar flashes through a shell hook (WeChat 4.x signals new messages only that way: no
//!   toast, no readable window, so only "微信 · N 条新消息" can be shown);
//! - AI tools: Claude Code hooks / Codex `notify` post to a local port (127.0.0.1 + a token).
//! Notes live in memory only (message text is never written to disk or the log).

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use base64::Engine as _;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::procinfo::{self, wide};
use crate::{log, now_ms, settings};

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: u64,
    /// app key: exe stem, toast AppUserModelID, or "claude" / "codex"
    pub key: String,
    pub app: String,
    /// exe whose icon stands for the app (the page fetches it)
    pub icon: Option<String>,
    /// data: URL of the app's own icon file (toast apps registered with an IconUri)
    pub icon_url: Option<String>,
    /// data: URL of the sender's picture (toasts with an appLogoOverride image)
    pub avatar: Option<String>,
    pub title: String,
    pub body: String,
    /// small extra line ("用时 3 分 12 秒")
    pub sub: String,
    /// msg | flash | done | ask
    pub kind: String,
    /// toast | flash | ai | popup | micabar | test
    pub source: String,
    pub at: u64,
    pub count: u32,
    /// kept for the notification centre only (muted app, do not disturb, island switched off)
    pub silent: bool,
    pub read: bool,
    /// a screen recorder / sharing tool is running (the page hides the text in "smart" mode)
    pub capturing: bool,
    /// buttons on the card (MicaBar's own notes: "允许" / "阻止联网"…)
    pub actions: Vec<NoteAction>,
    /// what the actions act on (a program path…)
    pub data: String,
    #[serde(skip)]
    pub target: Target,
    /// notes with the same merge key (and still unread) become one, counted
    #[serde(skip)]
    pub merge: String,
    /// body for a merged note: "{n}" = count
    #[serde(skip)]
    pub tpl: Option<String>,
    /// exe stem of the program behind the note (a toast's AUMID belongs to it): bringing that
    /// program to the front marks the note read, and its taskbar flash for the same message is
    /// dropped
    #[serde(skip)]
    pub owner: String,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct NoteAction {
    pub id: String,
    pub label: String,
    /// primary | danger | plain
    pub style: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Target {
    #[default]
    None,
    /// a window (+ the exe to start when that window is gone / hidden)
    Hwnd(isize, Option<String>),
    /// a process: its main window
    Pid(u32),
    /// a toast app: activate through the shell
    Aumid(String),
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AppSeen {
    pub key: String,
    pub app: String,
    pub icon: Option<String>,
    pub icon_url: Option<String>,
    pub source: String,
    pub last: u64,
    pub count: u32,
}

#[derive(Default)]
struct Book {
    notes: VecDeque<Note>,
    apps: HashMap<String, AppSeen>,
}

static BOOK: OnceLock<Mutex<Book>> = OnceLock::new();
static APP: OnceLock<AppHandle> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);
static LAST_AI: Mutex<Option<Instant>> = Mutex::new(None);

fn book() -> &'static Mutex<Book> {
    BOOK.get_or_init(|| Mutex::new(Book::default()))
}

pub(crate) fn emit<S: Serialize + Clone>(ev: &str, v: S) {
    if let Some(a) = APP.get() {
        let _ = a.emit(ev, v);
    }
}

fn data_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    let d = PathBuf::from(base).join("MicaBar");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn privacy_mode() -> String {
    settings::str_at("notify.privacy", "source")
}

// ---------- the book ----------

fn app_cfg(key: &str) -> Value {
    settings::get()
        .pointer("/notify/apps")
        .and_then(|a| a.get(key))
        .cloned()
        .unwrap_or(Value::Null)
}

pub fn push(mut n: Note) {
    if !settings::bool_at("notify.on", true) {
        return;
    }
    let cfg = app_cfg(&n.key);
    let mute = cfg.get("mute").and_then(|x| x.as_bool()).unwrap_or(false);
    let island = cfg.get("island").and_then(|x| x.as_bool()).unwrap_or(true);
    n.silent = mute || !island || quiet_now();
    // a muted program only fills the notes page: nothing to catch up on, so no red dot
    if mute {
        n.read = true;
    }
    let privacy = privacy_mode();
    n.capturing = privacy == "smart" && capturing();
    n.at = now_ms();
    n.count = n.count.max(1);
    // the program behind the note: the flash window's / AI host's exe when not set by the source
    if n.owner.is_empty() {
        n.owner = match &n.target {
            Target::Hwnd(_, Some(exe)) => stem_of(exe),
            Target::Pid(pid) => procinfo::image_path(*pid).map(|p| stem_of(&p)).unwrap_or_default(),
            _ => String::new(),
        };
    }
    // already looking at that program: nothing to catch up on, so no red dot and no card
    if !n.owner.is_empty() && foreground_stem() == n.owner {
        n.read = true;
        n.silent = true;
    }
    let mut b = book().lock().unwrap();
    let seen = b.apps.entry(n.key.clone()).or_insert_with(|| AppSeen {
        key: n.key.clone(),
        app: n.app.clone(),
        icon: None,
        icon_url: None,
        source: n.source.clone(),
        last: 0,
        count: 0,
    });
    seen.app = n.app.clone();
    seen.last = n.at;
    seen.count += 1;
    if n.icon.is_some() {
        seen.icon = n.icon.clone();
    }
    if n.icon_url.is_some() {
        seen.icon_url = n.icon_url.clone();
    }
    if !n.merge.is_empty() {
        let at = n.at;
        if let Some(pos) = b
            .notes
            .iter()
            .position(|x| !x.read && x.merge == n.merge && at.saturating_sub(x.at) < 30 * 60_000)
        {
            let old = b.notes.remove(pos).unwrap();
            n.id = old.id;
            n.count += old.count;
            if n.avatar.is_none() {
                n.avatar = old.avatar;
            }
        }
    }
    if let Some(t) = &n.tpl {
        if n.count > 1 {
            n.body = t.replace("{n}", &n.count.to_string());
        }
    }
    // Enforce "source" privacy in Rust: never store or emit message body for external notifications
    if privacy == "source" && n.source != "micabar" {
        n.body.clear();
    }
    if n.id == 0 {
        n.id = NEXT.fetch_add(1, Ordering::SeqCst);
    }
    b.notes.push_front(n.clone());
    b.notes.truncate(300);
    drop(b);
    log(format!(
        "note: {} ({}, {}) x{}{}",
        n.key,
        n.source,
        n.kind,
        n.count,
        if n.silent { " silent" } else { "" }
    ));
    emit("note", n);
}

/// The app was brought to the front: its notes count as read.
fn mark_read_key(key: &str) {
    let mut b = book().lock().unwrap();
    // the page groups by key: a toast app's notes carry its AUMID as key, so tell it each key
    let mut keys: Vec<String> = Vec::new();
    for n in b.notes.iter_mut().filter(|n| (n.key == key || n.owner == key) && !n.read) {
        n.read = true;
        if !keys.contains(&n.key) {
            keys.push(n.key.clone());
        }
    }
    drop(b);
    for k in keys {
        emit("notes-read", k);
    }
}

pub fn list() -> Vec<Note> {
    let scrub = privacy_mode() == "source";
    book()
        .lock()
        .unwrap()
        .notes
        .iter()
        .map(|n| {
            let mut c = n.clone();
            if scrub && c.source != "micabar" {
                c.body.clear();
            }
            c
        })
        .collect()
}

pub fn apps() -> Vec<AppSeen> {
    let mut v: Vec<AppSeen> = book().lock().unwrap().apps.values().cloned().collect();
    v.sort_by(|a, b| b.last.cmp(&a.last));
    v
}

/// Removes notes (all of them when `ids` is empty).
pub fn dismiss(ids: &[u64]) {
    let mut b = book().lock().unwrap();
    if ids.is_empty() {
        b.notes.clear();
    } else {
        b.notes.retain(|n| !ids.contains(&n.id));
    }
}

pub fn mark_read(ids: &[u64]) {
    let mut b = book().lock().unwrap();
    for n in b.notes.iter_mut().filter(|n| ids.contains(&n.id)) {
        n.read = true;
    }
}

/// Brings the note's app (window) to the front.
pub fn open(id: u64) -> Result<(), String> {
    let target = {
        let mut b = book().lock().unwrap();
        let n = b
            .notes
            .iter_mut()
            .find(|n| n.id == id)
            .ok_or_else(|| crate::i18n::t("这条通知已经不在了"))?;
        n.read = true;
        n.target.clone()
    };
    focus_target(&target);
    Ok(())
}

/// Returns the running process's creation time in 100-ns FILETIME ticks, or `None` if it has exited.
pub(crate) fn process_create_time(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    #[link(name = "kernel32")]
    extern "system" {
        fn GetProcessTimes(
            hprocess: *mut core::ffi::c_void,
            lpcreationtime: *mut FILETIME,
            lpexittime: *mut FILETIME,
            lpkerneltime: *mut FILETIME,
            lpusertime: *mut FILETIME,
        ) -> i32;
    }
    if pid == 0 {
        return None;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut create: FILETIME = std::mem::zeroed();
        let mut exit: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        let ok = GetProcessTimes(h, &mut create, &mut exit, &mut kernel, &mut user);
        CloseHandle(h);
        if ok == 0 || exit.dwLowDateTime != 0 || exit.dwHighDateTime != 0 {
            return None;
        }
        let ticks = ((create.dwHighDateTime as u64) << 32) | (create.dwLowDateTime as u64);
        (ticks != 0).then_some(ticks)
    }
}

pub(crate) fn process_alive(pid: u32, create_time: u64) -> bool {
    process_create_time(pid) == Some(create_time)
}

/// The note's / session's window still exists (a process target: the process still runs).
pub(crate) fn target_alive(t: &Target) -> bool {
    match t {
        Target::Hwnd(h, _) => *h != 0 && unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(*h as _) != 0 },
        Target::Pid(p) => process_create_time(*p).is_some(),
        _ => true,
    }
}

fn find_window_by_exe(exe_path: &str) -> Option<isize> {
    let want_stem = stem_of(exe_path);
    for (pid, name) in procinfo::list_processes() {
        if stem_of(&name) == want_stem {
            if procinfo::image_path(pid)
                .map(|p| p.eq_ignore_ascii_case(exe_path))
                .unwrap_or(false)
            {
                if let Some(h) = main_window(pid) {
                    return Some(h);
                }
            }
        }
    }
    None
}

pub(crate) fn focus_target(target: &Target) {
    match target.clone() {
        Target::Hwnd(h, exe) => {
            if !focus_window(h) {
                if let Some(p) = exe {
                    if let Some(live_h) = find_window_by_exe(&p) {
                        if focus_window(live_h) {
                            return;
                        }
                    }
                    // de-elevated through explorer: a single-instance app shows its window again
                    let _ = std::process::Command::new("explorer.exe").arg(p).spawn();
                }
            }
        }
        Target::Pid(pid) => {
            if let Some(h) = main_window(pid) {
                focus_window(h);
            }
        }
        Target::Aumid(a) => {
            let _ = std::process::Command::new("explorer.exe")
                .arg(format!("shell:AppsFolder\\{a}"))
                .spawn();
        }
        Target::None => {}
    }
}

/// Marks one note read and drops its buttons (an action on it was taken).
pub fn settle(id: u64) {
    let mut b = book().lock().unwrap();
    if let Some(n) = b.notes.iter_mut().find(|n| n.id == id) {
        n.read = true;
        n.actions.clear();
    }
}

/// A sample, so the look can be tried from the settings.
pub fn test() {
    let samples = [
        (
            crate::i18n::t("微信"),
            crate::i18n::t("张三"),
            crate::i18n::t("晚上一起吃饭吗？我在楼下等你，顺便带一下钥匙"),
            "msg",
        ),
        (
            "Claude Code".to_string(),
            "MicaBar".to_string(),
            crate::i18n::t("通知模块写好了，虚拟机里验证通过"),
            "done",
        ),
        (
            "Telegram".to_string(),
            crate::i18n::t("李四"),
            crate::i18n::t("文件发你了，看一下"),
            "msg",
        ),
    ];
    static K: AtomicU32 = AtomicU32::new(0);
    let i = K.fetch_add(1, Ordering::SeqCst) as usize % samples.len();
    let (app, title, body, kind) = samples[i].clone();
    push(Note {
        key: format!("test-{i}"),
        app,
        title,
        body,
        sub: if kind == "done" {
            crate::i18n::t("完成 · 用时 3 分 12 秒")
        } else {
            String::new()
        },
        kind: kind.into(),
        source: "test".into(),
        merge: format!("test-{i}"),
        ..Default::default()
    });
}

// ---------- do not disturb / privacy ----------

fn windows_dnd() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    let k = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings");
    if let Ok(k) = k {
        if let Ok(v) = k.get_value::<u32, _>("NOC_GLOBAL_SETTING_TOASTS_ENABLED") {
            if v == 0 {
                return true;
            }
        }
    }
    // Windows 10 focus assist ("quiet hours")
    let mut st = 0;
    unsafe {
        if windows_sys::Win32::UI::Shell::SHQueryUserNotificationState(&mut st) == 0 {
            return st == 6; // QUNS_QUIET_TIME
        }
    }
    false
}

pub fn quiet_now() -> bool {
    if settings::bool_at("notify.followDnd", true) && windows_dnd() {
        return true;
    }
    if settings::bool_at("notify.quiet.on", false) {
        let from = settings::num_at("notify.quiet.from", 23.0) as i64;
        let to = settings::num_at("notify.quiet.to", 8.0) as i64;
        let h = local_hour();
        return if from <= to { h >= from && h < to } else { h >= from || h < to };
    }
    false
}

fn local_hour() -> i64 {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    t.wHour as i64
}

/// A screen recorder / sharing tool is running.
fn capturing() -> bool {
    const REC: [&str; 7] = [
        "obs64.exe",
        "obs32.exe",
        "obs.exe",
        "streamlabs obs.exe",
        "cpthost.exe",
        "bdcam.exe",
        "xsplit.core.exe",
    ];
    procinfo::list_processes()
        .iter()
        .any(|(_, n)| REC.contains(&n.to_ascii_lowercase().as_str()))
}

// ---------- windows ----------

fn window_pid(h: isize) -> u32 {
    let mut pid = 0;
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(h as _, &mut pid) };
    pid
}

fn window_text(h: isize) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowTextW(h as _, buf.as_mut_ptr(), 256) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn focus_window(h: isize) -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP, VK_MENU};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
        SetForegroundWindow, ShowWindow, SW_RESTORE,
    };
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThreadId() -> u32;
    }
    #[link(name = "user32")]
    extern "system" {
        fn AttachThreadInput(idattach: u32, idattachto: u32, fattach: i32) -> i32;
    }
    unsafe {
        let hwnd = h as windows_sys::Win32::Foundation::HWND;
        if h == 0 || IsWindow(hwnd) == 0 {
            return false;
        }
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        } else if IsWindowVisible(hwnd) == 0 {
            return false;
        }
        let fg = GetForegroundWindow();
        if fg == hwnd {
            return true;
        }
        let cur_tid = GetCurrentThreadId();
        let fg_tid = if !fg.is_null() {
            GetWindowThreadProcessId(fg, std::ptr::null_mut())
        } else {
            0
        };
        let attached = if fg_tid != 0 && fg_tid != cur_tid {
            AttachThreadInput(cur_tid, fg_tid, 1) != 0
        } else {
            false
        };
        let _ = BringWindowToTop(hwnd);
        let ok = SetForegroundWindow(hwnd) != 0;
        if attached {
            AttachThreadInput(cur_tid, fg_tid, 0);
        }
        if !ok && GetForegroundWindow() != hwnd {
            // Only fall back to VK_MENU when AttachThreadInput could not hand off foreground focus
            keybd_event(VK_MENU as u8, 0, 0, 0);
            let _ = SetForegroundWindow(hwnd);
            keybd_event(VK_MENU as u8, 0, KEYEVENTF_KEYUP, 0);
        }
    }
    true
}

/// The biggest visible, titled top-level window of a process.
fn main_window(pid: u32) -> Option<isize> {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindow, GetWindowRect, GetWindowTextLengthW, IsWindowVisible, GW_OWNER,
    };
    struct Find {
        pid: u32,
        best: Option<(isize, i64)>,
    }
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        let f = &mut *(lp as *mut Find);
        if window_pid(h as isize) == f.pid
            && IsWindowVisible(h) != 0
            && GetWindowTextLengthW(h) > 0
            && GetWindow(h, GW_OWNER).is_null()
        {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(h, &mut r);
            let area = ((r.right - r.left) as i64) * ((r.bottom - r.top) as i64);
            if f.best.map(|(_, a)| area > a).unwrap_or(true) {
                f.best = Some((h as isize, area));
            }
        }
        1
    }
    let mut f = Find { pid, best: None };
    unsafe { EnumWindows(Some(cb), &mut f as *mut _ as LPARAM) };
    f.best.map(|(h, _)| h)
}

/// (pid -> (parent pid, exe name)) for every process.
fn process_tree() -> HashMap<u32, (u32, String)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let mut out = HashMap::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                let end = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                out.insert(
                    e.th32ProcessID,
                    (e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..end])),
                );
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

fn stem_of(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

// ---------- source 1: taskbar flashes (shell hook) ----------

static SHELL_MSG: AtomicU32 = AtomicU32::new(0);
static TASKBAR_CREATED_MSG: AtomicU32 = AtomicU32::new(0);
static BURST: Mutex<Option<HashMap<isize, Instant>>> = Mutex::new(None);
static FLASH_TX: OnceLock<SyncSender<isize>> = OnceLock::new();
const TERMINALS: [&str; 10] = [
    "windowsterminal",
    "code",
    "wezterm-gui",
    "alacritty",
    "openconsole",
    "conhost",
    "cursor",
    "antigravity",
    "mintty",
    "pwsh",
];

unsafe extern "system" fn shell_proc(
    h: windows_sys::Win32::Foundation::HWND,
    msg: u32,
    wp: usize,
    lp: isize,
) -> isize {
    #[link(name = "user32")]
    extern "system" {
        fn DeregisterShellHookWindow(hwnd: windows_sys::Win32::Foundation::HWND) -> i32;
    }
    let sm = SHELL_MSG.load(Ordering::Relaxed);
    if sm != 0 && msg == sm {
        match wp as u32 {
            0x8006 => on_flash(lp),        // HSHELL_FLASH
            4 | 0x8004 => on_activate(lp), // HSHELL_WINDOWACTIVATED / HSHELL_RUDEAPPACTIVATED
            _ => {}
        }
        return 0;
    }
    let tb = TASKBAR_CREATED_MSG.load(Ordering::Relaxed);
    if tb != 0 && msg == tb {
        let _ = DeregisterShellHookWindow(h);
        let ok = windows_sys::Win32::UI::WindowsAndMessaging::RegisterShellHookWindow(h);
        log(format!(
            "notify: explorer restarted, shell hook {}",
            if ok != 0 { "re-registered" } else { "failed" }
        ));
        return 0;
    }
    // WM_POWERBROADCAST (0x0218): PBT_APMRESUMESUSPEND (0x07) / PBT_APMRESUMEAUTOMATIC (0x12)
    if msg == 0x0218 && matches!(wp, 0x07 | 0x12) {
        crate::on_system_resume("powerbroadcast");
        return 1;
    }
    // WM_DISPLAYCHANGE (0x007E) / WM_DPICHANGED (0x02E0)
    if msg == 0x007E || msg == 0x02E0 {
        crate::on_display_change();
    }
    windows_sys::Win32::UI::WindowsAndMessaging::DefWindowProcW(h, msg, wp, lp)
}

fn flash_tx() -> &'static SyncSender<isize> {
    FLASH_TX.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<isize>(128);
        std::thread::spawn(move || {
            while let Ok(hwnd) = rx.recv() {
                process_flash(hwnd);
            }
        });
        tx
    })
}

fn shell_thread() {
    let _ = flash_tx();
    std::thread::spawn(|| unsafe {
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let hinst = GetModuleHandleW(std::ptr::null());
        let cls = wide("MicaBarShellHook");
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(shell_proc);
        wc.hInstance = hinst;
        wc.lpszClassName = cls.as_ptr();
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            cls.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        let msg = RegisterWindowMessageW(wide("SHELLHOOK").as_ptr());
        SHELL_MSG.store(msg, Ordering::SeqCst);
        let tb_msg = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
        TASKBAR_CREATED_MSG.store(tb_msg, Ordering::SeqCst);
        // MicaBar runs elevated: let Explorer's (lower integrity) hook and TaskbarCreated messages through
        let filt = ChangeWindowMessageFilterEx(hwnd, msg, MSGFLT_ALLOW, std::ptr::null_mut());
        let _ = ChangeWindowMessageFilterEx(hwnd, tb_msg, MSGFLT_ALLOW, std::ptr::null_mut());
        let ok = RegisterShellHookWindow(hwnd);
        log(format!(
            "notify: shell hook {} (filter {})",
            if ok != 0 { "on" } else { "failed" },
            filt != 0
        ));
        // windows appearing anywhere (app-drawn popups such as Telegram's); out of context, so the
        // callback runs on this thread's message loop
        use windows_sys::Win32::UI::Accessibility::SetWinEventHook;
        let hook = SetWinEventHook(
            0x8002,
            0x8002,
            std::ptr::null_mut(),
            Some(show_hook),
            0,
            0,
            0x0000 | 0x0002, // OUTOFCONTEXT | SKIPOWNPROCESS
        );
        log(format!(
            "notify: window-show hook {}",
            if hook.is_null() { "failed" } else { "on" }
        ));
        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    });
}

/// Fast non-blocking burst filter on the shell hook thread; heavy Win32 / disk work runs on `flash_tx`.
fn on_flash(hwnd: isize) {
    if !settings::bool_at("notify.flash", true) || hwnd == 0 {
        return;
    }
    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() } as isize == hwnd {
        return;
    }
    // one message = a burst of 3-4 flashes within ~2 s
    {
        let mut g = BURST.lock().unwrap();
        let m = g.get_or_insert_with(HashMap::new);
        let now = Instant::now();
        let fresh = m
            .get(&hwnd)
            .map(|t| now.duration_since(*t) > Duration::from_millis(2500))
            .unwrap_or(true);
        m.insert(hwnd, now);
        if m.len() > 200 {
            m.retain(|_, t| now.duration_since(*t) < Duration::from_secs(60));
        }
        if !fresh {
            return;
        }
    }
    let _ = flash_tx().try_send(hwnd);
}

fn process_flash(hwnd: isize) {
    let pid = window_pid(hwnd);
    if pid == 0 || pid == std::process::id() {
        return;
    }
    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() } as isize == hwnd {
        return;
    }
    let path = procinfo::image_path(pid);
    let key = path.as_deref().map(stem_of).unwrap_or_default();
    if key.is_empty() {
        return;
    }
    // an AI tool rang the terminal's bell right after its own (better) note: same event
    if TERMINALS.contains(&key.as_str())
        && LAST_AI
            .lock()
            .unwrap()
            .map(|t| t.elapsed() < Duration::from_secs(8))
            .unwrap_or(false)
    {
        return;
    }
    // Telegram flashes its taskbar button AND shows its own popup, and chat apps that use toasts
    // (Slack, Discord, WhatsApp, Teams, Signal…) flash AND toast for the same message: the richer
    // note (chat + text) wins; the flash only counts when nothing else came
    let toasts = key == "telegram"
        || TOAST_OWNERS
            .lock()
            .unwrap()
            .as_ref()
            .map(|m| m.contains_key(&key))
            .unwrap_or(false);
    if toasts {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1800));
            if key == "telegram"
                && TG_LAST
                    .lock()
                    .unwrap()
                    .map(|t| t.elapsed() < Duration::from_secs(4))
                    .unwrap_or(false)
            {
                return;
            }
            if recent_toast(&key, Duration::from_secs(6)) {
                return;
            }
            flash_note(hwnd, key, path);
        });
        return;
    }
    flash_note(hwnd, key, path);
}

/// Programs (exe stems) whose toasts were seen, with the time of the latest one.
static TOAST_OWNERS: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

fn recent_toast(owner: &str, within: Duration) -> bool {
    TOAST_OWNERS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(owner))
        .map(|t| t.elapsed() < within)
        .unwrap_or(false)
}

/// A toast arrived for `owner`: remember it, and drop the program's flash note that the same
/// message caused a moment earlier (flash and toast come within a second or two of each other).
fn toast_seen(owner: &str) {
    if owner.is_empty() {
        return;
    }
    TOAST_OWNERS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(owner.to_string(), Instant::now());
    let now = now_ms();
    let mut b = book().lock().unwrap();
    let before = b.notes.len();
    let mut dropped = Vec::new();
    b.notes.retain(|n| {
        // read or not (the app may have come to the front in between): it is the same message
        let dup = n.source == "flash" && n.key == owner && n.count <= 1 && now.saturating_sub(n.at) < 8_000;
        if dup {
            dropped.push(n.id);
        }
        !dup
    });
    let changed = b.notes.len() != before;
    drop(b);
    if changed {
        log(format!("notify: toast from {owner} replaced its taskbar flash"));
        for id in dropped {
            emit("note-gone", id);
        }
    }
}

fn flash_note(hwnd: isize, key: String, path: Option<String>) {
    let wechat = key == "weixin" || key == "wechat";
    let app = if wechat {
        "微信".to_string()
    } else {
        path.as_deref()
            .and_then(procinfo::file_description)
            .map(|n| tidy_app_name(&n))
            .unwrap_or_else(|| key.clone())
    };
    let title_now = window_text(hwnd);
    let body = if wechat {
        crate::i18n::t("新消息")
    } else if !title_now.trim().is_empty() && title_now != app {
        title_now.chars().take(80).collect()
    } else {
        crate::i18n::t("需要你注意")
    };
    push(Note {
        owner: key.clone(),
        key: key.clone(),
        app: app.clone(),
        icon: path.clone(),
        title: app,
        body,
        kind: "flash".into(),
        source: "flash".into(),
        target: Target::Hwnd(hwnd, path),
        merge: format!("flash|{key}"),
        tpl: wechat.then(|| crate::i18n::t("{n} 条新消息")),
        ..Default::default()
    });
}

/// Exe stem of the program a top-level window belongs to. Store apps hosted by
/// ApplicationFrameHost count as the app inside (its CoreWindow child belongs to the real exe).
fn window_owner_stem(hwnd: isize) -> String {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::EnumChildWindows;
    if hwnd == 0 {
        return String::new();
    }
    let pid = window_pid(hwnd);
    let stem = procinfo::image_path(pid).map(|p| stem_of(&p)).unwrap_or_default();
    if stem != "applicationframehost" {
        return stem;
    }
    struct Find {
        host: u32,
        pid: u32,
    }
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        let f = &mut *(lp as *mut Find);
        let p = window_pid(h as isize);
        if p != 0 && p != f.host {
            f.pid = p;
            return 0;
        }
        1
    }
    let mut f = Find { host: pid, pid: 0 };
    unsafe { EnumChildWindows(hwnd as _, Some(cb), &mut f as *mut Find as LPARAM) };
    if f.pid == 0 {
        return stem;
    }
    procinfo::image_path(f.pid).map(|p| stem_of(&p)).unwrap_or(stem)
}

/// The program in front right now (exe stem), not MicaBar itself.
fn foreground_stem() -> String {
    let h = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() } as isize;
    if h == 0 || window_pid(h) == std::process::id() {
        return String::new();
    }
    window_owner_stem(h)
}

fn on_activate(hwnd: isize) {
    if hwnd == 0 || window_pid(hwnd) == std::process::id() {
        return;
    }
    let key = window_owner_stem(hwnd);
    if !key.is_empty() {
        mark_read_key(&key);
    }
}

// ---------- source 1b: app-drawn popups (Telegram Desktop) ----------
// With Windows notifications off, Telegram shows its own small Qt tool windows at the screen
// corner (class "Qt…QWindowToolSaveBits", ~320x80). The popup itself has no readable text, but
// the main window's chat list is exposed to UI Automation: "会话名, N 个新消息, …, 发送人：预览, 时间".
// So: popup shown -> read the chat list -> the chats whose count went up become notes.

struct TgQueue {
    running: bool,
    dirty: bool,
    pid: u32,
    path: String,
}

static TG_LAST: Mutex<Option<Instant>> = Mutex::new(None);
static TG_CHATS: Mutex<Option<HashMap<String, (u32, String)>>> = Mutex::new(None);
static TG_QUEUE: Mutex<TgQueue> = Mutex::new(TgQueue {
    running: false,
    dirty: false,
    pid: 0,
    path: String::new(),
});

unsafe extern "system" fn show_hook(
    _: windows_sys::Win32::UI::Accessibility::HWINEVENTHOOK,
    _ev: u32,
    hwnd: windows_sys::Win32::Foundation::HWND,
    obj: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    if obj != 0 || hwnd.is_null() {
        return;
    }
    let mut cls = [0u16; 64];
    let n = windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW(hwnd, cls.as_mut_ptr(), 64);
    let cls = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
    if !(cls.starts_with("Qt") && cls.ends_with("ToolSaveBits")) {
        return;
    }
    let pid = window_pid(hwnd as isize);
    let Some(path) = procinfo::image_path(pid) else { return };
    if stem_of(&path) != "telegram" {
        return;
    }
    on_tg_popup(hwnd as isize, pid, path);
}

fn split_tg_sender(s: &str) -> Option<(&str, &str)> {
    let (a, b) = s.split_once('：').or_else(|| s.split_once(": "))?;
    let a = a.trim();
    if a.is_empty() || a.chars().count() > 24 || a.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((a, b.trim()))
}

fn on_tg_popup(popup: isize, pid: u32, path: String) {
    if !settings::bool_at("notify.on", true) || !settings::bool_at("notify.flash", true) {
        return;
    }
    // "take over": the popup goes away at once, the island shows the message instead
    if app_cfg("telegram").get("takeover").and_then(|x| x.as_bool()).unwrap_or(false) {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                popup as _,
                windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE,
            )
        };
    }
    // Schedule a trailing check instead of dropping popups during the 1200ms cooldown window
    {
        let mut q = TG_QUEUE.lock().unwrap();
        q.pid = pid;
        q.path = path;
        if q.running {
            q.dirty = true;
            return;
        }
        q.running = true;
        q.dirty = false;
    }
    std::thread::spawn(|| {
        let mut trailing = false;
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let (pid, path) = {
                let mut q = TG_QUEUE.lock().unwrap();
                q.dirty = false;
                (q.pid, q.path.clone())
            };
            *TG_LAST.lock().unwrap() = Some(Instant::now());
            poll_tg_chats(pid, &path, trailing);
            std::thread::sleep(Duration::from_millis(950));
            let mut q = TG_QUEUE.lock().unwrap();
            if !q.dirty {
                q.running = false;
                break;
            }
            trailing = true;
        }
    });
}

fn poll_tg_chats(pid: u32, path: &str, trailing: bool) {
    let source_only = privacy_mode() == "source";
    let main = tg_main_window(pid);
    let chats = main.map(tg_chats).unwrap_or_default();
    let mut fresh = Vec::new();
    {
        let mut g = TG_CHATS.lock().unwrap();
        let had = g.is_some();
        let prev = g.get_or_insert_with(HashMap::new);
        if source_only {
            for val in prev.values_mut() {
                val.1.clear();
            }
        }
        for c in &chats {
            let before = prev.get(&c.name);
            let up = match before {
                Some((n, p)) => c.unread > *n || (!source_only && c.unread > 0 && *p != c.preview),
                None => had && c.unread > 0,
            };
            if up {
                fresh.push((
                    c.clone(),
                    c.unread.saturating_sub(before.map(|b| b.0).unwrap_or(0)).max(1),
                ));
            }
        }
        // no baseline yet: the newest chat with new messages (the list is sorted by time)
        if !had && fresh.is_empty() {
            if let Some(c) = chats.iter().find(|c| c.unread > 0 && !c.muted) {
                fresh.push((c.clone(), 1));
            }
        }
        for c in &chats {
            let stored_preview = if source_only { String::new() } else { c.preview.clone() };
            prev.insert(c.name.clone(), (c.unread, stored_preview));
        }
    }
    let target = Target::Hwnd(main.unwrap_or(0), Some(path.to_string()));
    if fresh.is_empty() {
        if !trailing {
            push(Note {
                owner: "telegram".into(),
                key: "telegram".into(),
                app: "Telegram".into(),
                icon: Some(path.to_string()),
                title: "Telegram".into(),
                body: crate::i18n::t("新消息"),
                kind: "msg".into(),
                source: "popup".into(),
                target,
                merge: "tg|?".into(),
                tpl: Some(crate::i18n::t("{n} 条新消息")),
                ..Default::default()
            });
        }
        return;
    }
    for (c, n) in fresh.into_iter().take(4) {
        if source_only {
            push(Note {
                owner: "telegram".into(),
                key: "telegram".into(),
                app: "Telegram".into(),
                icon: Some(path.to_string()),
                title: c.name.clone(),
                body: String::new(),
                kind: "msg".into(),
                source: "popup".into(),
                target: target.clone(),
                merge: format!("tg|{}", c.name),
                count: n,
                ..Default::default()
            });
            continue;
        }
        let (who, text) = match split_tg_sender(&c.preview) {
            Some((a, b)) => (Some(a.to_string()), b.to_string()),
            None => (None, c.preview.clone()),
        };
        push(Note {
            owner: "telegram".into(),
            key: "telegram".into(),
            app: "Telegram".into(),
            icon: Some(path.to_string()),
            title: match &who {
                Some(w) if *w != c.name => format!("{} · {w}", c.name),
                _ => c.name.clone(),
            },
            body: if text.is_empty() {
                crate::i18n::t("新消息")
            } else {
                text
            },
            kind: "msg".into(),
            source: "popup".into(),
            target: target.clone(),
            merge: format!("tg|{}", c.name),
            count: n,
            ..Default::default()
        });
    }
}

/// Telegram's main window, visible or hidden in the tray (the biggest "Qt…QWindowIcon" window).
fn tg_main_window(pid: u32) -> Option<isize> {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowRect};
    struct Find {
        pid: u32,
        best: Option<(isize, i64)>,
    }
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        let f = &mut *(lp as *mut Find);
        if window_pid(h as isize) != f.pid {
            return 1;
        }
        let mut cls = [0u16; 64];
        let n = GetClassNameW(h, cls.as_mut_ptr(), 64);
        if !String::from_utf16_lossy(&cls[..n.max(0) as usize]).ends_with("QWindowIcon") {
            return 1;
        }
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(h, &mut r);
        let area = ((r.right - r.left) as i64) * ((r.bottom - r.top) as i64);
        if f.best.map(|(_, a)| area > a).unwrap_or(true) {
            f.best = Some((h as isize, area));
        }
        1
    }
    let mut f = Find { pid, best: None };
    unsafe { EnumWindows(Some(cb), &mut f as *mut _ as LPARAM) };
    f.best.map(|(h, _)| h)
}

#[derive(Clone, Debug, Default)]
struct TgChat {
    name: String,
    unread: u32,
    muted: bool,
    preview: String,
}

/// The chat list items' accessible names (UI Automation), parsed.
fn tg_chats(main: isize) -> Vec<TgChat> {
    tg_list_names(main).iter().filter_map(|s| parse_tg_item(s)).collect()
}

fn tg_list_names(main: isize) -> Vec<String> {
    use windows::core::Interface;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationCondition, TreeScope_Descendants, UIA_ControlTypePropertyId,
        UIA_ListItemControlTypeId,
    };

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct RawVariant {
        vt: u16,
        r1: u16,
        r2: u16,
        r3: u16,
        data: [u64; 2],
    }

    struct ComGuard(bool);
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    let mut out = Vec::new();
    unsafe {
        // Balance CoUninitialize whenever CoInitializeEx succeeded (both S_OK and S_FALSE)
        let _com = ComGuard(CoInitializeEx(None, COINIT_MULTITHREADED).is_ok());
        let Ok(uia) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) else {
            return out;
        };
        let Ok(el) = uia.ElementFromHandle(windows::Win32::Foundation::HWND(main as _)) else {
            return out;
        };
        // CreatePropertyCondition(UIA_ControlTypePropertyId, VARIANT(VT_I4 = UIA_ListItemControlTypeId = 50007))
        let create_prop_cond: unsafe extern "system" fn(
            *mut core::ffi::c_void,
            i32,
            RawVariant,
            *mut *mut core::ffi::c_void,
        ) -> windows::core::HRESULT = std::mem::transmute_copy(&Interface::vtable(&uia).CreatePropertyCondition);
        let var = RawVariant {
            vt: 3, // VT_I4
            r1: 0,
            r2: 0,
            r3: 0,
            data: [UIA_ListItemControlTypeId.0 as u32 as u64, 0],
        };
        let mut raw_cond: *mut core::ffi::c_void = std::ptr::null_mut();
        let cond = if create_prop_cond(
            Interface::as_raw(&uia),
            UIA_ControlTypePropertyId.0 as i32,
            var,
            &mut raw_cond,
        )
        .is_ok()
            && !raw_cond.is_null()
        {
            Some(IUIAutomationCondition::from_raw(raw_cond))
        } else {
            uia.CreateTrueCondition().ok()
        };
        let Some(cond) = cond else { return out };
        let Ok(arr) = el.FindAll(TreeScope_Descendants, &cond) else { return out };
        let n = arr.Length().unwrap_or(0).min(150);
        for i in 0..n {
            let Ok(e) = arr.GetElement(i) else { continue };
            if e.CurrentControlType()
                .map(|t| t == UIA_ListItemControlTypeId)
                .unwrap_or(false)
            {
                if let Ok(name) = e.CurrentName() {
                    out.push(name.to_string());
                }
                if out.len() >= 60 {
                    break;
                }
            }
        }
    }
    out
}

/// "频道, 会话名, 已静音, 12 个新消息, 发送人：预览, 已收到, 昨天 9:30 PM" (fields vary; the
/// preview itself may contain ", "). Both Chinese and English UI words/separators are supported.
fn parse_tg_item(s: &str) -> Option<TgChat> {
    let is_kind = |w: &str| {
        matches!(
            w.to_ascii_lowercase().as_str(),
            "频道" | "机器人" | "群组" | "超级群组" | "channel" | "bot" | "group" | "supergroup" | "megagroup" | "forum"
        )
    };
    let is_status = |w: &str| {
        matches!(
            w.to_ascii_lowercase().as_str(),
            "已收到" | "已读" | "已发送" | "图片" | "sent" | "read" | "delivered" | "photo"
        )
    };
    let clean = |t: &str| {
        t.chars()
            .filter(|c| !matches!(*c, '\u{200e}' | '\u{200f}' | '\u{2066}'..='\u{2069}'))
            .collect::<String>()
            .trim()
            .to_string()
    };
    let fields: Vec<String> = s.split(", ").map(clean).collect();
    let mut i = 0;
    while i < fields.len() && is_kind(&fields[i]) {
        i += 1;
    }
    let name = fields.get(i)?.clone();
    if name.is_empty() {
        return None;
    }
    let is_time = |t: &str| {
        let has_clock = t
            .split(|c: char| !c.is_ascii_digit() && c != ':')
            .any(|w| w.len() >= 4 && w.contains(':'));
        has_clock || (t.contains('/') && t.chars().filter(|c| c.is_ascii_digit()).count() >= 6)
    };
    let mut unread = 0;
    let mut muted = false;
    let mut rest = Vec::new();
    for f in &fields[i + 1..] {
        let lower = f.to_ascii_lowercase();
        if let Some(n) = f
            .split_whitespace()
            .next()
            .and_then(|w| w.parse::<u32>().ok())
            .filter(|_| f.contains("新消息") || lower.contains("unread"))
        {
            unread = n;
        } else if f == "已静音" || lower == "muted" {
            muted = true;
        } else if is_status(f) || is_time(f) || f.is_empty() {
            // status / time
        } else {
            rest.push(f.clone());
        }
    }
    // reactions ("表情回应：👍 11, ❤ 4" / "Reactions: ...") come after the message: cut there
    let cut = rest
        .iter()
        .position(|f| f.starts_with("表情回应：") || f.starts_with("表情回应:") || f.starts_with("Reactions:"))
        .unwrap_or(rest.len());
    rest.truncate(cut);
    let sender_line = rest.iter().position(|f| split_tg_sender(f).is_some());
    let preview = rest[sender_line.unwrap_or(0).min(rest.len())..].join(", ");
    let preview: String = preview.split_whitespace().collect::<Vec<_>>().join(" ");
    let preview = if preview.chars().count() > 160 {
        preview.chars().take(160).collect::<String>() + "…"
    } else {
        preview
    };
    Some(TgChat {
        name,
        unread,
        muted,
        preview,
    })
}

// ---------- source 2: Windows toasts (notification database) ----------

fn wpn_db() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join(r"Microsoft\Windows\Notifications\wpndatabase.db")
}

type DbStamp = (Option<SystemTime>, u64, Option<SystemTime>, u64);

fn db_stamp(db: &Path, wal: &Path) -> DbStamp {
    let meta = |p: &Path| {
        std::fs::metadata(p)
            .ok()
            .map(|m| (m.modified().ok(), m.len()))
            .unwrap_or((None, 0))
    };
    let (wm, wl) = meta(wal);
    let (dm, dl) = meta(db);
    (wm, wl, dm, dl)
}

fn toast_thread() {
    std::thread::spawn(|| {
        let db = wpn_db();
        let wal = PathBuf::from(format!("{}-wal", db.display()));
        let mut last_stamp: Option<DbStamp> = None;
        let mut last_id: Option<i64> = None;
        let mut failed = false;
        loop {
            std::thread::sleep(Duration::from_millis(900));
            crate::perf::tick("notify");
            if !settings::bool_at("notify.on", true) || !settings::bool_at("notify.toast", true) {
                last_id = None; // start fresh when switched back on (no flood of old toasts)
                last_stamp = None;
                continue;
            }
            let stamp = db_stamp(&db, &wal);
            if stamp.0.is_none() && stamp.2.is_none() {
                continue;
            }
            if last_stamp.as_ref() == Some(&stamp) && last_id.is_some() {
                continue;
            }
            match read_toasts(&db, &mut last_id) {
                Ok(list) => {
                    // Only advance last_stamp when read_toasts() succeeded
                    last_stamp = Some(stamp);
                    failed = false;
                    for n in list {
                        push(n);
                    }
                }
                Err(e) => {
                    if !failed {
                        log(format!("notify: notification database unreadable: {e}"));
                    }
                    failed = true;
                }
            }
        }
    });
}

fn read_toasts(db: &Path, last_id: &mut Option<i64>) -> Result<Vec<Note>, String> {
    use rusqlite::{types::ValueRef, Connection, OpenFlags};
    let c = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| e.to_string())?;
    c.busy_timeout(Duration::from_millis(500)).ok();
    let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
    let Some(from) = *last_id else {
        let max: i64 = tx
            .query_row("SELECT COALESCE(MAX(Id), 0) FROM Notification", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        *last_id = Some(max);
        log(format!("notify: reading toasts from the notification database (after id {max})"));
        return Ok(vec![]);
    };
    let mut raw_toasts: Vec<(String, String)> = Vec::new();
    let mut max_id = from;
    let mut any_rows = false;
    {
        let mut st = tx
            .prepare(
                "SELECT n.Id, COALESCE(n.Type, ''), COALESCE(h.PrimaryId, ''), n.Payload \
                 FROM Notification n \
                 LEFT JOIN NotificationHandler h ON h.RecordId = n.HandlerId \
                 WHERE n.Id > ?1 \
                 ORDER BY n.Id ASC",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = st.query([from]).map_err(|e| e.to_string())?;
        while let Some(r) = rows.next().map_err(|e| e.to_string())? {
            any_rows = true;
            let id: i64 = r.get(0).map_err(|e| e.to_string())?;
            if id > max_id {
                max_id = id;
            }
            let ntype: String = r.get(1).unwrap_or_default();
            if ntype != "toast" {
                continue;
            }
            let aumid: String = r.get(2).unwrap_or_default();
            if aumid.is_empty() || aumid == crate::guard::APP_ID {
                continue;
            }
            let payload = match r.get_ref(3) {
                Ok(ValueRef::Blob(b)) => String::from_utf8_lossy(b).to_string(),
                Ok(ValueRef::Text(t)) => String::from_utf8_lossy(t).to_string(),
                _ => continue,
            };
            raw_toasts.push((aumid, payload));
        }
    }
    if any_rows {
        *last_id = Some(max_id);
    } else {
        // Detect if the notification database was cleared inside the same read snapshot
        let cur_max: i64 = tx
            .query_row("SELECT COALESCE(MAX(Id), 0) FROM Notification", [], |r| r.get(0))
            .unwrap_or(from);
        if cur_max < from {
            *last_id = Some(cur_max);
        }
    }
    drop(tx);
    drop(c);

    let mut out = Vec::new();
    for (aumid, payload) in raw_toasts {
        if let Some(n) = toast_note(&aumid, &payload) {
            if n.owner.is_empty() {
                log(format!("notify: toast from {aumid}: program not found"));
            }
            toast_seen(&n.owner);
            out.push(n);
        }
    }
    Ok(out)
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(j) = rest.find(';').filter(|&j| j <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..j];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[j + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let pat = format!("{name}=\"");
    let i = tag.find(&pat)? + pat.len();
    let j = tag[i..].find('"')? + i;
    Some(unescape(&tag[i..j]))
}

/// Texts of a toast's XML (in order) and its appLogoOverride image source.
fn parse_toast(xml: &str) -> (Vec<String>, Option<String>) {
    let mut texts = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<text") {
        rest = &rest[i + 5..];
        let Some(gt) = rest.find('>') else { break };
        if rest[..gt].ends_with('/') {
            rest = &rest[gt + 1..];
            continue;
        }
        let body = &rest[gt + 1..];
        let Some(end) = body.find("</text>") else { break };
        let t = unescape(body[..end].trim());
        if !t.is_empty() {
            texts.push(t);
        }
        rest = &body[end..];
    }
    let mut img = None;
    let mut r = xml;
    while let Some(i) = r.find("<image") {
        r = &r[i..];
        let end = r.find('>').unwrap_or(r.len());
        let tag = &r[..end];
        if img.is_none() || tag.contains("appLogoOverride") {
            if let Some(src) = attr(tag, "src") {
                img = Some(src);
                if tag.contains("appLogoOverride") {
                    break;
                }
            }
        }
        r = &r[end.min(r.len())..];
        if r.len() <= 1 {
            break;
        }
        r = &r[1..];
    }
    (texts, img)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                b'A'..=b'F' => Some(c - b'A' + 10),
                _ => None,
            };
            if let (Some(hi), Some(lo)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn file_data_url(path: &str) -> Option<String> {
    let raw = unescape(path.trim());
    let is_file_uri = raw.to_ascii_lowercase().starts_with("file:");
    let stripped = raw
        .trim_start_matches("file:///")
        .trim_start_matches("FILE:///")
        .trim_start_matches("file://")
        .trim_start_matches("FILE://");
    let stripped = stripped.split('?').next().unwrap_or(stripped);
    let decoded = percent_decode(stripped).replace('/', "\\");
    let literal = stripped.replace('/', "\\");
    let p = if is_file_uri {
        if Path::new(&decoded).is_file() {
            decoded
        } else {
            literal
        }
    } else if Path::new(&literal).is_file() {
        literal
    } else {
        decoded
    };
    let meta = std::fs::metadata(&p).ok()?;
    if meta.len() == 0 || meta.len() > 1_500_000 {
        return None;
    }
    let bytes = std::fs::read(&p).ok()?;
    let mime = if bytes.starts_with(&[0x89, b'P']) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF") {
        "image/gif"
    } else if bytes.starts_with(b"RIFF") {
        "image/webp"
    } else {
        return None;
    };
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

struct ToastApp {
    name: String,
    exe: Option<String>,
    icon_url: Option<String>,
}

static TOAST_APPS: Mutex<Option<HashMap<String, (String, Option<String>, Option<String>)>>> = Mutex::new(None);

/// "KakaoTalk(32-bit)" -> "KakaoTalk"
fn tidy_app_name(n: &str) -> String {
    let mut s = n.trim().to_string();
    for suf in ["(32-bit)", "(64-bit)", "(32 bit)", "(64 bit)", "(x86)", "(x64)"] {
        if s.to_ascii_lowercase().ends_with(suf) {
            s.truncate(s.len() - suf.len());
            s = s.trim_end().to_string();
        }
    }
    s
}

fn xml_attr(s: &str, name: &str) -> Option<String> {
    let pat = format!(" {name}=\"");
    let i = s.find(&pat)? + pat.len();
    let j = s[i..].find('"')? + i;
    Some(unescape(&s[i..j]))
}

/// "@{Pkg?ms-resource://…}" / "ms-resource:AppName" -> the text, through the shell.
fn indirect_string(src: &str, full: &str, pkg_name: &str) -> Option<String> {
    let s = if src.starts_with('@') {
        src.to_string()
    } else if let Some(r) = src.strip_prefix("ms-resource:") {
        let r = if r.starts_with("//") {
            r.to_string()
        } else if r.starts_with('/') {
            format!("//{pkg_name}{r}")
        } else {
            format!("//{pkg_name}/resources/{r}")
        };
        format!("@{{{full}?ms-resource:{r}}}")
    } else {
        return Some(src.to_string()).filter(|x| !x.trim().is_empty());
    };
    let w = wide(&s);
    let mut buf = vec![0u16; 512];
    let hr = unsafe {
        windows_sys::Win32::UI::Shell::SHLoadIndirectString(
            w.as_ptr(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            std::ptr::null(),
        )
    };
    if hr != 0 {
        return None;
    }
    let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..n])).filter(|x| !x.trim().is_empty())
}

/// The best file for a manifest logo "Assets\AppList.png": the shell keeps scale / target-size
/// variants next to it (AppList.targetsize-48_altform-unplated.png, AppList.scale-200.png…).
fn package_logo(root: &Path, rel: &str) -> Option<String> {
    let p = root.join(rel.replace('/', "\\"));
    let dir = p.parent()?;
    let stem = p.file_stem()?.to_string_lossy().to_ascii_lowercase();
    let mut best: Option<(i32, PathBuf)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let n = e.file_name().to_string_lossy().to_ascii_lowercase();
        if !n.ends_with(".png") || !(n == format!("{stem}.png") || n.starts_with(&format!("{stem}."))) {
            continue;
        }
        let mut score = 0;
        for (k, v) in [
            ("targetsize-48", 60),
            ("targetsize-64", 55),
            ("targetsize-32", 50),
            ("targetsize-96", 45),
            ("scale-200", 40),
            ("scale-150", 35),
            ("scale-100", 30),
        ] {
            if n.contains(k) {
                score = v;
                break;
            }
        }
        if n.contains("altform-unplated") || n.contains("altform-lightunplated") {
            score += 5;
        }
        if n.contains("contrast-") {
            score -= 100;
        }
        if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
            best = Some((score, e.path()));
        }
    }
    best.and_then(|(_, p)| file_data_url(&p.to_string_lossy()))
}

/// A packaged (Store / MSIX) app behind "Family!AppId": display name, main exe and logo from its
/// AppxManifest.xml (the package registry says where it is installed).
fn packaged_app(aumid: &str) -> Option<(Option<String>, Option<String>, Option<String>)> {
    use winreg::enums::HKEY_CURRENT_USER;
    let (family, app_id) = aumid.split_once('!')?;
    let (pkg_name, publisher) = family.rsplit_once('_')?;
    let repo = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages")
        .ok()?;
    let (pre, post) = (
        format!("{}_", pkg_name.to_ascii_lowercase()),
        format!("_{}", publisher.to_ascii_lowercase()),
    );
    let parse_ver = |k: &str| -> Vec<u64> {
        let rest = k.get(pre.len()..).unwrap_or("");
        let ver = rest.split('_').next().unwrap_or("");
        ver.split('.').map(|n| n.parse::<u64>().unwrap_or(0)).collect()
    };
    let full = repo
        .enum_keys()
        .flatten()
        .filter(|k| {
            let l = k.to_ascii_lowercase();
            l.starts_with(&pre) && l.ends_with(&post) && !l.contains("_neutral_~_")
        })
        .max_by(|a, b| parse_ver(a).cmp(&parse_ver(b)).then_with(|| a.cmp(b)))?;
    let k = repo.open_subkey(&full).ok()?;
    let root: String = k.get_value("PackageRootFolder").ok()?;
    let root = PathBuf::from(root);
    // a sparse package (LINE): its files live in the app's own folder (PackageRootFolder), the
    // manifest and logos in WindowsApps\<full name>
    let store = PathBuf::from(
        std::env::var("ProgramW6432")
            .or_else(|_| std::env::var("ProgramFiles"))
            .unwrap_or_default(),
    )
    .join("WindowsApps")
    .join(&full);
    let bases: Vec<PathBuf> = [root.clone(), store]
        .into_iter()
        .filter(|b| b.join("AppxManifest.xml").is_file())
        .collect();
    let manifest = std::fs::read_to_string(bases.first()?.join("AppxManifest.xml")).ok()?;
    let bases: Vec<PathBuf> = std::iter::once(root).chain(bases).collect();
    // the <Application Id="…"> element of this app
    let tag = manifest.match_indices("<Application ").map(|(i, _)| i).find(|&i| {
        let end = manifest[i..].find('>').map(|e| i + e).unwrap_or(manifest.len());
        xml_attr(&manifest[i..end], "Id").as_deref() == Some(app_id)
    })?;
    let rest = &manifest[tag..];
    let body = &rest[..rest.find("</Application>").unwrap_or(rest.len())];
    let head = &body[..body.find('>').unwrap_or(body.len())];
    let exe = xml_attr(head, "Executable")
        .and_then(|e| bases.iter().map(|b| b.join(e.replace('/', "\\"))).find(|p| p.is_file()))
        .map(|p| p.to_string_lossy().to_string());
    let ve = body
        .find("VisualElements")
        .map(|i| &body[i..body[i..].find('>').map(|e| i + e).unwrap_or(body.len())])
        .unwrap_or("");
    let reg_name: Option<String> = k.get_value("DisplayName").ok();
    let name = [xml_attr(ve, "DisplayName"), reg_name]
        .into_iter()
        .flatten()
        .filter_map(|n| indirect_string(&n, &full, pkg_name))
        .find(|n| !n.starts_with('@') && !n.starts_with("ms-resource"));
    let logo = xml_attr(ve, "Square44x44Logo").and_then(|l| bases.iter().find_map(|b| package_logo(b, &l)));
    Some((name, exe, logo))
}

/// Display name + icon for a toast AppUserModelID.
fn toast_app(aumid: &str) -> ToastApp {
    if let Some((n, e, i)) = TOAST_APPS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(aumid))
        .cloned()
    {
        return ToastApp {
            name: n,
            exe: e,
            icon_url: i,
        };
    }
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    let mut name = None;
    let mut icon_url = None;
    let mut exe = None;
    if aumid.contains('!') {
        if let Some((n, e, l)) = packaged_app(aumid) {
            name = n;
            exe = e;
            icon_url = l;
        }
    }
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        if let Ok(k) = winreg::RegKey::predef(root).open_subkey(format!(r"Software\Classes\AppUserModelId\{aumid}")) {
            if let Ok(n) = k.get_value::<String, _>("DisplayName") {
                if !n.starts_with('@') && !n.starts_with("ms-resource") && !n.is_empty() {
                    name = Some(n);
                }
            }
            if let Ok(i) = k.get_value::<String, _>("IconUri") {
                icon_url = file_data_url(&i);
            }
            break;
        }
    }
    // "{KnownFolder GUID}\path\app.exe"
    let lower = aumid.to_ascii_lowercase();
    for (guid, var) in [
        ("{6d809377-6af0-444b-8957-a3773f02200e}", "ProgramW6432"),
        ("{7c5a40ef-a0fb-4bfc-874a-c0f2e0b9fa8e}", "ProgramFiles(x86)"),
        ("{f1b32785-6fba-4fcf-9d55-7b8e7f157091}", "LOCALAPPDATA"),
        ("{f38bf404-1d43-42f2-9305-67de0b28fc23}", "SystemRoot"),
        ("{1ac14e77-02e7-4e5d-b744-2eb1ae5198b7}", "SystemRoot"),
    ] {
        if lower.starts_with(guid) {
            if let Ok(base) = std::env::var(var) {
                let base = if guid.starts_with("{1ac") {
                    format!("{base}\\System32")
                } else {
                    base
                };
                exe = Some(format!("{base}{}", &aumid[guid.len()..]));
            }
        }
    }
    if exe.as_deref().map(|p| !Path::new(p).is_file()).unwrap_or(false) {
        exe = None;
    }
    // a running program named like a whole word of the id ("Telegram.TelegramDesktop" ->
    // Telegram.exe, "org.whispersystems.signal-desktop" -> Signal.exe — not "System", which only
    // sits inside "whispersystems")
    if exe.is_none() {
        let words: Vec<&str> = lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        let mut cands: Vec<(usize, u32)> = procinfo::list_processes()
            .into_iter()
            .filter_map(|(pid, n)| {
                let s = stem_of(&n);
                let hit = s.len() >= 3 && (words.contains(&s.as_str()) || (s.len() >= 6 && words.iter().any(|w| w.starts_with(&s))));
                hit.then_some((s.len(), pid))
            })
            .collect();
        cands.sort_by(|a, b| b.0.cmp(&a.0));
        exe = cands.into_iter().find_map(|(_, pid)| procinfo::image_path(pid));
    }
    let known = name.is_some() || exe.is_some();
    let name = name
        .or_else(|| exe.as_deref().and_then(procinfo::file_description))
        .map(|n| tidy_app_name(&n))
        .unwrap_or_else(|| {
            // "Microsoft.WindowsStore_8wekyb3d8bbwe!App" -> "WindowsStore"
            let a = aumid.split('!').next().unwrap_or(aumid);
            let a = a.split('_').next().unwrap_or(a);
            let a = a.rsplit('\\').next().unwrap_or(a);
            a.trim_end_matches(".exe").rsplit('.').next().unwrap_or(a).to_string()
        });
    // a guess from the id alone is not kept: the app may be running (and found) next time
    if known {
        TOAST_APPS
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(aumid.to_string(), (name.clone(), exe.clone(), icon_url.clone()));
    }
    ToastApp { name, exe, icon_url }
}

fn toast_note(aumid: &str, xml: &str) -> Option<Note> {
    let (texts, img) = parse_toast(xml);
    if texts.is_empty() {
        return None;
    }
    let a = toast_app(aumid);
    let (title, body) = if texts.len() == 1 {
        (a.name.clone(), texts[0].clone())
    } else {
        (texts[0].clone(), texts[1..].join("\n"))
    };
    let owner = a.exe.as_deref().map(stem_of).unwrap_or_default();
    // Prefer resolved exe / window target for Win32 toast apps (and open packaged apps)
    let target = match &a.exe {
        Some(exe_path) if !aumid.contains('!') => {
            let hwnd = find_window_by_exe(exe_path).unwrap_or(0);
            Target::Hwnd(hwnd, Some(exe_path.clone()))
        }
        Some(exe_path) => match find_window_by_exe(exe_path) {
            Some(hwnd) => Target::Hwnd(hwnd, None),
            None => Target::Aumid(aumid.to_string()),
        },
        None => Target::Aumid(aumid.to_string()),
    };
    Some(Note {
        owner,
        key: aumid.to_string(),
        app: a.name,
        icon: a.exe,
        icon_url: a.icon_url,
        avatar: img.as_deref().and_then(file_data_url),
        merge: format!("toast|{aumid}|{title}"),
        title,
        body,
        kind: "msg".into(),
        source: "toast".into(),
        target,
        ..Default::default()
    })
}

// ---------- taking over Windows' own popups (per toast app, reversible) ----------

fn takeover_file() -> PathBuf {
    data_dir().join("notify-takeover.json")
}

fn takeover_cache() -> &'static Mutex<HashMap<String, Option<u32>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<u32>>>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let map = std::fs::read_to_string(takeover_file())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Mutex::new(map)
    })
}

const NOTIF_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings";

pub fn takeover_on(aumid: &str) -> bool {
    takeover_cache().lock().unwrap().contains_key(aumid)
}

/// Switches Windows' banner for a toast app off (the toast still lands in the action centre,
/// where MicaBar reads it) or restores what was there before.
pub fn set_takeover(aumid: &str, on: bool) -> Result<(), String> {
    use winreg::enums::HKEY_CURRENT_USER;
    let hk = winreg::RegKey::predef(HKEY_CURRENT_USER);
    let mut bookv = takeover_cache().lock().unwrap();
    let path = format!(r"{NOTIF_SETTINGS}\{aumid}");
    if on {
        let (k, _) = hk.create_subkey(&path).map_err(|e| e.to_string())?;
        if !bookv.contains_key(aumid) {
            bookv.insert(aumid.to_string(), k.get_value::<u32, _>("ShowBanner").ok());
        }
        k.set_value("ShowBanner", &0u32).map_err(|e| e.to_string())?;
    } else if let Some(prev) = bookv.remove(aumid) {
        if let Ok(k) = hk.open_subkey_with_flags(&path, winreg::enums::KEY_ALL_ACCESS) {
            match prev {
                Some(v) => k.set_value("ShowBanner", &v).map_err(|e| e.to_string())?,
                None => {
                    let _ = k.delete_value("ShowBanner");
                }
            }
        }
    }
    let json_bytes = serde_json::to_vec(&*bookv).unwrap_or_default();
    drop(bookv);
    settings::atomic_write(&takeover_file(), &json_bytes).map_err(|e| e.to_string())?;
    log(format!(
        "notify: windows banner for {aumid} {}",
        if on { "off (taken over)" } else { "restored" }
    ));
    Ok(())
}

// ---------- source 3: AI tools (local hook endpoint) ----------

fn token() -> String {
    static T: OnceLock<String> = OnceLock::new();
    T.get_or_init(|| {
        let p = data_dir().join("notify-token");
        if let Ok(t) = std::fs::read_to_string(&p) {
            let t = t.trim().to_string();
            if t.len() >= 16 {
                return t;
            }
        }
        // RandomState is seeded from the OS random source: two of them = 128 random bits
        use std::hash::{BuildHasher, Hasher};
        let part = || {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(now_ms());
            h.finish()
        };
        let t = format!("{:016x}{:016x}", part(), part());
        let _ = settings::atomic_write(&p, t.as_bytes());
        t
    })
    .clone()
}

fn const_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub fn port() -> u16 {
    settings::num_at("notify.port", 47900.0).clamp(1024.0, 65535.0) as u16
}

static BOUND_PORT: AtomicU16 = AtomicU16::new(0);
static ACTIVE_HANDLERS: AtomicUsize = AtomicUsize::new(0);

struct HandlerSlotGuard;
impl Drop for HandlerSlotGuard {
    fn drop(&mut self) {
        ACTIVE_HANDLERS.fetch_sub(1, Ordering::SeqCst);
    }
}

fn ai_server() {
    let _ = token(); // created now, so the hook commands can be written any time
    sync_hook_endpoints();

    // Wake blocking accept() whenever settings `notify.port` changes so we rebind cleanly
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_secs(2));
        let cur = BOUND_PORT.load(Ordering::SeqCst);
        if cur != 0 && port() != cur {
            let addr = SocketAddr::from(([127, 0, 0, 1], cur));
            let _ = TcpStream::connect_timeout(&addr, Duration::from_millis(100));
        }
    });

    std::thread::spawn(|| loop {
        let want = port();
        let l = match TcpListener::bind(("127.0.0.1", want)) {
            Ok(l) => l,
            Err(e) => {
                BOUND_PORT.store(0, Ordering::SeqCst);
                log(format!("notify: AI hook port 127.0.0.1:{want} unavailable: {e}"));
                while port() == want {
                    std::thread::sleep(Duration::from_secs(2));
                }
                continue;
            }
        };
        BOUND_PORT.store(want, Ordering::SeqCst);
        log(format!("notify: AI hook endpoint on 127.0.0.1:{want}"));
        for res in l.incoming() {
            if port() != want {
                break;
            }
            let Ok(mut s) = res else { continue };
            let prev = ACTIVE_HANDLERS.fetch_add(1, Ordering::SeqCst);
            if prev >= 16 {
                ACTIVE_HANDLERS.fetch_sub(1, Ordering::SeqCst);
                let _ = s.write_all(b"HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
                continue;
            }
            std::thread::spawn(move || {
                let _slot = HandlerSlotGuard;
                handle(s, want);
            });
        }
        BOUND_PORT.store(0, Ordering::SeqCst);
        drop(l);
        log(format!("notify: rebinding AI hook endpoint from {want} to {}", port()));
    });
}

fn read_until_deadline(s: &mut TcpStream, tmp: &mut [u8], deadline: Instant) -> Option<usize> {
    let rem = deadline.checked_duration_since(Instant::now())?;
    if rem.is_zero() {
        return None;
    }
    let _ = s.set_read_timeout(Some(rem));
    match s.read(tmp) {
        Ok(0) | Err(_) => None,
        Ok(n) => Some(n),
    }
}

fn handle(mut s: TcpStream, port: u16) {
    let deadline = Instant::now() + Duration::from_secs(3);
    let _ = s.set_write_timeout(Some(Duration::from_secs(1)));
    let mut buf = Vec::with_capacity(2048);
    let mut tmp = [0u8; 8192];
    let mut head_end = None;
    while head_end.is_none() && buf.len() < 64 * 1024 {
        let Some(n) = read_until_deadline(&mut s, &mut tmp, deadline) else { break };
        buf.extend_from_slice(&tmp[..n]);
        head_end = buf.windows(4).position(|w| w == b"\r\n\r\n");
    }
    let Some(he) = head_end else { return };
    let head = String::from_utf8_lossy(&buf[..he]).to_string();

    // 1. Reject any request with an Origin: header (blocks browser cross-origin no-cors POSTs)
    let has_origin = head.lines().skip(1).any(|l| {
        l.split_once(':')
            .map(|(k, _)| k.trim().eq_ignore_ascii_case("origin"))
            .unwrap_or(false)
    });
    if has_origin {
        let _ = s.write_all(b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
        return;
    }

    // 2. Validate method, path & token BEFORE reading body or calling client_host()
    let line = head.lines().next().unwrap_or("");
    let mut req_parts = line.split_whitespace();
    let method = req_parts.next().unwrap_or("");
    let target = req_parts.next().unwrap_or("");
    if method != "POST" {
        let _ = s.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
        return;
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let tok = query.split('&').find_map(|kv| kv.strip_prefix("t=")).unwrap_or("");
    if !const_time_eq(tok.as_bytes(), token().as_bytes()) {
        log("notify: hook call with a wrong token ignored");
        let _ = s.write_all(b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
        return;
    }
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    if parts.len() != 3 || parts[0] != "n" {
        let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
        return;
    }

    // 3. Answer Expect: 100-continue immediately so curl.exe never stalls 1000ms on >1KB payloads
    let len = head
        .lines()
        .skip(1)
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0)
        .min(2 * 1024 * 1024);
    let mut body = buf[he + 4..].to_vec();
    let expect_continue = head.lines().skip(1).any(|l| {
        l.split_once(':')
            .map(|(k, v)| k.trim().eq_ignore_ascii_case("expect") && v.to_ascii_lowercase().contains("100-continue"))
            .unwrap_or(false)
    });
    if expect_continue && body.len() < len {
        let _ = s.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
    }
    while body.len() < len {
        let Some(n) = read_until_deadline(&mut s, &mut tmp, deadline) else { break };
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(len);

    // 4. Identify caller before closing socket (curl disconnects once 204 arrives)
    let peer = s.peer_addr().map(|a| a.port()).unwrap_or(0);
    let host = client_host(peer, port);
    let _ = s.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
    drop(s);

    let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if !settings::bool_at("notify.on", true) {
        return;
    }
    let owner = match &host.target {
        Target::Hwnd(h, exe) => format!("window {h:#x} {}", exe.as_deref().map(stem_of).unwrap_or_default()),
        Target::Pid(p) => format!(
            "pid {p} {}",
            procinfo::image_path(*p).map(|x| stem_of(&x)).unwrap_or_default()
        ),
        _ => "unknown".into(),
    };
    log(format!("notify: {} {} from {owner}", parts[1], parts[2]));
    ai_event(parts[1], parts[2], &json, host);
}

struct HostInfo {
    target: Target,
    tool_exe: Option<String>,
    tool_pid: Option<(u32, u64)>,
}

/// Finds the AI session's terminal / IDE window purely via `process_tree` + `EnumWindows`
/// (zero `AttachConsole` / `FreeConsole`), and captures the live CLI `(pid, create_time)`.
fn client_host(peer_port: u16, our_port: u16) -> HostInfo {
    let pid = crate::conn::snapshot()
        .into_iter()
        .find(|c| {
            c.proto == crate::conn::Proto::Tcp
                && c.local_port == peer_port
                && c.remote.map(|(_, p)| p == our_port).unwrap_or(false)
        })
        .map(|c| c.pid);
    let Some(mut pid) = pid else {
        log(format!("notify: hook caller (port {peer_port}) not found among connections"));
        return HostInfo {
            target: Target::None,
            tool_exe: None,
            tool_pid: None,
        };
    };
    let tree = process_tree();
    let mut tool_exe = None;
    let mut tool_pid = None;
    let mut node_pid = None;
    let mut chain = Vec::new();
    let mut has_shell = false;

    for _ in 0..20 {
        let Some((parent, name)) = tree.get(&pid).cloned() else { break };
        let lname = name.to_ascii_lowercase();
        if lname == "explorer.exe" {
            break;
        }
        if matches!(
            lname.as_str(),
            "cmd.exe" | "powershell.exe" | "pwsh.exe" | "bash.exe" | "sh.exe" | "wsl.exe" | "mintty.exe"
        ) {
            has_shell = true;
        }
        if tool_pid.is_none() && (lname.starts_with("claude") || lname.starts_with("codex")) {
            tool_exe = procinfo::image_path(pid);
            tool_pid = process_create_time(pid).map(|ct| (pid, ct));
        } else if node_pid.is_none() && lname == "node.exe" {
            node_pid = process_create_time(pid).map(|ct| (pid, ct));
        }
        chain.push(pid);
        if parent == 0 || parent == pid || chain.contains(&parent) {
            break;
        }
        pid = parent;
    }
    if tool_pid.is_none() {
        tool_pid = node_pid;
    }

    // 1. Direct ancestor window in process_tree (VS Code, Cursor, Antigravity, Windows Terminal, WezTerm, Alacritty, Mintty, classic cmd/pwsh)
    for &p in &chain {
        if let Some(h) = main_window(p) {
            let exe = procinfo::image_path(p);
            return HostInfo {
                target: Target::Hwnd(h, exe),
                tool_exe,
                tool_pid,
            };
        }
    }

    // 2. PseudoConsole / ConPTY window owned by a terminal window (via EnumWindows + GA_ROOTOWNER, no AttachConsole)
    if let Some(top) = find_chain_console_owner(&chain, &tree) {
        let exe = procinfo::image_path(window_pid(top));
        return HostInfo {
            target: Target::Hwnd(top, exe),
            tool_exe,
            tool_pid,
        };
    }

    // 3. DefTerm fallback: shell started from Explorer/Win+R handed to Windows Terminal (OpenConsole.exe child of WindowsTerminal.exe)
    if has_shell {
        if let Some((top, exe)) = find_defterm_window(&tree) {
            return HostInfo {
                target: Target::Hwnd(top, exe),
                tool_exe,
                tool_pid,
            };
        }
    }

    HostInfo {
        target: Target::None,
        tool_exe,
        tool_pid,
    }
}

fn find_chain_console_owner(chain: &[u32], tree: &HashMap<u32, (u32, String)>) -> Option<isize> {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;
    struct Ctx<'a> {
        chain: &'a [u32],
        tree: &'a HashMap<u32, (u32, String)>,
        found: Option<isize>,
    }
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        let ctx = &mut *(lp as *mut Ctx<'_>);
        let wpid = window_pid(h as isize);
        if ctx.chain.contains(&wpid) {
            if let Some(top) = console_top_window(h as isize, ctx.tree) {
                ctx.found = Some(top);
                return 0;
            }
        }
        1
    }
    let mut ctx = Ctx {
        chain,
        tree,
        found: None,
    };
    unsafe { EnumWindows(Some(cb), &mut ctx as *mut Ctx<'_> as LPARAM) };
    ctx.found
}

fn console_top_window(h: isize, tree: &HashMap<u32, (u32, String)>) -> Option<isize> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetAncestor, IsWindowVisible, GA_ROOTOWNER};
    unsafe {
        let owner = GetAncestor(h as _, GA_ROOTOWNER) as isize;
        if owner != 0 && owner != h && IsWindowVisible(owner as _) != 0 {
            return Some(owner);
        }
        if IsWindowVisible(h as _) != 0 {
            return Some(h);
        }
    }
    let (parent, name) = tree.get(&window_pid(h))?;
    let name = name.to_ascii_lowercase();
    if name != "openconsole.exe" && name != "conhost.exe" {
        return None;
    }
    let p = tree.get(parent)?;
    (!p.1.eq_ignore_ascii_case("explorer.exe"))
        .then(|| main_window(*parent))
        .flatten()
}

fn find_defterm_window(tree: &HashMap<u32, (u32, String)>) -> Option<(isize, Option<String>)> {
    const HOSTS: [&str; 7] = [
        "windowsterminal.exe",
        "wezterm-gui.exe",
        "alacritty.exe",
        "mintty.exe",
        "antigravity.exe",
        "cursor.exe",
        "code.exe",
    ];
    let fg = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() } as isize;
    if fg != 0 {
        let fg_pid = window_pid(fg);
        if let Some((_, name)) = tree.get(&fg_pid) {
            if HOSTS.contains(&name.to_ascii_lowercase().as_str()) {
                return Some((fg, procinfo::image_path(fg_pid)));
            }
        }
    }
    // Windows Terminal DefTerm hosts OpenConsole.exe under WindowsTerminal.exe
    for (&pid, (_, name)) in tree {
        if name.eq_ignore_ascii_case("windowsterminal.exe") {
            if let Some(h) = main_window(pid) {
                return Some((h, procinfo::image_path(pid)));
            }
        }
    }
    None
}

static STARTS: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
static DONE_AT: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
const SESSION_MAP_TTL: Duration = Duration::from_secs(24 * 3600);

fn dur_text(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        crate::tr!("{0} 秒", s)
    } else if s < 3600 {
        crate::tr!("{0} 分 {1} 秒", s / 60, s % 60)
    } else {
        crate::tr!("{0} 小时 {1} 分", s / 3600, (s % 3600) / 60)
    }
}

fn project_of(cwd: &str) -> String {
    cwd.trim_end_matches(['\\', '/'])
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(cwd)
        .to_string()
}

/// First meaningful line of a reply (headings skipped when there is body text), markdown
/// stripped, at most ~140 characters.
fn first_line(text: &str) -> String {
    let body: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with('#')).collect();
    let lines: Vec<&str> = if body.iter().any(|l| l.trim().chars().count() >= 2) {
        body
    } else {
        text.lines().collect()
    };
    for l in lines {
        let t = l
            .trim()
            .trim_start_matches(['#', '*', '-', '>', ' '])
            .replace("**", "")
            .replace('`', "");
        let t = t.trim();
        if t.chars().count() >= 2 {
            let mut s: String = t.chars().take(140).collect();
            if t.chars().count() > 140 {
                s.push('…');
            }
            return s;
        }
    }
    String::new()
}

/// The last assistant text in a Claude Code transcript (JSON lines).
fn last_reply(transcript: &str) -> String {
    let Ok(mut f) = std::fs::File::open(transcript) else {
        return String::new();
    };
    use std::io::{Seek, SeekFrom};
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(512 * 1024);
    let _ = f.seek(SeekFrom::Start(start));
    let mut raw = Vec::new();
    let _ = f.read_to_end(&mut raw);
    let s = String::from_utf8_lossy(&raw);
    let slice = if start > 0 {
        match s.find('\n') {
            Some(i) => &s[i + 1..],
            None => &s[..],
        }
    } else {
        &s[..]
    };
    for line in slice.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("type").and_then(|x| x.as_str()) != Some("assistant") {
            continue;
        }
        let text: String = v
            .pointer("/message/content")
            .and_then(|c| c.as_array())
            .map(|a| {
                a.iter()
                    .filter(|x| x.get("type").and_then(|t| t.as_str()) == Some("text"))
                    .filter_map(|x| x.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        if !text.trim().is_empty() {
            return first_line(&text);
        }
    }
    String::new()
}

fn ai_event(tool: &str, event: &str, j: &Value, host: HostInfo) {
    let s = |k: &str| j.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let target = host.target;
    let (key, app) = match tool {
        "claude" => ("claude", "Claude Code"),
        "codex" => ("codex", "Codex"),
        _ => return,
    };
    let session = s("session_id");
    let project = |cwd: &str| if cwd.is_empty() { app.to_string() } else { project_of(cwd) };
    let source_only = privacy_mode() == "source";

    match (tool, event) {
        ("claude", "prompt") => {
            {
                let mut g = STARTS.lock().unwrap();
                let m = g.get_or_insert_with(HashMap::new);
                m.retain(|_, t| t.elapsed() < SESSION_MAP_TTL);
                m.insert(session.clone(), Instant::now());
            }
            crate::agents::started_with_pid(
                &session,
                tool,
                &project(&s("cwd")),
                &s("transcript_path"),
                target,
                host.tool_pid,
            );
        }
        ("claude", "stop") => {
            crate::agents::stopped(&session);
            let cwd = s("cwd");
            let took = {
                let mut g = STARTS.lock().unwrap();
                let m = g.get_or_insert_with(HashMap::new);
                m.retain(|_, t| t.elapsed() < SESSION_MAP_TTL);
                m.remove(&session).map(|t| t.elapsed())
            };
            {
                let mut g = DONE_AT.lock().unwrap();
                let m = g.get_or_insert_with(HashMap::new);
                m.retain(|_, t| t.elapsed() < SESSION_MAP_TTL);
                m.insert(session.clone(), Instant::now());
            }
            let reply = if source_only {
                String::new()
            } else {
                last_reply(&s("transcript_path"))
            };
            *LAST_AI.lock().unwrap() = Some(Instant::now());
            push(Note {
                key: key.into(),
                app: app.into(),
                icon: host.tool_exe,
                title: if cwd.is_empty() { app.into() } else { project_of(&cwd) },
                body: if reply.is_empty() {
                    crate::i18n::t("任务完成")
                } else {
                    reply
                },
                sub: took
                    .map(|d| crate::tr!("完成 · 用时 {0}", dur_text(d)))
                    .unwrap_or_else(|| crate::i18n::t("完成")),
                kind: "done".into(),
                source: "ai".into(),
                target,
                merge: format!("ai|claude|{session}"),
                ..Default::default()
            });
        }
        ("claude", "notify") => {
            let ntype = s("notification_type");
            if ntype == "auth_success" {
                return;
            }
            let msg = s("message");
            let lower = msg.to_ascii_lowercase();
            let (is_ask, is_idle) = match ntype.as_str() {
                "permission_prompt" | "elicitation_dialog" => (true, false),
                "idle_prompt" => (false, true),
                "" => {
                    let idle = lower.contains("waiting for your input");
                    (!idle, idle)
                }
                _ => (false, false),
            };
            // "waiting for your input" / idle_prompt means the turn is over: clear any live spinner first,
            // and skip the note if a Stop hook already announced completion within the last 10 minutes
            if is_idle {
                crate::agents::stopped(&session);
                let recent = {
                    let mut g = DONE_AT.lock().unwrap();
                    let m = g.get_or_insert_with(HashMap::new);
                    m.retain(|_, t| t.elapsed() < SESSION_MAP_TTL);
                    m.get(&session).map(|t| t.elapsed() < Duration::from_secs(600)).unwrap_or(false)
                };
                if recent {
                    return;
                }
            }
            let tool_name = msg
                .split("permission to use ")
                .nth(1)
                .map(|r| r.trim_end_matches('.').to_string());
            if is_ask {
                let what = tool_name.as_ref().map(|t| crate::tr!("批准使用 {0}", t)).unwrap_or_else(|| {
                    if msg.is_empty() {
                        crate::i18n::t("需要你看一下")
                    } else {
                        msg.clone()
                    }
                });
                crate::agents::asking_with_pid(
                    &session,
                    tool,
                    &project(&s("cwd")),
                    &what,
                    &s("transcript_path"),
                    target.clone(),
                    host.tool_pid,
                );
            }
            let body = if let Some(rest) = &tool_name {
                crate::tr!("需要你批准：使用 {0}", rest)
            } else if is_idle {
                crate::i18n::t("在等你回复")
            } else if msg.is_empty() {
                crate::i18n::t("需要你看一下")
            } else {
                msg
            };
            let cwd = s("cwd");
            *LAST_AI.lock().unwrap() = Some(Instant::now());
            push(Note {
                key: key.into(),
                app: app.into(),
                icon: host.tool_exe,
                title: if cwd.is_empty() { app.into() } else { project_of(&cwd) },
                body,
                sub: crate::i18n::t("等你确认"),
                kind: "ask".into(),
                source: "ai".into(),
                target,
                merge: format!("ai|claude|ask|{session}"),
                ..Default::default()
            });
        }
        ("codex", _) => {
            // agent-turn-complete: { cwd?, input-messages, last-assistant-message }
            let cwd = s("cwd");
            let reply = if source_only {
                String::new()
            } else {
                first_line(&s("last-assistant-message"))
            };
            *LAST_AI.lock().unwrap() = Some(Instant::now());
            push(Note {
                key: key.into(),
                app: app.into(),
                icon: host.tool_exe,
                title: if cwd.is_empty() { app.into() } else { project_of(&cwd) },
                body: if reply.is_empty() {
                    crate::i18n::t("任务完成")
                } else {
                    reply
                },
                sub: crate::i18n::t("完成"),
                kind: "done".into(),
                source: "ai".into(),
                target,
                merge: format!("ai|codex|{}", s("thread-id")),
                ..Default::default()
            });
        }
        _ => {}
    }
}

// ---------- installing the hooks ----------

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HookStatus {
    pub tool: String,
    pub name: String,
    /// the tool's config folder exists
    pub found: bool,
    pub installed: bool,
    /// a different `notify` command is already configured (Codex)
    pub conflict: bool,
    pub path: String,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into()))
}

fn claude_settings() -> PathBuf {
    std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".claude"))
        .join("settings.json")
}

fn codex_config() -> PathBuf {
    std::env::var("CODEX_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".codex"))
        .join("config.toml")
}

fn hook_url(tool: &str, event: &str) -> String {
    format!("http://127.0.0.1:{}/n/{tool}/{event}?t={}", port(), token())
}

const CLAUDE_EVENTS: [(&str, &str); 3] = [
    ("UserPromptSubmit", "prompt"),
    ("Stop", "stop"),
    ("Notification", "notify"),
];

fn is_ours(cmd: &str, tool: &str) -> bool {
    cmd.contains("127.0.0.1:") && cmd.contains(&format!("/n/{tool}/"))
}

fn claude_event_seg(event_key: &str) -> Option<&'static str> {
    CLAUDE_EVENTS.iter().find(|(k, _)| *k == event_key).map(|(_, seg)| *seg)
}

fn upgrade_claude_cmd_with(orig: &str, event_key: &str, cur_port: u16, cur_tok: &str) -> String {
    let mut next = orig.to_string();
    if next.contains("--data-binary '@-'") {
        next = next.replace("--data-binary '@-'", "--data-binary \"@-\"");
    }
    if next.contains("--data-binary @-") {
        next = next.replace("--data-binary @-", "--data-binary \"@-\"");
    }
    if !next.contains("Expect:") && next.contains("--data-binary") {
        next = next.replace("--data-binary", "-H \"Expect:\" --data-binary");
    }
    // Keep the endpoint URL in sync with the current port & token without touching removed events
    if let Some(u_start) = next.find("http://127.0.0.1:") {
        let rest = &next[u_start..];
        let u_len = rest
            .find(|c: char| matches!(c, '"' | '\'' | ' ' | '\t'))
            .unwrap_or(rest.len());
        let old_url = &rest[..u_len];
        let seg = old_url
            .split_once("/n/claude/")
            .and_then(|(_, tail)| tail.split(['?', '"', '\'', ' ']).next())
            .filter(|s| !s.is_empty())
            .or_else(|| claude_event_seg(event_key));
        if let Some(seg) = seg {
            let want_url = format!("http://127.0.0.1:{cur_port}/n/claude/{seg}?t={cur_tok}");
            let quoted = u_start > 0 && next.as_bytes()[u_start - 1] == b'"'
                && next.as_bytes().get(u_start + u_len) == Some(&b'"');
            if quoted {
                if old_url != want_url {
                    next.replace_range(u_start..u_start + u_len, &want_url);
                }
            } else {
                next.replace_range(u_start..u_start + u_len, &format!("\"{want_url}\""));
            }
        }
    }
    next
}

/// Repairs earlier MicaBar hooks in place (at start and when `notify.port` changes): quotes `@-`
/// for PowerShell, adds `-H "Expect:"` so >1KB payloads never wait for 100-continue, and refreshes
/// the port/token on already-installed hooks. Hooks the user intentionally removed (e.g. `Stop`)
/// stay removed.
fn repair_claude_hooks() {
    let p = claude_settings();
    let Ok(text) = std::fs::read_to_string(&p) else { return };
    let Ok(mut v) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) else { return };
    let cur_port = port();
    let cur_tok = token();
    let mut fixed = 0;
    if let Some(o) = v.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for (ev_key, val) in o.iter_mut() {
            let Some(groups) = val.as_array_mut() else { continue };
            for g in groups {
                for x in g.get_mut("hooks").and_then(|h| h.as_array_mut()).into_iter().flatten() {
                    let Some(orig) = x.get("command").and_then(|c| c.as_str()).map(str::to_string) else {
                        continue;
                    };
                    if !is_ours(&orig, "claude") {
                        continue;
                    }
                    let next = upgrade_claude_cmd_with(&orig, ev_key, cur_port, &cur_tok);
                    if next != orig {
                        x["command"] = Value::String(next);
                        fixed += 1;
                    }
                }
            }
        }
    }
    if fixed == 0 {
        return;
    }
    backup_once(&p);
    match serde_json::to_string_pretty(&v)
        .map_err(|e| e.to_string())
        .and_then(|out| settings::atomic_write(&p, (out + "\n").as_bytes()).map_err(|e| e.to_string()))
    {
        Ok(()) => log(format!("notify: {fixed} Claude Code hook(s) upgraded")),
        Err(e) => log(format!("notify: fixing Claude Code hooks failed: {e}")),
    }
}

fn repair_codex_hook() {
    let p = codex_config();
    let Ok(raw_text) = std::fs::read_to_string(&p) else { return };
    let has_bom = raw_text.starts_with('\u{feff}');
    let clean_text = raw_text.trim_start_matches('\u{feff}');
    let Some((start_idx, end_idx, span)) = codex_notify_span(clean_text) else { return };
    if !is_ours(&span, "codex") {
        return;
    }
    let want_line = format!(
        "notify = [\"curl.exe\", \"-s\", \"-m\", \"2\", \"-o\", \"NUL\", \"-H\", \"Expect:\", \"{}\", \"--data-binary\"]",
        hook_url("codex", "done")
    );
    if span.trim() == want_line {
        return;
    }
    backup_once(&p);
    let mut lines: Vec<String> = clean_text.lines().map(str::to_string).collect();
    lines.splice(start_idx..=end_idx, std::iter::once(want_line));
    let mut out = String::new();
    if has_bom {
        out.push('\u{feff}');
    }
    out.push_str(&lines.join("\n"));
    out.push('\n');
    match settings::atomic_write(&p, out.as_bytes()) {
        Ok(()) => log("notify: Codex notify hook upgraded"),
        Err(e) => log(format!("notify: fixing Codex hook failed: {e}")),
    }
}

/// Refreshes already-installed Claude Code / Codex hook endpoints when `notify.port` changes.
pub fn sync_hook_endpoints() {
    repair_claude_hooks();
    repair_codex_hook();
}

/// True when any MicaBar Claude Code hook is installed (including when `/hooks/Stop` was manually
/// removed while keeping `UserPromptSubmit` + `Notification`).
fn claude_installed(v: &Value) -> bool {
    v.get("hooks")
        .and_then(|h| h.as_object())
        .map(|obj| {
            obj.values().any(|arr| {
                arr.as_array()
                    .map(|groups| {
                        groups.iter().any(|g| {
                            g.get("hooks")
                                .and_then(|h| h.as_array())
                                .map(|list| {
                                    list.iter().any(|x| {
                                        x.get("command")
                                            .and_then(|c| c.as_str())
                                            .map(|c| is_ours(c, "claude"))
                                            .unwrap_or(false)
                                    })
                                })
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn strip_toml_line_comment(line: &str) -> &str {
    let mut in_double = false;
    let mut in_single = false;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if in_double {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            }
        } else if in_single {
            if ch == '\'' {
                in_single = false;
            }
        } else {
            match ch {
                '"' => in_double = true,
                '\'' => in_single = true,
                '#' => return &line[..idx],
                _ => {}
            }
        }
    }
    line
}

fn bracket_delta(s: &str) -> i32 {
    let mut delta = 0i32;
    let mut in_double = false;
    let mut in_single = false;
    let mut escaped = false;
    for ch in s.chars() {
        if in_double {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            }
        } else if in_single {
            if ch == '\'' {
                in_single = false;
            }
        } else {
            match ch {
                '"' => in_double = true,
                '\'' => in_single = true,
                '[' => delta += 1,
                ']' => delta -= 1,
                _ => {}
            }
        }
    }
    delta
}

/// Locates top-level `notify = ...` in Codex `config.toml`, supporting both single-line and
/// multi-line `notify = [ ... ]` arrays and ignoring leading UTF-8 BOM.
fn codex_notify_span(text: &str) -> Option<(usize, usize, String)> {
    let clean = text.trim_start_matches('\u{feff}');
    let lines: Vec<&str> = clean.lines().collect();
    for (i, &l) in lines.iter().enumerate() {
        let code = strip_toml_line_comment(l);
        let t = code.trim_start();
        if t.starts_with('[') {
            return None; // tables start: no top-level notify
        }
        let Some((k, rhs)) = t.split_once('=') else { continue };
        if k.trim() != "notify" {
            continue;
        }
        let mut depth = bracket_delta(rhs);
        let mut end = i;
        while depth > 0 && end + 1 < lines.len() {
            end += 1;
            depth += bracket_delta(strip_toml_line_comment(lines[end]));
        }
        let span = lines[i..=end].join("\n");
        return Some((i, end, span));
    }
    None
}

pub fn hooks_status() -> Vec<HookStatus> {
    let cs = claude_settings();
    let cv: Value = std::fs::read_to_string(&cs)
        .ok()
        .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
        .unwrap_or(Value::Null);
    let xc = codex_config();
    let xt = std::fs::read_to_string(&xc).unwrap_or_default();
    let xs = codex_notify_span(&xt);
    vec![
        HookStatus {
            tool: "claude".into(),
            name: "Claude Code".into(),
            found: cs.parent().map(|p| p.is_dir()).unwrap_or(false),
            installed: claude_installed(&cv),
            conflict: false,
            path: cs.display().to_string(),
        },
        HookStatus {
            tool: "codex".into(),
            name: "Codex".into(),
            found: xc.parent().map(|p| p.is_dir()).unwrap_or(false),
            installed: xs.as_ref().map(|(_, _, s)| is_ours(s, "codex")).unwrap_or(false),
            conflict: xs.as_ref().map(|(_, _, s)| !is_ours(s, "codex")).unwrap_or(false),
            path: xc.display().to_string(),
        },
    ]
}

fn backup_once(p: &Path) {
    let b = PathBuf::from(format!("{}.micabar-backup", p.display()));
    if p.is_file() && !b.exists() {
        let _ = std::fs::copy(p, &b);
    }
}

/// Adds or removes MicaBar's hooks in the tool's own config (backed up once before the first change).
pub fn set_hook(tool: &str, on: bool) -> Result<String, String> {
    match tool {
        "claude" => {
            let p = claude_settings();
            if !p.parent().map(|d| d.is_dir()).unwrap_or(false) {
                return Err(crate::i18n::t("没找到 Claude Code 的配置文件夹（~/.claude）"));
            }
            let text = std::fs::read_to_string(&p).unwrap_or_else(|_| "{}".into());
            let mut v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
                .map_err(|e| crate::tr!("settings.json 不是有效的 JSON：{0}", e))?;
            if !v.is_object() {
                return Err(crate::i18n::t("settings.json 的格式不对"));
            }
            backup_once(&p);
            if v.get("hooks").map(|h| !h.is_object()).unwrap_or(true) {
                v["hooks"] = json!({});
            }
            // First strip our earlier entries across every event key in `hooks` (even non-default keys)
            if let Some(obj) = v["hooks"].as_object_mut() {
                let keys: Vec<String> = obj.keys().cloned().collect();
                for k in keys {
                    let Some(arr) = obj.get(&k).and_then(|a| a.as_array()).cloned() else {
                        continue;
                    };
                    let kept: Vec<Value> = arr
                        .into_iter()
                        .filter_map(|mut g| {
                            if let Some(h) = g.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                                h.retain(|x| {
                                    !x.get("command")
                                        .and_then(|c| c.as_str())
                                        .map(|c| is_ours(c, "claude"))
                                        .unwrap_or(false)
                                });
                                if h.is_empty() {
                                    return None;
                                }
                            }
                            Some(g)
                        })
                        .collect();
                    if kept.is_empty() {
                        obj.remove(&k);
                    } else {
                        obj.insert(k, Value::Array(kept));
                    }
                }
            }
            if on {
                for (ev, name) in CLAUDE_EVENTS {
                    let mut kept = v["hooks"]
                        .get(ev)
                        .and_then(|a| a.as_array())
                        .cloned()
                        .unwrap_or_default();
                    // Claude Code runs hooks through PowerShell, where a bare @- is a parse error;
                    // -H "Expect:" avoids curl's 100-continue wait on >1KB hook JSON payloads.
                    let cmd = format!(
                        "curl.exe -s -m 2 -o NUL -H \"Expect:\" --data-binary \"@-\" \"{}\"",
                        hook_url("claude", name)
                    );
                    kept.push(json!({ "hooks": [{ "type": "command", "command": cmd }] }));
                    v["hooks"][ev] = Value::Array(kept);
                }
            }
            if v["hooks"].as_object().map(|o| o.is_empty()).unwrap_or(false) {
                if let Some(o) = v.as_object_mut() {
                    o.remove("hooks");
                }
            }
            let out = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
            settings::atomic_write(&p, (out + "\n").as_bytes()).map_err(|e| e.to_string())?;
            log(format!("notify: Claude Code hooks {}", if on { "installed" } else { "removed" }));
            Ok(if on {
                crate::i18n::t("已加到 Claude Code（新开的会话生效；已开着的会话里输入 /hooks 看一眼也会生效）")
            } else {
                crate::i18n::t("已从 Claude Code 移除")
            })
        }
        "codex" => {
            let p = codex_config();
            if !p.parent().map(|d| d.is_dir()).unwrap_or(false) {
                return Err(crate::i18n::t("没找到 Codex 的配置文件夹（~/.codex）"));
            }
            let raw_text = std::fs::read_to_string(&p).unwrap_or_default();
            let has_bom = raw_text.starts_with('\u{feff}');
            let clean_text = raw_text.trim_start_matches('\u{feff}');
            let span = codex_notify_span(clean_text);
            if let Some((_, _, ref s)) = span {
                if !is_ours(s, "codex") {
                    return Err(crate::i18n::t(
                        "config.toml 里已经有别的 notify 设置，MicaBar 不覆盖它（可以手动合并）",
                    ));
                }
            }
            backup_once(&p);
            let mut lines: Vec<String> = clean_text.lines().map(|s| s.to_string()).collect();
            if let Some((start_idx, end_idx, _)) = span {
                lines.drain(start_idx..=end_idx);
            }
            if on {
                // Codex appends the event JSON as the last argument: it becomes --data-binary's value
                let l = format!(
                    "notify = [\"curl.exe\", \"-s\", \"-m\", \"2\", \"-o\", \"NUL\", \"-H\", \"Expect:\", \"{}\", \"--data-binary\"]",
                    hook_url("codex", "done")
                );
                lines.insert(0, l);
            }
            let mut out = String::new();
            if has_bom && (!lines.is_empty() || on) {
                out.push('\u{feff}');
            }
            out.push_str(&lines.join("\n"));
            if !lines.is_empty() {
                out.push('\n');
            }
            settings::atomic_write(&p, out.as_bytes()).map_err(|e| e.to_string())?;
            log(format!("notify: Codex notify {}", if on { "installed" } else { "removed" }));
            Ok(if on {
                crate::i18n::t("已加到 Codex（重新打开 Codex 后生效）")
            } else {
                crate::i18n::t("已从 Codex 移除")
            })
        }
        _ => Err(crate::i18n::t("未知的工具")),
    }
}

// ---------- start ----------

pub fn start(app: AppHandle) {
    let _ = APP.set(app);
    shell_thread();
    toast_thread();
    ai_server();
    crate::agents::start();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_installed_without_stop_hook() {
        let partial = json!({
            "hooks": {
                "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "curl.exe -s -m 2 -o NUL -H \"Expect:\" --data-binary \"@-\" \"http://127.0.0.1:47900/n/claude/prompt?t=abc\"" }] }],
                "Notification": [{ "hooks": [{ "type": "command", "command": "curl.exe -s -m 2 -o NUL -H \"Expect:\" --data-binary \"@-\" \"http://127.0.0.1:47900/n/claude/notify?t=abc\"" }] }]
            }
        });
        assert!(claude_installed(&partial));

        let foreign = json!({
            "hooks": {
                "Stop": [{ "hooks": [{ "type": "command", "command": "echo done" }] }]
            }
        });
        assert!(!claude_installed(&foreign));
    }

    #[test]
    fn upgrades_claude_cmd_ps_safe_and_port() {
        let old = "curl.exe -s -m 2 -o NUL --data-binary @- http://127.0.0.1:47900/n/claude/prompt?t=oldtok";
        let up = upgrade_claude_cmd_with(old, "UserPromptSubmit", 47901, "newtok");
        assert_eq!(
            up,
            "curl.exe -s -m 2 -o NUL -H \"Expect:\" --data-binary \"@-\" \"http://127.0.0.1:47901/n/claude/prompt?t=newtok\""
        );
        // Idempotent once upgraded
        assert_eq!(upgrade_claude_cmd_with(&up, "UserPromptSubmit", 47901, "newtok"), up);
    }
}
