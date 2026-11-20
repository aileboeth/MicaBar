//! Per-app traffic accounting by local day, split into "through a proxy node" and "direct".
//! One JSON file per day in %LOCALAPPDATA%\MicaBar\traffic\YYYY-MM-DD.json.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppTraffic {
    pub name: String,
    pub path: Option<String>,
    /// bytes: proxy up / down, direct up / down
    pub pu: f64,
    pub pd: f64,
    pub du: f64,
    pub dd: f64,
}

#[derive(Default)]
pub struct Book {
    date: String,
    apps: HashMap<String, AppTraffic>,
    dirty: bool,
}

fn dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    let d = PathBuf::from(base).join("MicaBar").join("traffic");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Offset of local time from UTC in milliseconds (current, includes DST).
fn local_offset_ms() -> i64 {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    let (mut l, mut u): (SYSTEMTIME, SYSTEMTIME) = unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
    unsafe {
        GetLocalTime(&mut l);
        GetSystemTime(&mut u);
    }
    let mins = |s: &SYSTEMTIME| days_from_civil(s.wYear as i64, s.wMonth as i64, s.wDay as i64) * 1440 + s.wHour as i64 * 60 + s.wMinute as i64;
    (mins(&l) - mins(&u)) * 60_000
}

// Howard Hinnant's civil <-> days algorithms (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Local date `days_ago` days before today, "YYYY-MM-DD".
pub fn local_date(days_ago: i64) -> String {
    let local_ms = crate::now_ms() as i64 + local_offset_ms();
    let (y, m, d) = civil_from_days(local_ms.div_euclid(86_400_000) - days_ago);
    format!("{y:04}-{m:02}-{d:02}")
}

fn load(date: &str) -> HashMap<String, AppTraffic> {
    std::fs::read_to_string(dir().join(format!("{date}.json")))
        .ok()
        .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default()
}

/// Records are kept for today plus this many days before it; older day files are removed.
pub const KEEP_DAYS: i64 = 30;

/// Removes day files older than KEEP_DAYS (names are YYYY-MM-DD, so they compare as text).
fn prune() {
    let oldest = local_date(KEEP_DAYS);
    let Ok(rd) = std::fs::read_dir(dir()) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(stem) = name.strip_suffix(".json") else { continue };
        let b = stem.as_bytes();
        let is_date = b.len() == 10 && b[4] == b'-' && b[7] == b'-' && stem.chars().filter(|c| c.is_ascii_digit()).count() == 8;
        if is_date && stem < oldest.as_str() {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

impl Book {
    pub fn new() -> Book {
        prune();
        let date = local_date(0);
        let apps = load(&date);
        Book { date, apps, dirty: false }
    }

    /// Adds one tick of bytes for an app.
    pub fn add(&mut self, key: &str, name: &str, path: Option<&String>, proxy: (f64, f64), direct: (f64, f64)) {
        let today = local_date(0);
        if today != self.date {
            self.flush();
            self.date = today;
            self.apps.clear();
            prune();
        }
        if proxy.0 + proxy.1 + direct.0 + direct.1 <= 0.0 {
            return;
        }
        let e = self.apps.entry(key.to_string()).or_default();
        e.name = name.to_string();
        if e.path.is_none() {
            e.path = path.cloned();
        }
        e.pu += proxy.0;
        e.pd += proxy.1;
        e.du += direct.0;
        e.dd += direct.1;
        self.dirty = true;
    }

    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        if let Ok(t) = serde_json::to_string(&self.apps) {
            let _ = std::fs::write(dir().join(format!("{}.json", self.date)), t);
        }
        self.dirty = false;
    }

    pub fn today(&self) -> (String, HashMap<String, AppTraffic>) {
        (self.date.clone(), self.apps.clone())
    }
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct DayTotal {
    pub date: String,
    pub pu: f64,
    pub pd: f64,
    pub du: f64,
    pub dd: f64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppTotal {
    pub key: String,
    #[serde(flatten)]
    pub t: AppTraffic,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub days: Vec<DayTotal>,
    pub apps: Vec<AppTotal>,
}

/// The last `n` days (oldest first) with per-day totals, and per-app totals over the range
/// (or over the one day `focus`, "YYYY-MM-DD", when given).
pub fn report(book: &Book, n: i64, focus: Option<&str>) -> Report {
    let (today, today_apps) = book.today();
    let mut days = Vec::new();
    let mut apps: HashMap<String, AppTraffic> = HashMap::new();
    for ago in (0..n).rev() {
        let date = local_date(ago);
        let data = if date == today { today_apps.clone() } else { load(&date) };
        let mut d = DayTotal { date: date.clone(), ..Default::default() };
        let count = focus.map_or(true, |f| f == date);
        for (k, a) in data {
            d.pu += a.pu;
            d.pd += a.pd;
            d.du += a.du;
            d.dd += a.dd;
            if !count {
                continue;
            }
            let e = apps.entry(k).or_default();
            e.name = a.name;
            if e.path.is_none() {
                e.path = a.path;
            }
            e.pu += a.pu;
            e.pd += a.pd;
            e.du += a.du;
            e.dd += a.dd;
        }
        days.push(d);
    }
    let mut apps: Vec<AppTotal> = apps.into_iter().map(|(key, t)| AppTotal { key, t }).collect();
    apps.sort_by(|a, b| (b.t.pu + b.t.pd + b.t.du + b.t.dd).partial_cmp(&(a.t.pu + a.t.pd + a.t.du + a.t.dd)).unwrap_or(std::cmp::Ordering::Equal));
    apps.truncate(60);
    Report { days, apps }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn civil_roundtrip() {
        for z in [-1000i64, 0, 19000, 20500] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }
}
