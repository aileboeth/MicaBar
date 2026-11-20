//! Notifications in the island. Three sources:
//! - Windows toasts, read from the system's notification database (Telegram, mail, browsers…);
//! - taskbar flashes through a shell hook (WeChat 4.x signals new messages only that way: no
//!   toast, no readable window, so only "微信 · N 条新消息" can be shown);
//! - AI tools: Claude Code hooks / Codex `notify` post to a local port (127.0.0.1 + a token).
//! Notes live in memory only (message text is never written to disk or the log).

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

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
    /// toast | flash | ai
    pub source: String,
    pub at: u64,
    pub count: u32,
    /// kept for the notification centre only (muted app, do not disturb, island switched off)
    pub silent: bool,
    pub read: bool,
    /// a screen recorder / sharing tool is running (the page hides the text in "smart" mode)
    pub capturing: bool,
    #[serde(skip)]
    pub target: Target,
    /// notes with the same merge key (and still unread) become one, counted
    #[serde(skip)]
    pub merge: String,
    /// body for a merged note: "{n}" = count
    #[serde(skip)]
    pub tpl: Option<String>,
}

#[derive(Clone, Debug, Default)]
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

fn emit<S: Serialize + Clone>(ev: &str, v: S) {
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

// ---------- the book ----------

fn app_cfg(key: &str) -> Value {
    settings::get().pointer("/notify/apps").and_then(|a| a.get(key)).cloned().unwrap_or(Value::Null)
}

pub fn push(mut n: Note) {
    if !settings::bool_at("notify.on", true) {
        return;
    }
    let cfg = app_cfg(&n.key);
    let mute = cfg.get("mute").and_then(|x| x.as_bool()).unwrap_or(false);
    let island = cfg.get("island").and_then(|x| x.as_bool()).unwrap_or(true);
    n.silent = mute || !island || quiet_now();
    n.capturing = capturing();
    n.at = now_ms();
    n.count = n.count.max(1);
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
        if let Some(pos) = b.notes.iter().position(|x| !x.read && x.merge == n.merge && at.saturating_sub(x.at) < 30 * 60_000) {
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
    if n.id == 0 {
        n.id = NEXT.fetch_add(1, Ordering::SeqCst);
    }
    b.notes.push_front(n.clone());
    b.notes.truncate(300);
    drop(b);
    log(format!("note: {} ({}, {}) x{}{}", n.key, n.source, n.kind, n.count, if n.silent { " silent" } else { "" }));
    emit("note", n);
}

/// The app was brought to the front: its notes count as read.
fn mark_read_key(key: &str) {
    let mut b = book().lock().unwrap();
    let mut any = false;
    for n in b.notes.iter_mut().filter(|n| n.key == key && !n.read) {
        n.read = true;
        any = true;
    }
    drop(b);
    if any {
        emit("notes-read", key.to_string());
    }
}

pub fn list() -> Vec<Note> {
    book().lock().unwrap().notes.iter().cloned().collect()
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
        let n = b.notes.iter_mut().find(|n| n.id == id).ok_or("这条通知已经不在了")?;
        n.read = true;
        n.target.clone()
    };
    match target {
        Target::Hwnd(h, exe) => {
            if !focus_window(h) {
                if let Some(p) = exe {
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
            let _ = std::process::Command::new("explorer.exe").arg(format!("shell:AppsFolder\\{a}")).spawn();
        }
        Target::None => {}
    }
    Ok(())
}

/// A sample, so the look can be tried from the settings.
pub fn test() {
    let samples = [
        ("微信", "张三", "晚上一起吃饭吗？我在楼下等你，顺便带一下钥匙", "msg"),
        ("Claude Code", "MicaBar", "通知模块写好了，虚拟机里验证通过", "done"),
        ("Telegram", "李四", "文件发你了，看一下", "msg"),
    ];
    static K: AtomicU32 = AtomicU32::new(0);
    let i = K.fetch_add(1, Ordering::SeqCst) as usize % samples.len();
    let (app, title, body, kind) = samples[i];
    push(Note {
        key: format!("test-{i}"),
        app: app.into(),
        title: title.into(),
        body: body.into(),
        sub: if kind == "done" { "完成 · 用时 3 分 12 秒".into() } else { String::new() },
        kind: kind.into(),
        source: "test".into(),
        merge: format!("test-{i}"),
        ..Default::default()
    });
}

// ---------- do not disturb / privacy ----------

fn windows_dnd() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    let k = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings");
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
    const REC: [&str; 7] = ["obs64.exe", "obs32.exe", "obs.exe", "streamlabs obs.exe", "cpthost.exe", "bdcam.exe", "xsplit.core.exe"];
    procinfo::list_processes().iter().any(|(_, n)| REC.contains(&n.to_ascii_lowercase().as_str()))
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
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow, IsWindowVisible, SetForegroundWindow, ShowWindow, SW_RESTORE};
    unsafe {
        if h == 0 || IsWindow(h as _) == 0 {
            return false;
        }
        if IsIconic(h as _) != 0 {
            ShowWindow(h as _, SW_RESTORE);
        } else if IsWindowVisible(h as _) == 0 {
            return false;
        }
        // a tapped Alt lets a background process hand the foreground over
        keybd_event(VK_MENU as u8, 0, 0, 0);
        keybd_event(VK_MENU as u8, 0, KEYEVENTF_KEYUP, 0);
        SetForegroundWindow(h as _);
    }
    true
}

/// The biggest visible, titled top-level window of a process.
fn main_window(pid: u32) -> Option<isize> {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindow, GetWindowRect, GetWindowTextLengthW, IsWindowVisible, GW_OWNER};
    struct Find {
        pid: u32,
        best: Option<(isize, i64)>,
    }
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        let f = &mut *(lp as *mut Find);
        if window_pid(h as isize) == f.pid && IsWindowVisible(h) != 0 && GetWindowTextLengthW(h) > 0 && GetWindow(h, GW_OWNER).is_null() {
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
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
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
                out.insert(e.th32ProcessID, (e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..end])));
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
    std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

// ---------- source 1: taskbar flashes (shell hook) ----------

static SHELL_MSG: AtomicU32 = AtomicU32::new(0);
static BURST: Mutex<Option<HashMap<isize, Instant>>> = Mutex::new(None);
const TERMINALS: [&str; 7] = ["windowsterminal", "code", "wezterm-gui", "alacritty", "openconsole", "conhost", "cursor"];

unsafe extern "system" fn shell_proc(h: windows_sys::Win32::Foundation::HWND, msg: u32, wp: usize, lp: isize) -> isize {
    let sm = SHELL_MSG.load(Ordering::Relaxed);
    if sm != 0 && msg == sm {
        match wp as u32 {
            0x8006 => on_flash(lp), // HSHELL_FLASH
            4 | 0x8004 => on_activate(lp), // HSHELL_WINDOWACTIVATED / HSHELL_RUDEAPPACTIVATED
            _ => {}
        }
        return 0;
    }
    windows_sys::Win32::UI::WindowsAndMessaging::DefWindowProcW(h, msg, wp, lp)
}

fn shell_thread() {
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
        let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, cls.as_ptr(), std::ptr::null(), WS_POPUP, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), hinst, std::ptr::null());
        let msg = RegisterWindowMessageW(wide("SHELLHOOK").as_ptr());
        SHELL_MSG.store(msg, Ordering::SeqCst);
        // MicaBar runs elevated: let Explorer's (lower integrity) hook messages through
        let filt = ChangeWindowMessageFilterEx(hwnd, msg, MSGFLT_ALLOW, std::ptr::null_mut());
        let ok = RegisterShellHookWindow(hwnd);
        log(format!("notify: shell hook {} (filter {})", if ok != 0 { "on" } else { "failed" }, filt != 0));
        // windows appearing anywhere (app-drawn popups such as Telegram's); out of context, so the
        // callback runs on this thread's message loop
        use windows_sys::Win32::UI::Accessibility::SetWinEventHook;
        let hook = SetWinEventHook(0x8002, 0x8002, std::ptr::null_mut(), Some(show_hook), 0, 0, 0x0000 | 0x0002); // OUTOFCONTEXT | SKIPOWNPROCESS
        log(format!("notify: window-show hook {}", if hook.is_null() { "failed" } else { "on" }));
        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    });
}

fn on_flash(hwnd: isize) {
    if !settings::bool_at("notify.flash", true) || hwnd == 0 {
        return;
    }
    let pid = window_pid(hwnd);
    if pid == std::process::id() || unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() } as isize == hwnd {
        return;
    }
    // one message = a burst of 3-4 flashes within ~2 s
    {
        let mut g = BURST.lock().unwrap();
        let m = g.get_or_insert_with(HashMap::new);
        let now = Instant::now();
        let fresh = m.get(&hwnd).map(|t| now.duration_since(*t) > Duration::from_millis(2500)).unwrap_or(true);
        m.insert(hwnd, now);
        if m.len() > 200 {
            m.retain(|_, t| now.duration_since(*t) < Duration::from_secs(60));
        }
        if !fresh {
            return;
        }
    }
    let path = procinfo::image_path(pid);
    let key = path.as_deref().map(stem_of).unwrap_or_default();
    if key.is_empty() {
        return;
    }
    // an AI tool rang the terminal's bell right after its own (better) note: same event
    if TERMINALS.contains(&key.as_str()) && LAST_AI.lock().unwrap().map(|t| t.elapsed() < Duration::from_secs(8)).unwrap_or(false) {
        return;
    }
    // Telegram flashes its taskbar button AND shows its own popup: the popup's note (with the
    // chat and text) wins; the flash only counts when no popup came
    if key == "telegram" {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            if TG_LAST.lock().unwrap().map(|t| t.elapsed() < Duration::from_secs(4)).unwrap_or(false) {
                return;
            }
            flash_note(hwnd, key, path);
        });
        return;
    }
    flash_note(hwnd, key, path);
}

fn flash_note(hwnd: isize, key: String, path: Option<String>) {
    let wechat = key == "weixin" || key == "wechat";
    let app = if wechat {
        "微信".to_string()
    } else {
        path.as_deref().and_then(procinfo::file_description).unwrap_or_else(|| key.clone())
    };
    let title_now = window_text(hwnd);
    let body = if wechat {
        "新消息".to_string()
    } else if !title_now.trim().is_empty() && title_now != app {
        title_now.chars().take(80).collect()
    } else {
        "需要你注意".to_string()
    };
    push(Note {
        key: key.clone(),
        app: app.clone(),
        icon: path.clone(),
        title: app,
        body,
        kind: "flash".into(),
        source: "flash".into(),
        target: Target::Hwnd(hwnd, path),
        merge: format!("flash|{key}"),
        tpl: wechat.then(|| "{n} 条新消息".to_string()),
        ..Default::default()
    });
}

fn on_activate(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let pid = window_pid(hwnd);
    if pid == std::process::id() {
        return;
    }
    if let Some(p) = procinfo::image_path(pid) {
        let key = stem_of(&p);
        if !key.is_empty() {
            mark_read_key(&key);
        }
    }
}

// ---------- source 1b: app-drawn popups (Telegram Desktop) ----------
// With Windows notifications off, Telegram shows its own small Qt tool windows at the screen
// corner (class "Qt…QWindowToolSaveBits", ~320x80). The popup itself has no readable text, but
// the main window's chat list is exposed to UI Automation: "会话名, N 个新消息, …, 发送人：预览, 时间".
// So: popup shown -> read the chat list -> the chats whose count went up become notes.

static TG_LAST: Mutex<Option<Instant>> = Mutex::new(None);
static TG_CHATS: Mutex<Option<HashMap<String, (u32, String)>>> = Mutex::new(None);

unsafe extern "system" fn show_hook(_: windows_sys::Win32::UI::Accessibility::HWINEVENTHOOK, _ev: u32, hwnd: windows_sys::Win32::Foundation::HWND, obj: i32, _child: i32, _thread: u32, _time: u32) {
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

fn on_tg_popup(popup: isize, pid: u32, path: String) {
    if !settings::bool_at("notify.on", true) || !settings::bool_at("notify.flash", true) {
        return;
    }
    // "take over": the popup goes away at once, the island shows the message instead
    if app_cfg("telegram").get("takeover").and_then(|x| x.as_bool()).unwrap_or(false) {
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(popup as _, windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE) };
    }
    // several popups at once (and the "N more" bar) = one read
    {
        let mut g = TG_LAST.lock().unwrap();
        if g.map(|t| t.elapsed() < Duration::from_millis(1200)).unwrap_or(false) {
            return;
        }
        *g = Some(Instant::now());
    }
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(250));
        let main = tg_main_window(pid);
        let chats = main.map(tg_chats).unwrap_or_default();
        let mut fresh = Vec::new();
        {
            let mut g = TG_CHATS.lock().unwrap();
            let had = g.is_some();
            let prev = g.get_or_insert_with(HashMap::new);
            for c in &chats {
                let before = prev.get(&c.name);
                let up = match before {
                    Some((n, p)) => c.unread > *n || (c.unread > 0 && *p != c.preview),
                    None => had && c.unread > 0,
                };
                if up {
                    fresh.push((c.clone(), c.unread.saturating_sub(before.map(|b| b.0).unwrap_or(0)).max(1)));
                }
            }
            // no baseline yet: the newest chat with new messages (the list is sorted by time)
            if !had && fresh.is_empty() {
                if let Some(c) = chats.iter().find(|c| c.unread > 0 && !c.muted) {
                    fresh.push((c.clone(), 1));
                }
            }
            for c in &chats {
                prev.insert(c.name.clone(), (c.unread, c.preview.clone()));
            }
        }
        let target = Target::Hwnd(main.unwrap_or(0), Some(path.clone()));
        if fresh.is_empty() {
            push(Note {
                key: "telegram".into(),
                app: "Telegram".into(),
                icon: Some(path.clone()),
                title: "Telegram".into(),
                body: "新消息".into(),
                kind: "msg".into(),
                source: "popup".into(),
                target,
                merge: "tg|?".into(),
                tpl: Some("{n} 条新消息".into()),
                ..Default::default()
            });
            return;
        }
        for (c, n) in fresh.into_iter().take(4) {
            let (who, text) = match c.preview.split_once('：') {
                Some((a, b)) if a.chars().count() <= 24 && !a.is_empty() => (Some(a.trim().to_string()), b.trim().to_string()),
                _ => (None, c.preview.clone()),
            };
            push(Note {
                key: "telegram".into(),
                app: "Telegram".into(),
                icon: Some(path.clone()),
                title: match &who {
                    Some(w) if *w != c.name => format!("{} · {w}", c.name),
                    _ => c.name.clone(),
                },
                body: if text.is_empty() { "新消息".into() } else { text },
                kind: "msg".into(),
                source: "popup".into(),
                target: target.clone(),
                merge: format!("tg|{}", c.name),
                count: n,
                ..Default::default()
            });
        }
    });
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
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, TreeScope_Descendants, UIA_ListItemControlTypeId};
    let mut out = Vec::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(uia) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) else { return out };
        let Ok(el) = uia.ElementFromHandle(windows::Win32::Foundation::HWND(main as _)) else { return out };
        let Ok(cond) = uia.CreateTrueCondition() else { return out };
        let Ok(arr) = el.FindAll(TreeScope_Descendants, &cond) else { return out };
        let n = arr.Length().unwrap_or(0).min(600);
        for i in 0..n {
            let Ok(e) = arr.GetElement(i) else { continue };
            if e.CurrentControlType().map(|t| t == UIA_ListItemControlTypeId).unwrap_or(false) {
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
/// preview itself may contain ", "). English UI words are understood too.
fn parse_tg_item(s: &str) -> Option<TgChat> {
    const KIND: [&str; 8] = ["频道", "机器人", "群组", "超级群组", "channel", "bot", "group", "Channel"];
    const STATUS: [&str; 8] = ["已收到", "已读", "已发送", "图片", "sent", "read", "delivered", "Photo"];
    let clean = |t: &str| t.chars().filter(|c| !matches!(*c, '\u{200e}' | '\u{200f}' | '\u{2066}'..='\u{2069}')).collect::<String>().trim().to_string();
    let fields: Vec<String> = s.split(", ").map(clean).collect();
    let mut i = 0;
    while i < fields.len() && KIND.contains(&fields[i].as_str()) {
        i += 1;
    }
    let name = fields.get(i)?.clone();
    if name.is_empty() {
        return None;
    }
    let is_time = |t: &str| {
        let has_clock = t.split(|c: char| !c.is_ascii_digit() && c != ':').any(|w| w.len() >= 4 && w.contains(':'));
        has_clock || t.contains('/') && t.chars().filter(|c| c.is_ascii_digit()).count() >= 6
    };
    let mut unread = 0;
    let mut muted = false;
    let mut rest = Vec::new();
    for f in &fields[i + 1..] {
        let lower = f.to_ascii_lowercase();
        if let Some(n) = f.split_whitespace().next().and_then(|w| w.parse::<u32>().ok()).filter(|_| f.contains("新消息") || lower.contains("unread")) {
            unread = n;
        } else if f == "已静音" || lower == "muted" {
            muted = true;
        } else if STATUS.contains(&f.as_str()) || is_time(f) || f.is_empty() {
            // status / time
        } else {
            rest.push(f.clone());
        }
    }
    // reactions ("表情回应：👍 11, ❤ 4") come after the message: cut there; a message containing
    // ", " was split into several fields: join them again; in groups it starts at "发送人：…"
    let cut = rest.iter().position(|f| f.starts_with("表情回应：") || f.starts_with("Reactions:")).unwrap_or(rest.len());
    rest.truncate(cut);
    let sender_line = rest.iter().position(|f| f.split_once('：').map(|(a, _)| !a.is_empty() && a.chars().count() <= 24).unwrap_or(false));
    let preview = rest[sender_line.unwrap_or(0).min(rest.len())..].join(", ");
    let preview: String = preview.split_whitespace().collect::<Vec<_>>().join(" ");
    let preview = if preview.chars().count() > 160 { preview.chars().take(160).collect::<String>() + "…" } else { preview };
    Some(TgChat { name, unread, muted, preview })
}

// ---------- source 2: Windows toasts (notification database) ----------

fn wpn_db() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join(r"Microsoft\Windows\Notifications\wpndatabase.db")
}

fn toast_thread() {
    std::thread::spawn(|| {
        let db = wpn_db();
        let wal = PathBuf::from(format!("{}-wal", db.display()));
        let mut last_m = None;
        let mut last_id: Option<i64> = None;
        let mut failed = false;
        loop {
            std::thread::sleep(Duration::from_millis(900));
            if !settings::bool_at("notify.on", true) || !settings::bool_at("notify.toast", true) {
                last_id = None; // start fresh when switched back on (no flood of old toasts)
                continue;
            }
            let m = std::fs::metadata(&wal).or_else(|_| std::fs::metadata(&db)).and_then(|m| m.modified()).ok();
            if m.is_some() && m == last_m && last_id.is_some() {
                continue;
            }
            last_m = m;
            match read_toasts(&db, &mut last_id) {
                Ok(list) => {
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

fn read_toasts(db: &std::path::Path, last_id: &mut Option<i64>) -> Result<Vec<Note>, String> {
    use rusqlite::{types::ValueRef, Connection, OpenFlags};
    let c = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| e.to_string())?;
    c.busy_timeout(Duration::from_millis(500)).ok();
    let max: i64 = c.query_row("select coalesce(max(Id), 0) from Notification", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    let Some(from) = *last_id else {
        *last_id = Some(max);
        log(format!("notify: reading toasts from the notification database (after id {max})"));
        return Ok(vec![]);
    };
    if max <= from {
        if max < from {
            *last_id = Some(max); // the database was cleared
        }
        return Ok(vec![]);
    }
    let mut st = c
        .prepare("select n.Id, h.PrimaryId, n.Payload from Notification n join NotificationHandler h on h.RecordId = n.HandlerId where n.Id > ?1 and n.Type = 'toast' order by n.Id")
        .map_err(|e| e.to_string())?;
    let mut rows = st.query([from]).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().map_err(|e| e.to_string())? {
        let aumid: String = r.get(1).unwrap_or_default();
        let payload = match r.get_ref(2) {
            Ok(ValueRef::Blob(b)) => String::from_utf8_lossy(b).to_string(),
            Ok(ValueRef::Text(t)) => String::from_utf8_lossy(t).to_string(),
            _ => continue,
        };
        if aumid == crate::guard::APP_ID {
            continue; // our own leak alerts
        }
        if let Some(n) = toast_note(&aumid, &payload) {
            out.push(n);
        }
    }
    *last_id = Some(max);
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

fn file_data_url(path: &str) -> Option<String> {
    let p = path.trim_start_matches("file:///").trim_start_matches("file://").replace('/', "\\");
    let p = unescape(&p).replace("%20", " ");
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
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&bytes)))
}

struct ToastApp {
    name: String,
    exe: Option<String>,
    icon_url: Option<String>,
}

static TOAST_APPS: Mutex<Option<HashMap<String, (String, Option<String>, Option<String>)>>> = Mutex::new(None);

/// Display name + icon for a toast AppUserModelID.
fn toast_app(aumid: &str) -> ToastApp {
    if let Some((n, e, i)) = TOAST_APPS.lock().unwrap().as_ref().and_then(|m| m.get(aumid)).cloned() {
        return ToastApp { name: n, exe: e, icon_url: i };
    }
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    let mut name = None;
    let mut icon_url = None;
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
    let mut exe = None;
    let lower = aumid.to_ascii_lowercase();
    for (guid, var) in [
        ("{6d809377-6af0-444b-8957-a3773f02200e}", "ProgramW6432"),
        ("{7c5a40ef-a0fb-4bfc-874a-c0f2e0b9fa8e}", "ProgramFiles(x86)"),
        ("{f38bf404-1d43-42f2-9305-67de0b28fc23}", "SystemRoot"),
        ("{1ac14e77-02e7-4e5d-b744-2eb1ae5198b7}", "SystemRoot"),
    ] {
        if lower.starts_with(guid) {
            if let Ok(base) = std::env::var(var) {
                let base = if guid.starts_with("{1ac") { format!("{base}\\System32") } else { base };
                exe = Some(format!("{base}{}", &aumid[guid.len()..]));
            }
        }
    }
    if exe.as_deref().map(|p| !std::path::Path::new(p).is_file()).unwrap_or(false) {
        exe = None;
    }
    // a running program whose name is part of the id ("Telegram.TelegramDesktop" -> Telegram.exe)
    if exe.is_none() {
        let mut best: Option<(usize, u32)> = None;
        for (pid, n) in procinfo::list_processes() {
            let s = stem_of(&n);
            if s.len() >= 4 && lower.contains(&s) && best.map(|(l, _)| s.len() > l).unwrap_or(true) {
                best = Some((s.len(), pid));
            }
        }
        exe = best.and_then(|(_, pid)| procinfo::image_path(pid));
    }
    let name = name
        .or_else(|| exe.as_deref().and_then(procinfo::file_description))
        .unwrap_or_else(|| {
            // "Microsoft.WindowsStore_8wekyb3d8bbwe!App" -> "WindowsStore"
            let a = aumid.split('!').next().unwrap_or(aumid);
            let a = a.split('_').next().unwrap_or(a);
            let a = a.rsplit('\\').next().unwrap_or(a);
            a.trim_end_matches(".exe").rsplit('.').next().unwrap_or(a).to_string()
        });
    TOAST_APPS.lock().unwrap().get_or_insert_with(HashMap::new).insert(aumid.to_string(), (name.clone(), exe.clone(), icon_url.clone()));
    ToastApp { name, exe, icon_url }
}

fn toast_note(aumid: &str, xml: &str) -> Option<Note> {
    let (texts, img) = parse_toast(xml);
    if texts.is_empty() {
        return None;
    }
    let a = toast_app(aumid);
    let (title, body) = if texts.len() == 1 { (a.name.clone(), texts[0].clone()) } else { (texts[0].clone(), texts[1..].join("\n")) };
    Some(Note {
        key: aumid.to_string(),
        app: a.name.clone(),
        icon: a.exe.clone(),
        icon_url: a.icon_url,
        avatar: img.as_deref().and_then(file_data_url),
        merge: format!("toast|{aumid}|{title}"),
        title,
        body,
        kind: "msg".into(),
        source: "toast".into(),
        target: Target::Aumid(aumid.to_string()),
        ..Default::default()
    })
}

// ---------- taking over Windows' own popups (per toast app, reversible) ----------

fn takeover_file() -> PathBuf {
    data_dir().join("notify-takeover.json")
}

fn takeover_book() -> HashMap<String, Option<u32>> {
    std::fs::read_to_string(takeover_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

const NOTIF_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings";

pub fn takeover_on(aumid: &str) -> bool {
    takeover_book().contains_key(aumid)
}

/// Switches Windows' banner for a toast app off (the toast still lands in the action centre,
/// where MicaBar reads it) or restores what was there before.
pub fn set_takeover(aumid: &str, on: bool) -> Result<(), String> {
    use winreg::enums::HKEY_CURRENT_USER;
    let hk = winreg::RegKey::predef(HKEY_CURRENT_USER);
    let mut bookv = takeover_book();
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
    std::fs::write(takeover_file(), serde_json::to_string(&bookv).unwrap_or_default()).map_err(|e| e.to_string())?;
    log(format!("notify: windows banner for {aumid} {}", if on { "off (taken over)" } else { "restored" }));
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
        let _ = std::fs::write(&p, &t);
        t
    })
    .clone()
}

pub fn port() -> u16 {
    settings::num_at("notify.port", 47900.0).clamp(1024.0, 65535.0) as u16
}

fn ai_server() {
    std::thread::spawn(|| {
        let _ = token(); // created now, so the hook commands can be written any time
        let port = port();
        let l = match std::net::TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => l,
            Err(e) => {
                log(format!("notify: AI hook port 127.0.0.1:{port} unavailable: {e}"));
                return;
            }
        };
        log(format!("notify: AI hook endpoint on 127.0.0.1:{port}"));
        for s in l.incoming().flatten() {
            std::thread::spawn(move || handle(s, port));
        }
    });
}

fn handle(mut s: std::net::TcpStream, port: u16) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let mut head_end = None;
    while head_end.is_none() && buf.len() < 64 * 1024 {
        let n = match s.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buf.extend_from_slice(&tmp[..n]);
        head_end = buf.windows(4).position(|w| w == b"\r\n\r\n");
    }
    let Some(he) = head_end else { return };
    let head = String::from_utf8_lossy(&buf[..he]).to_string();
    let len = head
        .lines()
        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
        .unwrap_or(0)
        .min(2 * 1024 * 1024);
    let mut body = buf[he + 4..].to_vec();
    while body.len() < len {
        match s.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(n) => body.extend_from_slice(&tmp[..n]),
        }
    }
    let _ = s.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
    let peer = s.peer_addr().map(|a| a.port()).unwrap_or(0);
    drop(s);
    // "POST /n/claude/stop?t=TOKEN HTTP/1.1"
    let line = head.lines().next().unwrap_or("");
    let target = line.split_whitespace().nth(1).unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let tok = query.split('&').find_map(|kv| kv.strip_prefix("t=")).unwrap_or("");
    if tok != token() {
        log("notify: hook call with a wrong token ignored");
        return;
    }
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    if parts.len() != 3 || parts[0] != "n" {
        return;
    }
    let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if !settings::bool_at("notify.on", true) {
        return;
    }
    ai_event(parts[1], parts[2], &json, client_host(peer, port));
}

/// The AI session's host: walks up from the hook's client process (curl) to the first process
/// owning a window (Windows Terminal, VS Code…), and finds the AI tool's exe on the way (icon).
fn client_host(peer_port: u16, our_port: u16) -> (Option<u32>, Option<String>) {
    let pid = crate::conn::snapshot()
        .into_iter()
        .find(|c| c.proto == crate::conn::Proto::Tcp && c.local_port == peer_port && c.remote.map(|(_, p)| p == our_port).unwrap_or(false))
        .map(|c| c.pid);
    let Some(mut pid) = pid else { return (None, None) };
    let tree = process_tree();
    let mut tool_exe = None;
    for _ in 0..16 {
        let Some((parent, name)) = tree.get(&pid).cloned() else { break };
        let lname = name.to_ascii_lowercase();
        if tool_exe.is_none() && (lname.starts_with("claude") || lname.starts_with("codex")) {
            tool_exe = procinfo::image_path(pid);
        }
        if main_window(pid).is_some() {
            return (Some(pid), tool_exe);
        }
        if parent == 0 || parent == pid {
            break;
        }
        pid = parent;
    }
    (None, tool_exe)
}

static STARTS: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
static DONE_AT: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

fn dur_text(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s} 秒")
    } else if s < 3600 {
        format!("{} 分 {} 秒", s / 60, s % 60)
    } else {
        format!("{} 小时 {} 分", s / 3600, (s % 3600) / 60)
    }
}

fn project_of(cwd: &str) -> String {
    cwd.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or(cwd).to_string()
}

/// First meaningful line of a reply (headings skipped when there is body text), markdown
/// stripped, at most ~140 characters.
fn first_line(text: &str) -> String {
    let body: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with('#')).collect();
    let lines: Vec<&str> = if body.iter().any(|l| l.trim().chars().count() >= 2) { body } else { text.lines().collect() };
    for l in lines {
        let t = l.trim().trim_start_matches(['#', '*', '-', '>', ' ']).replace("**", "").replace('`', "");
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
    let Ok(mut f) = std::fs::File::open(transcript) else { return String::new() };
    use std::io::{Seek, SeekFrom};
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(512 * 1024);
    let _ = f.seek(SeekFrom::Start(start));
    let mut s = String::new();
    let mut raw = Vec::new();
    let _ = f.read_to_end(&mut raw);
    s.push_str(&String::from_utf8_lossy(&raw));
    for line in s.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("type").and_then(|x| x.as_str()) != Some("assistant") {
            continue;
        }
        let text: String = v
            .pointer("/message/content")
            .and_then(|c| c.as_array())
            .map(|a| a.iter().filter(|x| x.get("type").and_then(|t| t.as_str()) == Some("text")).filter_map(|x| x.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n"))
            .unwrap_or_default();
        if !text.trim().is_empty() {
            return first_line(&text);
        }
    }
    String::new()
}

fn ai_event(tool: &str, event: &str, j: &Value, host: (Option<u32>, Option<String>)) {
    let s = |k: &str| j.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let target = host.0.map(Target::Pid).unwrap_or_default();
    let (key, app) = match tool {
        "claude" => ("claude", "Claude Code"),
        "codex" => ("codex", "Codex"),
        _ => return,
    };
    let session = s("session_id");
    match (tool, event) {
        ("claude", "prompt") => {
            STARTS.lock().unwrap().get_or_insert_with(HashMap::new).insert(session, Instant::now());
        }
        ("claude", "stop") => {
            let cwd = s("cwd");
            let took = STARTS.lock().unwrap().get_or_insert_with(HashMap::new).remove(&session).map(|t| t.elapsed());
            DONE_AT.lock().unwrap().get_or_insert_with(HashMap::new).insert(session.clone(), Instant::now());
            let reply = last_reply(&s("transcript_path"));
            *LAST_AI.lock().unwrap() = Some(Instant::now());
            push(Note {
                key: key.into(),
                app: app.into(),
                icon: host.1,
                title: if cwd.is_empty() { app.into() } else { project_of(&cwd) },
                body: if reply.is_empty() { "任务完成".into() } else { reply },
                sub: took.map(|d| format!("完成 · 用时 {}", dur_text(d))).unwrap_or_else(|| "完成".into()),
                kind: "done".into(),
                source: "ai".into(),
                target,
                merge: format!("ai|claude|{session}"),
                ..Default::default()
            });
        }
        ("claude", "notify") => {
            let msg = s("message");
            let lower = msg.to_ascii_lowercase();
            // "waiting for your input" comes ~1 min after a finished turn: the done note said it already
            if lower.contains("waiting for your input") {
                let recent = DONE_AT.lock().unwrap().get_or_insert_with(HashMap::new).get(&session).map(|t| t.elapsed() < Duration::from_secs(600)).unwrap_or(false);
                if recent {
                    return;
                }
            }
            let body = if let Some(rest) = msg.split("permission to use ").nth(1) {
                format!("需要你批准：使用 {}", rest.trim_end_matches('.'))
            } else if lower.contains("waiting for your input") {
                "在等你回复".to_string()
            } else if msg.is_empty() {
                "需要你看一下".to_string()
            } else {
                msg
            };
            let cwd = s("cwd");
            *LAST_AI.lock().unwrap() = Some(Instant::now());
            push(Note {
                key: key.into(),
                app: app.into(),
                icon: host.1,
                title: if cwd.is_empty() { app.into() } else { project_of(&cwd) },
                body,
                sub: "等你确认".into(),
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
            let reply = first_line(&s("last-assistant-message"));
            *LAST_AI.lock().unwrap() = Some(Instant::now());
            push(Note {
                key: key.into(),
                app: app.into(),
                icon: host.1,
                title: if cwd.is_empty() { app.into() } else { project_of(&cwd) },
                body: if reply.is_empty() { "任务完成".into() } else { reply },
                sub: "完成".into(),
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
    home().join(".claude").join("settings.json")
}

fn codex_config() -> PathBuf {
    home().join(".codex").join("config.toml")
}

fn hook_url(tool: &str, event: &str) -> String {
    format!("http://127.0.0.1:{}/n/{tool}/{event}?t={}", port(), token())
}

const CLAUDE_EVENTS: [(&str, &str); 3] = [("UserPromptSubmit", "prompt"), ("Stop", "stop"), ("Notification", "notify")];

fn is_ours(cmd: &str, tool: &str) -> bool {
    cmd.contains("127.0.0.1:") && cmd.contains(&format!("/n/{tool}/"))
}

fn claude_installed(v: &Value) -> bool {
    v.pointer("/hooks/Stop")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().any(|g| g.get("hooks").and_then(|h| h.as_array()).map(|h| h.iter().any(|x| x.get("command").and_then(|c| c.as_str()).map(|c| is_ours(c, "claude")).unwrap_or(false))).unwrap_or(false)))
        .unwrap_or(false)
}

fn codex_notify_line(text: &str) -> Option<(usize, String)> {
    for (i, l) in text.lines().enumerate() {
        let t = l.trim_start();
        if t.starts_with('[') {
            return None; // tables start: no top-level notify
        }
        if t.starts_with("notify") && t["notify".len()..].trim_start().starts_with('=') {
            return Some((i, l.to_string()));
        }
    }
    None
}

pub fn hooks_status() -> Vec<HookStatus> {
    let cs = claude_settings();
    let cv: Value = std::fs::read_to_string(&cs).ok().and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok()).unwrap_or(Value::Null);
    let xc = codex_config();
    let xt = std::fs::read_to_string(&xc).unwrap_or_default();
    let xl = codex_notify_line(&xt);
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
            installed: xl.as_ref().map(|(_, l)| is_ours(l, "codex")).unwrap_or(false),
            conflict: xl.as_ref().map(|(_, l)| !is_ours(l, "codex")).unwrap_or(false),
            path: xc.display().to_string(),
        },
    ]
}

fn backup_once(p: &std::path::Path) {
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
                return Err("没找到 Claude Code 的配置文件夹（~/.claude）".into());
            }
            let text = std::fs::read_to_string(&p).unwrap_or_else(|_| "{}".into());
            let mut v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("settings.json 不是有效的 JSON：{e}"))?;
            if !v.is_object() {
                return Err("settings.json 的格式不对".into());
            }
            backup_once(&p);
            if v.get("hooks").map(|h| !h.is_object()).unwrap_or(true) {
                v["hooks"] = json!({});
            }
            for (ev, name) in CLAUDE_EVENTS {
                let arr = v["hooks"].get(ev).and_then(|a| a.as_array()).cloned().unwrap_or_default();
                // drop our earlier entries (also an old port / token), keep everything else
                let mut kept: Vec<Value> = arr
                    .into_iter()
                    .filter_map(|mut g| {
                        if let Some(h) = g.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                            h.retain(|x| !x.get("command").and_then(|c| c.as_str()).map(|c| is_ours(c, "claude")).unwrap_or(false));
                            if h.is_empty() {
                                return None;
                            }
                        }
                        Some(g)
                    })
                    .collect();
                if on {
                    let cmd = format!("curl.exe -s -m 2 -o NUL --data-binary @- \"{}\"", hook_url("claude", name));
                    kept.push(json!({ "hooks": [{ "type": "command", "command": cmd }] }));
                }
                if kept.is_empty() {
                    if let Some(o) = v["hooks"].as_object_mut() {
                        o.remove(ev);
                    }
                } else {
                    v["hooks"][ev] = Value::Array(kept);
                }
            }
            if v["hooks"].as_object().map(|o| o.is_empty()).unwrap_or(false) {
                if let Some(o) = v.as_object_mut() {
                    o.remove("hooks");
                }
            }
            let out = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
            std::fs::write(&p, out + "\n").map_err(|e| e.to_string())?;
            log(format!("notify: Claude Code hooks {}", if on { "installed" } else { "removed" }));
            Ok(if on { "已加到 Claude Code（新开的会话生效；已开着的会话里输入 /hooks 看一眼也会生效）".into() } else { "已从 Claude Code 移除".into() })
        }
        "codex" => {
            let p = codex_config();
            if !p.parent().map(|d| d.is_dir()).unwrap_or(false) {
                return Err("没找到 Codex 的配置文件夹（~/.codex）".into());
            }
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            let line = codex_notify_line(&text);
            if let Some((_, l)) = &line {
                if !is_ours(l, "codex") {
                    return Err("config.toml 里已经有别的 notify 设置，MicaBar 不覆盖它（可以手动合并）".into());
                }
            }
            backup_once(&p);
            let mut lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
            if let Some((i, _)) = line {
                lines.remove(i);
            }
            if on {
                // Codex appends the event JSON as the last argument: it becomes --data-binary's value
                let l = format!(
                    "notify = [\"curl.exe\", \"-s\", \"-m\", \"2\", \"-o\", \"NUL\", \"{}\", \"--data-binary\"]",
                    hook_url("codex", "done")
                );
                lines.insert(0, l);
            }
            let mut out = lines.join("\n");
            if !out.is_empty() {
                out.push('\n');
            }
            std::fs::write(&p, out).map_err(|e| e.to_string())?;
            log(format!("notify: Codex notify {}", if on { "installed" } else { "removed" }));
            Ok(if on { "已加到 Codex（重新打开 Codex 后生效）".into() } else { "已从 Codex 移除".into() })
        }
        _ => Err("未知的工具".into()),
    }
}

// ---------- start ----------

pub fn start(app: AppHandle) {
    let _ = APP.set(app);
    shell_thread();
    toast_thread();
    ai_server();
}
