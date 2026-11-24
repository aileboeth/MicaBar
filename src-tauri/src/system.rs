//! Windows integration: start with Windows (scheduled task, elevated, no UAC prompt), the
//! Windows "animation effects" setting, and install-vs-portable detection.

use std::os::windows::process::CommandExt;

use crate::log;

const TASK: &str = "MicaBar";
const LEGACY_TASKS: [&str; 2] = ["NetIsland Autostart", "MicaBar Autostart"];
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn decode_console(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    if bytes.starts_with(&[0xFF, 0xFE]) && bytes.len() >= 2 {
        let wide: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&wide);
    }
    use windows_sys::Win32::Globalization::MultiByteToWideChar;
    const CP_OEMCP: u32 = 1;
    const CP_ACP: u32 = 0;
    const MB_ERR_INVALID_CHARS: u32 = 0x08;
    for (cp, flags) in [(CP_OEMCP, MB_ERR_INVALID_CHARS), (CP_ACP, MB_ERR_INVALID_CHARS), (CP_OEMCP, 0), (CP_ACP, 0)] {
        unsafe {
            let len = MultiByteToWideChar(cp, flags, bytes.as_ptr(), bytes.len() as i32, std::ptr::null_mut(), 0);
            if len > 0 {
                let mut wide = vec![0u16; len as usize];
                let written = MultiByteToWideChar(cp, flags, bytes.as_ptr(), bytes.len() as i32, wide.as_mut_ptr(), len);
                if written > 0 {
                    wide.truncate(written as usize);
                    return String::from_utf16_lossy(&wide);
                }
            }
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn schtasks(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("schtasks.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(decode_console(&out.stdout))
    } else {
        let err = decode_console(&out.stderr);
        let msg = if err.trim().is_empty() { decode_console(&out.stdout) } else { err };
        Err(msg.trim().to_string())
    }
}

pub fn autostart_enabled() -> bool {
    schtasks(&["/Query", "/TN", TASK]).is_ok()
}

/// On: a logon task for the interactive user, highest privileges (the app needs admin), no battery
/// or 72-hour execution time limits.
pub fn set_autostart(on: bool) -> Result<(), String> {
    for old in LEGACY_TASKS {
        if schtasks(&["/Query", "/TN", old]).is_ok() {
            let _ = schtasks(&["/Delete", "/TN", old, "/F"]);
        }
    }
    if !on {
        if autostart_enabled() {
            schtasks(&["/Delete", "/TN", TASK, "/F"])?;
            log("autostart removed");
        }
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_str = exe.display().to_string();
    let escaped_exe = xml_escape(&exe_str);
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\r\n\
         <Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\r\n\
           <Triggers>\r\n\
             <LogonTrigger>\r\n\
               <Enabled>true</Enabled>\r\n\
             </LogonTrigger>\r\n\
           </Triggers>\r\n\
           <Principals>\r\n\
             <Principal id=\"Author\">\r\n\
               <LogonType>InteractiveToken</LogonType>\r\n\
               <RunLevel>HighestAvailable</RunLevel>\r\n\
             </Principal>\r\n\
           </Principals>\r\n\
           <Settings>\r\n\
             <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\r\n\
             <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\r\n\
             <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\r\n\
             <AllowHardTerminate>true</AllowHardTerminate>\r\n\
             <StartWhenAvailable>true</StartWhenAvailable>\r\n\
             <AllowStartOnDemand>true</AllowStartOnDemand>\r\n\
             <Enabled>true</Enabled>\r\n\
             <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>\r\n\
           </Settings>\r\n\
           <Actions Context=\"Author\">\r\n\
             <Exec>\r\n\
               <Command>{escaped_exe}</Command>\r\n\
             </Exec>\r\n\
           </Actions>\r\n\
         </Task>\r\n"
    );
    let mut utf16 = Vec::with_capacity(2 + xml.len() * 2);
    utf16.extend_from_slice(&0xFEFFu16.to_le_bytes());
    for u in xml.encode_utf16() {
        utf16.extend_from_slice(&u.to_le_bytes());
    }
    let tmp_path = std::env::temp_dir().join(format!("micabar-autostart-{}.xml", std::process::id()));
    std::fs::write(&tmp_path, &utf16).map_err(|e| e.to_string())?;
    let tmp_str = tmp_path.display().to_string();
    let res = schtasks(&["/Create", "/TN", TASK, "/XML", &tmp_str, "/F"]);
    let _ = std::fs::remove_file(&tmp_path);
    res?;
    log(format!("autostart task created: {exe_str}"));
    Ok(())
}

/// Keeps the task pointing at the current exe (after an update or a move) and upgrades legacy
/// 72-hour / battery-restricted tasks.
pub fn sync_autostart(wanted: bool) {
    for old in LEGACY_TASKS {
        if schtasks(&["/Query", "/TN", old]).is_ok() {
            let _ = schtasks(&["/Delete", "/TN", old, "/F"]);
            log(format!("autostart: removed legacy task {old}"));
        }
    }
    let exists = autostart_enabled();
    if wanted {
        let escaped_exe = std::env::current_exe()
            .map(|p| xml_escape(&p.display().to_string()).to_lowercase())
            .unwrap_or_default();
        let xml = schtasks(&["/Query", "/TN", TASK, "/XML"]).unwrap_or_default().to_lowercase();
        let exe_ok = !escaped_exe.is_empty()
            && (xml.contains(&escaped_exe) || xml.replace('\'', "&apos;").contains(&escaped_exe));
        let battery_ok = xml.contains("<disallowstartifonbatteries>false</disallowstartifonbatteries>");
        let limit_ok = xml.contains("<executiontimelimit>pt0s</executiontimelimit>");
        if !exists || !exe_ok || !battery_ok || !limit_ok {
            if let Err(e) = set_autostart(true) {
                log(format!("autostart: {e}"));
            }
        }
    } else if exists {
        let _ = set_autostart(false);
    }
}

/// Syncs a native Win32 window's immersive dark/light titlebar (DWMWA_USE_IMMERSIVE_DARK_MODE = 20,
/// plus legacy Win10 1809-1909 attribute 19) and forces an immediate Win10 non-client caption repaint.
pub fn set_dark_titlebar(hwnd: isize, dark: bool) {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    use windows_sys::Win32::Graphics::Gdi::{RedrawWindow, RDW_FRAME, RDW_INVALIDATE, RDW_UPDATENOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, IsWindowVisible, SendMessageW, SetWindowPos, SWP_FRAMECHANGED, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, WM_NCACTIVATE,
    };
    let h = hwnd as HWND;
    if h.is_null() {
        return;
    }
    let v: i32 = dark as i32;
    unsafe {
        if DwmSetWindowAttribute(h, 20, &v as *const i32 as *const _, 4) != 0 {
            let _ = DwmSetWindowAttribute(h, 19, &v as *const i32 as *const _, 4);
        }
        // On Windows 10, DWM caches the caption bar brush on already-visible windows until both
        // SWP_FRAMECHANGED and a WM_NCACTIVATE toggle run.
        if IsWindowVisible(h) != 0 {
            let _ = SetWindowPos(
                h,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
            let active = (GetForegroundWindow() == h) as usize;
            let _ = SendMessageW(h, WM_NCACTIVATE, active ^ 1, -1);
            let _ = SendMessageW(h, WM_NCACTIVATE, active, -1);
            let _ = RedrawWindow(h, std::ptr::null(), std::ptr::null_mut(), RDW_FRAME | RDW_INVALIDATE | RDW_UPDATENOW);
        }
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
    // the installer puts its uninstaller next to the exe, wherever the user chose to install
    // (a path check alone took D:\MicaBar for the portable build)
    std::env::current_exe()
        .map(|p| {
            let s = p.display().to_string().to_lowercase();
            p.parent().map(|d| d.join("uninstall.exe").is_file()).unwrap_or(false)
                || s.contains("\\program files")
                || s.contains("\\appdata\\local\\micabar\\app")
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
    let mut bars: Vec<(HWND, bool)> = Vec::new();
    unsafe {
        let tray = FindWindowW(w("Shell_TrayWnd").as_ptr(), std::ptr::null());
        if !tray.is_null() {
            let notify = find(tray, "TrayNotifyWnd");
            if !notify.is_null() {
                let pager = find(notify, "SysPager");
                let bar = if pager.is_null() { find(notify, "ToolbarWindow32") } else { find(pager, "ToolbarWindow32") };
                bars.push((bar, false));
            }
        }
        let overflow = FindWindowW(w("NotifyIconOverflowWindow").as_ptr(), std::ptr::null());
        if !overflow.is_null() {
            bars.push((find(overflow, "ToolbarWindow32"), true));
        }
    }
    let mut n = 0;
    for (bar, is_overflow) in bars.into_iter().filter(|(b, _)| !b.is_null()) {
        let mut r: RECT = unsafe { std::mem::zeroed() };
        let _ = unsafe { GetClientRect(bar, &mut r) };
        // On Win10 before `^` has been opened, NotifyIconOverflowWindow's ToolbarWindow32 reports
        // a 0x0 client rect even when ghost icons sit inside it: sweep a fallback grid in that case.
        let max_x = if r.right < 32 {
            if is_overflow { 220 } else { 120 }
        } else {
            r.right
        };
        let max_y = if r.bottom < 24 {
            if is_overflow { 160 } else { 48 }
        } else {
            r.bottom
        };
        let step = if is_overflow && r.right < 32 { 8 } else { 6 };
        let mut y = 0;
        while y < max_y {
            let mut x = 0;
            while x < max_x {
                let lp = ((y as u32 & 0xffff) << 16 | (x as u32 & 0xffff)) as isize;
                let mut res = 0usize;
                unsafe { SendMessageTimeoutW(bar, WM_MOUSEMOVE, 0, lp, SMTO_ABORTIFHUNG, 50, &mut res) };
                x += step;
            }
            y += step;
        }
        n += 1;
    }
    n
}
