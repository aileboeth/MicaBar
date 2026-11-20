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
    adapter: Option<(String, u64)>,
    last_used: Option<Instant>,
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

/// The hardware adapter with the most dedicated video memory: (name, bytes).
fn main_adapter() -> Option<(String, u64)> {
    use windows::Win32::Graphics::Dxgi::*;
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut best: Option<(String, u64)> = None;
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
            if best.as_ref().map(|b| mem > b.1).unwrap_or(true) {
                best = Some((name, mem));
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
        Some(Gpu { query: q, engine, memory })
    }
}

/// (instance name, value) of a wildcard counter.
fn counter_items(counter: *mut c_void) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    unsafe {
        let (mut size, mut count) = (0u32, 0u32);
        let st = PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100, &mut size, &mut count, std::ptr::null_mut());
        if st != PDH_MORE_DATA || size == 0 {
            return out;
        }
        // items followed by their names, in one buffer
        let mut buf = vec![0u64; (size as usize + 7) / 8];
        let items = buf.as_mut_ptr() as *mut PdhItem;
        if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100, &mut size, &mut count, items) != 0 {
            return out;
        }
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
        }
    }

    pub fn sample(&mut self) -> Sample {
        // unused for a while: start over (the old deltas would span the whole pause)
        if self.last_used.map(|t| t.elapsed().as_secs() > 20).unwrap_or(false) {
            self.prev_cores.clear();
            self.prev_proc.clear();
            self.prev_at = None;
            self.gpu = None;
            self.gpu_tried = false;
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
            self.gpu = open_gpu();
            if self.adapter.is_none() {
                self.adapter = main_adapter();
            }
        }
        if let Some((name, total)) = &self.adapter {
            s.gpu_name = Some(name.clone());
            s.vram_total = (*total > 0).then_some(*total);
        }
        if let Some(g) = &self.gpu {
            if unsafe { PdhCollectQueryData(g.query) } == 0 {
                // "pid_1234_luid_0x0_0xD1F2_phys_0_eng_3_engtype_3D": sum each engine over all processes
                let mut engines: HashMap<String, f64> = HashMap::new();
                for (name, v) in counter_items(g.engine) {
                    let key = name.find("luid_").map(|i| name[i..].to_string()).unwrap_or(name);
                    *engines.entry(key).or_default() += v;
                }
                if !engines.is_empty() {
                    s.gpu = Some(engines.values().cloned().fold(0.0, f64::max).clamp(0.0, 100.0));
                }
                if !g.memory.is_null() {
                    let used = counter_items(g.memory).into_iter().map(|(_, v)| v).fold(0.0, f64::max);
                    if used > 0.0 {
                        s.vram_used = Some(used as u64);
                    }
                }
            }
        }

        // --- apps: processes grouped by their friendly name ---
        let list = procinfo::list_processes();
        self.procs.refresh(&list);
        let elapsed = self.prev_at.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
        self.prev_at = Some(Instant::now());
        let ncpu = s.eff.len().max(1) as f64;
        let mut next = HashMap::with_capacity(list.len());
        let mut groups: HashMap<String, AppUse> = HashMap::new();
        for (pid, _) in &list {
            if *pid == 0 {
                continue;
            }
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
                    let mut mc = MemCounters { cb: std::mem::size_of::<MemCounters>() as u32, ..Default::default() };
                    if K32GetProcessMemoryInfo(h, &mut mc, mc.cb) != 0 {
                        mem = if mc.private_ws > 0 { mc.private_ws as u64 } else { mc.ws as u64 };
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
            let g = groups.entry(info.display.clone()).or_insert_with(|| AppUse {
                name: info.display.clone(),
                path: info.path.clone(),
                icon: info.icon.clone(),
                ..Default::default()
            });
            g.cpu += cpu;
            g.mem += mem;
            g.procs += 1;
        }
        self.prev_proc = next;
        // the top apps by CPU and by memory (the page sorts by either)
        let mut all: Vec<AppUse> = groups.into_values().collect();
        all.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal));
        let mut keep: Vec<AppUse> = all.iter().take(15).cloned().collect();
        all.sort_by(|a, b| b.mem.cmp(&a.mem));
        for a in all.into_iter().take(15) {
            if !keep.iter().any(|k| k.name == a.name) {
                keep.push(a);
            }
        }
        s.apps = keep;
        s
    }
}

pub fn monitor() -> &'static std::sync::Mutex<Monitor> {
    static M: std::sync::OnceLock<std::sync::Mutex<Monitor>> = std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(Monitor::new()))
}
