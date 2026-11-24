//! System monitor page: usage per CPU core (performance / efficiency cores told apart), GPU load and
//! video memory, memory, and the busiest apps by CPU and by memory. Nothing runs in the background:
//! the page asks for a sample about once a second while it is open.

use std::collections::HashMap;
use std::ffi::c_void;
use std::time::Instant;

use serde::Serialize;
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

use crate::procinfo::{self, ProcCache};

#[link(name = "ntdll")]
extern "system" {
    fn NtQuerySystemInformation(class: u32, buf: *mut c_void, len: u32, ret: *mut u32) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetSystemCpuSetInformation(info: *mut u8, len: u32, ret: *mut u32, process: *mut c_void, flags: u32) -> i32;
    fn K32GetProcessMemoryInfo(h: *mut c_void, counters: *mut MemCounters, cb: u32) -> i32;
}

#[link(name = "pdh")]
extern "system" {
    fn PdhOpenQueryW(source: *const u16, user: usize, query: *mut *mut c_void) -> u32;
    fn PdhAddEnglishCounterW(query: *mut c_void, path: *const u16, user: usize, counter: *mut *mut c_void) -> u32;
    fn PdhCollectQueryData(query: *mut c_void) -> u32;
    fn PdhGetFormattedCounterArrayW(counter: *mut c_void, format: u32, size: *mut u32, count: *mut u32, items: *mut PdhItem) -> u32;
    fn PdhCloseQuery(query: *mut c_void) -> u32;
}

/// SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CoreTimes {
    idle: i64,
    kernel: i64,
    user: i64,
    dpc: i64,
    interrupt: i64,
    interrupts: u32,
}

/// PROCESS_MEMORY_COUNTERS_EX2 (Windows 10 1809+): has the private working set, the figure
/// Task Manager shows as "memory".
#[repr(C)]
#[derive(Default)]
struct MemCounters {
    cb: u32,
    page_faults: u32,
    peak_ws: usize,
    ws: usize,
    quota_peak_paged: usize,
    quota_paged: usize,
    quota_peak_nonpaged: usize,
    quota_nonpaged: usize,
    pagefile: usize,
    peak_pagefile: usize,
    private: usize,
    private_ws: usize,
    shared_commit: u64,
}

/// PDH_FMT_COUNTERVALUE_ITEM_W with a double value
#[repr(C)]
struct PdhItem {
    name: *const u16,
    status: u32,
    value: f64,
}

const PDH_FMT_DOUBLE: u32 = 0x0000_0200;
const PDH_FMT_NOCAP100: u32 = 0x0000_8000;
const PDH_MORE_DATA: u32 = 0x8000_07D2;

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppUse {
    pub name: String,
    pub path: Option<String>,
    pub icon: Option<String>,
    /// percent of the whole CPU
    pub cpu: f64,
    /// private working set, bytes
    pub mem: u64,
    pub procs: u32,
    /// false for Windows' own core processes and MicaBar itself (no "end" button)
    pub killable: bool,
}

/// Processes that keep Windows itself running: ending them crashes or logs off the session.
const PROTECTED: [&str; 16] = [
    "system", "idle", "registry", "memory compression", "secure system", "smss.exe", "csrss.exe", "wininit.exe",
    "winlogon.exe", "services.exe", "lsass.exe", "lsaiso.exe", "svchost.exe", "dwm.exe", "fontdrvhost.exe", "micabar.exe",
];

/// (pid, parent pid, exe) of every process.
fn process_tree() -> Vec<(u32, u32, String)> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
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
                let end = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                out.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..end])));
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

/// MicaBar and everything it started (its WebView2 processes): never ended from the list.
fn own_tree(tree: &[(u32, u32, String)]) -> std::collections::HashSet<u32> {
    let mut own = std::collections::HashSet::from([std::process::id()]);
    loop {
        let before = own.len();
        for (pid, parent, _) in tree {
            if own.contains(parent) && *pid != 0 {
                own.insert(*pid);
            }
        }
        if own.len() == before {
            return own;
        }
    }
}

fn protected(exe: &str) -> bool {
    PROTECTED.contains(&exe.to_ascii_lowercase().as_str())
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    /// percent per logical processor; empty on the first sample
    pub cores: Vec<f64>,
    /// efficiency class per logical processor (higher = faster core); all equal on non-hybrid CPUs
    pub eff: Vec<u8>,
    pub cpu: Option<f64>,
    pub cpu_name: String,
    pub gpu: Option<f64>,
    pub gpu_name: Option<String>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub apps: Vec<AppUse>,
}

struct Gpu {
    query: *mut c_void,
    engine: *mut c_void,
    memory: *mut c_void,
    primed: bool,
}

impl Drop for Gpu {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.query) };
    }
}

pub struct Monitor {
    prev_cores: Vec<CoreTimes>,
    eff: Option<Vec<u8>>,
    cpu_name: Option<String>,
    procs: ProcCache,
    /// (pid, creation time) -> cpu time (100 ns)
    prev_proc: HashMap<(u32, u64), u64>,
    prev_at: Option<Instant>,
    gpu: Option<Gpu>,
    gpu_tried: bool,
    /// (name, dedicated_vram_bytes, lowercase_luid_tag)
    adapter: Option<(String, u64, String)>,
    last_used: Option<Instant>,
    cached_sample: Option<(Instant, Sample)>,
    /// app display name -> (pid, exe_lower, created_filetime) of processes that may be ended
    kill: HashMap<String, Vec<(u32, String, u64)>>,
}

// the PDH handles are only touched behind the monitor's mutex
unsafe impl Send for Monitor {}

fn ft(f: &FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

fn core_times() -> Vec<CoreTimes> {
    let mut buf = vec![CoreTimes::default(); 256];
    let mut ret = 0u32;
    // SystemProcessorPerformanceInformation = 8 (processor group of the calling thread)
    let st = unsafe {
        NtQuerySystemInformation(8, buf.as_mut_ptr() as *mut c_void, (buf.len() * std::mem::size_of::<CoreTimes>()) as u32, &mut ret)
    };
    if st < 0 {
        return Vec::new();
    }
    buf.truncate(ret as usize / std::mem::size_of::<CoreTimes>());
    buf
}

/// Efficiency class per logical processor (index = LogicalProcessorIndex, group 0).
fn efficiency_classes(n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n];
    let mut len = 0u32;
    unsafe {
        GetSystemCpuSetInformation(std::ptr::null_mut(), 0, &mut len, std::ptr::null_mut(), 0);
        if len == 0 {
            return out;
        }
        let mut buf = vec![0u8; len as usize];
        if GetSystemCpuSetInformation(buf.as_mut_ptr(), len, &mut len, std::ptr::null_mut(), 0) == 0 {
            return out;
        }
        let mut off = 0usize;
        // SYSTEM_CPU_SET_INFORMATION: Size u32, Type u32, then the CpuSet fields
        while off + 20 <= len as usize {
            let size = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap()) as usize;
            if size == 0 {
                break;
            }
            let kind = u32::from_le_bytes(buf[off + 4..off + 8].try_into().unwrap());
            let group = u16::from_le_bytes(buf[off + 12..off + 14].try_into().unwrap());
            let index = buf[off + 14] as usize;
            let class = buf[off + 18];
            if kind == 0 && group == 0 && index < n {
                out[index] = class;
            }
            off += size;
        }
    }
    out
}

fn cpu_name() -> String {
    winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0")
        .and_then(|k| k.get_value::<String, _>("ProcessorNameString"))
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

/// The hardware adapter with the most dedicated video memory: (name, bytes, luid_tag).
fn main_adapter() -> Option<(String, u64, String)> {
    use windows::Win32::Graphics::Dxgi::*;
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut best: Option<(String, u64, String)> = None;
        let mut i = 0;
        while let Ok(a) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(d) = a.GetDesc1() else { continue };
            if d.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
                continue;
            }
            let end = d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len());
            let name = String::from_utf16_lossy(&d.Description[..end]).trim().to_string();
            let mem = d.DedicatedVideoMemory as u64;
            let luid = format!("luid_0x{:08x}_0x{:08x}", d.AdapterLuid.HighPart as u32, d.AdapterLuid.LowPart);
            if best.as_ref().map(|b| mem > b.1).unwrap_or(true) {
                best = Some((name, mem, luid));
            }
        }
        best
    }
}

fn open_gpu() -> Option<Gpu> {
    unsafe {
        let mut q = std::ptr::null_mut();
        if PdhOpenQueryW(std::ptr::null(), 0, &mut q) != 0 {
            return None;
        }
        let mut engine = std::ptr::null_mut();
        let mut memory = std::ptr::null_mut();
        let p1 = procinfo::wide("\\GPU Engine(*)\\Utilization Percentage");
        let p2 = procinfo::wide("\\GPU Adapter Memory(*)\\Dedicated Usage");
        if PdhAddEnglishCounterW(q, p1.as_ptr(), 0, &mut engine) != 0 {
            PdhCloseQuery(q);
            return None;
        }
        if PdhAddEnglishCounterW(q, p2.as_ptr(), 0, &mut memory) != 0 {
            memory = std::ptr::null_mut();
        }
        PdhCollectQueryData(q);
        Some(Gpu { query: q, engine, memory, primed: false })
    }
}

/// (instance name, value) of a wildcard counter, retrying on PDH_MORE_DATA.
fn counter_items(counter: *mut c_void) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    unsafe {
        let (mut size, mut count) = (0u32, 0u32);
        let mut st = PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100, &mut size, &mut count, std::ptr::null_mut());
        if st != PDH_MORE_DATA || size == 0 {
            return out;
        }
        let mut buf: Vec<u64> = Vec::new();
        for _ in 0..3 {
            buf.resize((size as usize + 7) / 8, 0);
            st = PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE | PDH_FMT_NOCAP100,
                &mut size,
                &mut count,
                buf.as_mut_ptr() as *mut PdhItem,
            );
            if st != PDH_MORE_DATA {
                break;
            }
        }
        if st != 0 {
            return out;
        }
        let items = buf.as_ptr() as *const PdhItem;
        for i in 0..count as usize {
            let it = &*items.add(i);
            // PDH_CSTATUS_VALID_DATA = 0, PDH_CSTATUS_NEW_DATA = 1
            if it.status > 1 || it.name.is_null() {
                continue;
            }
            let mut n = 0;
            while *it.name.add(n) != 0 {
                n += 1;
            }
            out.push((String::from_utf16_lossy(std::slice::from_raw_parts(it.name, n)), it.value));
        }
    }
    out
}

impl Monitor {
    fn new() -> Self {
        Monitor {
            prev_cores: Vec::new(),
            eff: None,
            cpu_name: None,
            procs: ProcCache::default(),
            prev_proc: HashMap::new(),
            prev_at: None,
            gpu: None,
            gpu_tried: false,
            adapter: None,
            last_used: None,
            cached_sample: None,
            kill: HashMap::new(),
        }
    }

    pub fn sample(&mut self) -> Sample {
        if let Some((at, ref cached)) = self.cached_sample {
            if at.elapsed() < std::time::Duration::from_millis(650) {
                self.last_used = Some(Instant::now());
                return cached.clone();
            }
        }
        // unused for a while: start over (the old deltas would span the whole pause)
        if self.last_used.map(|t| t.elapsed().as_secs() > 20).unwrap_or(false) {
            self.prev_cores.clear();
            self.prev_proc.clear();
            self.prev_at = None;
            self.gpu = None;
            self.gpu_tried = false;
            self.cached_sample = None;
        }
        self.last_used = Some(Instant::now());
        let mut s = Sample::default();

        // --- cores ---
        let now = core_times();
        if self.eff.as_ref().map(|e| e.len() != now.len()).unwrap_or(true) {
            self.eff = Some(efficiency_classes(now.len()));
        }
        s.eff = self.eff.clone().unwrap_or_default();
        if self.prev_cores.len() == now.len() {
            let (mut busy, mut total) = (0i64, 0i64);
            for (a, b) in self.prev_cores.iter().zip(&now) {
                // kernel time includes idle time
                let t = (b.kernel - a.kernel) + (b.user - a.user);
                let idle = b.idle - a.idle;
                s.cores.push(if t > 0 { ((1.0 - idle as f64 / t as f64) * 100.0).clamp(0.0, 100.0) } else { 0.0 });
                busy += (t - idle).max(0);
                total += t.max(0);
            }
            if total > 0 {
                s.cpu = Some((busy as f64 / total as f64 * 100.0).clamp(0.0, 100.0));
            }
        }
        self.prev_cores = now;
        s.cpu_name = self.cpu_name.get_or_insert_with(cpu_name).clone();

        // --- memory ---
        let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 {
            s.mem_total = m.ullTotalPhys;
            s.mem_used = m.ullTotalPhys.saturating_sub(m.ullAvailPhys);
        }

        // --- GPU: the busiest engine (Task Manager's figure) and dedicated memory ---
        if !self.gpu_tried {
            self.gpu_tried = true;
            if self.adapter.is_none() {
                self.adapter = main_adapter();
            }
            self.gpu = open_gpu();
        }
        let luid_filter = self.adapter.as_ref().map(|(_, _, luid)| luid.clone());
        if let Some((name, total, _)) = &self.adapter {
            s.gpu_name = Some(name.clone());
            s.vram_total = (*total > 0).then_some(*total);
        }
        if let Some(g) = &mut self.gpu {
            if !g.primed {
                // open_gpu() just performed the baseline PdhCollectQueryData; don't double-collect <1ms later.
                g.primed = true;
                if !g.memory.is_null() {
                    let mem_items = counter_items(g.memory);
                    let mut used = 0.0f64;
                    let mut matched = false;
                    if let Some(ref luid) = luid_filter {
                        for (name, v) in &mem_items {
                            if name.to_ascii_lowercase().contains(luid) {
                                matched = true;
                                used += *v;
                            }
                        }
                    }
                    if !matched {
                        used = mem_items.into_iter().map(|(_, v)| v).fold(0.0, f64::max);
                    }
                    if used > 0.0 {
                        s.vram_used = Some(used as u64);
                    }
                }
            } else if unsafe { PdhCollectQueryData(g.query) } == 0 {
                // "pid_1234_luid_0x00000000_0x0000D1F2_phys_0_eng_3_engtype_3D": sum each engine on main_adapter
                let eng_items = counter_items(g.engine);
                let mut engines: HashMap<String, f64> = HashMap::new();
                let mut matched_luid = false;
                if let Some(ref luid) = luid_filter {
                    for (name, v) in &eng_items {
                        let lower = name.to_ascii_lowercase();
                        if lower.contains(luid) {
                            matched_luid = true;
                            let key = lower.find("luid_").map(|i| lower[i..].to_string()).unwrap_or(lower);
                            *engines.entry(key).or_default() += *v;
                        }
                    }
                }
                if !matched_luid {
                    for (name, v) in eng_items {
                        let lower = name.to_ascii_lowercase();
                        let key = lower.find("luid_").map(|i| lower[i..].to_string()).unwrap_or(lower);
                        *engines.entry(key).or_default() += v;
                    }
                }
                if !engines.is_empty() {
                    s.gpu = Some(engines.values().cloned().fold(0.0, f64::max).clamp(0.0, 100.0));
                }
                if !g.memory.is_null() {
                    let mem_items = counter_items(g.memory);
                    let mut used = 0.0f64;
                    let mut matched_mem = false;
                    if let Some(ref luid) = luid_filter {
                        for (name, v) in &mem_items {
                            if name.to_ascii_lowercase().contains(luid) {
                                matched_mem = true;
                                used += *v;
                            }
                        }
                    }
                    if !matched_mem {
                        used = mem_items.into_iter().map(|(_, v)| v).fold(0.0, f64::max);
                    }
                    if used > 0.0 {
                        s.vram_used = Some(used as u64);
                    }
                }
            }
        }

        // --- apps: processes grouped by (exe_lower, display), excluding MicaBar's own tree ---
        let tree = process_tree();
        let own = own_tree(&tree);
        let list: Vec<(u32, String)> = tree
            .iter()
            .filter(|(p, _, _)| *p != 0 && !own.contains(p))
            .map(|(p, _, e)| (*p, e.clone()))
            .collect();
        self.procs.refresh(&list);
        let elapsed = self.prev_at.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
        self.prev_at = Some(Instant::now());
        let ncpu = s.eff.len().max(1) as f64;
        let mut next = HashMap::with_capacity(list.len());
        let mut groups: HashMap<(String, String), (AppUse, Vec<(u32, String, u64)>)> = HashMap::new();
        const CB_EX2: u32 = std::mem::size_of::<MemCounters>() as u32;
        const CB_EX: u32 = CB_EX2 - std::mem::size_of::<usize>() as u32 - std::mem::size_of::<u64>() as u32;

        for (pid, _) in &list {
            let Some(info) = self.procs.get(*pid) else { continue };
            let (mut cpu_t, mut created, mut mem) = (None, 0u64, 0u64);
            unsafe {
                let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, *pid);
                if !h.is_null() {
                    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
                    let (mut c, mut e, mut k, mut u) = (z, z, z, z);
                    if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) != 0 {
                        cpu_t = Some(ft(&k) + ft(&u));
                        created = ft(&c);
                    }
                    let mut mc = MemCounters { cb: CB_EX2, ..Default::default() };
                    if K32GetProcessMemoryInfo(h, &mut mc, CB_EX2) != 0 {
                        mem = if mc.private_ws > 0 { mc.private_ws as u64 } else { mc.ws as u64 };
                    } else {
                        // Windows 10 builds prior to PROCESS_MEMORY_COUNTERS_EX2 (96 B) accept EX (80 B).
                        mc = MemCounters { cb: CB_EX, ..Default::default() };
                        if K32GetProcessMemoryInfo(h, &mut mc, CB_EX) != 0 {
                            mem = if mc.private > 0 { mc.private as u64 } else { mc.ws as u64 };
                        }
                    }
                    CloseHandle(h);
                }
            }
            let mut cpu = 0.0;
            if let Some(t) = cpu_t {
                if let Some(prev) = self.prev_proc.get(&(*pid, created)) {
                    if elapsed > 0.0 {
                        cpu = (t.saturating_sub(*prev) as f64 / 1e7 / elapsed / ncpu * 100.0).clamp(0.0, 100.0);
                    }
                }
                next.insert((*pid, created), t);
            }
            if cpu_t.is_none() && mem == 0 {
                continue;
            }
            let exe_lower = info.exe.to_ascii_lowercase();
            let gkey = (exe_lower.clone(), info.display.clone());
            let (g, targets) = groups.entry(gkey).or_insert_with(|| {
                (
                    AppUse {
                        name: info.display.clone(),
                        path: info.path.clone(),
                        icon: info.icon.clone(),
                        ..Default::default()
                    },
                    Vec::new(),
                )
            });
            g.cpu += cpu;
            g.mem += mem;
            g.procs += 1;
            if !protected(&exe_lower) && *pid != 4 && created != 0 {
                g.killable = true;
                targets.push((*pid, exe_lower, created));
            }
        }
        self.prev_proc = next;

        // Disambiguate display names when two different executables share the same friendly name,
        // so `end_app(name)` never terminates an unrelated executable's processes.
        let mut display_counts: HashMap<String, usize> = HashMap::new();
        for (_, disp) in groups.keys() {
            *display_counts.entry(disp.clone()).or_default() += 1;
        }
        let mut kill: HashMap<String, Vec<(u32, String, u64)>> = HashMap::new();
        let mut all: Vec<AppUse> = Vec::with_capacity(groups.len());
        for ((exe_lower, disp), (mut app, targets)) in groups {
            if display_counts.get(&disp).copied().unwrap_or(0) > 1 {
                app.name = format!("{disp} ({exe_lower})");
            }
            if !targets.is_empty() {
                kill.insert(app.name.clone(), targets);
            }
            all.push(app);
        }
        self.kill = kill;

        // the top apps by CPU and by memory (the page sorts by either)
        all.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal));
        let mut keep: Vec<AppUse> = all.iter().take(15).cloned().collect();
        all.sort_by(|a, b| b.mem.cmp(&a.mem));
        for a in all.into_iter().take(15) {
            if !keep.iter().any(|k| k.name == a.name) {
                keep.push(a);
            }
        }
        s.apps = keep;
        self.cached_sample = Some((Instant::now(), s.clone()));
        s
    }
}

/// Ends every process of one app from the last sample (verifies pid, exe, and creation timestamp;
/// never Windows core processes or MicaBar's own). Returns (ended, failed).
pub fn end_app(name: &str) -> (u32, u32) {
    use windows_sys::Win32::System::Threading::{TerminateProcess, PROCESS_TERMINATE};
    let targets = monitor().lock().unwrap().kill.get(name).cloned().unwrap_or_default();
    let tree = process_tree();
    let own = own_tree(&tree);
    let (mut ok, mut bad) = (0, 0);
    for (pid, exe, expected_created) in targets {
        if pid == 0 || pid == 4 || own.contains(&pid) || protected(&exe) {
            continue;
        }
        let still = tree.iter().any(|(p, _, e)| *p == pid && e.eq_ignore_ascii_case(&exe));
        if !still {
            continue;
        }
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                bad += 1;
                continue;
            }
            let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            let (mut c, mut e, mut k, mut u) = (z, z, z, z);
            if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) == 0 || ft(&c) != expected_created {
                // PID exited and was reused by a new process since the sample was taken.
                CloseHandle(h);
                continue;
            }
            if TerminateProcess(h, 1) != 0 {
                ok += 1;
            } else {
                bad += 1;
            }
            CloseHandle(h);
        }
    }
    if ok > 0 {
        monitor().lock().unwrap().cached_sample = None;
    }
    crate::log(format!("sysmon: ended {name}: {ok} ok, {bad} failed"));
    (ok, bad)
}

pub fn monitor() -> &'static std::sync::Mutex<Monitor> {
    static M: std::sync::OnceLock<std::sync::Mutex<Monitor>> = std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(Monitor::new()))
}
