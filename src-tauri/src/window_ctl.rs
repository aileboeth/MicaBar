//! Island window behaviour: placement (docked edge, chosen monitor), hover state machine (dwell or
//! click to open, no open while a mouse button is held, collapse on leave / outside click),
//! pass-through mode, click-through, topmost, fullscreen (game) and maximized detection, and the
//! configurable hotkey.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow};
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
    DWMWA_TRANSITIONS_FORCEDISABLED,
};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, VK_LBUTTON, VK_MBUTTON, VK_MENU, VK_RBUTTON,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetCursorPos, GetForegroundWindow, GetMessageW, GetWindowLongW, GetWindowRect, IsIconic, IsWindow,
    IsWindowVisible, IsZoomed, PeekMessageW, SetForegroundWindow, SetWindowLongW, SetWindowPos, ShowWindow,
    GWL_EXSTYLE, GWL_STYLE, HWND_TOPMOST, MSG, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER,
    SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOWNOACTIVATE, WM_HOTKEY, WS_CAPTION, WS_EX_APPWINDOW, WS_EX_LAYERED,
    WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_MAXIMIZEBOX, WS_MINIMIZEBOX,
};

use crate::log;

/// Rectangles in physical pixels relative to the window's top-left corner.
#[derive(Clone, Copy, Default, Debug)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn contains(&self, px: f64, py: f64) -> bool {
        self.w > 0.0 && px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
}

#[derive(Default)]
pub struct WinState {
    pub pill: Mutex<Rect>,
    pub panel: Mutex<Rect>,
    pub expanded: AtomicBool,
    pub game: AtomicBool,
    pub maximized: AtomicBool,
    pub hidden: AtomicBool,
    pub hotkey: AtomicBool,
    pub hotkey_ok: AtomicBool,
    pub hotkey_thread: std::sync::atomic::AtomicU32,
    pub collapse_req: AtomicBool,
    /// The page saw a mouse press on the collapsed island (click-to-open mode).
    pub click_req: AtomicBool,
    /// Layout editing (drag handles): the window takes all mouse input, no hover logic.
    pub edit: AtomicBool,
    /// The island is grabbed / being flung along the edge: all input to the window, which spans
    /// the whole edge meanwhile.
    pub grab: AtomicBool,
    /// The grabbed island was pulled off its edge (to dock on another one): the window covers the
    /// whole monitor meanwhile, so the glass capture pauses (the page draws a solid pill).
    pub free: AtomicBool,
    /// After a grab: don't reopen until the cursor has left the pill.
    pub disarm: AtomicBool,
    /// A notification card is showing: the cursor on it holds it (and clicks reach it) instead
    /// of opening the panel.
    pub noting: AtomicBool,
    /// The first-run tour is on: collapsed, its card takes clicks like a notification card;
    /// open, the panel doesn't close when the cursor leaves or clicks elsewhere.
    pub tour: AtomicBool,
    /// The page asks the island to open (the tour's "start" / "continue").
    pub open_req: AtomicBool,
    /// Display topology / DPI / power state changed: reposition immediately on next 15ms tick.
    pub reposition_req: AtomicBool,
    /// Mirrors the island window's current mouse click-through state so post-`show()` main-thread
    /// resyncs restore the latest `WS_EX_TRANSPARENT | WS_EX_LAYERED` bits.
    pub ignoring: AtomicBool,
    pub hwnd: AtomicIsize,
}

const LEAVE: Duration = Duration::from_millis(350);
const WM_APP_REHOTKEY: u32 = 0x8000 + 1;

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    pub name: String,
    pub primary: bool,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

struct Mon {
    name: String,
    primary: bool,
    full: RECT,
    work: RECT,
}

fn monitors() -> Vec<Mon> {
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR, MONITORINFOEXW};
    unsafe extern "system" fn cb(m: HMONITOR, _: HDC, _: *mut RECT, data: isize) -> i32 {
        let out = &mut *(data as *mut Vec<Mon>);
        let mut mi: MONITORINFOEXW = std::mem::zeroed();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(m, &mut mi as *mut _ as *mut MONITORINFO) != 0 {
            let n = mi.szDevice.iter().position(|&c| c == 0).unwrap_or(mi.szDevice.len());
            out.push(Mon {
                name: String::from_utf16_lossy(&mi.szDevice[..n]),
                primary: mi.monitorInfo.dwFlags & 1 != 0,
                full: mi.monitorInfo.rcMonitor,
                work: mi.monitorInfo.rcWork,
            });
        }
        1
    }
    let mut v: Vec<Mon> = Vec::new();
    unsafe {
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(cb), &mut v as *mut _ as isize);
    }
    v
}

pub fn monitor_list() -> Vec<MonitorInfo> {
    monitors()
        .into_iter()
        .map(|m| MonitorInfo {
            name: m.name,
            primary: m.primary,
            x: m.full.left,
            y: m.full.top,
            w: m.full.right - m.full.left,
            h: m.full.bottom - m.full.top,
        })
        .collect()
}

/// The monitor chosen in settings: primary, the one under the cursor, or a named one.
fn target_monitor() -> Option<Mon> {
    let mons = monitors();
    let pref = crate::settings::str_at("general.monitor", "primary");
    let pick = match pref.as_str() {
        "cursor" => {
            let mut pt = POINT { x: 0, y: 0 };
            unsafe { GetCursorPos(&mut pt) };
            mons.iter().position(|m| pt.x >= m.full.left && pt.x < m.full.right && pt.y >= m.full.top && pt.y < m.full.bottom)
        }
        "primary" => None,
        name => mons.iter().position(|m| m.name == name),
    };
    let idx = pick.or_else(|| mons.iter().position(|m| m.primary)).or(if mons.is_empty() { None } else { Some(0) })?;
    mons.into_iter().nth(idx)
}

/// Puts the window against the docked edge of the chosen monitor (bottom: above the taskbar).
pub fn place_window(w: &WebviewWindow) {
    if let Ok(h) = w.hwnd() {
        if unsafe { IsIconic(h.0 as HWND) } != 0 {
            return;
        }
    }
    let Some(m) = target_monitor() else { return };
    let Ok(size) = w.outer_size() else { return };
    let (ww, wh) = (size.width as i32, size.height as i32);
    let f = m.full;
    let work = m.work;
    let max_x = (f.right - ww).max(f.left);
    let max_y = (f.bottom - wh).max(f.top);
    // position along the edge: start (left / top), centre, end (right / bottom)
    let anchor = crate::settings::str_at("shape.anchor", "center");
    let along_x = match anchor.as_str() {
        "start" => f.left,
        "end" => f.right - ww,
        _ => f.left + (f.right - f.left - ww) / 2,
    }
    .clamp(f.left, max_x);
    let along_y = match anchor.as_str() {
        "start" => work.top,
        "end" => work.bottom - wh,
        _ => f.top + (f.bottom - f.top - wh) / 2,
    }
    .clamp(f.top, max_y);
    let (x, y) = match crate::settings::str_at("shape.dock", "top").as_str() {
        "bottom" => (along_x, (work.bottom - wh).clamp(f.top, max_y)),
        "left" => (f.left, along_y),
        "right" => (max_x, along_y),
        _ => (along_x, f.top),
    };
    if let Ok(cur) = w.outer_position() {
        if cur.x == x && cur.y == y {
            return;
        }
    }
    let _ = w.set_position(PhysicalPosition::new(x, y));
}

/// While the island is grabbed the window spans the whole docked edge of its monitor, so the
/// pill can slide from wall to wall inside it. `fit_window` + `place_window` undo it afterwards.
pub fn span_edge(w: &WebviewWindow) {
    let Some(m) = target_monitor() else { return };
    let Ok(size) = w.outer_size() else { return };
    let Ok(pos) = w.outer_position() else { return };
    let f = m.full;
    let vertical = matches!(crate::settings::str_at("shape.dock", "top").as_str(), "left" | "right");
    if vertical {
        let _ = w.set_size(tauri::PhysicalSize::new(size.width, (f.bottom - f.top) as u32));
        let _ = w.set_position(PhysicalPosition::new(pos.x, f.top));
    } else {
        let _ = w.set_size(tauri::PhysicalSize::new((f.right - f.left) as u32, size.height));
        let _ = w.set_position(PhysicalPosition::new(f.left, pos.y));
    }
}

/// The grabbed island left its edge: the window covers the whole monitor, so the pill can be
/// carried anywhere and dropped at another edge. `fit_window` + `place_window` undo it afterwards.
pub fn span_monitor(w: &WebviewWindow) {
    let Some(m) = target_monitor() else { return };
    let f = m.full;
    let _ = w.set_size(tauri::PhysicalSize::new((f.right - f.left) as u32, (f.bottom - f.top) as u32));
    let _ = w.set_position(PhysicalPosition::new(f.left, f.top));
}

/// Foreground window maximized on the island's monitor (taskbar still visible).
fn foreground_is_maximized(own: isize) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() || own == 0 || fg as isize == own {
            return false;
        }
        if IsZoomed(fg) == 0 || IsIconic(fg) != 0 || IsWindowVisible(fg) == 0 || cloaked(fg) {
            return false;
        }
        let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        let own_mon = MonitorFromWindow(own as HWND, MONITOR_DEFAULTTONEAREST);
        !mon.is_null() && mon == own_mon
    }
}

/// True when the foreground window (on the island's own monitor, not shell/overlay/maximized)
/// covers its whole monitor.
fn foreground_is_fullscreen(own: isize) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() || own == 0 || fg as isize == own {
            return false;
        }
        let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        let own_mon = MonitorFromWindow(own as HWND, MONITOR_DEFAULTTONEAREST);
        if mon.is_null() || mon != own_mon {
            return false;
        }
        let mut cls = [0u16; 256];
        let n = GetClassNameW(fg, cls.as_mut_ptr(), cls.len() as i32);
        let cls = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
        if matches!(
            cls.as_str(),
            "Progman"
                | "WorkerW"
                | "Shell_TrayWnd"
                | "Shell_SecondaryTrayWnd"
                | "XamlExplorerHostIslandWindow"
                | "Windows.UI.Core.CoreWindow"
                | "MultitaskingViewFrame"
                | "ForegroundStaging"
                | "TaskListThumbnailWnd"
                | "Xaml_WindowedPopupClass"
        ) {
            return false;
        }
        // not on screen: minimized, hidden, or cloaked by the shell.
        if IsIconic(fg) != 0 || IsWindowVisible(fg) == 0 || cloaked(fg) {
            return false;
        }
        let style = GetWindowLongW(fg, GWL_STYLE) as u32;
        let ex_style = GetWindowLongW(fg, GWL_EXSTYLE) as u32;
        if (ex_style & (WS_EX_TOOLWINDOW as u32)) != 0
            || ((ex_style & (WS_EX_LAYERED as u32)) != 0 && (ex_style & (WS_EX_TRANSPARENT as u32)) != 0)
        {
            return false;
        }
        if IsZoomed(fg) != 0 && (style & (WS_CAPTION as u32)) != 0 {
            return false;
        }
        let mut r: RECT = std::mem::zeroed();
        if DwmGetWindowAttribute(fg, DWMWA_EXTENDED_FRAME_BOUNDS as u32, &mut r as *mut RECT as *mut _, std::mem::size_of::<RECT>() as u32) != 0
            && GetWindowRect(fg, &mut r) == 0
        {
            return false;
        }
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) == 0 {
            return false;
        }
        let m = mi.rcMonitor;
        r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
    }
}

fn cloaked(h: HWND) -> bool {
    let mut v: u32 = 0;
    let hr = unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED as u32, &mut v as *mut u32 as *mut _, 4) };
    hr == 0 && v != 0
}

/// The foreground window for the log: "exe | class | rect", so a wrong game-mode switch can be
/// traced to the window that caused it.
fn describe_foreground() -> String {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() {
            return "none".into();
        }
        let mut cls = [0u16; 256];
        let n = GetClassNameW(fg, cls.as_mut_ptr(), cls.len() as i32);
        let cls = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, &mut pid);
        let exe = crate::procinfo::image_path(pid).map(|p| p.rsplit('\\').next().unwrap_or("").to_string()).unwrap_or_else(|| format!("pid {pid}"));
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(fg, &mut r);
        format!("{exe} | {cls} | {},{} {}x{}{}{}", r.left, r.top, r.right - r.left, r.bottom - r.top, if IsIconic(fg) != 0 { " min" } else { "" }, if cloaked(fg) { " cloaked" } else { "" })
    }
}

/// (held now, pressed since the previous call) — the second bit catches clicks shorter than a poll.
fn key_state(vk: u16) -> (bool, bool) {
    let s = unsafe { GetAsyncKeyState(vk as i32) } as u16;
    (s & 0x8000 != 0, s & 1 != 0)
}

fn island_style_bits(style: u32) -> u32 {
    style & !(WS_MINIMIZEBOX | WS_MAXIMIZEBOX)
}

fn island_ex_style_bits(ex_style: u32) -> u32 {
    (ex_style | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW
}

fn click_through_ex_style_bits(ex_style: u32, ignore: bool) -> u32 {
    let base = island_ex_style_bits(ex_style);
    if ignore {
        base | WS_EX_TRANSPARENT | WS_EX_LAYERED
    } else {
        base & !(WS_EX_TRANSPARENT | WS_EX_LAYERED)
    }
}

/// Disables DWM open/close/minimize transition animations so the island never plays a fly/shrink
/// animation. Call before the first `show()`.
pub fn disable_dwm_transitions(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let on: i32 = 1;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd as HWND,
            DWMWA_TRANSITIONS_FORCEDISABLED as u32,
            &on as *const i32 as *const _,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

fn configure_island_window_inner(hwnd: isize, force_shell_refresh: bool) {
    if hwnd == 0 {
        return;
    }
    disable_dwm_transitions(hwnd);
    unsafe {
        let h = hwnd as HWND;
        let old_style = GetWindowLongW(h, GWL_STYLE) as u32;
        let new_style = island_style_bits(old_style);
        let old_ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
        let new_ex = island_ex_style_bits(old_ex);
        let style_changed = new_style != old_style;
        let ex_changed = new_ex != old_ex;
        if !style_changed && !ex_changed {
            return;
        }
        let was_visible = force_shell_refresh && IsWindowVisible(h) != 0;
        if was_visible {
            ShowWindow(h, SW_HIDE);
        }
        if style_changed {
            SetWindowLongW(h, GWL_STYLE, new_style as i32);
        }
        if ex_changed {
            SetWindowLongW(h, GWL_EXSTYLE, new_ex as i32);
        }
        SetWindowPos(
            h,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
        if was_visible {
            ShowWindow(h, SW_SHOWNOACTIVATE);
            raise_topmost(hwnd);
        }
    }
}

/// Configures the island as a non-minimizable tool window (without `WS_EX_NOACTIVATE` so keyboard
/// focus in `#search` and `Ctrl+Alt+N` keep working) and disables DWM transitions.
pub fn configure_island_window(hwnd: isize) {
    configure_island_window_inner(hwnd, false);
}

/// Startup variant called right after the initial `w.show()` (before WebView paints): performs a
/// fast `SW_HIDE` -> style update -> `SW_SHOWNOACTIVATE` cycle with DWM transitions disabled so
/// Explorer / Alt+Tab registers `WS_EX_TOOLWINDOW` (and drops `WS_EX_APPWINDOW`) from frame 0.
pub fn configure_island_window_initial(hwnd: isize) {
    configure_island_window_inner(hwnd, true);
}

/// Toggles mouse click-through directly on `GWL_EXSTYLE` instead of `tao`'s
/// `set_ignore_cursor_events`, whose `WindowFlags::apply_diff` overwrites `GWL_EXSTYLE` (wiping
/// `WS_EX_TOOLWINDOW` and restoring `WS_EX_APPWINDOW`) and calls `ShowWindow(SW_SHOW)`.
fn set_click_through(hwnd: isize, ignore: bool) {
    if hwnd == 0 {
        return;
    }
    unsafe {
        let h = hwnd as HWND;
        let old = GetWindowLongW(h, GWL_EXSTYLE) as u32;
        let new = click_through_ex_style_bits(old, ignore);
        if new != old {
            SetWindowLongW(h, GWL_EXSTYLE, new as i32);
        }
    }
}

/// Queues style + click-through + topmost restoration on the main thread right after `w.show()`,
/// executing FIFO after `tao`'s `WindowMessage::Show` (`apply_diff`).
fn resync_island_after_show(app: &AppHandle, own: isize, ws: &Arc<WinState>) {
    if own == 0 {
        return;
    }
    let ws = ws.clone();
    let _ = app.run_on_main_thread(move || {
        configure_island_window(own);
        set_click_through(own, ws.ignoring.load(Ordering::SeqCst));
        if !ws.hidden.load(Ordering::SeqCst) {
            raise_topmost(own);
        }
    });
}

fn is_desktop_class_utf16(cls: &[u16]) -> bool {
    const PROGMAN: [u16; 7] = [b'P' as u16, b'r' as u16, b'o' as u16, b'g' as u16, b'm' as u16, b'a' as u16, b'n' as u16];
    const WORKERW: [u16; 7] = [b'W' as u16, b'o' as u16, b'r' as u16, b'k' as u16, b'e' as u16, b'r' as u16, b'W' as u16];
    cls == PROGMAN || cls == WORKERW
}

fn is_desktop_window(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe {
        let mut cls = [0u16; 64];
        let n = GetClassNameW(hwnd as HWND, cls.as_mut_ptr(), cls.len() as i32);
        n > 0 && is_desktop_class_utf16(&cls[..n as usize])
    }
}

fn raise_topmost(hwnd: isize) {
    unsafe {
        SetWindowPos(
            hwnd as HWND,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        );
    }
}

/// "Ctrl+Alt+N" -> (modifiers, virtual key)
pub fn parse_hotkey(s: &str) -> Option<(u32, u32)> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MOD_SHIFT, MOD_WIN};
    let mut mods = 0u32;
    let mut vk = None;
    for part in s.split('+').map(|p| p.trim()) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "alt" => mods |= MOD_ALT,
            "shift" => mods |= MOD_SHIFT,
            "win" | "meta" => mods |= MOD_WIN,
            k => {
                let up = k.to_ascii_uppercase();
                vk = if up.len() == 1 && up.chars().all(|c| c.is_ascii_alphanumeric()) {
                    Some(up.as_bytes()[0] as u32)
                } else if let Some(n) = up.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()).filter(|n| (1..=24).contains(n)) {
                    Some(0x70 + n - 1)
                } else {
                    match up.as_str() {
                        "SPACE" => Some(0x20),
                        "TAB" => Some(0x09),
                        "`" | "BACKQUOTE" => Some(0xC0),
                        "HOME" => Some(0x24),
                        "END" => Some(0x23),
                        "INSERT" => Some(0x2D),
                        "PAGEUP" => Some(0x21),
                        "PAGEDOWN" => Some(0x22),
                        _ => None,
                    }
                };
            }
        }
    }
    // a bare key would swallow normal typing: require a modifier
    if mods == 0 {
        return None;
    }
    vk.map(|v| (mods, v))
}

/// The settings hotkey toggles the panel (works in game mode too). Re-registered on change.
pub fn hotkey_thread(ws: Arc<WinState>) {
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey;
    std::thread::spawn(move || unsafe {
        let mut msg: MSG = std::mem::zeroed();
        // Force Win32 message queue creation BEFORE publishing TID so early PostThreadMessageW succeeds.
        PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, 0);
        ws.hotkey_thread.store(GetCurrentThreadId(), Ordering::SeqCst);
        let register = |ws: &WinState| {
            UnregisterHotKey(std::ptr::null_mut(), 1);
            let s = crate::settings::str_at("interaction.hotkey", "Ctrl+Alt+N");
            let ok = match parse_hotkey(&s) {
                Some((m, vk)) => RegisterHotKey(std::ptr::null_mut(), 1, m | MOD_NOREPEAT, vk) != 0,
                None => false,
            };
            if !ok {
                log(format!("hotkey {s} could not be registered (taken or invalid)"));
            }
            ws.hotkey_ok.store(ok, Ordering::SeqCst);
        };
        register(&ws);
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_HOTKEY {
                ws.hotkey.store(true, Ordering::SeqCst);
            } else if msg.message == WM_APP_REHOTKEY {
                register(&ws);
            }
        }
    });
}

/// Asks the hotkey thread to re-read the hotkey from settings.
pub fn reload_hotkey(ws: &WinState) {
    use windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
    let t = ws.hotkey_thread.load(Ordering::SeqCst);
    if t != 0 {
        unsafe { PostThreadMessageW(t, WM_APP_REHOTKEY, 0, 0) };
    }
}

struct Hover {
    dwell: Option<Instant>,
    leave: Option<Instant>,
    /// Collapsed island opens only after the cursor arrived without a button held.
    armed: bool,
    /// Opened by hotkey: stays open until the cursor has entered and left again.
    pinned: bool,
    entered: bool,
    press_inside: bool,
    btn_prev: bool,
    prev_fg: isize,
    last_fg: isize,
    fg_desktop: bool,
    was_iconic: bool,
    ignoring: bool,
    peek: bool,
}

pub fn hover_loop(app: AppHandle, ws: Arc<WinState>) {
    let Some(w) = app.get_webview_window("main") else { return };
    let own = w.hwnd().map(|h| h.0 as isize).unwrap_or(0);
    ws.hwnd.store(own, Ordering::SeqCst);
    ws.ignoring.store(true, Ordering::SeqCst);
    set_click_through(own, true);
    let mut h = Hover {
        dwell: None,
        leave: None,
        armed: true,
        pinned: false,
        entered: false,
        press_inside: false,
        btn_prev: false,
        prev_fg: 0,
        last_fg: 0,
        fg_desktop: false,
        was_iconic: false,
        ignoring: true,
        peek: false,
    };
    let mut n: u64 = 0;
    let mut fs_streak: u8 = 0;

    let set_expanded = |h: &mut Hover, open: bool, why: &str| {
        if ws.expanded.swap(open, Ordering::SeqCst) == open {
            return;
        }
        log(format!("island {} ({why})", if open { "open" } else { "close" }));
        if open {
            h.prev_fg = unsafe { GetForegroundWindow() } as isize;
            if why == "hotkey" {
                // keyboard use: take focus so Tab / arrows / Enter / Esc work right away
                let _ = w.set_focus();
            }
        } else {
            // give focus back to the app the user was in before the panel took it
            // (except when closing due to an outside click, where the user chose a new window)
            if why != "click outside" {
                unsafe {
                    let fg = GetForegroundWindow() as isize;
                    if fg == own && h.prev_fg != 0 && h.prev_fg != own && IsWindow(h.prev_fg as HWND) != 0 {
                        SetForegroundWindow(h.prev_fg as HWND);
                    }
                }
            }
            h.pinned = false;
            h.armed = false;
        }
        h.dwell = None;
        h.leave = None;
        let _ = app.emit("island", open);
    };

    let drain_buttons = |h: &mut Hover| {
        let (l, r, m) = (key_state(VK_LBUTTON), key_state(VK_RBUTTON), key_state(VK_MBUTTON));
        h.btn_prev = l.0 || r.0 || m.0;
    };

    loop {
        std::thread::sleep(Duration::from_millis(15));
        n += 1;
        if n % 4 == 0 {
            crate::perf::tick("hover");
        }
        // Safety net: if anything still minimizes the island, restore without activating.
        if own != 0 && !ws.hidden.load(Ordering::SeqCst) && unsafe { IsIconic(own as HWND) } != 0 {
            if !h.was_iconic {
                h.was_iconic = true;
                log("island minimized unexpectedly; restoring");
            }
            unsafe {
                ShowWindow(own as HWND, SW_SHOWNOACTIVATE);
            }
            configure_island_window(own);
            set_click_through(own, h.ignoring);
            place_window(&w);
            raise_topmost(own);
        } else {
            h.was_iconic = false;
        }
        // Edge trigger: when foreground switches to the desktop (e.g. Win+D), immediately re-pin topmost.
        let fg = unsafe { GetForegroundWindow() } as isize;
        if fg != h.last_fg {
            h.last_fg = fg;
            let is_desktop = is_desktop_window(fg);
            if is_desktop
                && !h.fg_desktop
                && own != 0
                && !ws.hidden.load(Ordering::SeqCst)
                && (!ws.game.load(Ordering::SeqCst) || ws.expanded.load(Ordering::SeqCst))
            {
                raise_topmost(own);
            }
            h.fg_desktop = is_desktop;
        }
        let hot = crate::settings::hot();
        // placement: every 2 s, every 0.3 s when following the cursor across monitors, or immediately on display/resume
        let follow = n % 20 == 0 && !ws.expanded.load(Ordering::SeqCst) && hot.monitor_cursor;
        let force_pos = ws.reposition_req.swap(false, Ordering::SeqCst);
        if (n % 130 == 0 || follow || force_pos) && !ws.grab.load(Ordering::SeqCst) {
            place_window(&w);
            if force_pos && !ws.hidden.load(Ordering::SeqCst) {
                raise_topmost(own);
            }
        }
        if n % 66 == 0 && own != 0 && !ws.hidden.load(Ordering::SeqCst) {
            let ex = unsafe { GetWindowLongW(own as HWND, GWL_EXSTYLE) } as u32;
            let ct_mask = WS_EX_TRANSPARENT | WS_EX_LAYERED;
            let ct_ok = if h.ignoring { (ex & ct_mask) == ct_mask } else { (ex & ct_mask) == 0 };
            if (ex & WS_EX_TOOLWINDOW) == 0 || (ex & WS_EX_APPWINDOW) != 0 || !ct_ok {
                configure_island_window(own);
                set_click_through(own, h.ignoring);
            }
            if !ws.game.load(Ordering::SeqCst) || ws.expanded.load(Ordering::SeqCst) {
                raise_topmost(own);
            }
        }
        if n % 33 == 0 {
            let raw_fs = foreground_is_fullscreen(own);
            if raw_fs {
                fs_streak = fs_streak.saturating_add(1);
            } else {
                fs_streak = 0;
            }
            // Require 2 consecutive checks (~1s) to enter game mode; exit immediately on 1 check.
            let fs = if ws.game.load(Ordering::SeqCst) { raw_fs } else { fs_streak >= 2 };
            if ws.game.swap(fs, Ordering::SeqCst) != fs {
                log(format!("fullscreen foreground: {fs} ({})", describe_foreground()));
                let _ = app.emit("game", fs);
            }
            // fullscreen behaviour: keep the pill, or hide the island completely (unless open)
            let hide = fs && hot.fullscreen_hide && !ws.expanded.load(Ordering::SeqCst);
            if ws.hidden.swap(hide, Ordering::SeqCst) != hide {
                if hide {
                    let _ = w.hide();
                } else {
                    let _ = w.show();
                    resync_island_after_show(&app, own, &ws);
                }
            }
            let max = !fs && foreground_is_maximized(own);
            if ws.maximized.swap(max, Ordering::SeqCst) != max {
                let _ = app.emit("maximized", max);
            }
        }
        let dwell = Duration::from_millis(hot.hover_delay_ms);
        let click_mode = hot.click_mode;
        let passthrough = hot.passthrough;
        if !click_mode || passthrough {
            ws.click_req.store(false, Ordering::SeqCst);
        }

        if ws.disarm.swap(false, Ordering::SeqCst) {
            h.armed = false;
            h.dwell = None;
        }
        if ws.grab.load(Ordering::SeqCst) {
            // grabbed: the page follows the pointer; nothing opens or closes meanwhile
            ws.hotkey.store(false, Ordering::SeqCst);
            ws.click_req.store(false, Ordering::SeqCst);
            ws.collapse_req.store(false, Ordering::SeqCst);
            if h.ignoring {
                h.ignoring = false;
                ws.ignoring.store(false, Ordering::SeqCst);
                set_click_through(own, false);
            }
            drain_buttons(&mut h);
            continue;
        }
        if ws.edit.load(Ordering::SeqCst) {
            if ws.collapse_req.swap(false, Ordering::SeqCst) && ws.expanded.load(Ordering::SeqCst) {
                set_expanded(&mut h, false, "layout edit");
            }
            ws.hotkey.store(false, Ordering::SeqCst);
            ws.click_req.store(false, Ordering::SeqCst);
            if h.ignoring {
                h.ignoring = false;
                ws.ignoring.store(false, Ordering::SeqCst);
                set_click_through(own, false);
            }
            drain_buttons(&mut h);
            continue;
        }

        let mut pt = POINT { x: 0, y: 0 };
        let mut wr: RECT = unsafe { std::mem::zeroed() };
        if own == 0 || unsafe { GetCursorPos(&mut pt) } == 0 || unsafe { GetWindowRect(own as HWND, &mut wr) } == 0 {
            continue;
        }
        let cx = (pt.x - wr.left) as f64;
        let cy = (pt.y - wr.top) as f64;
        let expanded = ws.expanded.load(Ordering::SeqCst);
        let rect = if expanded { *ws.panel.lock().unwrap() } else { *ws.pill.lock().unwrap() };
        let inside = rect.contains(cx, cy);
        let (l, r, m) = (key_state(VK_LBUTTON), key_state(VK_RBUTTON), key_state(VK_MBUTTON));
        let alt = key_state(VK_MENU).0;
        let btn = l.0 || r.0 || m.0;
        let pressed = (btn && !h.btn_prev) || l.1 || r.1 || m.1;
        if pressed {
            h.press_inside = inside;
        }
        let game = ws.game.load(Ordering::SeqCst);

        if ws.hotkey.swap(false, Ordering::SeqCst) {
            if expanded {
                set_expanded(&mut h, false, "hotkey");
            } else {
                if ws.hidden.swap(false, Ordering::SeqCst) {
                    let _ = w.show();
                    resync_island_after_show(&app, own, &ws);
                }
                set_expanded(&mut h, true, "hotkey");
                h.pinned = true;
                h.entered = false;
            }
        } else if ws.collapse_req.swap(false, Ordering::SeqCst) {
            if expanded {
                set_expanded(&mut h, false, "esc");
            }
        } else if ws.open_req.swap(false, Ordering::SeqCst) {
            if !expanded {
                if ws.hidden.swap(false, Ordering::SeqCst) {
                    let _ = w.show();
                    resync_island_after_show(&app, own, &ws);
                }
                set_expanded(&mut h, true, "tour");
                h.pinned = true;
                h.entered = false;
            }
        } else if !expanded && (ws.noting.load(Ordering::SeqCst) || ws.tour.load(Ordering::SeqCst)) {
            // a notification card: the page handles hover / clicks on it; it never opens the panel,
            // and after it the cursor has to leave before the pill opens
            h.dwell = None;
            if inside {
                h.armed = false;
            }
        } else if !expanded {
            if passthrough {
                // only the hotkey opens the island
                h.dwell = None;
            } else if click_mode {
                // reported by the page on release (a long press grabs the island instead)
                let clicked = ws.click_req.swap(false, Ordering::SeqCst);
                if clicked && !game {
                    set_expanded(&mut h, true, "click");
                }
            } else if !inside {
                h.armed = true;
                h.dwell = None;
            } else if alt {
                // Alt + long press grabs the collapsed pill: don't open meanwhile
                h.dwell = None;
            } else if btn || pressed || game {
                // a click went through to the window below (the user meant that window), a window is
                // being dragged across, or a game owns the screen: don't open until the cursor leaves
                h.armed = false;
                h.dwell = None;
            } else if h.armed {
                let t = *h.dwell.get_or_insert_with(Instant::now);
                if t.elapsed() >= dwell {
                    set_expanded(&mut h, true, "hover");
                }
            }
        } else if inside {
            h.leave = None;
            h.entered = true;
        } else if ws.tour.load(Ordering::SeqCst) {
            // the tour keeps the panel open (Esc / the hotkey / its own buttons close it)
            h.leave = None;
        } else if pressed {
            set_expanded(&mut h, false, "click outside");
        } else if btn && h.press_inside {
            h.leave = None; // dragging a scrollbar / selecting text that started inside
        } else if h.pinned && !h.entered {
            // opened by hotkey and the cursor never came in: keep it open
        } else {
            let t = *h.leave.get_or_insert_with(Instant::now);
            if t.elapsed() >= LEAVE {
                set_expanded(&mut h, false, "leave");
            }
        }
        h.btn_prev = btn;

        let expanded = ws.expanded.load(Ordering::SeqCst);
        let mut capture = inside || (expanded && btn && h.press_inside);
        // Collapsed in hover mode the pill is "hollow": opening only needs the cursor position
        // (polled above), so clicks always go to whatever is under it (a browser tab…).
        // Click-to-open needs the press itself; game / pass-through never capture.
        let noting = !expanded && (ws.noting.load(Ordering::SeqCst) || ws.tour.load(Ordering::SeqCst));
        if !expanded && (game || passthrough || !click_mode) && !(noting && !game) {
            capture = false;
        }
        // Alt over the collapsed pill makes it solid, so a long press grabs it (not the window below)
        if !expanded && inside && alt && !game && !passthrough {
            capture = true;
        }
        // see-through while the cursor rests on the collapsed pill (hover mode)
        let peek = !expanded && !noting && inside && !alt && !click_mode && !passthrough && !game && hot.peek;
        if peek != h.peek {
            h.peek = peek;
            let _ = app.emit("peek", peek);
        }
        if capture == h.ignoring {
            h.ignoring = !capture;
            ws.ignoring.store(h.ignoring, Ordering::SeqCst);
            set_click_through(own, h.ignoring);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{WS_EX_NOACTIVATE, WS_EX_TOPMOST};

    #[test]
    fn island_styles_strip_min_max_and_set_toolwindow_without_noactivate() {
        let style = WS_CAPTION | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;
        let cleaned = island_style_bits(style);
        assert_eq!(cleaned & WS_MINIMIZEBOX, 0);
        assert_eq!(cleaned & WS_MAXIMIZEBOX, 0);
        assert_ne!(cleaned & WS_CAPTION, 0);

        let ex = WS_EX_APPWINDOW | WS_EX_TOPMOST;
        let cleaned_ex = island_ex_style_bits(ex);
        assert_ne!(cleaned_ex & WS_EX_TOOLWINDOW, 0);
        assert_eq!(cleaned_ex & WS_EX_APPWINDOW, 0);
        assert_ne!(cleaned_ex & WS_EX_TOPMOST, 0);
        assert_eq!(cleaned_ex & WS_EX_NOACTIVATE, 0);
    }

    #[test]
    fn click_through_preserves_toolwindow_and_toggles_transparent_layered() {
        let base = WS_EX_APPWINDOW | WS_EX_TOPMOST;
        let ig = click_through_ex_style_bits(base, true);
        assert_ne!(ig & WS_EX_TOOLWINDOW, 0);
        assert_eq!(ig & WS_EX_APPWINDOW, 0);
        assert_ne!(ig & WS_EX_TRANSPARENT, 0);
        assert_ne!(ig & WS_EX_LAYERED, 0);
        assert_eq!(ig & WS_EX_NOACTIVATE, 0);

        let solid = click_through_ex_style_bits(ig, false);
        assert_ne!(solid & WS_EX_TOOLWINDOW, 0);
        assert_eq!(solid & WS_EX_APPWINDOW, 0);
        assert_eq!(solid & WS_EX_TRANSPARENT, 0);
        assert_eq!(solid & WS_EX_LAYERED, 0);
        assert_ne!(solid & WS_EX_TOPMOST, 0);
    }

    #[test]
    fn desktop_class_detection_matches_progman_and_workerw() {
        let wide = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert!(is_desktop_class_utf16(&wide("Progman")));
        assert!(is_desktop_class_utf16(&wide("WorkerW")));
        assert!(!is_desktop_class_utf16(&wide("Shell_TrayWnd")));
        assert!(!is_desktop_class_utf16(&wide("Chrome_WidgetWin_1")));
    }
}
