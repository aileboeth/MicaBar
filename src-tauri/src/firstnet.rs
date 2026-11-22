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
    PathBuf::from(base).join("MicaBar").join("known-apps.json")
}

fn save(b: &Book) {
    if let Ok(s) = serde_json::to_string(b) {
        let _ = std::fs::write(file(), s);
    }
}

/// A path's identity across updates: lower case, version-like folders replaced by "*".
pub fn norm(path: &str) -> String {
    let p = path.to_lowercase().replace('/', "\\");
    p.split('\\')
        .map(|seg| {
            let b = seg.as_bytes();
            // "1.2", "app-1.2.3", "v2.0", "chrome-125.0.1": digits, a dot, digits
            let versiony = b.windows(3).any(|w| w[0].is_ascii_digit() && w[1] == b'.' && w[2].is_ascii_digit());
            let last = seg.ends_with(".exe");
            if versiony && !last {
                "*"
            } else {
                seg
            }
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
    let b = match std::fs::read_to_string(file()).ok().and_then(|s| serde_json::from_str::<Book>(&s).ok()) {
        Some(b) => b,
        None => {
            let now = now_ms();
            let mut b = Book { learn_until: now + LEARN_MS, apps: HashMap::new() };
            for (name, path) in history {
                b.apps.entry(norm(&path)).or_insert(Known { name, path: path.clone(), exe: exe_of(&path), first: now, ..Default::default() });
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
    let mut g = BOOK.lock().unwrap();
    let Some(b) = g.as_mut() else { return Vec::new() };
    let now = now_ms();
    let learning = now < b.learn_until;
    let mut fresh = Vec::new();
    let mut changed = false;
    for (name, path, dest) in seen {
        if skip(path) {
            continue;
        }
        let key = norm(path);
        if b.apps.contains_key(&key) {
            continue;
        }
        let k = Known { name: name.clone(), exe: exe_of(path), path: path.clone(), first: now, dest: dest.clone(), fresh: !learning, ..Default::default() };
        log(format!("firstnet: {} {} -> {dest}", if learning { "learned" } else { "NEW" }, path));
        if !learning {
            fresh.push(k.clone());
        }
        b.apps.insert(key, k);
        changed = true;
    }
    // keep the book small: quiet entries beyond 2000 go (oldest first)
    if b.apps.len() > 2000 {
        let mut v: Vec<(String, u64)> = b.apps.iter().filter(|(_, k)| !k.blocked).map(|(n, k)| (n.clone(), k.first)).collect();
        v.sort_by_key(|x| x.1);
        for (n, _) in v.into_iter().take(b.apps.len() - 2000) {
            b.apps.remove(&n);
        }
    }
    if changed {
        save(b);
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

fn rule_name(exe: &str) -> String {
    format!("MicaBar 禁止联网 - {exe}")
}

fn netsh(args: &[String]) -> Result<(), String> {
    let out = std::process::Command::new("netsh").args(args).creation_flags(CREATE_NO_WINDOW).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("netsh 失败（{}）", String::from_utf8_lossy(&out.stdout).trim()))
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
    netsh(&["advfirewall".into(), "firewall".into(), "delete".into(), "rule".into(), format!("name={name}"), format!("program={path}")])
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
    }
}
