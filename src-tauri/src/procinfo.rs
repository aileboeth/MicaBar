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
    pub exe: String,
    pub path: Option<String>,
    pub display: String,
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

fn file_description(path: &str) -> Option<String> {
    unsafe {
        let wpath = wide(path);
        let mut dummy = 0u32;
        let size = GetFileVersionInfoSizeW(wpath.as_ptr(), &mut dummy);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(wpath.as_ptr(), 0, size, data.as_mut_ptr() as *mut c_void) == 0 {
            return None;
        }
        let mut ptr: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let mut langs: Vec<(u16, u16)> = Vec::new();
        let q = wide("\\VarFileInfo\\Translation");
        if VerQueryValueW(data.as_ptr() as *const c_void, q.as_ptr(), &mut ptr, &mut len) != 0 && len >= 4 {
            let n = (len / 4) as usize;
            let pairs = std::slice::from_raw_parts(ptr as *const u16, n * 2);
            for i in 0..n {
                langs.push((pairs[i * 2], pairs[i * 2 + 1]));
            }
        }
        langs.push((0x0409, 0x04b0));
        langs.push((0x0409, 0x04e4));
        for (lang, cp) in langs {
            let q = wide(&format!("\\StringFileInfo\\{:04x}{:04x}\\FileDescription", lang, cp));
            if VerQueryValueW(data.as_ptr() as *const c_void, q.as_ptr(), &mut ptr, &mut len) != 0 && len > 1 {
                let s = std::slice::from_raw_parts(ptr as *const u16, len as usize);
                let d = from_wide(s).trim().to_string();
                if !d.is_empty() {
                    return Some(d);
                }
            }
        }
        None
    }
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

    fn build(&mut self, pid: u32, exe: &str) -> ProcInfo {
        if pid == 0 {
            return ProcInfo { exe: "Idle".into(), path: None, display: "系统空闲进程".into() };
        }
        if pid == 4 {
            return ProcInfo { exe: "System".into(), path: None, display: "System".into() };
        }
        let path = image_path(pid);
        let desc = match &path {
            Some(p) => self
                .desc_by_path
                .entry(p.clone())
                .or_insert_with(|| file_description(p))
                .clone(),
            None => None,
        };
        let display = match desc {
            // Some descriptions are long sentences; keep short, meaningful ones.
            Some(d) if d.chars().count() <= 32 => d,
            _ => stem(exe),
        };
        ProcInfo { exe: exe.to_string(), path, display }
    }

    pub fn get(&self, pid: u32) -> Option<&ProcInfo> {
        self.by_pid.get(&pid)
    }
}
