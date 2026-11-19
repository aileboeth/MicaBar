//! Windows integration: start with Windows (scheduled task, elevated, no UAC prompt), the
//! Windows "animation effects" setting, and install-vs-portable detection.

use std::os::windows::process::CommandExt;

use crate::log;

const TASK: &str = "NetIsland Autostart";
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
            s.contains("\\program files") || s.contains("\\appdata\\local\\netisland\\app")
        })
        .unwrap_or(false)
}
