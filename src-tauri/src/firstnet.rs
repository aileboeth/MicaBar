//! A program reaching the internet for the first time: shown as a note with "允许" / "阻止联网".
//!
//! Programs are remembered by path, with version folders folded ("app-1.2.3", "125.0.6422.60" →
//! "*") so an update doesn't make a known program new again. The first run learns quietly: the
//! programs of the traffic history plus everything seen in the first ten minutes count as known.
//! Blocking adds a Windows Firewall rule for all of the program's outgoing traffic (loopback is
//! never filtered, so a program using the system proxy is stopped by a REJECT rule in the core
//! when exit control is on).

use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::{log, now_ms};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const LEARN_MS: u64 = 10 * 60_000;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Known {
    pub name: String,
    pub exe: String,
    pub path: String,
    pub first: u64,
    pub last: u64,
    /// first destination (host or IP, with country)
    pub dest: String,
    /// seen after learning (it got a note)
    pub fresh: bool,
    /// the user pressed "允许"
    pub allowed: bool,
    pub blocked: bool,
    /// firewall rule name while blocked
    pub rule: String,
    /// the core REJECT rule's id while blocked (exit control on)
    pub core_rule: String,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Book {
    learn_until: u64,
    apps: HashMap<String, Known>,
}

static BOOK: Mutex<Option<Book>> = Mutex::new(None);

fn file() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    let dir = PathBuf::from(base).join("MicaBar");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("known-apps.json")
}

fn save(b: &Book) {
    if let Ok(s) = serde_json::to_string(b) {
        let _ = crate::settings::atomic_write(&file(), s.as_bytes());
    }
}

fn has_version_pattern(s: &str) -> bool {
    s.as_bytes()
        .windows(3)
        .any(|w| w[0].is_ascii_digit() && w[1] == b'.' && w[2].is_ascii_digit())
}

fn is_top_container(prev: Option<&str>) -> bool {
    let Some(p) = prev else { return true };
    p.ends_with(':')
        || matches!(
            p,
            "program files" | "program files (x86)" | "programs" | "local" | "roaming" | "windowsapps"
        )
}

fn fold_segment(seg: &str, prev: Option<&str>, is_last: bool) -> String {
    if is_last || seg.ends_with(".exe") {
        return seg.to_string();
    }
    // WindowsApps\<Name>_<Version>_<Arch>_<ResourceId>_<PublisherId>: fold only the version field
    // inside the package family so different Store apps never collapse into `windowsapps\*`.
    if prev == Some("windowsapps") {
        let mut parts: Vec<&str> = seg.split('_').collect();
        if parts.len() >= 2 && has_version_pattern(parts[1]) {
            parts[1] = "*";
            return parts.join("_");
        }
        return seg.to_string();
    }
    if !has_version_pattern(seg) {
        return seg.to_string();
    }
    let bytes = seg.as_bytes();
    // Locate the start of the `<digits>.<digits>` run.
    let mut ver_start = None;
    for i in 0..bytes.len() {
        if bytes[i].is_ascii_digit() {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j + 1 < bytes.len() && bytes[j] == b'.' && bytes[j + 1].is_ascii_digit() {
                ver_start = Some(i);
                break;
            }
        }
    }
    let Some(mut vi) = ver_start else { return seg.to_string() };
    // Include an optional leading 'v' right before the version (e.g. "v2.0" or "app-v2.0").
    if vi > 0 && bytes[vi - 1] == b'v' && (vi == 1 || matches!(bytes[vi - 2], b'-' | b'_' | b'.' | b' ')) {
        vi -= 1;
    }
    let prefix = &seg[..vi];
    let generic_prefix = matches!(prefix, "" | "app-" | "app_" | "ver-" | "version-");
    if generic_prefix {
        if is_top_container(prev) {
            // Directly under drive root / Program Files with no distinct app prefix: keep as-is.
            seg.to_string()
        } else {
            "*".to_string()
        }
    } else {
        // Keep the app prefix ("good-1.0" -> "good-*") so Good-1.0 and Evil-2.0 never collide.
        format!("{prefix}*")
    }
}

/// A path's identity across updates: lower case, version-like folders folded safely.
pub fn norm(path: &str) -> String {
    let p = path.to_lowercase().replace('/', "\\");
    let segs: Vec<&str> = p.split('\\').collect();
    let n = segs.len();
    segs.iter()
        .enumerate()
        .map(|(i, seg)| {
            let prev = if i > 0 { Some(segs[i - 1]) } else { None };
            fold_segment(seg, prev, i + 1 == n)
        })
        .collect::<Vec<_>>()
        .join("\\")
}

/// Loads the book; the first time it seeds from the traffic history and starts learning.
pub fn init(history: Vec<(String, String)>) {
    let mut g = BOOK.lock().unwrap();
    if g.is_some() {
        return;
    }
    let b = match std::fs::read_to_string(file()).ok().and_then(|s| serde_json::from_str::<Book>(s.trim_start_matches('\u{feff}')).ok()) {
        Some(b) => b,
        None => {
            let now = now_ms();
            let mut b = Book { learn_until: now + LEARN_MS, apps: HashMap::new() };
            for (name, path) in history {
                b.apps.entry(norm(&path)).or_insert(Known {
                    name,
                    path: path.clone(),
                    exe: exe_of(&path),
                    first: now,
                    last: now,
                    ..Default::default()
                });
            }
            log(format!("firstnet: new book, {} programs from the traffic history, learning 10 min", b.apps.len()));
            save(&b);
            b
        }
    };
    *g = Some(b);
}

fn exe_of(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

fn skip(path: &str) -> bool {
    let l = path.to_lowercase();
    let win = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()).to_lowercase();
    if !crate::settings::bool_at("firstNet.system", false) && l.starts_with(&(win + "\\")) {
        return true;
    }
    // MicaBar itself
    std::env::current_exe().map(|e| e.to_string_lossy().eq_ignore_ascii_case(path)).unwrap_or(false)
}

/// One sighting per collector round: (name, path, destination) of programs with a connection
/// to a public address. Returns the programs that are new.
pub fn observe(seen: &[(String, String, String)]) -> Vec<Known> {
    if !crate::settings::bool_at("firstNet.on", true) {
        return Vec::new();
    }
    let now = now_ms();
    let mut fresh = Vec::new();
    let mut reblock: Vec<(String, String, String)> = Vec::new();
    {
        let mut g = BOOK.lock().unwrap();
        let Some(b) = g.as_mut() else { return Vec::new() };
        let learning = now < b.learn_until;
        let mut changed = false;
        for (name, path, dest) in seen {
            if skip(path) {
                continue;
            }
            let key = norm(path);
            if let Some(k) = b.apps.get_mut(&key) {
                if now.saturating_sub(k.last) >= 3_600_000 {
                    changed = true;
                }
                k.last = now;
                if k.blocked && !k.path.eq_ignore_ascii_case(path) {
                    reblock.push((key, k.path.clone(), path.clone()));
                }
                continue;
            }
            let k = Known {
                name: name.clone(),
                exe: exe_of(path),
                path: path.clone(),
                first: now,
                last: now,
                dest: dest.clone(),
                fresh: !learning,
                ..Default::default()
            };
            log(format!("firstnet: {} {} -> {dest}", if learning { "learned" } else { "NEW" }, path));
            if !learning {
                fresh.push(k.clone());
            }
            b.apps.insert(key, k);
            changed = true;
        }
        // keep the book small: evict oldest-seen (`last`) non-blocked entries beyond 2000
        if b.apps.len() > 2000 {
            let mut v: Vec<(String, u64)> = b
                .apps
                .iter()
                .filter(|(_, k)| !k.blocked)
                .map(|(n, k)| (n.clone(), k.last.max(k.first)))
                .collect();
            v.sort_by_key(|x| x.1);
            let drop_n = b.apps.len().saturating_sub(2000);
            for (n, _) in v.into_iter().take(drop_n) {
                b.apps.remove(&n);
                changed = true;
            }
        }
        if changed {
            save(b);
        }
    }
    // Re-apply firewall block outside BOOK lock when a blocked program updated its path.
    if !reblock.is_empty() {
        let mut applied = Vec::new();
        for (key, old_path, new_path) in reblock {
            match firewall_block(&new_path) {
                Ok(new_rule) => {
                    log(format!("firstnet: updated blocked app path {old_path} -> {new_path}"));
                    applied.push((key, new_path, new_rule));
                }
                Err(e) => log(format!("firstnet: re-block {new_path} failed: {e}")),
            }
        }
        if !applied.is_empty() {
            let mut g = BOOK.lock().unwrap();
            if let Some(b) = g.as_mut() {
                for (key, new_path, new_rule) in applied {
                    if let Some(k) = b.apps.get_mut(&key) {
                        k.exe = exe_of(&new_path);
                        k.path = new_path;
                        k.rule = new_rule;
                    }
                }
                save(b);
            }
        }
    }
    fresh
}

/// Programs that got a note (newest first), for the guard page.
pub fn recent() -> Vec<Known> {
    let g = BOOK.lock().unwrap();
    let Some(b) = g.as_ref() else { return Vec::new() };
    let mut v: Vec<Known> = b.apps.values().filter(|k| k.fresh || k.blocked).cloned().collect();
    v.sort_by(|a, b| b.first.cmp(&a.first));
    v.truncate(50);
    v
}

pub fn learning_until() -> u64 {
    BOOK.lock().unwrap().as_ref().map(|b| b.learn_until).unwrap_or(0)
}

/// Network profiles whose Windows Firewall is switched off ("域" / "专用" / "公用"); a group
/// policy setting wins over the local one. Rules added there don't do anything.
pub fn firewall_off() -> Vec<String> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let hk = winreg::RegKey::predef(HKEY_LOCAL_MACHINE);
    let read = |path: &str| hk.open_subkey(path).ok().and_then(|k| k.get_value::<u32, _>("EnableFirewall").ok());
    [("DomainProfile", "DomainProfile", crate::i18n::t("域")), ("StandardProfile", "PrivateProfile", crate::i18n::t("专用")), ("PublicProfile", "PublicProfile", crate::i18n::t("公用"))]
        .into_iter()
        .filter(|(local, policy, _)| {
            let p = read(&format!(r"SOFTWARE\Policies\Microsoft\WindowsFirewall\{policy}"));
            let l = read(&format!(r"SYSTEM\CurrentControlSet\Services\SharedAccess\Parameters\FirewallPolicy\{local}"));
            p.or(l) == Some(0)
        })
        .map(|(_, _, name)| name)
        .collect()
}

/// A warning to add after a firewall rule was written, when the firewall can't enforce it.
pub fn firewall_warning() -> Option<String> {
    let off = firewall_off();
    match off.len() {
        0 => None,
        3 => Some(crate::i18n::t("但 Windows 防火墙现在是关着的，规则要等防火墙打开后才生效").into()),
        _ => Some(crate::tr!("但 Windows 防火墙在「{0}」网络下是关着的，连这类网络时规则不生效", off.join("、"))),
    }
}

fn rule_name(exe: &str) -> String {
    format!("MicaBar 禁止联网 - {exe}")
}

fn netsh(args: &[String]) -> Result<(), String> {
    let out = std::process::Command::new("netsh").args(args).creation_flags(CREATE_NO_WINDOW).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        let stdout = crate::guard::decode_console(&out.stdout);
        let stderr = crate::guard::decode_console(&out.stderr);
        let msg = if stdout.trim().is_empty() { stderr } else { stdout };
        Err(crate::tr!("netsh 失败（{0}）", msg.trim()))
    }
}

/// Firewall: no outgoing traffic at all for the program. Returns the rule name.
pub fn firewall_block(path: &str) -> Result<String, String> {
    let name = rule_name(&exe_of(path));
    let _ = firewall_unblock(&name, path);
    netsh(&[
        "advfirewall".into(),
        "firewall".into(),
        "add".into(),
        "rule".into(),
        format!("name={name}"),
        "dir=out".into(),
        "action=block".into(),
        format!("program={path}"),
        "enable=yes".into(),
        "profile=any".into(),
        "description=Added by MicaBar: blocked when it first went online. Delete to allow it again.".into(),
    ])?;
    log(format!("firstnet: firewall block {path}"));
    Ok(name)
}

pub fn firewall_unblock(name: &str, path: &str) -> Result<(), String> {
    let r = netsh(&[
        "advfirewall".into(),
        "firewall".into(),
        "delete".into(),
        "rule".into(),
        format!("name={name}"),
        format!("program={path}"),
    ]);
    if r.is_ok() {
        return Ok(());
    }
    let r2 = netsh(&[
        "advfirewall".into(),
        "firewall".into(),
        "delete".into(),
        "rule".into(),
        format!("name={name}"),
    ]);
    if r2.is_ok() || !crate::guard::firewall_rule_exists(name) {
        Ok(())
    } else {
        r
    }
}

/// Records the block / unblock in the book.
pub fn set_blocked(path: &str, blocked: bool, rule: &str, core_rule: &str) {
    let mut g = BOOK.lock().unwrap();
    let Some(b) = g.as_mut() else { return };
    let key = norm(path);
    let e = b.apps.entry(key).or_insert_with(|| Known { name: exe_of(path), exe: exe_of(path), path: path.into(), first: now_ms(), fresh: true, ..Default::default() });
    e.blocked = blocked;
    e.allowed = !blocked;
    e.rule = rule.into();
    e.core_rule = core_rule.into();
    save(b);
}

pub fn get(path: &str) -> Option<Known> {
    BOOK.lock().unwrap().as_ref()?.apps.get(&norm(path)).cloned()
}

/// "允许": stays in the list as allowed.
pub fn allow(path: &str) {
    let mut g = BOOK.lock().unwrap();
    if let Some(b) = g.as_mut() {
        if let Some(k) = b.apps.get_mut(&norm(path)) {
            k.allowed = true;
        }
        save(b);
    }
}

/// Removes a program from the list (it counts as known; nothing else changes).
pub fn dismiss(path: &str) {
    let mut g = BOOK.lock().unwrap();
    if let Some(b) = g.as_mut() {
        if let Some(k) = b.apps.get_mut(&norm(path)) {
            if !k.blocked {
                k.fresh = false;
            }
        }
        save(b);
    }
}

#[cfg(test)]
mod tests {
    use super::norm;
    #[test]
    fn folds_versions() {
        assert_eq!(norm(r"C:\Users\a\AppData\Local\Discord\app-1.0.9175\Discord.exe"), r"c:\users\a\appdata\local\discord\*\discord.exe");
        assert_eq!(norm(r"C:\Program Files\Google\Chrome\Application\chrome.exe"), r"c:\program files\google\chrome\application\chrome.exe");
        assert_eq!(norm(r"C:\x\125.0.6422.60\y.exe"), r"c:\x\*\y.exe");
        assert_eq!(norm(r"C:\x\tool-2.1.exe"), r"c:\x\tool-2.1.exe");
        assert_ne!(norm(r"C:\Program Files\Good-1.0\helper.exe"), norm(r"C:\Program Files\Evil-2.0\helper.exe"));
        assert_eq!(norm(r"C:\Program Files\Good-1.0\helper.exe"), norm(r"C:\Program Files\Good-1.2\helper.exe"));
        assert_ne!(
            norm(r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1.20.11781.0_x64__8wekyb3d8bbwe\wt.exe"),
            norm(r"C:\Program Files\WindowsApps\Other.App_1.0.0.0_x64__abcdef\wt.exe")
        );
        assert_eq!(
            norm(r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1.20.11781.0_x64__8wekyb3d8bbwe\wt.exe"),
            norm(r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1.21.2361.0_x64__8wekyb3d8bbwe\wt.exe")
        );
    }
}
