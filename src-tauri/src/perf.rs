//! Self-measurement: CPU time per background part and a few counters, one log line every 10 minutes
//! ("perf: …"), plus the WebView2 processes (the page) and memory. Zero-cost on hot loops: each
//! worker thread duplicates its thread handle once on its first `tick` and is queried every 10 min.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};
use windows_sys::Win32::Foundation::{CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, FILETIME, HANDLE};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, GetExitCodeThread, GetProcessTimes, GetThreadTimes, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::log;
use crate::window_ctl::WinState;

const STILL_ACTIVE_CODE: u32 = 259;

struct RegThread {
    handle: isize,
    part: &'static str,
    prev_cpu: u64,
}

static THREADS: Mutex<Vec<RegThread>> = Mutex::new(Vec::new());
/// counter name -> count since the last line
static COUNTS: Mutex<Option<HashMap<&'static str, u64>>> = Mutex::new(None);

thread_local! {
    static REGISTERED: Cell<bool> = const { Cell::new(false) };
}

fn ft(f: &FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

unsafe fn thread_cpu(h: HANDLE) -> Option<u64> {
    let (mut a, mut b, mut k, mut u) = std::mem::zeroed::<(FILETIME, FILETIME, FILETIME, FILETIME)>();
    if GetThreadTimes(h, &mut a, &mut b, &mut k, &mut u) == 0 {
        None
    } else {
        Some(ft(&k) + ft(&u))
    }
}

/// Registers the calling thread for `part` once via thread-local flag; subsequent calls are a no-op.
pub fn tick(part: &'static str) {
    if REGISTERED.with(|r| r.get()) {
        return;
    }
    REGISTERED.with(|r| r.set(true));
    unsafe {
        let mut h: HANDLE = std::ptr::null_mut();
        let proc_h = GetCurrentProcess();
        if DuplicateHandle(proc_h, GetCurrentThread(), proc_h, &mut h, 0, 0, DUPLICATE_SAME_ACCESS) == 0 || h.is_null() {
            return;
        }
        let prev_cpu = thread_cpu(h).unwrap_or(0);
        THREADS.lock().unwrap().push(RegThread { handle: h as isize, part, prev_cpu });
    }
}

pub fn count(name: &'static str, n: u64) {
    *COUNTS.lock().unwrap().get_or_insert_with(HashMap::new).entry(name).or_insert(0) += n;
}

unsafe fn proc_cpu_mem(pid: u32) -> Option<(u64, u64)> {
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if h.is_null() {
        return None;
    }
    let (mut a, mut b, mut k, mut u) = std::mem::zeroed::<(FILETIME, FILETIME, FILETIME, FILETIME)>();
    let cpu = if GetProcessTimes(h, &mut a, &mut b, &mut k, &mut u) != 0 { ft(&k) + ft(&u) } else { 0 };
    let mut m: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
    m.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    let private = if K32GetProcessMemoryInfo(h, &mut m as *mut _ as *mut _, m.cb) != 0 { m.PrivateUsage as u64 } else { 0 };
    CloseHandle(h);
    Some((cpu, private))
}

/// Our process and every process below it (the WebView2 browser, GPU, renderer…): pid -> exe.
fn tree() -> Vec<(u32, String)> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let mut all = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 {
            return Vec::new();
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            all.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..n])));
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    let me = std::process::id();
    let mut out = vec![(me, "micabar".to_string())];
    let mut i = 0;
    while i < out.len() {
        let p = out[i].0;
        for (pid, parent, exe) in &all {
            if *parent == p && *pid != p && !out.iter().any(|(x, _)| x == pid) {
                out.push((*pid, exe.trim_end_matches(".exe").to_string()));
            }
        }
        i += 1;
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WvMemAction {
    None,
    PulseLow,
    Reload,
}

fn evaluate_wv_memory(max_wv_mb: u64, idle: bool, high_streak: &mut u32) -> WvMemAction {
    if idle && max_wv_mb >= 1500 {
        *high_streak = high_streak.saturating_add(1);
        if *high_streak >= 2 {
            *high_streak = 0;
            return WvMemAction::Reload;
        }
    } else {
        *high_streak = 0;
    }
    if idle && max_wv_mb >= 600 {
        WvMemAction::PulseLow
    } else {
        WvMemAction::None
    }
}

/// Starts the line every 10 minutes (percentages are averages over that time) and self-heals
/// runaway WebView2 memory while the island is collapsed and idle.
pub fn start(app: AppHandle, ws: Arc<WinState>, glass: crate::glass::FrameChannel) {
    std::thread::spawn(move || {
        let mut prev_procs: HashMap<u32, u64> = HashMap::new();
        for (pid, _) in tree() {
            if let Some((cpu, _)) = unsafe { proc_cpu_mem(pid) } {
                prev_procs.insert(pid, cpu);
            }
        }
        let mut at = Instant::now();
        let mut high_wv_streak: u32 = 0;
        loop {
            std::thread::sleep(Duration::from_secs(600));
            let wall = (at.elapsed().as_secs_f64() * 1e7).max(1.0); // in 100 ns
            at = Instant::now();
            let pct = |d: u64| d as f64 / wall * 100.0;
            // parts of MicaBar's own process
            let mut parts: HashMap<&'static str, u64> = HashMap::new();
            {
                let mut threads = THREADS.lock().unwrap();
                threads.retain_mut(|t| unsafe {
                    let h = t.handle as HANDLE;
                    if let Some(cpu) = thread_cpu(h) {
                        let d = cpu.saturating_sub(t.prev_cpu);
                        *parts.entry(t.part).or_insert(0) += d;
                        t.prev_cpu = cpu;
                    }
                    let mut code: u32 = 0;
                    let alive = GetExitCodeThread(h, &mut code) != 0 && code == STILL_ACTIVE_CODE;
                    if !alive {
                        CloseHandle(h);
                    }
                    alive
                });
            }
            // processes: MicaBar + the WebView2 tree
            let mut procs: Vec<(String, u64, u64)> = Vec::new();
            let mut alive = HashMap::new();
            for (pid, exe) in tree() {
                if let Some((cpu, mem)) = unsafe { proc_cpu_mem(pid) } {
                    let d = prev_procs.get(&pid).map(|p| cpu.saturating_sub(*p)).unwrap_or(0);
                    alive.insert(pid, cpu);
                    procs.push((exe, d, mem));
                }
            }
            prev_procs = alive;
            let max_wv_mb = procs
                .iter()
                .skip(1)
                .filter(|(e, _, _)| e.to_ascii_lowercase().contains("webview2"))
                .map(|(_, _, m)| *m / 1_048_576)
                .max()
                .unwrap_or(0);
            let own = procs.first().map(|p| p.1).unwrap_or(0);
            let tracked: u64 = parts.values().sum();
            let mut pv: Vec<(&str, u64)> = parts.into_iter().filter(|(_, d)| *d > 0).collect();
            pv.sort_by(|a, b| b.1.cmp(&a.1));
            let other = pct(own.saturating_sub(tracked));
            let mut line = if pv.is_empty() {
                format!("perf: micabar {:.1}% [other {other:.1}%]", pct(own))
            } else {
                let parts_str = pv.iter().map(|(p, d)| format!("{p} {:.1}%", pct(*d))).collect::<Vec<_>>().join(", ");
                format!("perf: micabar {:.1}% [{parts_str}, other {other:.1}%]", pct(own))
            };
            let web: Vec<String> = procs
                .iter()
                .skip(1)
                .filter(|p| p.2 > 0)
                .map(|(e, d, m)| format!("{} {:.1}%/{}M", e.replace("msedgewebview2", "wv"), pct(*d), m / 1_048_576))
                .collect();
            line += &format!(" | mem {}M", procs.first().map(|p| p.2 / 1_048_576).unwrap_or(0));
            if !web.is_empty() {
                line += &format!(" | {}", web.join(", "));
            }
            let mut counts = COUNTS.lock().unwrap().take().unwrap_or_default();
            let diff_sum = counts.remove("glass diff_sum").unwrap_or(0);
            let diff_n = counts.remove("glass diff_n").unwrap_or(0);
            let mut cv_strs: Vec<String> = {
                let mut cv: Vec<_> = counts.into_iter().collect();
                cv.sort();
                cv.into_iter().map(|(k, v)| format!("{k} {v}")).collect()
            };
            if diff_n > 0 {
                let avg_diff = diff_sum as f64 / diff_n as f64 / 100.0;
                cv_strs.push(format!("glass diff {avg_diff:.1}"));
                cv_strs.sort();
            }
            if !cv_strs.is_empty() {
                line += &format!(" | {}", cv_strs.join(", "));
            }
            log(line);

            let idle = !ws.expanded.load(Ordering::SeqCst)
                && !ws.edit.load(Ordering::SeqCst)
                && !ws.grab.load(Ordering::SeqCst)
                && !ws.tour.load(Ordering::SeqCst);
            match evaluate_wv_memory(max_wv_mb, idle, &mut high_wv_streak) {
                WvMemAction::PulseLow => {
                    log(format!("perf: high WebView2 memory ({max_wv_mb}M); pulsing LOW memory target"));
                    crate::window_ctl::pulse_webview_memory_low(&app);
                }
                WvMemAction::Reload => {
                    log(format!(
                        "perf: WebView2 renderer stuck at {max_wv_mb}M; reloading island webview to reclaim memory"
                    ));
                    crate::glass::set_channel(&glass, None);
                    if let Some(w) = app.get_webview_window("main") {
                        ws.mem_is_low.store(false, Ordering::SeqCst);
                        crate::window_ctl::schedule_mem_low(&ws, 5000, false);
                        let _ = w.eval("location.reload()");
                    }
                }
                WvMemAction::None => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wv_memory_self_heal_thresholds_and_streak() {
        let mut streak = 0;
        assert_eq!(evaluate_wv_memory(599, true, &mut streak), WvMemAction::None);
        assert_eq!(streak, 0);

        assert_eq!(evaluate_wv_memory(600, true, &mut streak), WvMemAction::PulseLow);
        assert_eq!(streak, 0);

        // First >=1500M window pulses LOW and bumps streak to 1.
        assert_eq!(evaluate_wv_memory(1500, true, &mut streak), WvMemAction::PulseLow);
        assert_eq!(streak, 1);

        // Non-idle resets streak and performs no action.
        assert_eq!(evaluate_wv_memory(1600, false, &mut streak), WvMemAction::None);
        assert_eq!(streak, 0);

        // Two consecutive idle >=1500M windows trigger Reload and reset streak.
        assert_eq!(evaluate_wv_memory(1550, true, &mut streak), WvMemAction::PulseLow);
        assert_eq!(streak, 1);
        assert_eq!(evaluate_wv_memory(1650, true, &mut streak), WvMemAction::Reload);
        assert_eq!(streak, 0);
    }
}
