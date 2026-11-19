//! Windows integration: start with Windows (scheduled task, elevated, no UAC prompt), the
//! Windows "animation effects" setting, and install-vs-portable detection.

use std::os::windows::process::CommandExt;

use crate::log;

const TASK: &str = "MicaBar Autostart";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn schtasks(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("schtasks").args(args).creation_flags(CREATE_NO_WINDOW).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub fn autostart_enabled() -> bool {
    schtasks(&["/Query", "/TN", TASK]).is_ok()
}

/// On: a logon task for the current user, highest privileges (the app needs admin), 10 s delay.
pub fn set_autostart(on: bool) -> Result<(), String> {
    if !on {
        if autostart_enabled() {
            schtasks(&["/Delete", "/TN", TASK, "/F"])?;
            log("autostart removed");
        }
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let user = format!(
        "{}\\{}",
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default()
    );
    let tr = format!("\"{}\"", exe.display());
    schtasks(&["/Create", "/TN", TASK, "/TR", &tr, "/SC", "ONLOGON", "/RU", &user, "/IT", "/RL", "HIGHEST", "/DELAY", "0000:10", "/F"])?;
    log(format!("autostart task created for {user}: {tr}"));
    Ok(())
}

/// Keeps the task pointing at the current exe (after an update or a move).
pub fn sync_autostart(wanted: bool) {
    // the logon task of the app's earlier name (NetIsland)
    if schtasks(&["/Query", "/TN", "NetIsland Autostart"]).is_ok() {
        let _ = schtasks(&["/Delete", "/TN", "NetIsland Autostart", "/F"]);
        log("autostart: removed the NetIsland task");
    }
    let exists = autostart_enabled();
    if wanted {
        let exe = std::env::current_exe().map(|p| p.display().to_string().to_lowercase()).unwrap_or_default();
        let xml = schtasks(&["/Query", "/TN", TASK, "/XML"]).unwrap_or_default().to_lowercase();
        if !exists || !xml.contains(&exe) {
            if let Err(e) = set_autostart(true) {
                log(format!("autostart: {e}"));
            }
        }
    } else if exists {
        let _ = set_autostart(false);
    }
}

/// Windows Settings > Accessibility > "Animation effects".
pub fn animations_enabled() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;
    const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;
    let mut on: i32 = 1;
    unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, &mut on as *mut i32 as *mut _, 0) };
    on != 0
}

/// Installed (under Program Files, updatable by the installer) or a portable copy.
pub fn is_installed() -> bool {
    std::env::current_exe()
        .map(|p| {
            let s = p.display().to_string().to_lowercase();
            s.contains("\\program files") || s.contains("\\appdata\\local\\micabar\\app")
        })
        .unwrap_or(false)
}

/// Removes "ghost" tray icons: icons of processes that were killed (not quit) stay in the
/// notification area until the mouse passes over them. Moving a virtual mouse across the tray
/// toolbars makes Explorer drop every icon whose owner window is gone. Works with the Windows 10
/// style taskbar (visible area + the overflow flyout); a no-op where those windows don't exist.
pub fn sweep_tray() -> usize {
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowExW, FindWindowW, GetClientRect, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_MOUSEMOVE};
    let w = crate::procinfo::wide;
    let find = |parent: HWND, class: &str| unsafe { FindWindowExW(parent, std::ptr::null_mut(), w(class).as_ptr(), std::ptr::null()) };
    let mut bars: Vec<HWND> = Vec::new();
    unsafe {
        let tray = FindWindowW(w("Shell_TrayWnd").as_ptr(), std::ptr::null());
        if !tray.is_null() {
            let notify = find(tray, "TrayNotifyWnd");
            if !notify.is_null() {
                let pager = find(notify, "SysPager");
                let bar = if pager.is_null() { find(notify, "ToolbarWindow32") } else { find(pager, "ToolbarWindow32") };
                bars.push(bar);
            }
        }
        let overflow = FindWindowW(w("NotifyIconOverflowWindow").as_ptr(), std::ptr::null());
        if !overflow.is_null() {
            bars.push(find(overflow, "ToolbarWindow32"));
        }
    }
    let mut n = 0;
    for bar in bars.into_iter().filter(|b| !b.is_null()) {
        let mut r: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetClientRect(bar, &mut r) } == 0 {
            continue;
        }
        let mut y = 0;
        while y < r.bottom.max(1) {
            let mut x = 0;
            while x < r.right.max(1) {
                let lp = ((y as u32 & 0xffff) << 16 | (x as u32 & 0xffff)) as isize;
                let mut res = 0usize;
                unsafe { SendMessageTimeoutW(bar, WM_MOUSEMOVE, 0, lp, SMTO_ABORTIFHUNG, 50, &mut res) };
                x += 6;
            }
            y += 6;
        }
        n += 1;
    }
    n
}
