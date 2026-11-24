//! Process names, image paths and friendly names (FileDescription), cached per PID.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};

#[derive(Clone, Debug)]
pub struct ProcInfo {
    /// Pre-lowercased identity (`path` if known, else `exe`), avoiding per-socket `to_lowercase()` allocations.
    pub key: String,
    pub exe: String,
    pub path: Option<String>,
    pub display: String,
    /// exe to take the icon from when this one has none of its own (helper of a bigger app)
    pub icon: Option<String>,
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// (pid, exe name) for every running process.
pub fn list_processes() -> Vec<(u32, String)> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                out.push((e.th32ProcessID, from_wide(&e.szExeFile)));
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

pub fn image_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if ok == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

pub fn file_description(path: &str) -> Option<String> {
    unsafe {
        let wpath = wide(path);
        let mut dummy = 0u32;
        let size = GetFileVersionInfoSizeW(wpath.as_ptr(), &mut dummy);
        if size == 0 {
            return None;
        }
        // Allocate u32-aligned backing storage so u16 queries inside it are properly aligned.
        let mut data = vec![0u32; (size as usize + 3) / 4];
        if GetFileVersionInfoW(wpath.as_ptr(), 0, size, data.as_mut_ptr() as *mut c_void) == 0 {
            return None;
        }
        let read_u16_vec = |p: *const c_void, count: usize| -> Vec<u16> {
            if (p as usize) % 2 == 0 {
                std::slice::from_raw_parts(p as *const u16, count).to_vec()
            } else {
                let raw = p as *const u8;
                (0..count)
                    .map(|i| std::ptr::read_unaligned(raw.add(i * 2) as *const u16))
                    .collect()
            }
        };
        let mut ptr: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let mut langs: Vec<(u16, u16)> = Vec::new();
        let q = wide("\\VarFileInfo\\Translation");
        if VerQueryValueW(data.as_ptr() as *const c_void, q.as_ptr(), &mut ptr, &mut len) != 0 && len >= 4 && !ptr.is_null() {
            let n = (len / 4) as usize;
            let pairs = read_u16_vec(ptr, n * 2);
            for i in 0..n {
                langs.push((pairs[i * 2], pairs[i * 2 + 1]));
            }
        }
        langs.push((0x0409, 0x04b0));
        langs.push((0x0409, 0x04e4));
        for (lang, cp) in langs {
            let q = wide(&format!("\\StringFileInfo\\{:04x}{:04x}\\FileDescription", lang, cp));
            if VerQueryValueW(data.as_ptr() as *const c_void, q.as_ptr(), &mut ptr, &mut len) != 0 && len > 1 && !ptr.is_null() {
                let s = read_u16_vec(ptr, len as usize);
                let d = from_wide(&s).trim().to_string();
                if !d.is_empty() {
                    return Some(d);
                }
            }
        }
        None
    }
}

/// The application a program file belongs to, from its install folder: the first folder under
/// Program Files, %LOCALAPPDATA%\Programs, %LOCALAPPDATA% or %APPDATA% ("Antigravity" for
/// ...\Programs\Antigravity\resources\bin\language_server.exe), plus that app's main exe if found.
fn owner_app(path: &str) -> Option<(String, Option<String>)> {
    let parts: Vec<&str> = path.split('\\').collect();
    let lower: Vec<String> = parts.iter().map(|s| s.to_ascii_lowercase()).collect();
    let mut i = None;
    for (k, s) in lower.iter().enumerate() {
        let next = lower.get(k + 1).map(|x| x.as_str()).unwrap_or("");
        let hit = match s.as_str() {
            "program files" | "program files (x86)" => !matches!(next, "common files" | "windowsapps"),
            "programs" => k > 0 && lower[k - 1] == "local",
            "local" | "roaming" => k > 0 && lower[k - 1] == "appdata" && !matches!(next, "programs" | "temp" | "microsoft" | "packages"),
            _ => false,
        };
        if hit {
            i = Some(k + 1);
            break;
        }
    }
    let i = i?;
    // the file must sit at least one level below the app folder
    if i + 1 >= parts.len() {
        return None;
    }
    let app = parts[i].to_string();
    let root = parts[..=i].join("\\");
    let main = [format!("{root}\\{app}.exe")]
        .into_iter()
        .find(|p| Path::new(p).is_file())
        .or_else(|| {
            std::fs::read_dir(&root).ok()?.flatten().map(|e| e.path()).find_map(|p| {
                let n = p.file_name()?.to_string_lossy().to_ascii_lowercase();
                (n.ends_with(".exe") && !n.contains("unins") && !n.contains("update") && !n.contains("crash")).then(|| p.display().to_string())
            })
        });
    Some((app, main))
}

fn stem(exe: &str) -> String {
    Path::new(exe)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| exe.to_string())
}

#[derive(Default)]
pub struct ProcCache {
    by_pid: HashMap<u32, ProcInfo>,
    desc_by_path: HashMap<String, Option<String>>,
    owner_by_dir: HashMap<String, Option<(String, Option<String>)>>,
}

impl ProcCache {
    /// Refreshes the cache from a process list; entries whose (pid, exe) changed are re-queried.
    pub fn refresh(&mut self, procs: &[(u32, String)]) {
        let mut next = HashMap::with_capacity(procs.len());
        for (pid, exe) in procs {
            if let Some(old) = self.by_pid.remove(pid) {
                if &old.exe == exe {
                    next.insert(*pid, old);
                    continue;
                }
            }
            let info = self.build(*pid, exe);
            next.insert(*pid, info);
        }
        self.by_pid = next;
    }

    /// Lightweight refresh that retains already-cached running PIDs and only queries Win32 version
    /// info / image paths for PIDs in `needed_pids` (e.g. processes with active sockets or traffic).
    pub fn refresh_active(&mut self, procs: &[(u32, String)], needed_pids: &std::collections::HashSet<u32>) {
        let mut next = HashMap::with_capacity(needed_pids.len().max(32));
        for (pid, exe) in procs {
            if let Some(old) = self.by_pid.remove(pid) {
                if &old.exe == exe {
                    next.insert(*pid, old);
                    continue;
                }
            }
            if needed_pids.contains(pid) {
                let info = self.build(*pid, exe);
                next.insert(*pid, info);
            }
        }
        self.by_pid = next;
    }

    fn owner_cached(&mut self, path: &str) -> Option<(String, Option<String>)> {
        let dir_key = Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_else(|| path.to_ascii_lowercase());
        if self.owner_by_dir.len() > 4096 {
            self.owner_by_dir.clear();
        }
        self.owner_by_dir
            .entry(dir_key)
            .or_insert_with(|| owner_app(path))
            .clone()
    }

    fn build(&mut self, pid: u32, exe: &str) -> ProcInfo {
        if pid == 0 {
            return ProcInfo { key: "idle".into(), exe: "Idle".into(), path: None, display: crate::i18n::t("系统空闲进程").into(), icon: None };
        }
        if pid == 4 {
            return ProcInfo { key: "system".into(), exe: "System".into(), path: None, display: "System".into(), icon: None };
        }
        let path = image_path(pid);
        if self.desc_by_path.len() > 4096 {
            self.desc_by_path.clear();
        }
        let path_lower = path.as_ref().map(|p| p.to_lowercase());
        let desc = match (&path, &path_lower) {
            (Some(p), Some(pl)) => self
                .desc_by_path
                .entry(pl.clone())
                .or_insert_with(|| file_description(p))
                .clone(),
            _ => None,
        };
        let mut icon = None;
        let display = match desc {
            // Some descriptions are long sentences; keep short, meaningful ones.
            Some(d) if d.chars().count() <= 32 => d,
            _ => {
                // no description: a helper inside a bigger app's folder (Antigravity's
                // language_server.exe…) is named after that app and borrows its icon
                match path.as_deref().and_then(|p| self.owner_cached(p)) {
                    Some((app, main)) if !app.eq_ignore_ascii_case(&stem(exe)) => {
                        icon = main;
                        format!("{app} · {}", stem(exe))
                    }
                    _ => stem(exe),
                }
            }
        };
        let key = path_lower.unwrap_or_else(|| exe.to_lowercase());
        ProcInfo { key, exe: exe.to_string(), path, display, icon }
    }

    pub fn get(&self, pid: u32) -> Option<&ProcInfo> {
        self.by_pid.get(&pid)
    }
}
