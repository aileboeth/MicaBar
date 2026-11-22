//! Island window behaviour: placement (docked edge, chosen monitor), hover state machine (dwell or
//! click to open, no open while a mouse button is held, collapse on leave / outside click),
//! pass-through mode, click-through, topmost, fullscreen (game) and maximized detection, and the
//! configurable hotkey.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow};
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, VK_LBUTTON, VK_MBUTTON, VK_MENU, VK_RBUTTON,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetCursorPos, GetForegroundWindow, GetMessageW, GetWindowRect, IsWindow, SetForegroundWindow,
    SetWindowPos, HWND_TOPMOST, MSG, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, WM_HOTKEY,
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
    let Some(m) = target_monitor() else { return };
    let Ok(size) = w.outer_size() else { return };
    let (ww, wh) = (size.width as i32, size.height as i32);
    let f = m.full;
    let work = m.work;
    // position along the edge: start (left / top), centre, end (right / bottom)
    let anchor = crate::settings::str_at("shape.anchor", "center");
    let along_x = match anchor.as_str() {
        "start" => f.left,
        "end" => f.right - ww,
        _ => f.left + (f.right - f.left - ww) / 2,
    };
    let along_y = match anchor.as_str() {
        "start" => work.top,
        "end" => work.bottom - wh,
        _ => f.top + (f.bottom - f.top - wh) / 2,
    };
    let (x, y) = match crate::settings::str_at("shape.dock", "top").as_str() {
        "bottom" => (along_x, work.bottom - wh),
        "left" => (f.left, along_y),
        "right" => (f.right - ww, along_y),
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
    use windows_sys::Win32::UI::WindowsAndMessaging::IsZoomed;
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() || fg as isize == own {
            return false;
        }
        if IsZoomed(fg) == 0 {
            return false;
        }
        let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        let own_mon = MonitorFromWindow(own as HWND, MONITOR_DEFAULTTONEAREST);
        mon == own_mon
    }
}

/// True when the foreground window (not ours, not the desktop/taskbar) covers its whole monitor.
fn foreground_is_fullscreen(own: isize) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() || fg as isize == own {
            return false;
        }
        let mut cls = [0u16; 64];
        let n = GetClassNameW(fg, cls.as_mut_ptr(), 64);
        let cls = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
        if matches!(cls.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
            return false;
        }
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(fg, &mut r) == 0 {
            return false;
        }
        let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) == 0 {
            return false;
        }
        let m = mi.rcMonitor;
        r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
    }
}

/// (held now, pressed since the previous call) — the second bit catches clicks shorter than a poll.
fn key_state(vk: u16) -> (bool, bool) {
    let s = unsafe { GetAsyncKeyState(vk as i32) } as u16;
    (s & 0x8000 != 0, s & 1 != 0)
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
        let mut msg: MSG = std::mem::zeroed();
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
    ignoring: bool,
    peek: bool,
}

pub fn hover_loop(app: AppHandle, ws: Arc<WinState>) {
    let Some(w) = app.get_webview_window("main") else { return };
    let own = w.hwnd().map(|h| h.0 as isize).unwrap_or(0);
    ws.hwnd.store(own, Ordering::SeqCst);
    let _ = w.set_ignore_cursor_events(true);
    let mut h = Hover {
        dwell: None,
        leave: None,
        armed: true,
        pinned: false,
        entered: false,
        press_inside: false,
        btn_prev: false,
        prev_fg: 0,
        ignoring: true,
        peek: false,
    };
    let mut n: u64 = 0;
    let mut pos = w.outer_position().unwrap_or_default();

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
            unsafe {
                let fg = GetForegroundWindow() as isize;
                if fg == own && h.prev_fg != 0 && h.prev_fg != own && IsWindow(h.prev_fg as HWND) != 0 {
                    SetForegroundWindow(h.prev_fg as HWND);
                }
            }
            h.pinned = false;
            h.armed = false;
        }
        h.dwell = None;
        h.leave = None;
        let _ = app.emit("island", open);
    };

    loop {
        std::thread::sleep(Duration::from_millis(15));
        n += 1;
        // placement: every 2 s, or every 0.3 s when following the cursor across monitors
        let follow = n % 20 == 0 && !ws.expanded.load(Ordering::SeqCst) && crate::settings::str_at("general.monitor", "primary") == "cursor";
        if (n % 130 == 0 || follow) && !ws.grab.load(Ordering::SeqCst) {
            place_window(&w);
            pos = w.outer_position().unwrap_or(pos);
        }
        if n % 66 == 0 && !ws.hidden.load(Ordering::SeqCst) {
            raise_topmost(own);
        }
        if n % 33 == 0 {
            let fs = foreground_is_fullscreen(own);
            if ws.game.swap(fs, Ordering::SeqCst) != fs {
                log(format!("fullscreen foreground: {fs}"));
                let _ = app.emit("game", fs);
            }
            // fullscreen behaviour: keep the pill, or hide the island completely
            let hide = fs && crate::settings::str_at("interaction.fullscreen", "pill") == "hide";
            if ws.hidden.swap(hide, Ordering::SeqCst) != hide {
                let _ = if hide { w.hide() } else { w.show() };
            }
            let max = !fs && foreground_is_maximized(own);
            if ws.maximized.swap(max, Ordering::SeqCst) != max {
                let _ = app.emit("maximized", max);
            }
        }
        let dwell = Duration::from_millis(crate::settings::num_at("interaction.hoverDelay", 500.0).clamp(0.0, 3000.0) as u64);
        let click_mode = crate::settings::str_at("interaction.expand", "hover") == "click";
        let passthrough = crate::settings::bool_at("interaction.passthrough", false);
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
                let _ = w.set_ignore_cursor_events(false);
            }
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
                let _ = w.set_ignore_cursor_events(false);
            }
            continue;
        }

        let mut pt = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut pt) } == 0 {
            continue;
        }
        let cx = (pt.x - pos.x) as f64;
        let cy = (pt.y - pos.y) as f64;
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
        let peek = !expanded && !noting && inside && !alt && !click_mode && !passthrough && !game && crate::settings::bool_at("interaction.peek", true);
        if peek != h.peek {
            h.peek = peek;
            let _ = app.emit("peek", peek);
        }
        if capture == h.ignoring {
            h.ignoring = !capture;
            let _ = w.set_ignore_cursor_events(h.ignoring);
        }
    }
}
