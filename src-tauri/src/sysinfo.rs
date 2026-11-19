//! CPU and memory usage for the pill (read only when those items are switched on).

use serde::Serialize;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::GetSystemTimes;

#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SysInfo {
    /// 0..100, None on the first sample
    pub cpu: Option<f64>,
    pub mem_used: u64,
    pub mem_total: u64,
}

fn ft(f: &FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

#[derive(Default)]
pub struct Sampler {
    prev: Option<(u64, u64)>, // (idle, total)
}

impl Sampler {
    pub fn sample(&mut self, cpu: bool, mem: bool) -> SysInfo {
        let mut out = SysInfo::default();
        if cpu {
            let (mut idle, mut kernel, mut user) = (FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }, FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }, FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 });
            if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } != 0 {
                // kernel time includes idle time
                let (i, t) = (ft(&idle), ft(&kernel) + ft(&user));
                if let Some((pi, pt)) = self.prev {
                    let dt = t.saturating_sub(pt);
                    if dt > 0 {
                        out.cpu = Some((1.0 - i.saturating_sub(pi) as f64 / dt as f64).clamp(0.0, 1.0) * 100.0);
                    }
                }
                self.prev = Some((i, t));
            }
        } else {
            self.prev = None;
        }
        if mem {
            let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
            m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
            if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 {
                out.mem_total = m.ullTotalPhys;
                out.mem_used = m.ullTotalPhys.saturating_sub(m.ullAvailPhys);
            }
        }
        out
    }
}
