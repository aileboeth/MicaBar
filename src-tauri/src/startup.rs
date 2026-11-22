//! Programs that start with Windows, as Task Manager's "Startup apps" lists them: the Run keys
//! (current user, all users, 32-bit) and the two Startup folders. Turning one off or on writes the
//! same StartupApproved flag Task Manager writes, so nothing is deleted and both stay in step.

use std::path::{Path, PathBuf};

use serde::Serialize;
use winreg::enums::{RegType, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE};
use winreg::{RegKey, RegValue, HKEY};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN32: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// "<source>|<name>", passed back to `set`
    pub id: String,
    pub name: String,
    /// the program's own description, when it differs from the name
    pub desc: Option<String>,
    /// 当前用户 / 所有用户 · 注册表 / 启动文件夹
    pub source: String,
    pub command: String,
    /// file to take the icon from
    pub path: Option<String>,
    pub enabled: bool,
    /// the program file is gone (the entry does nothing)
    pub missing: bool,
}

struct Source {
    key: &'static str,
    root: HKEY,
    label: &'static str,
    /// StartupApproved subkey with the on/off flags
    approved: &'static str,
}

const SOURCES: [Source; 3] = [
    Source { key: "hkcu", root: HKEY_CURRENT_USER, label: "当前用户 · 注册表", approved: "Run" },
    Source { key: "hklm", root: HKEY_LOCAL_MACHINE, label: "所有用户 · 注册表", approved: "Run" },
    Source { key: "hklm32", root: HKEY_LOCAL_MACHINE, label: "所有用户 · 注册表（32 位）", approved: "Run32" },
];

fn run_path(src: &Source) -> &'static str {
    if src.key == "hklm32" {
        RUN32
    } else {
        RUN
    }
}

fn folders() -> [(&'static str, HKEY, &'static str, Option<PathBuf>); 2] {
    let env = |v: &str| std::env::var(v).ok().map(PathBuf::from);
    [
        ("ufolder", HKEY_CURRENT_USER, "当前用户 · 启动文件夹", env("APPDATA").map(|p| p.join(r"Microsoft\Windows\Start Menu\Programs\Startup"))),
        ("cfolder", HKEY_LOCAL_MACHINE, "所有用户 · 启动文件夹", env("ProgramData").map(|p| p.join(r"Microsoft\Windows\Start Menu\Programs\StartUp"))),
    ]
}

/// Absent or an even first byte = on (Task Manager writes 02 / 06 for on, 03 for off).
fn approved(root: HKEY, sub: &str, name: &str) -> bool {
    RegKey::predef(root)
        .open_subkey(format!(r"{APPROVED}\{sub}"))
        .and_then(|k| k.get_raw_value(name))
        .map(|v| v.bytes.first().map(|b| b & 1 == 0).unwrap_or(true))
        .unwrap_or(true)
}

/// "%ProgramFiles%\x" -> the real folder.
fn expand(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(a) = rest.find('%') {
        let Some(b) = rest[a + 1..].find('%') else { break };
        let var = &rest[a + 1..a + 1 + b];
        out.push_str(&rest[..a]);
        match std::env::var(var) {
            Ok(v) if !var.is_empty() => out.push_str(&v),
            _ => out.push_str(&rest[a..a + b + 2]),
        }
        rest = &rest[a + b + 2..];
    }
    out.push_str(rest);
    out
}

/// The program a command line starts: "\"C:\\a b\\x.exe\" --min" or "C:\\a b\\x.exe --min".
fn program(cmd: &str) -> Option<String> {
    let c = expand(cmd.trim());
    if let Some(r) = c.strip_prefix('"') {
        return r.split('"').next().map(str::to_string).filter(|s| !s.is_empty());
    }
    let lower = c.to_ascii_lowercase();
    for ext in [".exe", ".bat", ".cmd", ".lnk", ".com"] {
        if let Some(i) = lower.find(ext) {
            return Some(c[..i + ext.len()].to_string());
        }
    }
    c.split_whitespace().next().map(str::to_string)
}

fn describe(name: &str, path: Option<&str>) -> Option<String> {
    let d = path.and_then(crate::procinfo::file_description)?;
    (!d.eq_ignore_ascii_case(name)).then_some(d)
}

pub fn list() -> Vec<Item> {
    let mut out = Vec::new();
    for src in &SOURCES {
        let Ok(k) = RegKey::predef(src.root).open_subkey_with_flags(run_path(src), KEY_READ) else { continue };
        for (name, _) in k.enum_values().flatten() {
            if name.is_empty() {
                continue;
            }
            let Ok(command) = k.get_value::<String, _>(&name) else { continue };
            let path = program(&command);
            let missing = path.as_deref().map(|p| p.contains('\\') && !Path::new(p).exists()).unwrap_or(false);
            out.push(Item {
                id: format!("{}|{name}", src.key),
                desc: describe(&name, path.as_deref()),
                source: crate::i18n::t(src.label),
                enabled: approved(src.root, src.approved, &name),
                name,
                command,
                path,
                missing,
            });
        }
    }
    for (key, root, label, dir) in folders() {
        let Some(dir) = dir else { continue };
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let file = e.file_name().to_string_lossy().to_string();
            if !p.is_file() || file.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            let name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| file.clone());
            let path = p.display().to_string();
            out.push(Item {
                id: format!("{key}|{file}"),
                desc: None,
                source: crate::i18n::t(label),
                enabled: approved(root, "StartupFolder", &file),
                name,
                command: path.clone(),
                path: Some(path),
                missing: false,
            });
        }
    }
    out.sort_by_key(|i| (!i.enabled, i.name.to_lowercase()));
    out
}

/// On / off through StartupApproved (12 bytes: flag, 3 zero bytes, FILETIME of turning it off).
pub fn set(id: &str, on: bool) -> Result<(), String> {
    let (key, name) = id.split_once('|').ok_or(crate::i18n::t("无效的启动项"))?;
    let (root, sub) = match key {
        "hkcu" => (HKEY_CURRENT_USER, "Run"),
        "hklm" => (HKEY_LOCAL_MACHINE, "Run"),
        "hklm32" => (HKEY_LOCAL_MACHINE, "Run32"),
        "ufolder" => (HKEY_CURRENT_USER, "StartupFolder"),
        "cfolder" => (HKEY_LOCAL_MACHINE, "StartupFolder"),
        _ => return Err(crate::i18n::t("无效的启动项").into()),
    };
    // only entries that exist (the page can't make up new names)
    if !list().iter().any(|i| i.id == id) {
        return Err(crate::i18n::t("这个启动项已经不在了").into());
    }
    let mut bytes = vec![if on { 2u8 } else { 3u8 }, 0, 0, 0];
    let ft: u64 = if on {
        0
    } else {
        let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() / 100).unwrap_or(0) as u64;
        unix + 116_444_736_000_000_000
    };
    bytes.extend_from_slice(&ft.to_le_bytes());
    let (k, _) = RegKey::predef(root)
        .create_subkey_with_flags(format!(r"{APPROVED}\{sub}"), KEY_SET_VALUE)
        .map_err(|e| crate::tr!("打不开注册表：{0}", e))?;
    k.set_raw_value(name, &RegValue { bytes, vtype: RegType::REG_BINARY }).map_err(|e| crate::tr!("写不进注册表：{0}", e))?;
    crate::log(format!("startup: {id} -> {}", if on { "on" } else { "off" }));
    Ok(())
}
