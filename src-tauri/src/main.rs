#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agents;
mod ai;
mod audio;
mod clipboard;
mod conn;
mod control;
mod dns;
mod exitip;
mod firstnet;
mod geo;
mod glass;
mod guard;
mod i18n;
mod icon;
mod iporg;
mod media;
mod mihomo;
mod netdiag;
mod notify;
mod perf;
mod procinfo;
mod proxy;
mod settings;
mod speed;
mod startup;
mod sysinfo;
mod sysmon;
mod system;
mod traffic;
mod window_ctl;

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager};
use window_ctl::{Rect, WinState};

use conn::Proto;
use exitip::ExitResult;
use geo::Geo;
use mihomo::{Controller, MConn};
use proxy::ProxyState;

// ---------- logging ----------

fn log_path() -> &'static std::path::Path {
    static PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
        let dir = std::path::PathBuf::from(base).join("MicaBar");
        let _ = std::fs::create_dir_all(&dir);
        dir.join("micabar.log")
    })
}

pub fn log(msg: impl AsRef<str>) {
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock();
    let p = log_path();
    if std::fs::metadata(p).map(|m| m.len() > 2_000_000).unwrap_or(false) {
        let _ = std::fs::rename(p, p.with_extension("log.old"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let t = now_ms();
        let _ = writeln!(f, "[{}] {}", fmt_time(t), msg.as_ref());
    }
}

/// Logs a recurring status line only when it changed, or every 30 minutes (the exit checks ran
/// once a minute and filled most of the log with the same two lines).
fn log_changed(key: &str, msg: String) {
    static LAST: Mutex<Option<HashMap<String, (String, u64)>>> = Mutex::new(None);
    let now = now_ms();
    let mut g = LAST.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    if let Some((last, at)) = m.get(key) {
        if *last == msg && now.saturating_sub(*at) < 30 * 60_000 {
            return;
        }
    }
    m.insert(key.to_string(), (msg.clone(), now));
    drop(g);
    log(msg);
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn local_tz_offset_secs() -> i64 {
    use std::sync::atomic::{AtomicI64, AtomicU64};
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    static OFF: AtomicI64 = AtomicI64::new(8 * 3600);
    static AT: AtomicU64 = AtomicU64::new(0);
    let now = now_ms();
    let prev = AT.load(Ordering::Relaxed);
    if prev != 0 && now.saturating_sub(prev) < 60_000 {
        return OFF.load(Ordering::Relaxed);
    }
    fn to_secs(st: &SYSTEMTIME) -> i64 {
        let y = st.wYear as i64;
        let m = st.wMonth as i64;
        let d = st.wDay as i64;
        let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * m + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146097 + doe - 719468;
        days * 86400 + (st.wHour as i64) * 3600 + (st.wMinute as i64) * 60 + (st.wSecond as i64)
    }
    let (mut utc, mut loc) = unsafe { std::mem::zeroed::<(SYSTEMTIME, SYSTEMTIME)>() };
    unsafe {
        GetSystemTime(&mut utc);
        GetLocalTime(&mut loc);
    }
    let raw = to_secs(&loc) - to_secs(&utc);
    // Round to nearest minute so a second rollover between the two calls cannot skew by 1s.
    let rounded = (raw + 30).div_euclid(60) * 60;
    OFF.store(rounded, Ordering::Relaxed);
    AT.store(now, Ordering::Relaxed);
    rounded
}

fn fmt_time(ms: u64) -> String {
    let s = ((ms / 1000) as i64 + local_tz_offset_secs()).rem_euclid(86400) as u64;
    format!("{:02}:{:02}:{:02}", (s / 3600) % 24, (s / 60) % 60, s % 60)
}

// ---------- snapshot model ----------

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct ConnRow {
    proto: &'static str,
    local: String,
    local_port: u16,
    remote: Option<String>,
    remote_port: Option<u16>,
    state: &'static str,
    host: Option<String>,
    cc: Option<String>,
    org: Option<String>,
    node: Option<String>,
    group: Option<String>,
    rule: Option<String>,
    /// The app talks to the local proxy port; remote shows the real destination.
    via_proxy_port: bool,
    /// mihomo is running but this connection never went through it.
    bypass: bool,
    /// Leak guard hit (library entry) when this "should be proxied" connection went direct.
    leak: Option<String>,
    up: f64,
    down: f64,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct AppRow {
    key: String,
    name: String,
    exe: String,
    path: Option<String>,
    /// exe to take the icon from (helpers borrow their app's icon)
    icon: Option<String>,
    pids: Vec<u32>,
    up: f64,
    down: f64,
    is_proxy: bool,
    /// at least one connection leaves through a proxy node
    via_proxy: bool,
    conns: Vec<ConnRow>,
    listen: Vec<String>,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct ExitState {
    proxy: Option<ExitResult>,
    direct: Option<ExitResult>,
    /// Exit per proxy group (measured through the group's own inbound).
    groups: Vec<GroupExit>,
    checking: bool,
    last_check: u64,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct GroupExit {
    /// the subscription's fallback (MATCH) group
    is_default: bool,
    /// r.group = the proxy group, r.node = the node it currently uses
    #[serde(flatten)]
    r: ExitResult,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    ts: u64,
    #[serde(default)]
    slim: bool,
    admin: bool,
    speed_ok: bool,
    speed_error: Option<String>,
    geo_source: Option<String>,
    conn_count: usize,
    listen_count: usize,
    hidden_loopback: usize,
    up: f64,
    down: f64,
    apps: Vec<AppRow>,
    proxy: ProxyState,
    exit: ExitState,
    control: control::ControlState,
    guard: GuardView,
    sys: Option<sysinfo::SysInfo>,
    latency: Latency,
    hotkey_ok: bool,
    maximized: bool,
    animations: bool,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct Latency {
    node: Option<String>,
    ms: Option<u32>,
    error: Option<String>,
    at: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LatencySample {
    at: u64,
    node: String,
    ms: Option<u32>,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct Alert {
    key: String,
    /// rule = went through mihomo but a rule sent it DIRECT; bypass = never reached mihomo
    kind: &'static str,
    app: String,
    exe: String,
    path: Option<String>,
    host: Option<String>,
    ip: Option<String>,
    port: Option<u16>,
    hit: String,
    rule: Option<String>,
    first: u64,
    last: u64,
    active: bool,
    acked: bool,
    blocked: bool,
}

#[derive(Default)]
struct AlertBook {
    alerts: HashMap<String, Alert>,
    notified: HashMap<String, u64>,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct GuardView {
    settings: guard::Settings,
    sources: Vec<(String, usize)>,
    total: usize,
    alerts: Vec<Alert>,
    unacked: usize,
    tun: bool,
}

// ---------- shared state ----------

struct MihomoLink {
    ctl: Option<Controller>,
    secret: Option<String>,
    last_try: Option<Instant>,
}

struct Shared {
    geo: Geo,
    proxy: Mutex<ProxyState>,
    exit: Mutex<ExitState>,
    last: Mutex<Option<Arc<Snapshot>>>,
    mihomo: Mutex<MihomoLink>,
    ws: Arc<WinState>,
    glass: glass::FrameChannel,
    control: Mutex<control::ControlState>,
    rules: Mutex<Vec<control::Rule>>,
    core_home: Mutex<Option<std::path::PathBuf>>,
    /// Select the MicaBar group's node once the next exit check knows the current proxy node.
    node_pending: AtomicBool,
    guard: Mutex<guard::Settings>,
    matcher: std::sync::RwLock<guard::Matcher>,
    alerts: Mutex<AlertBook>,
    latency: Mutex<Latency>,
    latency_history: Mutex<std::collections::VecDeque<LatencySample>>,
    traffic: Mutex<traffic::Book>,
    group_score: Mutex<HashMap<String, f64>>,
    ai: Mutex<Vec<ai::Quota>>,
    ai_trigger: AtomicBool,
    media: media::Shared,
    exit_trigger: AtomicBool,
    admin: bool,
}

fn is_admin() -> bool {
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}

fn is_loopback(ip: &IpAddr) -> bool {
    ip.is_loopback() || ip.is_unspecified()
}

fn fmt_ip(ip: &IpAddr) -> String {
    ip.to_string()
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn rule_text(c: &MConn) -> Option<String> {
    let r = non_empty(&c.rule)?;
    Some(match non_empty(&c.rule_payload) {
        Some(p) => format!("{r}({p})"),
        None => r,
    })
}

// ---------- system / network event coordination ----------

static GLOBAL_SHARED: OnceLock<Arc<Shared>> = OnceLock::new();
static COLLECTOR_WAKE: OnceLock<(Mutex<bool>, Condvar)> = OnceLock::new();
static NET_DIRTY: AtomicBool = AtomicBool::new(false);
static SYSTEM_RESUMED: AtomicBool = AtomicBool::new(false);
static LAST_RESUME_MS: AtomicU64 = AtomicU64::new(0);
/// Epoch ms when a proxy client was first seen without a local port or TUN (core still starting).
static PORTLESS_PROXY_SINCE_MS: AtomicU64 = AtomicU64::new(0);

fn collector_wake() -> &'static (Mutex<bool>, Condvar) {
    COLLECTOR_WAKE.get_or_init(|| (Mutex::new(false), Condvar::new()))
}

pub(crate) fn wake_collector() {
    let (lock, cvar) = collector_wake();
    if let Ok(mut dirty) = lock.lock() {
        *dirty = true;
        cvar.notify_one();
    }
}

/// Called by the IP Helper interface/address watcher on a Win32 threadpool thread.
pub(crate) fn on_network_change() {
    NET_DIRTY.store(true, Ordering::SeqCst);
    dns::invalidate_cache();
    netdiag::notify_network_change();
    wake_collector();
}

/// Called on `WM_POWERBROADCAST` resume or wall-clock jump after sleep / hibernate / VM pause.
pub(crate) fn on_system_resume(why: &str) {
    let now = now_ms();
    let prev = LAST_RESUME_MS.swap(now, Ordering::SeqCst);
    if prev != 0 && now.saturating_sub(prev) < 3_000 {
        return;
    }
    log(format!("system resumed ({why}); refreshing network, mihomo and backdrop"));
    NET_DIRTY.store(true, Ordering::SeqCst);
    SYSTEM_RESUMED.store(true, Ordering::SeqCst);
    dns::invalidate_cache();
    let in_game = GLOBAL_SHARED
        .get()
        .map(|sh| sh.ws.game.load(Ordering::SeqCst))
        .unwrap_or(false);
    if !in_game {
        glass::notify_display_or_resume();
    }
    netdiag::notify_resume();
    if let Some(sh) = GLOBAL_SHARED.get() {
        sh.ws.reposition_req.store(true, Ordering::SeqCst);
        window_ctl::schedule_mem_pulse(&sh.ws, 1500);
        if let Ok(mut link) = sh.mihomo.lock() {
            link.last_try = None;
        }
        sh.exit_trigger.store(true, Ordering::SeqCst);
    }
    wake_collector();
}

/// Called on `WM_DISPLAYCHANGE` / `WM_DPICHANGED`.
pub(crate) fn on_display_change() {
    if let Some(sh) = GLOBAL_SHARED.get() {
        if !sh.ws.game.load(Ordering::SeqCst) {
            glass::notify_display_or_resume();
        }
        window_ctl::schedule_reposition(&sh.ws, 200);
        window_ctl::schedule_mem_pulse(&sh.ws, 1500);
    } else {
        glass::notify_display_or_resume();
    }
}

fn cmp_ignore_case(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ia = a.chars().flat_map(char::to_lowercase);
    let mut ib = b.chars().flat_map(char::to_lowercase);
    loop {
        match (ia.next(), ib.next()) {
            (Some(ca), Some(cb)) => match ca.cmp(&cb) {
                std::cmp::Ordering::Equal => continue,
                ord => return ord,
            },
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (None, None) => return std::cmp::Ordering::Equal,
        }
    }
}

// ---------- collector ----------

fn is_clock_jump(wall_ms: u64, mono_ms: u64) -> bool {
    wall_ms > mono_ms.saturating_add(15_000) || mono_ms > 45_000
}

fn collector_period(profile: u8, game_idle: bool) -> Duration {
    if game_idle {
        Duration::from_millis(4000)
    } else if profile == 0 {
        Duration::from_millis(2000)
    } else {
        Duration::from_millis(1000)
    }
}

struct Collector {
    procs: procinfo::ProcCache,
    counters: Option<speed::Counters>,
    speed_warming: bool,
    speed_error: Option<String>,
    prev_counters: HashMap<u32, (u64, u64)>,
    prev_counter_at: Instant,
    prev_mihomo: HashMap<String, (u64, u64)>,
    prev_mihomo_at: Instant,
    dns: HashMap<IpAddr, String>,
    dns_at: Option<Instant>,
    cfg: mihomo::CoreConfig,
    cfg_at: Option<Instant>,
    tun: Option<String>,
    tun_at: Option<Instant>,
    net_sig: Option<String>,
    proxy_sig: Option<String>,
    resume_fast_until: Option<Instant>,
    prev_tick_mono: Instant,
    prev_tick_wall_ms: u64,
    control_at: Option<Instant>,
    sampler: sysinfo::Sampler,
    traffic_at: Option<Instant>,
    traffic_flush: Option<Instant>,
    animations: bool,
    animations_at: Option<Instant>,
    firstnet_at: Option<Instant>,
}

fn network_signature_from(ads: &[ipconfig::Adapter]) -> String {
    // physical uplinks only: TUN and virtual adapters churn or stay up while offline
    let mut parts: Vec<String> = ads
        .iter()
        .filter(|a| {
            a.oper_status() == ipconfig::OperStatus::IfOperStatusUp
                && !a.gateways().is_empty()
                && !proxy::looks_like_tun(a)
                && !proxy::is_virtual_nic(a)
        })
        .map(|a| {
            let mut v4: Vec<String> = a.ip_addresses().iter().filter(|i| i.is_ipv4()).map(|i| i.to_string()).collect();
            v4.sort();
            let gw: Vec<String> = a.gateways().iter().filter(|g| g.is_ipv4()).map(|g| g.to_string()).collect();
            format!("{}={}>{}", a.friendly_name(), v4.join(","), gw.join(","))
        })
        .collect();
    parts.sort();
    parts.join("|")
}

impl Collector {
    fn new_warming(admin: bool) -> Collector {
        Collector {
            procs: Default::default(),
            counters: None,
            speed_warming: admin,
            speed_error: (!admin).then(|| crate::i18n::t("需要管理员权限").into()),
            prev_counters: HashMap::new(),
            prev_counter_at: Instant::now(),
            prev_mihomo: HashMap::new(),
            prev_mihomo_at: Instant::now(),
            dns: HashMap::new(),
            dns_at: None,
            cfg: Default::default(),
            cfg_at: None,
            tun: None,
            tun_at: None,
            net_sig: None,
            proxy_sig: None,
            resume_fast_until: None,
            prev_tick_mono: Instant::now(),
            prev_tick_wall_ms: now_ms(),
            control_at: None,
            sampler: Default::default(),
            traffic_at: None,
            traffic_flush: Some(Instant::now()),
            animations: true,
            animations_at: None,
            firstnet_at: None,
        }
    }

    fn attach_counters(&mut self, counters: Result<speed::Counters, String>) {
        self.speed_warming = false;
        match counters {
            Ok(c) => {
                self.counters = Some(c);
                self.speed_error = None;
                self.prev_counter_at = Instant::now();
            }
            Err(e) => {
                self.counters = None;
                self.speed_error = Some(e);
            }
        }
    }

    fn due(at: &Option<Instant>, every: Duration) -> bool {
        at.map(|t| t.elapsed() >= every).unwrap_or(true)
    }

    fn tick(&mut self, sh: &Shared) -> Snapshot {
        // Detect wall-clock / Instant jumps (S3/S4 hibernate or VM suspend/resume where WM_POWERBROADCAST may not fire)
        let now_wall = now_ms();
        let mono_ms = self.prev_tick_mono.elapsed().as_millis() as u64;
        let wall_ms = now_wall.saturating_sub(self.prev_tick_wall_ms);
        self.prev_tick_mono = Instant::now();
        self.prev_tick_wall_ms = now_wall;
        if is_clock_jump(wall_ms, mono_ms) {
            on_system_resume("clock jump");
        }
        let resumed = SYSTEM_RESUMED.swap(false, Ordering::SeqCst);
        let net_dirty = NET_DIRTY.swap(false, Ordering::SeqCst);
        if resumed || net_dirty {
            self.tun_at = None;
            self.cfg_at = None;
            self.dns_at = None;
            sh.mihomo.lock().unwrap().last_try = None;
            if resumed {
                self.resume_fast_until = Some(Instant::now() + Duration::from_secs(15));
                sh.exit_trigger.store(true, Ordering::SeqCst);
            }
        }

        let list = procinfo::list_processes();
        let sockets = conn::snapshot();
        let (client, core_pids, core_name) = proxy::client_name(&list);
        let mut needed_pids: HashSet<u32> = sockets.iter().filter(|s| s.pid != 0).map(|s| s.pid).collect();
        needed_pids.extend(core_pids.iter().copied());
        self.procs.refresh_active(&list, &needed_pids);

        // --- proxy detection ---
        if Self::due(&self.cfg_at, Duration::from_secs(10)) {
            sh.geo.reload_if_needed();
            let cfg = mihomo::read_config();
            if cfg.file != self.cfg.file {
                log(format!("core config: {:?}", cfg.file));
            }
            self.cfg = cfg;
            self.cfg_at = Some(Instant::now());
            // the proxy client's exe, so the diagnosis can start it again after it quit
            // (the image file must carry the client's own name: an updater can show up under it)
            let found = list.iter().filter(|(_, exe)| proxy::is_client_exe(exe)).find_map(|(pid, exe)| {
                procinfo::image_path(*pid).filter(|p| p.rsplit(['\\', '/']).next().map(|f| f.eq_ignore_ascii_case(exe)).unwrap_or(false))
            });
            if let Some(path) = found {
                netdiag::remember_client(&path);
            }
        }
        let tun_every = if self.resume_fast_until.map(|t| Instant::now() < t).unwrap_or(false) {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(5)
        };
        if Self::due(&self.tun_at, tun_every) {
            let ads = proxy::adapters();
            self.tun = ads
                .iter()
                .find(|a| a.oper_status() == ipconfig::OperStatus::IfOperStatusUp && proxy::looks_like_tun(a))
                .map(|a| format!("{} ({})", a.friendly_name(), a.description()));
            self.tun_at = Some(Instant::now());
            let sig = network_signature_from(&ads);
            if let Some(prev) = &self.net_sig {
                if *prev != sig {
                    log("network changed; re-checking exit IP");
                    sh.exit_trigger.store(true, Ordering::SeqCst);
                    netdiag::notify_network_change();
                }
            }
            self.net_sig = Some(sig);
        }
        let sys_proxy = proxy::system_proxy();
        let listening: HashSet<u16> = sockets
            .iter()
            .filter(|s| s.proto == Proto::Tcp && s.state == "LISTEN")
            .map(|s| s.local_port)
            .collect();
        let core_running = !core_pids.is_empty();
        let mut local_port = None;
        if core_running || client.is_some() {
            local_port = [self.cfg.mixed_port, self.cfg.http_port]
                .into_iter()
                .flatten()
                .find(|p| listening.contains(p));
        }
        if local_port.is_none() {
            local_port = sys_proxy.as_deref().and_then(proxy::port_from_proxy_server).filter(|p| listening.contains(p));
        }
        if local_port.is_none() && (core_running || client.is_some()) {
            // clients that leave the system proxy alone (v2rayN "清除系统代理", TUN-only setups) and
            // whose config we don't read: their usual port, if the client or its core listens on it
            let owners: HashSet<u32> = list
                .iter()
                .filter(|(pid, exe)| core_pids.contains(pid) || proxy::is_client_exe(exe))
                .map(|(pid, _)| *pid)
                .collect();
            local_port = proxy::KNOWN_PORTS.iter().copied().find(|p| {
                sockets.iter().any(|s| s.proto == Proto::Tcp && s.state == "LISTEN" && s.local_port == *p && owners.contains(&s.pid))
            });
        }
        // only mihomo-type cores have a controller worth looking for (xray / sing-box cores don't)
        let mihomo_core = list.iter().any(|(pid, exe)| core_pids.contains(pid) && proxy::is_mihomo_core(exe));

        // --- mihomo controller (I/O strictly outside mutex; no nested sh.mihomo <-> sh.proxy lock) ---
        let mut mconns: Vec<MConn> = Vec::new();
        let mut mihomo_err = None;
        let mut discovered_ver: Option<String> = None;
        let (mut active_ctl, mut active_secret) = {
            let mut link = sh.mihomo.lock().unwrap();
            if !mihomo_core {
                link.ctl = None;
                (None, None)
            } else {
                (link.ctl.clone(), link.secret.clone())
            }
        };
        if mihomo_core && active_ctl.is_none() {
            let try_now = {
                let mut link = sh.mihomo.lock().unwrap();
                if Self::due(&link.last_try, Duration::from_secs(5)) {
                    link.last_try = Some(Instant::now());
                    true
                } else {
                    false
                }
            };
            if try_now {
                match mihomo::discover(&self.cfg) {
                    Some((c, ver)) => {
                        log(format!("mihomo controller: {} (version {ver})", c.describe()));
                        discovered_ver = non_empty(&ver);
                        active_secret = self.cfg.secret.clone();
                        active_ctl = Some(c.clone());
                        let mut link = sh.mihomo.lock().unwrap();
                        link.ctl = Some(c);
                        link.secret = active_secret.clone();
                    }
                    None => mihomo_err = Some(crate::i18n::t("找不到 mihomo 控制接口").to_string()),
                }
            }
        }
        if let Some(c) = active_ctl.clone() {
            match mihomo::connections(&c, active_secret.as_deref()) {
                Ok(v) => mconns = v,
                Err(e) => {
                    log(format!("mihomo /connections failed: {e}"));
                    mihomo_err = Some(e);
                    active_ctl = None;
                    sh.mihomo.lock().unwrap().ctl = None;
                }
            }
        }

        let client_label = match (&client, &core_name) {
            (Some(c), Some(k)) => Some(format!("{c} ({k})")),
            (Some(c), None) => Some(c.clone()),
            (None, Some(k)) => Some(k.clone()),
            (None, None) => None,
        };
        let ctl_desc = active_ctl.as_ref().map(|c| c.describe());
        let state = {
            let mut p = sh.proxy.lock().unwrap();
            if let Some(v) = discovered_ver {
                p.core_version = Some(v);
            }
            p.client = client_label;
            p.core_pids = core_pids.clone();
            p.system_proxy = sys_proxy.clone();
            p.tun = self.tun.clone().or_else(|| {
                if self.cfg.tun_enabled && core_running && !mconns.is_empty() && mconns.iter().any(|c| c.metadata.kind.eq_ignore_ascii_case("tun")) {
                    Some("TUN (mihomo)".to_string())
                } else {
                    None
                }
            });
            p.local_port = local_port;
            p.detected = p.client.is_some() || core_running || p.system_proxy.is_some() || p.tun.is_some();
            p.mihomo_ok = !mconns.is_empty() || (core_running && mihomo_err.is_none() && active_ctl.is_some());
            p.mihomo_error = if mihomo_core { mihomo_err } else { None };
            p.controller = ctl_desc;
            if !core_running {
                p.core_version = None;
            }
            p.clone()
        };
        if state.detected && state.local_port.is_none() && state.tun.is_none() {
            let _ = PORTLESS_PROXY_SINCE_MS.compare_exchange(0, now_ms(), Ordering::SeqCst, Ordering::Relaxed);
            // Re-read config on the next tick while the proxy client's core is still booting
            self.cfg_at = None;
        } else {
            PORTLESS_PROXY_SINCE_MS.store(0, Ordering::SeqCst);
        }
        let sig = format!(
            "{}|{:?}|{:?}|{:?}|{:?}|{}",
            state.detected, state.client, state.system_proxy, state.tun, state.local_port, state.mihomo_ok
        );
        if self.proxy_sig.as_ref() != Some(&sig) {
            if self.proxy_sig.is_some() {
                log(format!("proxy state changed: {sig}"));
                sh.exit_trigger.store(true, Ordering::SeqCst);
                netdiag::notify_network_change();
            }
            self.proxy_sig = Some(sig);
        }

        self.update_control(sh, &list, &core_pids);
        if Self::due(&self.animations_at, Duration::from_secs(5)) {
            self.animations = system::animations_enabled();
            self.animations_at = Some(Instant::now());
        }

        // --- DNS cache (reverse names) ---
        if Self::due(&self.dns_at, Duration::from_secs(10)) {
            let fresh = dns::read_cache();
            if self.dns.len() > 20000 {
                let active_ips: HashSet<IpAddr> = sockets.iter().filter_map(|s| s.remote.map(|(ip, _)| ip)).collect();
                self.dns.retain(|ip, _| active_ips.contains(ip));
            }
            for (ip, host) in fresh {
                self.dns.entry(ip).or_insert(host);
            }
            self.dns_at = Some(Instant::now());
        }
        // live resolutions from ETW (works when the cache is empty, e.g. TUN DNS with tiny TTLs)
        let live: Vec<(IpAddr, String)> = std::mem::take(&mut *speed::dns_log().lock().unwrap());
        self.dns.extend(live);

        let proxy_ports: HashSet<u16> = [self.cfg.mixed_port, self.cfg.http_port, self.cfg.socks_port, local_port].into_iter().flatten().collect();
        let proxy_ports_vec: Vec<u16> = proxy_ports.iter().copied().collect();
        speed::set_proxy_ports(&proxy_ports_vec);

        // --- speeds & exact byte deltas ---
        let alive_pids: HashSet<u32> = list.iter().map(|(pid, _)| *pid).collect();
        let mut pid_rate: HashMap<u32, (f64, f64)> = HashMap::new();
        let mut pid_bytes: HashMap<u32, (u64, u64)> = HashMap::new();
        if let Some(c) = &self.counters {
            speed::prune_pids(&alive_pids, c);
            let cur = c.lock().unwrap().clone();
            let first_round = self.prev_counters.is_empty();
            let dt = self.prev_counter_at.elapsed().as_secs_f64().max(0.2);
            for (pid, (s, r)) in &cur {
                let (ps, pr) = match self.prev_counters.get(pid) {
                    Some(prev) => *prev,
                    None if first_round => (*s, *r),
                    None => (0, 0),
                };
                let (du, dd) = (s.saturating_sub(ps), r.saturating_sub(pr));
                pid_bytes.insert(*pid, (du, dd));
                pid_rate.insert(*pid, (du as f64 / dt, dd as f64 / dt));
            }
            self.prev_counters = cur;
            self.prev_counter_at = Instant::now();
        }
        let mdt = self.prev_mihomo_at.elapsed().as_secs_f64().max(0.2);
        let first_mround = self.prev_mihomo.is_empty();
        let mut conn_rate: HashMap<String, (f64, f64)> = HashMap::new();
        let mut conn_bytes: HashMap<String, (u64, u64)> = HashMap::new();
        let mut next_prev = HashMap::with_capacity(mconns.len());
        for c in &mconns {
            let (du, dd) = match self.prev_mihomo.get(&c.id) {
                Some((pu, pd)) => (c.upload.saturating_sub(*pu), c.download.saturating_sub(*pd)),
                None if !first_mround && c.upload < 10_000_000 && c.download < 10_000_000 => (c.upload, c.download),
                None => (0, 0),
            };
            conn_bytes.insert(c.id.clone(), (du, dd));
            if self.prev_mihomo.contains_key(&c.id) {
                conn_rate.insert(c.id.clone(), (du as f64 / mdt, dd as f64 / mdt));
            }
            next_prev.insert(c.id.clone(), (c.upload, c.download));
        }
        self.prev_mihomo = next_prev;
        self.prev_mihomo_at = Instant::now();

        // --- merge ---
        let mut by_port: HashMap<(&'static str, u16), Vec<usize>> = HashMap::new();
        for (i, c) in mconns.iter().enumerate() {
            if c.metadata.kind.eq_ignore_ascii_case("inner") {
                continue;
            }
            let net = if c.metadata.network.eq_ignore_ascii_case("udp") { "udp" } else { "tcp" };
            if let Ok(p) = c.metadata.source_port.parse::<u16>() {
                by_port.entry((net, p)).or_default().push(i);
            }
        }
        let mut used: HashSet<usize> = HashSet::new();
        let mut apps: HashMap<String, AppRow> = HashMap::new();
        let mut app_proxy_bytes: HashMap<String, (u64, u64)> = HashMap::new();
        let mut hidden_loopback = 0usize;
        let core_set: HashSet<u32> = core_pids.iter().copied().collect();

        let app_key = |path: &Option<String>, exe: &str| -> String {
            path.as_ref().map(|p| p.to_lowercase()).unwrap_or_else(|| exe.to_lowercase())
        };

        for s in &sockets {
            if s.pid == 0 {
                continue;
            }
            let cached = self.procs.get(s.pid);
            let key: String = match cached {
                Some(info) => info.key.clone(),
                None => format!("pid {}", s.pid),
            };
            let proc_path: Option<&str> = cached.and_then(|i| i.path.as_deref());
            let app = apps.entry(key.clone()).or_insert_with(|| match cached {
                Some(info) => AppRow {
                    key: key.clone(),
                    name: info.display.clone(),
                    exe: info.exe.clone(),
                    path: info.path.clone(),
                    icon: info.icon.clone(),
                    is_proxy: core_set.contains(&s.pid) || proxy::is_client_exe(&info.exe),
                    ..Default::default()
                },
                None => AppRow {
                    key: key.clone(),
                    name: crate::tr!("进程 {0}", s.pid),
                    exe: format!("pid {}", s.pid),
                    is_proxy: core_set.contains(&s.pid),
                    ..Default::default()
                },
            });
            if !app.pids.contains(&s.pid) {
                app.pids.push(s.pid);
            }
            let net = if s.proto == Proto::Tcp { "tcp" } else { "udp" };
            let matched = by_port.get(&(net, s.local_port)).and_then(|ids| {
                ids.iter().copied().find(|i| {
                    if used.contains(i) {
                        return false;
                    }
                    let m = &mconns[*i].metadata;
                    m.process_path.is_empty()
                        || proc_path.map(|p| p.eq_ignore_ascii_case(&m.process_path)).unwrap_or(true)
                })
            });
            if let Some(i) = matched {
                used.insert(i);
                let row = self.row_from_mihomo(&mconns[i], s.proto, fmt_ip(&s.local), s.local_port, s.state, &sh.geo, &conn_rate, &proxy_ports, s.remote.as_ref());
                if row.node.as_deref().map(|n| n != "DIRECT" && !n.starts_with("REJECT")).unwrap_or(false) {
                    if let Some((bu, bd)) = conn_bytes.get(&mconns[i].id) {
                        let e = app_proxy_bytes.entry(key).or_insert((0, 0));
                        e.0 += *bu;
                        e.1 += *bd;
                    }
                }
                app.conns.push(row);
                continue;
            }
            match s.remote {
                None => {
                    if s.proto == Proto::Tcp {
                        app.listen.push(format!("TCP {}:{}", fmt_ip(&s.local), s.local_port));
                    } else {
                        app.listen.push(format!("UDP {}:{}", fmt_ip(&s.local), s.local_port));
                    }
                }
                Some((rip, rport)) => {
                    if is_loopback(&s.local) && is_loopback(&rip) {
                        hidden_loopback += 1;
                        continue;
                    }
                    let g = sh.geo.lookup(rip);
                    // public destination, proxy active, yet the connection never went through the proxy
                    let proxy_active = !mconns.is_empty() || (core_running && state.local_port.is_some());
                    let bypass = proxy_active
                        && !app.is_proxy
                        && !geo::is_private(&rip)
                        && !is_loopback(&rip)
                        && (s.state == "ESTABLISHED" || s.state == "SYN_SENT");
                    app.conns.push(ConnRow {
                        proto: if s.proto == Proto::Tcp { "TCP" } else { "UDP" },
                        local: fmt_ip(&s.local),
                        local_port: s.local_port,
                        remote: Some(fmt_ip(&rip)),
                        remote_port: Some(rport),
                        state: s.state,
                        host: self.dns.get(&rip).cloned(),
                        cc: g.cc,
                        org: g.org,
                        bypass,
                        ..Default::default()
                    });
                }
            }
        }
        // mihomo connections without a matching OS socket (mostly UDP), attached by process path.
        for (i, c) in mconns.iter().enumerate() {
            if used.contains(&i) || c.metadata.kind.eq_ignore_ascii_case("inner") {
                continue;
            }
            let path = non_empty(&c.metadata.process_path);
            let exe = non_empty(&c.metadata.process).unwrap_or_else(|| crate::i18n::t("未知进程").into());
            let key = app_key(&path, &exe);
            let app = apps.entry(key.clone()).or_insert_with(|| AppRow {
                key: key.clone(),
                name: std::path::Path::new(&exe).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(exe.clone()),
                exe: exe.clone(),
                path: path.clone(),
                ..Default::default()
            });
            let proto = if c.metadata.network.eq_ignore_ascii_case("udp") { Proto::Udp } else { Proto::Tcp };
            let lp = c.metadata.source_port.parse().unwrap_or(0);
            let row = self.row_from_mihomo(c, proto, c.metadata.source_ip.clone(), lp, if proto == Proto::Tcp { "ESTABLISHED" } else { "ACTIVE" }, &sh.geo, &conn_rate, &proxy_ports, None);
            if row.node.as_deref().map(|n| n != "DIRECT" && !n.starts_with("REJECT")).unwrap_or(false) {
                if let Some((bu, bd)) = conn_bytes.get(&c.id) {
                    let e = app_proxy_bytes.entry(key.clone()).or_insert((0, 0));
                    e.0 += *bu;
                    e.1 += *bd;
                }
            }
            app.conns.push(row);
        }

        // --- per-app speed ---
        let mut total_up = 0.0;
        let mut total_down = 0.0;
        let mut out: Vec<AppRow> = apps
            .into_values()
            .filter(|a| !a.conns.is_empty() || !a.listen.is_empty())
            .map(|mut a| {
                let etw: (f64, f64) = a.pids.iter().filter_map(|p| pid_rate.get(p)).fold((0.0, 0.0), |acc, r| (acc.0 + r.0, acc.1 + r.1));
                let mh: (f64, f64) = a.conns.iter().fold((0.0, 0.0), |acc, c| (acc.0 + c.up, acc.1 + c.down));
                (a.up, a.down) = if self.counters.is_some() { etw } else { mh };
                if !a.is_proxy {
                    total_up += a.up;
                    total_down += a.down;
                }
                a.conns.sort_by(|x, y| {
                    let sx = x.up + x.down;
                    let sy = y.up + y.down;
                    sy.partial_cmp(&sx).unwrap_or(std::cmp::Ordering::Equal).then((x.state != "ESTABLISHED").cmp(&(y.state != "ESTABLISHED")))
                });
                a.listen.sort();
                a.listen.dedup();
                a.pids.sort();
                a.via_proxy = a.conns.iter().any(|c| c.node.as_deref().map(|n| n != "DIRECT" && !n.starts_with("REJECT")).unwrap_or(false));
                a
            })
            .collect();
        // programs going through a proxy first, then by speed, connection count, name
        out.sort_by(|a, b| {
            let sa = a.up + a.down;
            let sb = b.up + b.down;
            b.via_proxy
                .cmp(&a.via_proxy)
                .then(sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal))
                .then(b.conns.len().cmp(&a.conns.len()))
                .then_with(|| cmp_ignore_case(&a.name, &b.name))
        });
        // group traffic score (decaying), for picking the busiest groups to measure
        {
            let mut score = sh.group_score.lock().unwrap();
            for v in score.values_mut() {
                *v *= 0.9995; // half-life about 23 minutes: groups used a while ago stay listed
            }
            for c in &mconns {
                if c.chains.first().map(|n| n == "DIRECT" || n.starts_with("REJECT")).unwrap_or(true) {
                    continue;
                }
                if let (Some(g), Some((u, d))) = (c.chains.last(), conn_rate.get(&c.id)) {
                    *score.entry(g.clone()).or_insert(0.0) += u + d;
                }
            }
            // traffic through a node that isn't measured yet (a group just switched, a speed test on
            // another node): measure soon, at most once a minute
            static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            if settings::str_at("exit.mode", "auto") == "auto" {
                let nodes: HashSet<String> = sh.exit.lock().unwrap().groups.iter().filter_map(|g| g.r.node.clone()).collect();
                let max = settings::num_at("exit.max", 4.0).clamp(1.0, 12.0) as usize;
                let new_busy = !nodes.is_empty()
                    && nodes.len() < max
                    && mconns.iter().any(|c| {
                        let n = c.chains.first().map(|s| s.as_str()).unwrap_or("DIRECT");
                        n != "DIRECT" && !n.starts_with("REJECT") && !nodes.contains(n) && conn_rate.get(&c.id).map(|(u, d)| u + d > 2048.0).unwrap_or(false)
                    });
                let now = now_ms();
                if new_busy && now.saturating_sub(LAST.load(Ordering::Relaxed)) > 60_000 {
                    LAST.store(now, Ordering::Relaxed);
                    sh.exit_trigger.store(true, Ordering::SeqCst);
                }
            }
        }
        let conn_count = out.iter().map(|a| a.conns.len()).sum();
        let listen_count = out.iter().map(|a| a.listen.len()).sum();
        self.account_traffic(sh, &out, &pid_bytes, &app_proxy_bytes);
        if Self::due(&self.firstnet_at, Duration::from_secs(2)) {
            self.firstnet_at = Some(Instant::now());
            first_net(&out);
        }
        let guard = self.detect_leaks(sh, &mut out, state.tun.is_some());
        let (want_cpu, want_mem) = (settings::item_on("pill.items", "cpu"), settings::item_on("pill.items", "mem"));
        let sys = (want_cpu || want_mem).then(|| self.sampler.sample(want_cpu, want_mem));

        Snapshot {
            ts: now_ms(),
            slim: false,
            admin: sh.admin,
            speed_ok: self.counters.is_some() || self.speed_warming,
            speed_error: self.speed_error.clone(),
            geo_source: sh.geo.source(),
            conn_count,
            listen_count,
            hidden_loopback,
            up: total_up,
            down: total_down,
            apps: out,
            proxy: state,
            exit: sh.exit.lock().unwrap().clone(),
            control: sh.control.lock().unwrap().clone(),
            guard,
            sys,
            latency: sh.latency.lock().unwrap().clone(),
            hotkey_ok: sh.ws.hotkey_ok.load(Ordering::SeqCst),
            maximized: sh.ws.maximized.load(Ordering::SeqCst),
            animations: self.animations,
        }
    }

    /// Adds this tick's exact bytes per app to today's traffic book: bytes on connections that left
    /// through a proxy node count as "proxy", the rest of the app's traffic as "direct".
    fn account_traffic(
        &mut self,
        sh: &Shared,
        apps: &[AppRow],
        pid_bytes: &HashMap<u32, (u64, u64)>,
        app_proxy_bytes: &HashMap<String, (u64, u64)>,
    ) {
        let dt = self.traffic_at.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0).min(5.0);
        self.traffic_at = Some(Instant::now());
        if dt <= 0.0 {
            return;
        }
        let mut book = sh.traffic.lock().unwrap();
        for a in apps.iter().filter(|a| !a.is_proxy) {
            if a.path.is_none() && a.key.starts_with("pid ") {
                continue;
            }
            let (pu, pd) = app_proxy_bytes.get(&a.key).map(|(u, d)| (*u as f64, *d as f64)).unwrap_or_else(|| {
                let via_node = |c: &&ConnRow| c.node.as_deref().map(|n| n != "DIRECT" && !n.starts_with("REJECT")).unwrap_or(false);
                a.conns.iter().filter(via_node).fold((0.0, 0.0), |s, c| (s.0 + c.up * dt, s.1 + c.down * dt))
            });
            let (du, dd) = if self.counters.is_some() {
                let (tu, td) = a.pids.iter().filter_map(|p| pid_bytes.get(p)).fold((0u64, 0u64), |s, b| (s.0 + b.0, s.1 + b.1));
                ((tu as f64 - pu).max(0.0), (td as f64 - pd).max(0.0))
            } else {
                let via_node = |c: &&ConnRow| c.node.as_deref().map(|n| n != "DIRECT" && !n.starts_with("REJECT")).unwrap_or(false);
                a.conns.iter().filter(|c| !via_node(c)).fold((0.0, 0.0), |s, c| (s.0 + c.up * dt, s.1 + c.down * dt))
            };
            if pu + pd + du + dd > 0.0 {
                book.add(&a.key, &a.name, a.path.as_ref(), (pu, pd), (du, dd));
            }
        }
        if Self::due(&self.traffic_flush, Duration::from_secs(60)) {
            book.flush();
            self.traffic_flush = Some(Instant::now());
        }
    }

    /// Marks connections to "should be proxied" destinations that went direct, keeps the alert
    /// book (active / acknowledged / history) and raises a toast for new leaks.
    fn detect_leaks(&mut self, sh: &Shared, apps: &mut [AppRow], tun: bool) -> GuardView {
        let now = now_ms();
        let matcher = sh.matcher.read().unwrap();
        let settings = sh.guard.lock().unwrap().clone();
        let blocked: HashSet<String> = settings.blocked.iter().map(|b| b.path.to_lowercase()).collect();
        let mut seen: Vec<Alert> = Vec::new();
        for app in apps.iter_mut() {
            if app.is_proxy {
                continue;
            }
            for c in app.conns.iter_mut() {
                let ip: Option<IpAddr> = c.remote.as_deref().and_then(|r| r.parse().ok());
                if ip.map(|i| geo::is_private(&i)).unwrap_or(false) {
                    continue;
                }
                let own_rule = c.rule.as_deref().map(|r| r.contains("micabar-") && !r.contains("micabar-guard")).unwrap_or(false);
                let kind = if c.bypass {
                    "bypass"
                } else if c.node.as_deref() == Some("DIRECT") && !own_rule {
                    "rule"
                } else {
                    continue;
                };
                let Some(hit) = matcher.hit(c.host.as_deref(), ip) else { continue };
                c.leak = Some(hit.clone());
                let target = c.host.clone().or_else(|| c.remote.clone()).unwrap_or_default();
                seen.push(Alert {
                    key: format!("{kind}|{}|{target}", app.exe.to_lowercase()),
                    kind,
                    app: app.name.clone(),
                    exe: app.exe.clone(),
                    path: app.path.clone(),
                    host: c.host.clone(),
                    ip: c.remote.clone(),
                    port: c.remote_port,
                    hit,
                    rule: c.rule.clone(),
                    first: now,
                    last: now,
                    active: true,
                    acked: false,
                    blocked: app.path.as_ref().map(|p| blocked.contains(&p.to_lowercase())).unwrap_or(false),
                });
            }
        }
        drop(matcher);

        let game = sh.ws.game.load(Ordering::SeqCst);
        let mut book = sh.alerts.lock().unwrap();
        for a in book.alerts.values_mut() {
            a.active = false;
        }
        let mut fresh: Vec<Alert> = Vec::new();
        for s in seen {
            match book.alerts.get_mut(&s.key) {
                Some(a) => {
                    // acknowledged, gone for 10+ minutes and back again: alert again
                    if a.acked && now.saturating_sub(a.last) > 10 * 60_000 {
                        a.acked = false;
                    }
                    a.last = now;
                    a.active = true;
                    a.blocked = s.blocked;
                    a.ip = s.ip;
                    a.rule = s.rule;
                }
                None => {
                    log(format!("leak ({}) {} -> {:?} {:?} hit {}", s.kind, s.exe, s.host, s.ip, s.hit));
                    fresh.push(s.clone());
                    book.alerts.insert(s.key.clone(), s);
                }
            }
        }
        if book.alerts.len() > 120 {
            let mut v: Vec<(String, u64)> = book.alerts.values().filter(|a| !a.active).map(|a| (a.key.clone(), a.last)).collect();
            v.sort_by_key(|x| x.1);
            for (k, _) in v.into_iter().take(book.alerts.len() - 120) {
                book.alerts.remove(&k);
            }
        }
        if settings.notify && !game {
            for f in &fresh {
                let recent = book.notified.get(&f.key).map(|t| now.saturating_sub(*t) < 10 * 60_000).unwrap_or(false);
                if recent {
                    continue;
                }
                book.notified.insert(f.key.clone(), now);
                let what = f.host.clone().or_else(|| f.ip.clone()).unwrap_or_default();
                let why = if f.kind == "bypass" { crate::i18n::t("没经过代理软件，直接连了出去") } else { crate::i18n::t("被分流规则判成了直连") };
                guard::toast(&crate::tr!("代理泄漏：{0}", f.app), &crate::tr!("{0} 本该走代理，{1}", what, why), &crate::tr!("命中规则库：{0}", f.hit));
            }
        }
        let mut alerts: Vec<Alert> = book.alerts.values().cloned().collect();
        alerts.sort_by(|a, b| b.active.cmp(&a.active).then(b.last.cmp(&a.last)));
        alerts.truncate(40);
        let unacked = book.alerts.values().filter(|a| a.active && !a.acked).count();
        let matcher = sh.matcher.read().unwrap();
        GuardView { settings, sources: matcher.sources.clone(), total: matcher.len(), alerts, unacked, tun }
    }

    /// Exit-control status: is Clash Party there, is the override installed, is it loaded.
    fn update_control(&mut self, sh: &Shared, list: &[(u32, String)], core_pids: &[u32]) {
        if !Self::due(&self.control_at, Duration::from_secs(4)) {
            return;
        }
        self.control_at = Some(Instant::now());
        if sh.control.lock().unwrap().status == "working" {
            return;
        }
        let cp = control::find_clash_party(list);
        let link = {
            let l = sh.mihomo.lock().unwrap();
            l.ctl.clone().map(|c| (c, l.secret.clone()))
        };
        let (status, message, client) = match &cp {
            None => {
                let other = sh.proxy.lock().unwrap().client.clone();
                let msg = other.map(|c| crate::tr!("出口控制目前只支持 Clash Party / Mihomo Party（检测到 {0}）", c));
                ("unavailable", msg, None)
            }
            Some(cp) => {
                let installed = control::override_installed(&cp.data_dir);
                let loaded = link.as_ref().map(|(c, s)| control::providers_loaded(c, s.as_deref()).unwrap_or(false)).unwrap_or(false);
                if installed && !control::override_current(&cp.data_dir) {
                    (
                        "available",
                        Some(crate::i18n::t("出口控制更新了规则结构，需要重新写入覆写（会重启 Clash Party 一次）").to_string()),
                        Some("Clash Party".to_string()),
                    )
                } else if loaded {
                    ("active", None, Some("Clash Party".to_string()))
                } else if installed {
                    ("pending", Some(crate::i18n::t("覆写已写入，等 Clash Party 重新加载内核后生效").to_string()), Some("Clash Party".to_string()))
                } else {
                    ("available", None, Some("Clash Party".to_string()))
                }
            }
        };
        let became_active = status == "active" && sh.control.lock().unwrap().status != "active";
        if status == "active" && sh.core_home.lock().unwrap().is_none() {
            let home = core_pids.first().and_then(|p| control::core_home(*p));
            log(format!("core home: {home:?}"));
            *sh.core_home.lock().unwrap() = home;
        }
        let node = if status == "active" {
            link.as_ref().and_then(|(c, s)| {
                let b = mihomo::get(c, &format!("/proxies/{}", control::GROUP), s.as_deref()).ok()?;
                let v: serde_json::Value = serde_json::from_slice(&b).ok()?;
                v.get("now").and_then(|n| n.as_str()).map(|s| s.to_string())
            })
        } else {
            None
        };
        let mut st = sh.control.lock().unwrap();
        if st.status == "error" && status == "available" {
            return; // keep showing the last error until the user retries
        }
        if st.status != status {
            log(format!("control status: {status}"));
        }
        st.status = status.into();
        st.message = message;
        st.client = client;
        st.node = node;
        st.rules = sh.rules.lock().unwrap().clone();
        drop(st);
        if became_active {
            // the guard library may have changed while MicaBar or the core was not running
            if let Err(e) = apply_guard(sh) {
                log(format!("guard providers: {e}"));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn row_from_mihomo(
        &self,
        c: &MConn,
        proto: Proto,
        local: String,
        local_port: u16,
        state: &'static str,
        geo: &Geo,
        rate: &HashMap<String, (f64, f64)>,
        proxy_ports: &HashSet<u16>,
        os_remote: Option<&(IpAddr, u16)>,
    ) -> ConnRow {
        let m = &c.metadata;
        let dest_ip: Option<IpAddr> = m.destination_ip.parse().ok();
        let via_proxy_port = os_remote.map(|(ip, p)| is_loopback(ip) && proxy_ports.contains(p)).unwrap_or(false)
            || m.kind.eq_ignore_ascii_case("http")
            || m.kind.eq_ignore_ascii_case("https")
            || m.kind.eq_ignore_ascii_case("socks5")
            || m.kind.eq_ignore_ascii_case("socks4");
        let g = dest_ip.map(|ip| geo.lookup(ip)).unwrap_or_default();
        let org = g.org.or_else(|| {
            // mihomo's own ASN text: "56046 China Mobile communications corporation"
            non_empty(&m.destination_asn).map(|s| s.split_once(' ').map(|(_, o)| o.to_string()).unwrap_or(s))
        });
        let host = non_empty(&m.host)
            .or_else(|| non_empty(&m.sniff_host))
            .or_else(|| dest_ip.and_then(|ip| self.dns.get(&ip).cloned()));
        let (up, down) = rate.get(&c.id).copied().unwrap_or((0.0, 0.0));
        ConnRow {
            proto: if proto == Proto::Tcp { "TCP" } else { "UDP" },
            local,
            local_port,
            remote: non_empty(&m.destination_ip).or_else(|| os_remote.map(|(ip, _)| fmt_ip(ip))),
            remote_port: m.destination_port.parse().ok(),
            state,
            host,
            cc: g.cc,
            org,
            node: c.chains.first().cloned(),
            group: if c.chains.len() > 1 { c.chains.last().cloned() } else { None },
            rule: rule_text(c),
            via_proxy_port,
            up,
            down,
            bypass: false,
            leak: None,
        }
    }
}

// ---------- exit IP ----------

async fn find_chain(sh: Arc<Shared>, host: String) -> Option<MConn> {
    let (ctl, secret) = {
        let l = sh.mihomo.lock().unwrap();
        (l.ctl.clone()?, l.secret.clone())
    };
    tauri::async_runtime::spawn_blocking(move || {
        let conns = mihomo::connections(&ctl, secret.as_deref()).ok()?;
        conns.into_iter().find(|c| c.metadata.host.eq_ignore_ascii_case(&host) || c.metadata.sniff_host.eq_ignore_ascii_case(&host))
    })
    .await
    .ok()
    .flatten()
}

enum Check {
    /// Bound to the physical NIC, domestic echo services.
    Direct,
    /// Through the local proxy port (or through TUN when there is no port), foreign services.
    Proxy(Option<u16>),
}

async fn check_one(sh: Arc<Shared>, kind: Check) -> ExitResult {
    let mut r = ExitResult { at: now_ms(), ..Default::default() };
    let mut keep_client = None;
    let res = match kind {
        Check::Direct => {
            let nic = proxy::physical_ipv4();
            r.via = nic.as_ref().map(|(n, ip)| format!("{n} {ip}"));
            exitip::probe_direct(nic.map(|(_, ip)| ip)).await
        }
        Check::Proxy(port) => match exitip::client(port) {
            Ok(c) => {
                let out = exitip::probe(&c, false).await;
                keep_client = Some(c);
                out
            }
            Err(e) => Err(e),
        },
    };
    match res {
        Ok(p) => {
            let g = sh.geo.lookup(p.ip);
            r.ip = Some(p.ip.to_string());
            r.cc = g.cc.or(p.cc);
            r.asn = g.asn;
            r.org = g.org.or(p.org);
            r.service = Some(p.host.clone());
            if let Some(c) = find_chain(sh.clone(), p.host).await {
                r.node = c.chains.first().cloned();
                r.group = if c.chains.len() > 1 { c.chains.last().cloned() } else { None };
                r.rule = rule_text(&c);
            }
        }
        Err(e) => r.error = Some(e),
    }
    drop(keep_client);
    r
}

/// The groups that matter most: settings `exit.mode` = auto (fallback group + busiest groups) or
/// manual (`exit.groups`); only groups that have an inbound in the running config.
fn groups_to_measure(sh: &Shared, listeners: &[(String, u16)], default_group: Option<&str>) -> Vec<(String, u16)> {
    let port_of = |g: &str| listeners.iter().find(|(n, _)| n == g).map(|(_, p)| *p);
    let mut picked: Vec<String> = Vec::new();
    if settings::str_at("exit.mode", "auto") == "manual" {
        if let Some(list) = settings::get().pointer("/exit/groups").and_then(|v| v.as_array()) {
            picked = list.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect();
        }
    } else {
        if let Some(d) = default_group {
            picked.push(d.to_string());
        }
        let score = sh.group_score.lock().unwrap();
        let mut busy: Vec<(&String, &f64)> = score.iter().filter(|(_, v)| **v > 512.0).collect();
        busy.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (g, _) in busy {
            if !picked.contains(g) {
                picked.push(g.clone());
            }
        }
    }
    picked.into_iter().filter_map(|g| port_of(&g).map(|p| (g, p))).collect()
}

/// Every group's current choice followed down to a real node, from one /proxies call, plus the
/// selector groups someone switched by hand (their choice isn't the subscription's first entry).
fn leaf_nodes(sh: &Shared) -> (HashMap<String, String>, HashSet<String>) {
    let mut out = HashMap::new();
    let mut switched = HashSet::new();
    let Ok((ctl, secret)) = ctl_link(sh) else { return (out, switched) };
    let Some(v) = mihomo::get(&ctl, "/proxies", secret.as_deref()).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) else {
        return (out, switched);
    };
    let Some(all) = v.get("proxies").and_then(|p| p.as_object()) else { return (out, switched) };
    let now = |n: &str| all.get(n).and_then(|p| p.get("now")).and_then(|x| x.as_str()).filter(|s| !s.is_empty()).map(str::to_string);
    for (name, p) in all {
        let Some(first) = now(name) else { continue };
        let is_selector = p.get("type").and_then(|t| t.as_str()) == Some("Selector");
        let head = p.get("all").and_then(|a| a.as_array()).and_then(|a| a.first()).and_then(|x| x.as_str());
        if is_selector && head.map(|h| h != first).unwrap_or(false) {
            switched.insert(name.clone());
        }
        let mut cur = first;
        for _ in 0..6 {
            match now(&cur) {
                Some(next) => cur = next,
                None => break,
            }
        }
        out.insert(name.clone(), cur);
    }
    (out, switched)
}

/// One exit probe per distinct node. Groups that leave through the same node share one IP, so
/// the fallback group and the busiest groups come first, then every other group a rule points at
/// whose node differs (e.g. the speed-test group switched to Singapore): each different exit is
/// measured once, up to `exit.max` of them. Groups on a measured node are listed with it.
async fn measure_groups(sh: Arc<Shared>) -> Vec<GroupExit> {
    let Some(file) = mihomo::read_config().file else { return Vec::new() };
    let path = std::path::PathBuf::from(file);
    let listeners = control::group_listeners(&path);
    if listeners.is_empty() {
        return Vec::new();
    }
    let default_group = control::match_group(&path);
    let picked = groups_to_measure(&sh, &listeners, default_group.as_deref());
    let max = settings::num_at("exit.max", 4.0).clamp(1.0, 12.0) as usize;
    let manual = settings::str_at("exit.mode", "auto") == "manual";
    let sh2 = sh.clone();
    let (leaf, switched) = tauri::async_runtime::spawn_blocking(move || leaf_nodes(&sh2)).await.unwrap_or_default();
    let port_of = |g: &str| listeners.iter().find(|(n, _)| n == g).map(|(_, p)| *p);
    let picked_names: Vec<String> = picked.iter().map(|(g, _)| g.clone()).collect();
    // candidates: the important groups, then (auto mode, nodes known) the other rule targets that
    // are in use or were chosen on purpose: switched by hand first (the speed-test group moved to
    // Singapore), then ones that carried some traffic lately; untouched idle groups are left out
    let mut cands: Vec<(String, u16)> = picked.clone();
    if !manual && !leaf.is_empty() {
        let score = sh.group_score.lock().unwrap().clone();
        let mut rest: Vec<(usize, String, u16)> = control::rule_groups(&path)
            .into_iter()
            .enumerate()
            .filter(|(_, g)| !cands.iter().any(|(c, _)| c == g))
            .filter_map(|(i, g)| port_of(&g).map(|p| (i, g, p)))
            .collect();
        let rank = |g: &str| if switched.contains(g) { 0 } else if score.get(g).copied().unwrap_or(0.0) > 1.0 { 1 } else { 2 };
        rest.retain(|(_, g, _)| rank(g) < 2);
        rest.sort_by_key(|(i, g, _)| (rank(g), *i));
        cands.extend(rest.into_iter().map(|(_, g, p)| (g, p)));
    }
    // node key: the leaf node, or the group itself when the node isn't known
    let key = |g: &str| leaf.get(g).cloned().unwrap_or_else(|| format!("group:{g}"));
    let mut probes: Vec<(String, String, u16)> = Vec::new(); // (node key, group probed, port)
    let mut members: HashMap<String, Vec<String>> = HashMap::new();
    for (g, p) in &cands {
        let k = key(g);
        let important = picked_names.contains(g);
        // blocked groups never leave; groups going DIRECT only matter when they carry traffic
        if k.starts_with("REJECT") || k == "PASS" || (k == "DIRECT" && !important) {
            continue;
        }
        if !probes.iter().any(|(pk, _, _)| *pk == k) {
            if probes.len() >= max {
                continue;
            }
            probes.push((k.clone(), g.clone(), *p));
        }
        // the main exit lists only the groups that matter (dozens of rule groups follow it);
        // another exit lists every group that uses it
        let main_key = default_group.as_deref().map(|d| key(d));
        if important || main_key.as_deref() != Some(k.as_str()) {
            members.entry(k).or_default().push(g.clone());
        }
    }
    let mut handles = Vec::new();
    for (k, group, port) in probes {
        let sh2 = sh.clone();
        handles.push(tauri::async_runtime::spawn(async move {
            let mut r = ExitResult { at: now_ms(), ..Default::default() };
            match exitip::client(Some(port)) {
                Ok(c) => match exitip::probe(&c, false).await {
                    Ok(p) => {
                        let g = sh2.geo.lookup(p.ip);
                        r.ip = Some(p.ip.to_string());
                        r.cc = g.cc.or(p.cc);
                        r.asn = g.asn;
                        r.org = g.org.or(p.org);
                        r.service = Some(p.host);
                    }
                    Err(e) => r.error = Some(e),
                },
                Err(e) => r.error = Some(e),
            }
            (k, group, r)
        }));
    }
    let mut out = Vec::new();
    for h in handles {
        let Ok((k, probed, mut r)) = h.await else { continue };
        if !k.starts_with("group:") {
            r.node = Some(k.clone());
        }
        let groups = members.get(&k).cloned().unwrap_or_else(|| vec![probed]);
        for g in groups {
            let mut r2 = r.clone();
            let is_default = default_group.as_deref() == Some(g.as_str());
            r2.group = Some(g);
            out.push(GroupExit { is_default, r: r2 });
        }
    }
    out
}

/// Returns true when every expected exit (direct while physical uplink exists, proxy while proxy
/// port/TUN is active) succeeded; false schedules a quick retry instead of waiting 5 minutes.
async fn exit_check(app: AppHandle, sh: Arc<Shared>) -> bool {
    {
        sh.exit.lock().unwrap().checking = true;
    }
    let _ = app.emit("exit", sh.exit.lock().unwrap().clone());
    let p = sh.proxy.lock().unwrap().clone();
    let has_uplink = proxy::physical_ipv4().is_some();
    let p_detected = p.detected;
    let p_port = p.local_port;
    let p_tun = p.tun.is_some();
    let portless_booting = p_detected && p_port.is_none() && !p_tun && {
        let since = PORTLESS_PROXY_SINCE_MS.load(Ordering::SeqCst);
        since == 0 || now_ms().saturating_sub(since) < 12_000
    };
    let sh_d = sh.clone();
    let sh_p = sh.clone();
    let direct_fut = check_one(sh_d, Check::Direct);
    let proxy_fut = async move {
        if p_detected {
            match p_port {
                Some(port) => Some(check_one(sh_p, Check::Proxy(Some(port))).await),
                None if p_tun => Some(check_one(sh_p, Check::Proxy(None)).await),
                // Avoid flashing red `出口检测失败` during the ~4s window between Clash Party GUI
                // launch and mihomo binding `:7890`; `proxy_sig` re-triggers immediately on bind.
                None if portless_booting => None,
                None => Some(ExitResult { error: Some(crate::i18n::t("检测到代理软件，但没找到本地代理端口").into()), at: now_ms(), ..Default::default() }),
            }
        } else {
            None
        }
    };
    let dh = tauri::async_runtime::spawn(direct_fut);
    let proxied: Option<ExitResult> = proxy_fut.await;
    let direct: ExitResult = dh.await.unwrap_or_default();
    log_changed("exit", format!(
        "exit check: direct={:?}/{:?} via={:?} node={:?} err={:?}; proxy={:?}",
        direct.ip,
        direct.cc,
        direct.via,
        direct.node,
        direct.error,
        proxied.as_ref().map(|r| (r.ip.clone(), r.cc.clone(), r.node.clone(), r.error.clone()))
    ));
    let node = proxied.as_ref().and_then(|r| r.node.clone()).filter(|n| n != control::GROUP && n != "DIRECT");
    if let Some(n) = node {
        if sh.node_pending.swap(false, Ordering::SeqCst) {
            let sh2 = sh.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                if let Ok((ctl, secret)) = ctl_link(&sh2) {
                    match control::select(&ctl, secret.as_deref(), &n) {
                        Ok(()) => log(format!("control: MicaBar group starts on {n}")),
                        Err(e) => log(format!("control: initial node select failed: {e}")),
                    }
                }
            })
            .await;
        }
    }
    let groups = if p_detected { measure_groups(sh.clone()).await } else { Vec::new() };
    let all_groups_failed = !groups.is_empty() && groups.iter().all(|g| g.r.ip.is_none());
    let direct_ok = !has_uplink || direct.ip.is_some();
    let proxy_expected = p_detected && (p_port.is_some() || p_tun);
    let proxy_ok = !portless_booting && (!proxy_expected || proxied.as_ref().map(|r| r.ip.is_some()).unwrap_or(false));
    if !groups.is_empty() {
        log_changed("groups", format!(
            "group exits: {}",
            groups.iter().map(|g| format!("{}={:?}", g.r.group.as_deref().unwrap_or(""), g.r.ip)).collect::<Vec<_>>().join(", ")
        ));
    }
    let snap = {
        let mut e = sh.exit.lock().unwrap();
        e.direct = Some(direct);
        e.proxy = proxied;
        e.groups = groups;
        e.checking = false;
        e.last_check = now_ms();
        fill_ip_org(&mut e);
        e.clone()
    };
    let _ = app.emit("exit", snap);
    enrich_ip_org(app, sh, p_port).await;
    direct_ok && proxy_ok && !all_groups_failed
}

fn exit_results(e: &mut ExitState) -> impl Iterator<Item = &mut ExitResult> {
    e.direct.iter_mut().chain(e.proxy.iter_mut()).chain(e.groups.iter_mut().map(|g| &mut g.r))
}

/// Puts the cached organisation / type on every exit result (none while the switch is off),
/// and backfills `r.cc` when the offline MMDB lacked a country code.
fn fill_ip_org(e: &mut ExitState) {
    let on = settings::bool_at("exit.ipOrg", true);
    for r in exit_results(e) {
        if !on {
            r.reg = None;
        } else if let Some(ip) = r.ip.clone() {
            if let Some(v) = iporg::cached(&ip) {
                if r.cc.is_none() {
                    r.cc = v.cc.clone();
                }
                r.reg = Some(v);
            }
        }
    }
}

/// Looks up the exit IPs that have no organisation yet, then sends the exits again.
async fn enrich_ip_org(app: AppHandle, sh: Arc<Shared>, port: Option<u16>) {
    if !settings::bool_at("exit.ipOrg", true) {
        return;
    }
    let (mut missing, direct_ip) = {
        let mut e = sh.exit.lock().unwrap();
        let dir = e.direct.as_ref().and_then(|r| r.ip.clone());
        let list: Vec<String> = exit_results(&mut e).filter(|r| r.reg.is_none()).filter_map(|r| r.ip.clone()).collect();
        (list, dir)
    };
    missing.sort();
    missing.dedup();
    if missing.is_empty() {
        return;
    }
    let mut tasks = Vec::new();
    for ip in missing {
        let is_direct = direct_ip.as_deref() == Some(ip.as_str());
        tasks.push(tauri::async_runtime::spawn(async move {
            iporg::lookup_ex(&ip, port, is_direct).await;
        }));
    }
    for t in tasks {
        let _ = t.await;
    }
    let snap = {
        let mut e = sh.exit.lock().unwrap();
        fill_ip_org(&mut e);
        e.clone()
    };
    let _ = app.emit("exit", snap);
}

// ---------- commands ----------

#[tauri::command]
async fn get_icon(path: String) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || icon::icon_data_url(&path)).await.ok().flatten()
}

/// Collapsed pill and expanded panel rectangles, physical px relative to the window.
#[tauri::command]
fn set_hit_rects(
    state: tauri::State<'_, Arc<Shared>>,
    pill: [f64; 4],
    panel: [f64; 4],
    glass_active: Option<bool>,
) {
    *state.ws.pill.lock().unwrap() = Rect { x: pill[0], y: pill[1], w: pill[2], h: pill[3] };
    *state.ws.panel.lock().unwrap() = Rect { x: panel[0], y: panel[1], w: panel[2], h: panel[3] };
    if let Some(active) = glass_active {
        let was_inactive = state.ws.glass_inactive.swap(!active, Ordering::SeqCst);
        if was_inactive && active {
            glass::request_frame();
        }
    }
}

// ---------- exit control commands ----------

fn ctl_link(sh: &Shared) -> Result<(Controller, Option<String>), String> {
    let l = sh.mihomo.lock().unwrap();
    l.ctl.clone().map(|c| (c, l.secret.clone())).ok_or_else(|| crate::i18n::t("mihomo 控制接口未连接").to_string())
}

fn set_control(sh: &Shared, status: &str, message: Option<String>) {
    let mut st = sh.control.lock().unwrap();
    st.status = status.into();
    st.message = message;
    st.rules = sh.rules.lock().unwrap().clone();
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn control_enable(state: tauri::State<'_, Arc<Shared>>) -> Result<(), String> {
    let sh = state.inner().clone();
    set_control(&sh, "working", Some(crate::i18n::t("正在写入覆写并重启 Clash Party…").into()));
    let r = blocking({
        let sh = sh.clone();
        move || {
            let list = procinfo::list_processes();
            let cores = proxy::client_name(&list).1;
            let cfg = mihomo::read_config();
            let res = control::set_installed(true, &list, &cores, &cfg)?;
            if let Some((ctl, secret)) = res {
                {
                    let mut l = sh.mihomo.lock().unwrap();
                    l.ctl = Some(ctl.clone());
                    l.secret = secret.clone();
                }
                let cores = proxy::client_name(&procinfo::list_processes()).1;
                let home = cores.first().and_then(|p| control::core_home(*p));
                *sh.core_home.lock().unwrap() = home;
                // the MicaBar group starts on the node the proxy exit uses; the exit check
                // right after the restart tells us which one
                sh.node_pending.store(true, Ordering::SeqCst);
            }
            Ok(())
        }
    })
    .await;
    match &r {
        Ok(()) => {
            set_control(&sh, "active", None);
            let sh2 = sh.clone();
            if let Err(e) = blocking(move || apply_guard(&sh2)).await {
                log(format!("guard providers after enable: {e}"));
            }
        }
        Err(e) => set_control(&sh, "error", Some(e.clone())),
    }
    sh.exit_trigger.store(true, Ordering::SeqCst);
    r
}

#[tauri::command]
async fn control_disable(state: tauri::State<'_, Arc<Shared>>) -> Result<(), String> {
    let sh = state.inner().clone();
    set_control(&sh, "working", Some(crate::i18n::t("正在移除覆写并重启 Clash Party…").into()));
    let r = blocking(move || {
        let list = procinfo::list_processes();
        let cores = proxy::client_name(&list).1;
        control::set_installed(false, &list, &cores, &mihomo::read_config()).map(|_| ())
    })
    .await;
    match &r {
        Ok(()) => set_control(&sh, "available", None),
        Err(e) => set_control(&sh, "error", Some(e.clone())),
    }
    sh.exit_trigger.store(true, Ordering::SeqCst);
    r
}

/// Adds / changes / removes (target "default") one rule; returns how many connections were reset.
#[tauri::command]
async fn control_set(
    state: tauri::State<'_, Arc<Shared>>,
    kind: String,
    process: Option<String>,
    value: Option<String>,
    target: String,
) -> Result<usize, String> {
    let sh = state.inner().clone();
    blocking(move || {
        let (ctl, secret) = ctl_link(&sh)?;
        let home = sh.core_home.lock().unwrap().clone().ok_or(crate::i18n::t("还不知道内核目录，稍等几秒再试"))?;
        let mut next = sh.rules.lock().unwrap().clone();
        let rule = control::upsert(&mut next, &kind, process, value, &target);
        control::persist(&home, &next)?;
        control::reload_providers(&ctl, secret.as_deref())?;
        *sh.rules.lock().unwrap() = next.clone();
        let n = control::close_matching(&ctl, secret.as_deref(), &rule);
        log(format!("control: {} {:?} {:?} -> {} (reset {n} connections)", rule.kind, rule.process, rule.value, target));
        sh.control.lock().unwrap().rules = next;
        Ok(n)
    })
    .await
}

#[tauri::command]
async fn control_remove(state: tauri::State<'_, Arc<Shared>>, id: String) -> Result<(), String> {
    let sh = state.inner().clone();
    blocking(move || {
        let (ctl, secret) = ctl_link(&sh)?;
        let home = sh.core_home.lock().unwrap().clone().ok_or(crate::i18n::t("还不知道内核目录"))?;
        let mut next = sh.rules.lock().unwrap().clone();
        let removed = next.iter().find(|r| r.id == id).cloned();
        next.retain(|r| r.id != id);
        control::persist(&home, &next)?;
        control::reload_providers(&ctl, secret.as_deref())?;
        *sh.rules.lock().unwrap() = next.clone();
        if let Some(r) = removed {
            control::close_matching(&ctl, secret.as_deref(), &r);
        }
        sh.control.lock().unwrap().rules = next;
        Ok(())
    })
    .await
}

#[tauri::command]
async fn control_nodes(state: tauri::State<'_, Arc<Shared>>, test: bool) -> Result<control::Nodes, String> {
    let sh = state.inner().clone();
    blocking(move || {
        let (ctl, secret) = ctl_link(&sh)?;
        if test {
            let _ = control::test_delays(&ctl, secret.as_deref());
        }
        control::nodes(&ctl, secret.as_deref())
    })
    .await
}

#[tauri::command]
async fn control_select(state: tauri::State<'_, Arc<Shared>>, name: String) -> Result<(), String> {
    let sh = state.inner().clone();
    let r = blocking({
        let sh = sh.clone();
        move || {
            let (ctl, secret) = ctl_link(&sh)?;
            control::select(&ctl, secret.as_deref(), &name)?;
            sh.control.lock().unwrap().node = Some(name);
            Ok(())
        }
    })
    .await;
    r
}

// ---------- node latency ----------

fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// Whether anything shown needs the node latency (pill item, flashes, node quality page).
fn latency_wanted() -> bool {
    settings::item_on("pill.items", "latency")
        || settings::bool_at("flash.latencyHigh", true)
        || settings::bool_at("flash.nodeDown", true)
        || settings::item_on("panel.tabs", "nodes")
}

/// Asks mihomo for the delay of every node the exits currently use: the main one (default group,
/// shown in the pill and used by the flashes) first, then the other groups' nodes. One round of
/// samples shares one timestamp so the page can draw a line per node.
fn probe_latency(sh: &Shared) {
    let is_real_node = |n: &str| !n.eq_ignore_ascii_case("DIRECT") && !n.to_ascii_uppercase().starts_with("REJECT") && !n.eq_ignore_ascii_case("PASS");
    let nodes: Vec<String> = {
        let ex = sh.exit.lock().unwrap();
        let main = ex
            .groups
            .iter()
            .find(|g| g.is_default)
            .and_then(|g| g.r.node.clone())
            .or_else(|| ex.proxy.as_ref().and_then(|e| e.node.clone()))
            .filter(|n| is_real_node(n));
        let mut v: Vec<String> = main.into_iter().collect();
        for g in &ex.groups {
            if let Some(n) = &g.r.node {
                if !v.contains(n) && is_real_node(n) {
                    v.push(n.clone());
                }
            }
        }
        v.truncate(8);
        v
    };
    if nodes.is_empty() {
        return;
    }
    let Ok((ctl, secret)) = ctl_link(sh) else { return };
    let url = "https%3A%2F%2Fwww.gstatic.com%2Fgenerate_204";
    let now = now_ms();
    for (i, node) in nodes.into_iter().enumerate() {
        let r = mihomo::get(&ctl, &format!("/proxies/{}/delay?url={url}&timeout=3000", pct(&node)), secret.as_deref());
        let (ms, error) = match r {
            Ok(b) => {
                let d = serde_json::from_slice::<serde_json::Value>(&b).ok().and_then(|v| v.get("delay").and_then(|d| d.as_u64()));
                (d.map(|d| d as u32), None)
            }
            Err(e) => (None, Some(if e.contains("504") || e.to_lowercase().contains("timeout") { crate::i18n::t("超时").to_string() } else { e })),
        };
        if i == 0 {
            *sh.latency.lock().unwrap() = Latency { node: Some(node.clone()), ms, error, at: now };
        }
        let mut h = sh.latency_history.lock().unwrap();
        h.push_back(LatencySample { at: now, node, ms });
        while h.len() > 1440 * 4 {
            h.pop_front();
        }
    }
}

fn profile_secs(saver: u64, balanced: u64, smooth: u64) -> u64 {
    match settings::str_at("general.profile", "balanced").as_str() {
        "saver" => saver,
        "smooth" => smooth,
        _ => balanced,
    }
}

/// Proxy groups that have a MicaBar inbound (for the manual group picker), in config order.
#[tauri::command]
async fn exit_group_options() -> Vec<String> {
    tauri::async_runtime::spawn_blocking(|| {
        mihomo::read_config()
            .file
            .map(|f| control::group_listeners(std::path::Path::new(&f)).into_iter().map(|(g, _)| g).collect())
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

#[tauri::command]
async fn traffic_query(app: AppHandle, days: i64, day: Option<String>) -> traffic::Report {
    let sh = app.state::<Arc<Shared>>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let book = sh.traffic.lock().unwrap();
        traffic::report(&book, days.clamp(1, traffic::KEEP_DAYS + 1), day.as_deref())
    })
    .await
    .unwrap_or_default()
}

#[tauri::command]
fn latency_history(state: tauri::State<'_, Arc<Shared>>) -> Vec<LatencySample> {
    state.latency_history.lock().unwrap().iter().cloned().collect()
}

#[tauri::command]
async fn latency_probe(state: tauri::State<'_, Arc<Shared>>) -> Result<Latency, String> {
    let sh = state.inner().clone();
    blocking(move || {
        probe_latency(&sh);
        Ok(sh.latency.lock().unwrap().clone())
    })
    .await
}

// ---------- updates / about ----------

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    version: String,
    installed: bool,
    autostart: bool,
    log_dir: String,
    exe: String,
}

#[tauri::command]
async fn app_info(app: AppHandle) -> AppInfo {
    let version = app.package_info().version.to_string();
    tauri::async_runtime::spawn_blocking(move || AppInfo {
        version,
        installed: system::is_installed(),
        autostart: system::autostart_enabled(),
        log_dir: log_path().parent().map(|p| p.display().to_string()).unwrap_or_default(),
        exe: std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
    })
    .await
    .unwrap_or_default()
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct UpdateInfo {
    current: String,
    available: bool,
    version: Option<String>,
    notes: Option<String>,
    installed: bool,
}

async fn check_update(app: &AppHandle) -> Result<UpdateInfo, String> {
    use tauri_plugin_updater::UpdaterExt;
    let current = app.package_info().version.to_string();
    let updater = app.updater().map_err(|e| e.to_string())?;
    match updater.check().await {
        Ok(Some(u)) => Ok(UpdateInfo { current, available: true, version: Some(u.version.clone()), notes: u.body.clone(), installed: system::is_installed() }),
        Ok(None) => Ok(UpdateInfo { current, installed: system::is_installed(), ..Default::default() }),
        Err(e) => Err(crate::tr!("检查更新失败：{0}", e)),
    }
}

#[tauri::command]
async fn update_check(app: AppHandle) -> Result<UpdateInfo, String> {
    let r = check_update(&app).await;
    log(format!("update check: {:?}", r.as_ref().map(|u| (u.available, u.version.clone())).map_err(|e| e.clone())));
    r
}

/// Downloads the signed installer and runs it (passive); the installer closes and restarts the app.
#[tauri::command]
async fn update_install(app: AppHandle, state: tauri::State<'_, Arc<Shared>>) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
    if !system::is_installed() {
        return Err(crate::i18n::t("这是便携版，不能自动更新；请到 GitHub 下载新版本").into());
    }
    let u = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?.ok_or(crate::i18n::t("已经是最新版本"))?;
    log(format!("update: installing {}", u.version));
    // progress for the update card: {phase: download|install, got, total}
    let _ = app.emit("update-progress", serde_json::json!({ "phase": "download", "got": 0, "total": null }));
    let bytes = u
        .download(
            {
                let app = app.clone();
                let mut got = 0u64;
                let mut sent = 0u64;
                move |n, total| {
                    got += n as u64;
                    // at most ~100 events: every 1 % (or every 256 KB when the size is unknown)
                    let step = total.map(|t| (t / 100).max(1)).unwrap_or(256 * 1024);
                    if got - sent >= step || total == Some(got) {
                        sent = got;
                        let _ = app.emit("update-progress", serde_json::json!({ "phase": "download", "got": got, "total": total }));
                    }
                }
            },
            || {},
        )
        .await
        .map_err(|e| {
            log(format!("update: download failed: {e}"));
            crate::tr!("下载失败：{0}", e)
        })?;
    log(format!("update: downloaded {} bytes, signature ok, installing", bytes.len()));
    let _ = app.emit("update-progress", serde_json::json!({ "phase": "install" }));
    // give the page a moment to show "正在安装" before the installer closes us
    tokio::time::sleep(Duration::from_millis(600)).await;
    state.traffic.lock().unwrap().flush();
    speed::stop();
    u.install(bytes).map_err(|e| {
        log(format!("update: install failed: {e}"));
        crate::tr!("安装失败：{0}", e)
    })?;
    app.restart();
}

// ---------- notifications ----------

#[tauri::command]
fn notify_list() -> Vec<notify::Note> {
    notify::list()
}

#[tauri::command]
async fn notify_open(id: u64) -> Result<(), String> {
    blocking(move || notify::open(id)).await
}

/// Removes notes from the centre (all when `ids` is empty).
#[tauri::command]
fn notify_dismiss(ids: Vec<u64>) {
    notify::dismiss(&ids);
}

#[tauri::command]
fn notify_read(ids: Vec<u64>) {
    notify::mark_read(&ids);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NotifyApp {
    #[serde(flatten)]
    seen: notify::AppSeen,
    takeover: bool,
}

#[tauri::command]
async fn notify_apps() -> Vec<NotifyApp> {
    tauri::async_runtime::spawn_blocking(|| {
        notify::apps().into_iter().map(|a| NotifyApp { takeover: a.source == "toast" && notify::takeover_on(&a.key), seen: a }).collect()
    })
    .await
    .unwrap_or_default()
}

#[tauri::command]
async fn notify_takeover(key: String, on: bool) -> Result<(), String> {
    blocking(move || notify::set_takeover(&key, on)).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NotifyHooks {
    port: u16,
    quiet: bool,
    hooks: Vec<notify::HookStatus>,
}

#[tauri::command]
async fn notify_hooks() -> NotifyHooks {
    tauri::async_runtime::spawn_blocking(|| NotifyHooks { port: notify::port(), quiet: notify::quiet_now(), hooks: notify::hooks_status() })
        .await
        .unwrap_or(NotifyHooks { port: 47900, quiet: false, hooks: Vec::new() })
}

#[tauri::command]
async fn notify_hook(tool: String, on: bool) -> Result<String, String> {
    blocking(move || notify::set_hook(&tool, on)).await
}

#[tauri::command]
fn notify_test() {
    notify::test();
}

/// The page shows (true) or hid (false) a notification card in the collapsed island.
#[tauri::command]
fn note_mode(state: tauri::State<'_, Arc<Shared>>, on: bool) {
    state.ws.noting.store(on, Ordering::SeqCst);
}

#[tauri::command]
fn open_url(url: String) {
    if url.starts_with("https://") || std::path::Path::new(&url).is_dir() {
        let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
    }
}

// ---------- settings commands ----------

#[tauri::command]
fn settings_get() -> serde_json::Value {
    settings::get()
}

fn is_dark_theme() -> bool {
    match settings::str_at("appearance.theme", "auto").as_str() {
        "glass-light" | "github-light" | "solarized-light" => false,
        "glass-dark" | "onedark" | "github-dark" | "dracula" | "nightowl" => true,
        "custom" => {
            let hex = settings::str_at("appearance.custom.bg", "#14141c");
            let h = hex.trim_start_matches('#');
            if h.len() >= 6 {
                let r = u8::from_str_radix(&h[0..2], 16).unwrap_or(20) as u32;
                let g = u8::from_str_radix(&h[2..4], 16).unwrap_or(20) as u32;
                let b = u8::from_str_radix(&h[4..6], 16).unwrap_or(28) as u32;
                (54 * r + 183 * g + 19 * b) < 128 * 256
            } else {
                true
            }
        }
        _ => {
            use winreg::enums::HKEY_CURRENT_USER;
            winreg::RegKey::predef(HKEY_CURRENT_USER)
                .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
                .and_then(|k| k.get_value::<u32, _>("AppsUseLightTheme"))
                .map(|v| v == 0)
                .unwrap_or(true)
        }
    }
}

fn manager_bg_color(dark: bool) -> tauri::window::Color {
    if settings::str_at("appearance.theme", "auto") == "custom" {
        let hex = settings::str_at("appearance.custom.bg", "#16161c");
        let h = hex.trim_start_matches('#');
        if h.len() >= 6 {
            let r = u8::from_str_radix(&h[0..2], 16).unwrap_or(22);
            let g = u8::from_str_radix(&h[2..4], 16).unwrap_or(22);
            let b = u8::from_str_radix(&h[4..6], 16).unwrap_or(28);
            return tauri::window::Color(r, g, b, 255);
        }
    }
    if dark {
        tauri::window::Color(20, 20, 26, 255)
    } else {
        tauri::window::Color(246, 247, 250, 255)
    }
}

fn sync_manager_titlebar(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("manager") {
        let dark = is_dark_theme();
        let _ = w.set_theme(Some(if dark { tauri::Theme::Dark } else { tauri::Theme::Light }));
        let _ = w.set_background_color(Some(manager_bg_color(dark)));
        if let Ok(hwnd) = w.hwnd() {
            system::set_dark_titlebar(hwnd.0 as isize, dark);
        }
    }
}

/// Side effects of a settings change that live in the backend.
fn apply_settings_backend(app: &AppHandle, ws: &WinState) {
    window_ctl::reload_hotkey(ws);
    if let Some(w) = app.get_webview_window("main") {
        window_ctl::place_window(&w);
    }
    sync_manager_titlebar(app);
    // language changed: new tray menu (the windows reload themselves after ui_lang)
    if i18n::set_lang(&i18n::wanted()) {
        if let (Some(tray), Ok(menu)) = (app.tray_by_id("main"), tray_menu(app)) {
            let _ = tray.set_menu(Some(menu));
        }
        // exit IP names (region, carrier) come in the new language with a fresh check
        app.state::<Arc<Shared>>().exit_trigger.store(true, Ordering::SeqCst);
    }
}

/// The resolved interface language (general.lang, auto = Windows display language).
#[tauri::command]
fn ui_lang() -> String {
    i18n::lang()
}

fn tray_menu<M: Manager<tauri::Wry>>(app: &M) -> tauri::Result<Menu<tauri::Wry>> {
    let t = i18n::t;
    let open_item = MenuItem::with_id(app, "open", t("打开 MicaBar"), true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, "settings", t("设置"), true, None::<&str>)?;
    let layout_item = MenuItem::with_id(app, "layout", t("调整岛的大小…"), true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, "refresh", t("重新检测出口 IP"), true, None::<&str>)?;
    let log_item = MenuItem::with_id(app, "log", t("打开日志"), true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", t("退出"), true, None::<&str>)?;
    let theme_auto = MenuItem::with_id(app, "theme:auto", t("外观：跟随系统"), true, None::<&str>)?;
    let theme_light = MenuItem::with_id(app, "theme:light", t("外观：白色磨砂"), true, None::<&str>)?;
    let theme_dark = MenuItem::with_id(app, "theme:dark", t("外观：黑色磨砂"), true, None::<&str>)?;
    let sep = tauri::menu::PredefinedMenuItem::separator(app)?;
    let sep0 = tauri::menu::PredefinedMenuItem::separator(app)?;
    Menu::with_items(
        app,
        &[&open_item, &settings_item, &layout_item, &sep0, &refresh, &theme_auto, &theme_light, &theme_dark, &sep, &log_item, &quit_item],
    )
}

#[tauri::command]
async fn settings_set(app: AppHandle, state: tauri::State<'_, Arc<Shared>>, patch: serde_json::Value) -> Result<serde_json::Value, String> {
    let sh = state.inner().clone();
    blocking(move || {
        // start with Windows: create / remove the logon task first, keep the setting only if it worked
        if let Some(on) = patch.pointer("/general/autostart").and_then(|v| v.as_bool()) {
            system::set_autostart(on).map_err(|e| crate::tr!("设置开机自启失败：{0}", e))?;
        }
        let s = settings::update(&patch);
        apply_settings_backend(&app, &sh.ws);
        if patch.pointer("/notify/port").is_some() {
            std::thread::spawn(notify::sync_hook_endpoints);
        }
        if patch.get("clip").is_some() {
            // limits / storage changed: applied off the main thread (it may move files)
            std::thread::spawn(clipboard::apply_settings);
        }
        let _ = app.emit("settings", &s);
        Ok(s)
    })
    .await
}

#[tauri::command]
async fn settings_reset(app: AppHandle) -> serde_json::Value {
    let sh = app.state::<Arc<Shared>>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _ = system::set_autostart(false);
        let s = settings::reset();
        clipboard::apply_settings();
        apply_settings_backend(&app, &sh.ws);
        let _ = app.emit("settings", &s);
        s
    })
    .await
    .unwrap_or_else(|_| settings::get())
}

/// Glass off: drop the frame channel, which pauses desktop duplication entirely.
#[tauri::command]
fn glass_off(state: tauri::State<'_, Arc<Shared>>) {
    glass::set_channel(&state.glass, None);
}

/// UI scale (percent) as WebView zoom; the page then refits the window to the new pixel ratio.
#[tauri::command]
fn set_zoom(app: AppHandle, scale: f64) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_zoom((scale / 100.0).clamp(0.7, 2.0));
    }
}

/// Installed font family names (from the registry font tables), for the font picker.
#[tauri::command]
async fn fonts_list() -> Vec<String> {
    tauri::async_runtime::spawn_blocking(|| {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        let mut names: std::collections::BTreeSet<String> = Default::default();
        for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
            let Ok(k) = winreg::RegKey::predef(root).open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts") else { continue };
            for (name, _) in k.enum_values().flatten() {
                // "Microsoft YaHei & Microsoft YaHei UI (TrueType)" -> both families, styles dropped
                let base = name.split(" (").next().unwrap_or(&name).to_string();
                for part in base.split('&') {
                    let mut fam = part.trim().to_string();
                    for style in [" Bold Italic", " Bold", " Italic", " Light", " Semibold", " SemiBold", " Black", " Regular", " Oblique", " Semilight", " SemiLight", " Medium", " Thin"] {
                        if let Some(f) = fam.strip_suffix(style) {
                            fam = f.to_string();
                        }
                    }
                    if !fam.is_empty() && fam.chars().count() < 48 {
                        names.insert(fam);
                    }
                }
            }
        }
        names.into_iter().collect()
    })
    .await
    .unwrap_or_default()
}

#[tauri::command]
fn monitors_list() -> Vec<window_ctl::MonitorInfo> {
    window_ctl::monitor_list()
}

/// Whether the configured hotkey is registered, and a candidate is valid.
#[tauri::command]
fn hotkey_check(state: tauri::State<'_, Arc<Shared>>, candidate: Option<String>) -> (bool, bool) {
    let valid = candidate.map(|c| window_ctl::parse_hotkey(&c).is_some()).unwrap_or(true);
    (state.ws.hotkey_ok.load(Ordering::SeqCst), valid)
}

// ---------- leak guard commands ----------

/// Saves the settings, rebuilds the matcher and, when exit control is active, refreshes the
/// guard rule-providers inside mihomo.
fn apply_guard(sh: &Shared) -> Result<(), String> {
    let s = sh.guard.lock().unwrap().clone();
    guard::save_settings(&s);
    let m = guard::Matcher::build(&s);
    let active = sh.control.lock().unwrap().status == "active";
    let home = sh.core_home.lock().unwrap().clone();
    if let (true, Some(home)) = (active, home) {
        m.write_providers(&home, s.force_proxy)?;
        if let Ok((ctl, secret)) = ctl_link(sh) {
            control::reload_guard(&ctl, secret.as_deref())?;
        }
    }
    log(format!("guard: {} entries, force_proxy={}", m.len(), s.force_proxy));
    *sh.matcher.write().unwrap() = m;
    Ok(())
}

fn proxy_port(sh: &Shared) -> Option<u16> {
    sh.proxy.lock().unwrap().local_port
}

#[tauri::command]
async fn guard_options(state: tauri::State<'_, Arc<Shared>>, builtin: bool, gfw: bool, force_proxy: bool, notify: bool) -> Result<(), String> {
    let sh = state.inner().clone();
    let need_gfw = {
        let mut s = sh.guard.lock().unwrap();
        let need = gfw && !s.gfw;
        s.builtin = builtin;
        s.gfw = gfw;
        s.force_proxy = force_proxy;
        s.notify = notify;
        need && s.gfw_count == 0
    };
    if need_gfw {
        let _ = update_gfw(sh.clone()).await;
    }
    blocking(move || apply_guard(&sh)).await
}

/// Custom entry: "example.com", "keyword:abc" / "关键词:abc", "1.2.3.0/24", "!example.com" (exclude).
#[tauri::command]
async fn guard_add(state: tauri::State<'_, Arc<Shared>>, text: String) -> Result<(), String> {
    let sh = state.inner().clone();
    blocking(move || {
        let t = text.trim();
        let entry = if let Some(v) = t.strip_prefix('!').or_else(|| t.strip_prefix("排除:")).or_else(|| t.strip_prefix("排除：")) {
            let mut e = guard::parse_list(v).into_iter().next().ok_or(crate::i18n::t("不是有效的域名"))?;
            e.kind = "exclude".into();
            e
        } else if let Some(v) = t.strip_prefix("keyword:").or_else(|| t.strip_prefix("关键词:")).or_else(|| t.strip_prefix("关键词：")) {
            let kw = guard::clean_keyword(v).ok_or(crate::i18n::t("没认出来：填域名、IP/CIDR、keyword:关键词，或 !域名 表示排除"))?;
            guard::Entry { kind: "keyword".into(), value: kw }
        } else {
            guard::parse_list(t).into_iter().next().ok_or(crate::i18n::t("没认出来：填域名、IP/CIDR、keyword:关键词，或 !域名 表示排除"))?
        };
        {
            let mut s = sh.guard.lock().unwrap();
            if !s.custom.iter().any(|e| e.kind == entry.kind && e.value == entry.value) {
                s.custom.push(entry);
            }
        }
        apply_guard(&sh)
    })
    .await
}

#[tauri::command]
async fn guard_remove(state: tauri::State<'_, Arc<Shared>>, kind: String, value: String) -> Result<(), String> {
    let sh = state.inner().clone();
    blocking(move || {
        sh.guard.lock().unwrap().custom.retain(|e| !(e.kind == kind && e.value == value));
        apply_guard(&sh)
    })
    .await
}

async fn update_gfw(sh: Arc<Shared>) -> Result<usize, String> {
    let urls: Vec<String> = guard::GFW_URLS.iter().map(|s| s.to_string()).collect();
    let r = guard::fetch_text(&urls, proxy_port(&sh)).await;
    let mut s = sh.guard.lock().unwrap();
    match r {
        Ok(t) => {
            let n = guard::save_gfw(&t);
            s.gfw_count = n;
            s.gfw_updated = now_ms();
            s.gfw_error = None;
            log(format!("guard: gfw list updated, {n} entries"));
            Ok(n)
        }
        Err(e) => {
            s.gfw_error = Some(e.clone());
            log(format!("guard: gfw update failed: {e}"));
            Err(crate::tr!("更新 gfw 列表失败：{0}", e))
        }
    }
}

#[tauri::command]
async fn guard_update_gfw(state: tauri::State<'_, Arc<Shared>>) -> Result<usize, String> {
    let sh = state.inner().clone();
    let n = update_gfw(sh.clone()).await?;
    let sh2 = sh.clone();
    blocking(move || apply_guard(&sh2)).await?;
    Ok(n)
}

#[tauri::command]
async fn guard_import(state: tauri::State<'_, Arc<Shared>>, url: String) -> Result<usize, String> {
    let sh = state.inner().clone();
    let url = url.trim().to_string();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(crate::i18n::t("填一个 http(s) 链接").into());
    }
    let t = guard::fetch_text(&[url.clone()], proxy_port(&sh))
        .await
        .map_err(|e| crate::tr!("下载失败：{0}", e))?;
    let n = guard::save_import(&url, &t);
    if n == 0 {
        guard::remove_import_file(&url);
        return Err(crate::i18n::t("这个链接里没认出任何域名 / IP 规则").into());
    }
    {
        let mut s = sh.guard.lock().unwrap();
        s.imports.retain(|i| i.url != url);
        s.imports.push(guard::Import { url: url.clone(), enabled: true, count: n, updated: now_ms(), error: None });
    }
    let sh2 = sh.clone();
    blocking(move || apply_guard(&sh2)).await?;
    Ok(n)
}

#[tauri::command]
async fn guard_import_set(state: tauri::State<'_, Arc<Shared>>, url: String, enabled: Option<bool>, remove: bool) -> Result<(), String> {
    let sh = state.inner().clone();
    blocking(move || {
        {
            let mut s = sh.guard.lock().unwrap();
            if remove {
                s.imports.retain(|i| i.url != url);
                guard::remove_import_file(&url);
            } else if let Some(i) = s.imports.iter_mut().find(|i| i.url == url) {
                i.enabled = enabled.unwrap_or(i.enabled);
            }
        }
        apply_guard(&sh)
    })
    .await
}

/// Windows Firewall: block the program's direct traffic to public addresses.
#[tauri::command]
async fn guard_block(state: tauri::State<'_, Arc<Shared>>, path: String, exe: String) -> Result<String, String> {
    let sh = state.inner().clone();
    blocking(move || {
        if !sh.admin {
            return Err(crate::i18n::t("改防火墙需要以管理员身份运行 MicaBar").into());
        }
        if sh.proxy.lock().unwrap().tun.is_some() {
            return Err(crate::i18n::t("TUN 模式下程序的流量也走虚拟网卡，用防火墙禁止会让它完全断网，所以不提供").into());
        }
        let name = guard::block(&path, &exe)?;
        {
            let mut s = sh.guard.lock().unwrap();
            s.blocked.retain(|b| !b.path.eq_ignore_ascii_case(&path));
            s.blocked.push(guard::Blocked { path: path.clone(), exe: exe.clone(), rule: name, at: now_ms() });
            guard::save_settings(&s);
        }
        let n = guard::reset_connections(&pids_of(&sh, &path));
        log(format!("guard: firewall block {path} (reset {n} connections)"));
        Ok(match firstnet::firewall_warning() {
            Some(w) => crate::tr!("已添加禁止 {0} 直连的规则，{1}", exe, w),
            None => crate::tr!("已禁止 {0} 直连{1}", exe, String::new()),
        })
    })
    .await
}

#[tauri::command]
async fn guard_unblock(state: tauri::State<'_, Arc<Shared>>, path: String) -> Result<(), String> {
    let sh = state.inner().clone();
    blocking(move || {
        if !sh.admin {
            return Err(crate::i18n::t("改防火墙需要以管理员身份运行 MicaBar").into());
        }
        let b = sh.guard.lock().unwrap().blocked.iter().find(|b| b.path.eq_ignore_ascii_case(&path)).cloned();
        if let Some(b) = b {
            guard::unblock(&b.rule, &b.path)?;
            log(format!("guard: firewall unblock {}", b.path));
        }
        let mut s = sh.guard.lock().unwrap();
        s.blocked.retain(|b| !b.path.eq_ignore_ascii_case(&path));
        guard::save_settings(&s);
        Ok(())
    })
    .await
}

/// key = one alert, or None = all active ones.
#[tauri::command]
fn alert_ack(state: tauri::State<'_, Arc<Shared>>, key: Option<String>) {
    let mut book = state.alerts.lock().unwrap();
    for a in book.alerts.values_mut() {
        if key.as_ref().map(|k| *k == a.key).unwrap_or(a.active) {
            a.acked = true;
        }
    }
}

#[tauri::command]
fn alert_clear(state: tauri::State<'_, Arc<Shared>>) {
    state.alerts.lock().unwrap().alerts.retain(|_, a| a.active);
}

// ---------- AI quota / now playing ----------

/// Proxy port for the AI services: the one set in the settings, else the proxy client's local port
/// (mixed / http port, or the system proxy's). None = no query at all, never a direct one.
// ---------- live AI sessions ----------

#[tauri::command]
async fn agents_get() -> Vec<agents::Session> {
    tauri::async_runtime::spawn_blocking(agents::list).await.unwrap_or_default()
}

#[tauri::command]
async fn agents_focus(id: String) -> Result<(), String> {
    blocking(move || agents::focus(&id)).await
}

// ---------- network diagnosis ----------

fn diag_info(sh: &Shared) -> netdiag::Info {
    let p = sh.proxy.lock().unwrap().clone();
    let node = {
        let ex = sh.exit.lock().unwrap();
        ex.groups.iter().find(|g| g.is_default).and_then(|g| g.r.node.clone()).or_else(|| ex.proxy.as_ref().and_then(|e| e.node.clone()))
    };
    netdiag::Info {
        detected: p.detected,
        client: p.client.clone(),
        core_running: !p.core_pids.is_empty(),
        local_port: p.local_port,
        tun: p.tun.is_some(),
        system_proxy: p.system_proxy.clone(),
        node: node.filter(|n| n != "DIRECT"),
        controller: sh.mihomo.lock().unwrap().ctl.is_some(),
        ..Default::default()
    }
}

#[tauri::command]
fn netdiag_get() -> netdiag::State {
    netdiag::state()
}

#[tauri::command]
async fn netdiag_run() -> Result<netdiag::Report, String> {
    blocking(|| Ok(netdiag::run_manual())).await
}

/// Picks the fastest working node of the main exit's group (a selector); an automatic group is
/// only re-tested (it moves to a working node on its own).
fn switch_node(sh: &Shared) -> Result<String, String> {
    let (ctl, secret) = ctl_link(sh)?;
    let group = {
        let ex = sh.exit.lock().unwrap();
        ex.groups.iter().find(|g| g.is_default).and_then(|g| g.r.group.clone()).or_else(|| ex.proxy.as_ref().and_then(|e| e.group.clone()))
    }
    .ok_or(crate::i18n::t("还不知道当前用的是哪个代理分组"))?;
    let b = mihomo::get(&ctl, &format!("/proxies/{}", pct(&group)), secret.as_deref())?;
    let v: serde_json::Value = serde_json::from_slice(&b).map_err(|e| e.to_string())?;
    let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
    let now = v.get("now").and_then(|t| t.as_str()).unwrap_or("").to_string();
    let url = "https%3A%2F%2Fwww.gstatic.com%2Fgenerate_204";
    let d = mihomo::get(&ctl, &format!("/group/{}/delay?url={url}&timeout=3000", pct(&group)), secret.as_deref())?;
    let delays: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(&d).unwrap_or_default();
    if !kind.eq_ignore_ascii_case("Selector") {
        sh.exit_trigger.store(true, Ordering::SeqCst);
        return Ok(crate::tr!("{0} 是自动选择的分组，已重新测速，它会自己换到能用的节点", group));
    }
    let best = delays
        .iter()
        .filter_map(|(n, ms)| ms.as_u64().filter(|m| *m > 0).map(|m| (n.clone(), m)))
        .filter(|(n, _)| *n != now && n != "DIRECT" && !n.starts_with("REJECT"))
        .min_by_key(|(_, m)| *m)
        .ok_or(crate::i18n::t("这个分组里没有能用的节点了，试试在代理软件里更新订阅"))?;
    let body = serde_json::json!({ "name": best.0 }).to_string();
    mihomo::request(&ctl, "PUT", &format!("/proxies/{}", pct(&group)), Some(&body), secret.as_deref())?;
    // existing connections move over
    if let Ok(conns) = mihomo::connections(&ctl, secret.as_deref()) {
        for c in conns.iter().filter(|c| c.chains.iter().any(|x| *x == group)) {
            let _ = mihomo::request(&ctl, "DELETE", &format!("/connections/{}", c.id), None, secret.as_deref());
        }
    }
    log(format!("netdiag: {group} switched {now} -> {} ({} ms)", best.0, best.1));
    sh.exit_trigger.store(true, Ordering::SeqCst);
    Ok(crate::tr!("{0} 换到了 {1}（{2} ms）", group, best.0, best.1))
}

#[tauri::command]
async fn netdiag_fix(state: tauri::State<'_, Arc<Shared>>, id: String) -> Result<String, String> {
    let sh = state.inner().clone();
    blocking(move || match id.as_str() {
        "netsettings" => {
            netdiag::open_net_settings();
            Ok(String::new())
        }
        "flushdns" => netdiag::flush_dns().map(|_| crate::i18n::t("DNS 缓存已刷新").into()),
        "restore-sysproxy" => netdiag::restore_system_proxy().map(|_| crate::i18n::t("系统代理已恢复").into()),
        "ignore-sysproxy" => {
            netdiag::ignore_system_proxy();
            Ok(crate::i18n::t("好的，系统代理再打开之前不再提醒").into())
        }
        "switch-node" => switch_node(&sh),
        "restart-proxy" | "start-proxy" => {
            let list = procinfo::list_processes();
            if let Some(cp) = control::find_clash_party(&list) {
                let cores = proxy::client_name(&list).1;
                control::restart_party(&cp, &cores)?;
                sh.exit_trigger.store(true, Ordering::SeqCst);
                Ok(crate::i18n::t("代理软件已重启").into())
            } else {
                let exe = netdiag::client_exe().ok_or(crate::i18n::t("不知道代理软件装在哪里，请手动打开它"))?;
                // de-elevated through explorer, like a double click
                std::process::Command::new("explorer.exe").arg(&exe).spawn().map_err(|e| e.to_string())?;
                sh.exit_trigger.store(true, Ordering::SeqCst);
                Ok(crate::i18n::t("正在打开代理软件").into())
            }
        }
        _ => Err(crate::i18n::t("未知的操作").into()),
    })
    .await
}

// ---------- programs going online for the first time ----------

/// Processes of a program: the last snapshot's pids for it, plus any running process whose image
/// is that path.
fn pids_of(sh: &Shared, path: &str) -> HashSet<u32> {
    let mut pids: HashSet<u32> = sh
        .last
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| s.apps.iter().filter(|a| a.path.as_deref().map(|p| p.eq_ignore_ascii_case(path)).unwrap_or(false)).flat_map(|a| a.pids.clone()).collect())
        .unwrap_or_default();
    pids.extend(
        procinfo::list_processes()
            .into_iter()
            .filter(|(p, _)| procinfo::image_path(*p).map(|x| x.eq_ignore_ascii_case(path)).unwrap_or(false))
            .map(|(p, _)| p),
    );
    pids
}

/// Programs with a connection to a public address this round; new ones get a note.
fn first_net(apps: &[AppRow]) {
    let seen: Vec<(String, String, String)> = apps
        .iter()
        .filter(|a| !a.is_proxy)
        .filter_map(|a| {
            let path = a.path.clone()?;
            let c = a.conns.iter().find(|c| {
                let ip: Option<IpAddr> = c.remote.as_deref().and_then(|r| r.parse().ok());
                ip.map(|i| !geo::is_private(&i) && !is_loopback(&i)).unwrap_or(false) && (c.state == "ESTABLISHED" || c.node.is_some())
            })?;
            let what = c.host.clone().or_else(|| c.remote.clone()).unwrap_or_default();
            let dest = match &c.cc {
                Some(cc) if !cc.is_empty() => format!("{what} · {}", cc.to_uppercase()),
                _ => what,
            };
            Some((a.name.clone(), path, dest))
        })
        .collect();
    for k in firstnet::observe(&seen) {
        notify::push(notify::Note {
            key: "micabar-firstnet".into(),
            app: crate::i18n::t("新程序联网").into(),
            icon: Some(k.path.clone()),
            title: k.name.clone(),
            body: crate::tr!("第一次连上网 → {0}", k.dest),
            sub: k.path.clone(),
            kind: "firstnet".into(),
            source: "micabar".into(),
            actions: vec![
                notify::NoteAction { id: "allow".into(), label: crate::i18n::t("允许").into(), style: "plain".into() },
                notify::NoteAction { id: "block".into(), label: crate::i18n::t("阻止联网").into(), style: "danger".into() },
            ],
            data: k.path.clone(),
            ..Default::default()
        });
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FirstNetView {
    on: bool,
    learn_until: u64,
    apps: Vec<firstnet::Known>,
}

#[tauri::command]
fn firstnet_list() -> FirstNetView {
    FirstNetView { on: settings::bool_at("firstNet.on", true), learn_until: firstnet::learning_until(), apps: firstnet::recent() }
}

/// allow | block | unblock | dismiss one program (and settle its note's buttons).
#[tauri::command]
async fn firstnet_action(state: tauri::State<'_, Arc<Shared>>, path: String, action: String, note: Option<u64>) -> Result<String, String> {
    let sh = state.inner().clone();
    let r = blocking(move || {
        let exe = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string();
        let msg = match action.as_str() {
            "allow" => {
                firstnet::allow(&path);
                crate::tr!("已允许 {0} 联网", exe)
            }
            "dismiss" => {
                firstnet::dismiss(&path);
                String::new()
            }
            "block" => {
                if !sh.admin {
                    return Err(crate::i18n::t("改防火墙需要以管理员身份运行 MicaBar").into());
                }
                let rule = firstnet::firewall_block(&path)?;
                // a program on the system proxy reaches the internet through the core: a REJECT
                // rule there (exit control on) stops that part too (using full PROCESS-PATH)
                let core_rule = if sh.control.lock().unwrap().status == "active" {
                    (|| -> Result<String, String> {
                        let (ctl, secret) = ctl_link(&sh)?;
                        let home = sh.core_home.lock().unwrap().clone().ok_or(crate::i18n::t("还不知道内核目录"))?;
                        let mut next = sh.rules.lock().unwrap().clone();
                        let r = control::upsert(&mut next, "process", Some(path.clone()), None, "reject");
                        control::persist(&home, &next)?;
                        control::reload_providers(&ctl, secret.as_deref())?;
                        *sh.rules.lock().unwrap() = next.clone();
                        control::close_matching(&ctl, secret.as_deref(), &r);
                        sh.control.lock().unwrap().rules = next;
                        Ok(r.id)
                    })()
                    .unwrap_or_else(|e| {
                        log(format!("firstnet: core reject rule failed: {e}"));
                        String::new()
                    })
                } else {
                    String::new()
                };
                let pids = pids_of(&sh, &path);
                let n = guard::reset_connections(&pids);
                log(format!("firstnet: reset {n} connection(s) of pids {pids:?}"));
                firstnet::set_blocked(&path, true, &rule, &core_rule);
                let p = sh.proxy.lock().unwrap().clone();
                let mut msg = if core_rule.is_empty() && p.system_proxy.is_some() && p.tun.is_none() {
                    format!("warn:{}", crate::tr!("已阻止 {0} 直接联网；它若走系统代理，还要开启「出口控制」才能完全拦住", exe))
                } else {
                    crate::tr!("已阻止 {0} 联网", exe)
                };
                if let Some(w) = firstnet::firewall_warning() {
                    msg = format!("warn:{}", crate::tr!("已添加 {0} 的阻止规则，{1}", exe, w));
                }
                msg
            }
            "unblock" => {
                if !sh.admin {
                    return Err(crate::i18n::t("改防火墙需要以管理员身份运行 MicaBar").into());
                }
                let k = firstnet::get(&path).ok_or(crate::i18n::t("没有这个程序的记录"))?;
                if !k.rule.is_empty() {
                    firstnet::firewall_unblock(&k.rule, &k.path)?;
                }
                if !k.core_rule.is_empty() {
                    let removed = {
                        let mut rules = sh.rules.lock().unwrap();
                        let r = rules.iter().find(|r| r.id == k.core_rule).cloned();
                        rules.retain(|r| r.id != k.core_rule);
                        if let Some(home) = sh.core_home.lock().unwrap().clone() {
                            let _ = control::persist(&home, &rules);
                        } else {
                            let _ = control::save_rules(&rules);
                        }
                        r
                    };
                    sh.control.lock().unwrap().rules = sh.rules.lock().unwrap().clone();
                    if let Ok((ctl, secret)) = ctl_link(&sh) {
                        let _ = control::reload_providers(&ctl, secret.as_deref());
                        if let Some(r) = removed {
                            control::close_matching(&ctl, secret.as_deref(), &r);
                        }
                    }
                }
                firstnet::set_blocked(&path, false, "", "");
                crate::tr!("已恢复 {0} 联网", exe)
            }
            _ => return Err(crate::i18n::t("未知的操作").into()),
        };
        Ok(msg)
    })
    .await;
    if r.is_ok() {
        if let Some(id) = note {
            notify::settle(id);
        }
    }
    r
}

fn ai_proxy(sh: &Shared) -> Option<u16> {
    let fixed = settings::num_at("ai.proxyPort", 0.0);
    if (1.0..=65535.0).contains(&fixed) {
        return Some(fixed as u16);
    }
    sh.proxy.lock().unwrap().local_port
}

fn ai_every_ms() -> u64 {
    (settings::num_at("ai.interval", 10.0).clamp(1.0, 120.0) * 60_000.0) as u64
}

/// Asks only the tools that are due (each has its own clock) concurrently; the others keep their last numbers.
async fn ai_round(app: &AppHandle, sh: &Arc<Shared>, manual: bool) {
    let proxy = ai_proxy(sh);
    {
        // say once which port the quota queries go through
        static LAST: Mutex<Option<Option<u16>>> = Mutex::new(None);
        let mut l = LAST.lock().unwrap();
        if *l != Some(proxy) {
            *l = Some(proxy);
            log(match proxy {
                Some(p) => format!("ai: queries go through the proxy at 127.0.0.1:{p}"),
                None => "ai: no proxy port found; online quota queries paused (never direct)".to_string(),
            });
        }
    }
    let prev = sh.ai.lock().unwrap().clone();
    let every = ai_every_ms();
    enum Slot {
        Cached(Option<ai::Quota>),
        Pending(&'static str, Option<ai::Quota>, tauri::async_runtime::JoinHandle<ai::Quota>),
    }
    let mut slots = Vec::with_capacity(ai::TOOLS.len());
    let mut asked = false;
    for (tool, _) in ai::TOOLS {
        if !settings::bool_at(&format!("ai.{tool}"), true) {
            continue;
        }
        let p = prev.iter().find(|p| p.tool == tool).cloned().or_else(|| ai::known_quota(tool));
        if !ai::due(tool, every, manual) {
            slots.push(Slot::Cached(p));
            continue;
        }
        asked = true;
        let p_clone = p.clone();
        let h = tauri::async_runtime::spawn(async move { ai::query(tool, proxy, p_clone.as_ref()).await });
        slots.push(Slot::Pending(tool, p, h));
    }
    let mut out = Vec::with_capacity(slots.len());
    for slot in slots {
        match slot {
            Slot::Cached(p) => out.extend(p),
            Slot::Pending(tool, p, h) => {
                if let Ok(q) = h.await {
                    let same = p.as_ref().map(|p| p.status == q.status && p.message == q.message).unwrap_or(false);
                    if q.status != "ok" && !same {
                        log(format!("ai {tool}: {} {}", q.status, q.message.clone().unwrap_or_default()));
                    }
                    out.push(q);
                } else {
                    out.extend(p);
                }
            }
        }
    }
    let changed = asked || out.len() != prev.len();
    *sh.ai.lock().unwrap() = out.clone();
    if asked {
        ai::save_cache();
    }
    if changed {
        let _ = app.emit("ai", out);
    }
}

fn ai_loop(app: AppHandle, sh: Arc<Shared>) {
    tauri::async_runtime::spawn(async move {
        loop {
            let ready = sh.last.lock().unwrap().is_some();
            tokio::time::sleep(if ready { Duration::from_secs(2) } else { Duration::from_millis(200) }).await;
            if !settings::bool_at("ai.on", true) {
                if !sh.ai.lock().unwrap().is_empty() {
                    sh.ai.lock().unwrap().clear();
                    let _ = app.emit("ai", Vec::<ai::Quota>::new());
                }
                continue;
            }
            // wait for the first snapshot: until then the proxy client isn't detected yet and the
            // services (blocked here without it) would be asked directly
            if !ready {
                continue;
            }
            let manual = sh.ai_trigger.swap(false, Ordering::SeqCst);
            ai_round(&app, &sh, manual).await;
        }
    });
}

#[tauri::command]
fn ai_get(state: tauri::State<'_, Arc<Shared>>) -> Vec<ai::Quota> {
    state.ai.lock().unwrap().clone()
}

static AUDIO_WANT: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn set_audio_viz_for(app: &AppHandle, label: &str, on: bool) {
    let any = {
        let mut w = AUDIO_WANT.lock().unwrap();
        w.retain(|l| l != label && app.get_webview_window(l).is_some());
        if on && app.get_webview_window(label).is_some() {
            w.push(label.to_string());
        }
        !w.is_empty()
    };
    let app2 = app.clone();
    audio::set_wanted(any, move |b| {
        let _ = app2.emit("viz", b);
    });
}

/// Music bars on or off (a page shows them while a song plays); bands arrive as "viz" events.
#[tauri::command]
fn audio_viz(app: AppHandle, window: tauri::Window, on: bool) {
    // the island and the desktop window each say whether they show bars; on while either does
    set_audio_viz_for(&app, window.label(), on);
}

/// Ends all processes of one app listed on the system monitor page: (ended, failed).
#[tauri::command]
async fn sysmon_end(name: String) -> (u32, u32) {
    tauri::async_runtime::spawn_blocking(move || sysmon::end_app(&name)).await.unwrap_or((0, 0))
}

/// Programs that start with Windows (Run keys + Startup folders) for the system monitor page.
#[tauri::command]
async fn startup_list() -> Vec<startup::Item> {
    tauri::async_runtime::spawn_blocking(startup::list).await.unwrap_or_default()
}

/// Turns one startup entry on or off (Task Manager's StartupApproved flag; nothing is deleted).
#[tauri::command]
async fn startup_set(id: String, on: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || startup::set(&id, on)).await.map_err(|e| e.to_string())?
}

/// One sample for the system monitor page (the page polls this while it is open).
#[tauri::command]
async fn sysmon_sample() -> sysmon::Sample {
    tauri::async_runtime::spawn_blocking(|| sysmon::monitor().lock().unwrap().sample()).await.unwrap_or_default()
}

/// Manual refresh; returns the tools that have to wait (Claude: 10 minutes between queries) as
/// "Claude 10:06" so the page can say so.
#[tauri::command]
fn ai_refresh(state: tauri::State<'_, Arc<Shared>>) -> Vec<String> {
    if !settings::bool_at("ai.on", true) {
        return Vec::new();
    }
    state.ai_trigger.store(true, Ordering::SeqCst);
    log("ai: manual refresh");
    let every = ai_every_ms();
    ai::TOOLS
        .iter()
        .filter(|(t, _)| settings::bool_at(&format!("ai.{t}"), true) && !ai::due(t, every, true))
        .map(|(t, n)| format!("{n} {}", fmt_time(ai::next_at(t, every, true)).get(..5).unwrap_or("")))
        .collect()
}

#[tauri::command]
fn media_get(state: tauri::State<'_, Arc<Shared>>) -> Option<media::NowPlaying> {
    state.media.lock().unwrap().clone()
}

#[tauri::command]
async fn media_control(action: String) -> Result<(), String> {
    blocking(move || media::control(&action)).await
}

// ---------- clipboard history ----------
// all async: the store may be busy writing an encrypted picture, and sync commands run on the
// main thread

#[tauri::command]
async fn clip_list(query: Option<String>) -> Result<Vec<clipboard::View>, String> {
    blocking(move || Ok(clipboard::list_filtered(query.as_deref()))).await
}

#[tauri::command]
async fn clip_copy(id: u64) -> Result<(), String> {
    blocking(move || clipboard::copy(id)).await
}

#[tauri::command]
async fn clip_pin(id: u64, on: bool) -> Result<(), String> {
    blocking(move || Ok(clipboard::pin(id, on))).await
}

#[tauri::command]
async fn clip_delete(id: u64) -> Result<(), String> {
    blocking(move || Ok(clipboard::delete(id))).await
}

#[tauri::command]
async fn clip_clear(all: bool) -> Result<(), String> {
    blocking(move || Ok(clipboard::clear(all))).await
}

// ---------- manager window / layout editing ----------

/// Opens (or focuses) the desktop manager window on a page.
fn open_manager_window(app: &AppHandle, page: &str) {
    if let Some(w) = app.get_webview_window("manager") {
        // timed: opening it lagged once while a game was running (2026-11-22)
        let t = std::time::Instant::now();
        sync_manager_titlebar(app);
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        let _ = app.emit_to("manager", "goto", page.to_string());
        log(format!("manager window shown ({page}) in {} ms, minimized={:?}", t.elapsed().as_millis(), w.is_minimized().ok()));
        return;
    }
    let s = settings::get();
    let g = s.pointer("/manager").cloned().unwrap_or_default();
    let num = |k: &str, d: f64| g.get(k).and_then(|v| v.as_f64()).unwrap_or(d);
    let dark = is_dark_theme();
    let url = tauri::WebviewUrl::App(format!("manager.html#{page}").into());
    let b = tauri::WebviewWindowBuilder::new(app, "manager", url)
        .title("MicaBar")
        .inner_size(num("w", 1120.0).max(820.0), num("h", 740.0).max(560.0))
        .min_inner_size(820.0, 560.0)
        .resizable(true)
        .theme(Some(if dark { tauri::Theme::Dark } else { tauri::Theme::Light }))
        .background_color(manager_bg_color(dark))
        // same WebView2 environment as the island window (must match), and keep painting while
        // Chromium thinks the window is covered (the island window overlaps it)
        .additional_browser_args("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,CalculateNativeWinOcclusion")
        .decorations(true)
        // created hidden and shown below: showing an existing window also makes its webview visible,
        // otherwise the page only repaints after mouse input
        .visible(false)
        .center();
    let saved_px = g.get("px").and_then(|v| v.as_i64()).zip(g.get("py").and_then(|v| v.as_i64())).and_then(|(px, py)| {
        let (px, py) = (px as i32, py as i32);
        let on_screen = window_ctl::monitor_list()
            .iter()
            .any(|m| px >= m.x - 50 && py >= m.y - 10 && px < m.x + m.w - 100 && py < m.y + m.h - 100);
        on_screen.then_some((px, py))
    });
    match b.build() {
        Ok(w) => {
            if let Some((px, py)) = saved_px {
                let _ = w.set_position(tauri::PhysicalPosition::new(px, py));
            }
            if let Ok(hwnd) = w.hwnd() {
                system::set_dark_titlebar(hwnd.0 as isize, dark);
            }
            if let Some(icon) = app.default_window_icon() {
                let _ = w.set_icon(icon.clone());
            }
            let _ = w.show();
            if let Ok(hwnd) = w.hwnd() {
                // Re-apply after show() so Win10 DWM refreshes the visible non-client frame too
                system::set_dark_titlebar(hwnd.0 as isize, dark);
            }
            let _ = w.set_focus();
            let w2 = w.clone();
            let app_ev = app.clone();
            w.on_window_event(move |ev| match ev {
                tauri::WindowEvent::CloseRequested { .. } => {
                    set_audio_viz_for(&app_ev, "manager", false);
                    let sf = w2.scale_factor().unwrap_or(1.0);
                    if let (Ok(p), Ok(sz), Ok(false)) = (w2.outer_position(), w2.inner_size(), w2.is_minimized()) {
                        settings::update(&serde_json::json!({ "manager": {
                            "px": p.x, "py": p.y,
                            "x": p.x as f64 / sf, "y": p.y as f64 / sf,
                            "w": sz.width as f64 / sf, "h": sz.height as f64 / sf
                        } }));
                    }
                }
                tauri::WindowEvent::Destroyed => {
                    set_audio_viz_for(&app_ev, "manager", false);
                }
                _ => {}
            });
            log(format!("manager window opened ({page})"));
        }
        Err(e) => log(format!("manager window failed: {e}")),
    }
}

/// async: on Windows, creating a webview window inside a synchronous command deadlocks it
#[tauri::command]
async fn open_manager(app: AppHandle, state: tauri::State<'_, Arc<Shared>>, page: Option<String>) -> Result<(), String> {
    // collapse the island first: collapsing hands focus back to the previous app, and a webview
    // window created during that hand-over doesn't repaint until it gets mouse input
    if state.ws.expanded.load(Ordering::SeqCst) {
        state.ws.collapse_req.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    // build it on the main (event loop) thread like the tray does; a webview created from a
    // command thread here only repainted after mouse input
    let app2 = app.clone();
    let page = page.unwrap_or_else(|| "home".into());
    app.run_on_main_thread(move || open_manager_window(&app2, &page)).map_err(|e| e.to_string())?;
    Ok(())
}

/// Long press grabbed the island: it follows the pointer along the edge, so the window spans
/// the whole edge and takes all input until `grab_end`.
#[tauri::command]
fn grab_start(app: AppHandle, state: tauri::State<'_, Arc<Shared>>) {
    state.ws.grab.store(true, Ordering::SeqCst);
    state.ws.free.store(false, Ordering::SeqCst);
    state.ws.expanded.store(false, Ordering::SeqCst);
    window_ctl::set_mem_normal(&app, &state.ws);
    if let Some(w) = app.get_webview_window("main") {
        window_ctl::span_edge(&w);
    }
}

/// The grabbed island was pulled away from its edge: the window covers the whole monitor until
/// `grab_end`, so it can be carried to another edge.
#[tauri::command]
fn grab_free(app: AppHandle, state: tauri::State<'_, Arc<Shared>>) {
    if !state.ws.grab.load(Ordering::SeqCst) || state.ws.free.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(w) = app.get_webview_window("main") {
        window_ctl::span_monitor(&w);
    }
}

/// The flung island came to rest at `anchor` (start | center | end), on edge `dock` when it was
/// carried to another one: remember it; the page then refits the window, which `place_window`
/// puts at that spot.
#[tauri::command]
fn grab_end(app: AppHandle, state: tauri::State<'_, Arc<Shared>>, anchor: String, dock: Option<String>) {
    let anchor = if matches!(anchor.as_str(), "start" | "end") { anchor } else { "center".into() };
    let mut shape = serde_json::json!({ "anchor": anchor });
    if let Some(d) = dock.filter(|d| matches!(d.as_str(), "top" | "bottom" | "left" | "right")) {
        shape["dock"] = serde_json::Value::String(d);
    }
    let s = settings::update(&serde_json::json!({ "shape": shape }));
    state.ws.grab.store(false, Ordering::SeqCst);
    state.ws.free.store(false, Ordering::SeqCst);
    state.ws.disarm.store(true, Ordering::SeqCst);
    if !state.ws.expanded.load(Ordering::SeqCst) && !state.ws.edit.load(Ordering::SeqCst) {
        window_ctl::schedule_mem_low(&state.ws, 2000, false);
    }
    log(format!(
        "island flung to {} {}",
        s.pointer("/shape/dock").and_then(|v| v.as_str()).unwrap_or(""),
        s.pointer("/shape/anchor").and_then(|v| v.as_str()).unwrap_or("")
    ));
    let _ = app.emit("settings", &s);
}

/// Layout editing: the island window takes all mouse input (drag handles) until done.
#[tauri::command]
fn layout_edit(app: AppHandle, state: tauri::State<'_, Arc<Shared>>, on: bool) {
    state.ws.edit.store(on, Ordering::SeqCst);
    if on {
        window_ctl::set_mem_normal(&app, &state.ws);
        state.ws.collapse_req.store(true, Ordering::SeqCst);
    } else if !state.ws.expanded.load(Ordering::SeqCst) && !state.ws.grab.load(Ordering::SeqCst) {
        window_ctl::schedule_mem_low(&state.ws, 2000, false);
    }
    let _ = app.emit_to("main", "layout-edit", on);
    log(format!("layout edit {}", if on { "on" } else { "off" }));
}

/// The first-run tour: `on` while it runs; `open` asks the island to open now.
#[tauri::command]
fn tour_mode(state: tauri::State<'_, Arc<Shared>>, on: bool, open: bool) {
    state.ws.tour.store(on, Ordering::SeqCst);
    if open {
        state.ws.open_req.store(true, Ordering::SeqCst);
    }
    log(format!("tour {}", if on { "on" } else { "off" }));
}

/// "重新看教程" in the desktop window: the island starts the tour again.
#[tauri::command]
fn tour_restart(app: AppHandle) {
    let _ = app.emit_to("main", "tour-start", true);
}

#[tauri::command]
fn pill_click(state: tauri::State<'_, Arc<Shared>>) {
    state.ws.click_req.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn collapse_island(state: tauri::State<'_, Arc<Shared>>) {
    state.ws.collapse_req.store(true, Ordering::SeqCst);
}

/// The page hands over a channel; blurred backdrop frames are pushed into it as raw bytes.
#[tauri::command]
fn start_glass(state: tauri::State<'_, Arc<Shared>>, on_frame: Channel<InvokeResponseBody>) {
    glass::set_channel(&state.glass, Some(on_frame));
}

#[tauri::command]
fn refresh_exit(state: tauri::State<'_, Arc<Shared>>) {
    state.exit_trigger.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn get_snapshot(state: tauri::State<'_, Arc<Shared>>) -> Option<Arc<Snapshot>> {
    state.last.lock().unwrap().clone()
}

/// The page asks for a window size in physical pixels (its CSS size × devicePixelRatio).
#[tauri::command]
fn fit_window(app: AppHandle, state: tauri::State<'_, Arc<Shared>>, w: u32, h: u32) {
    if w == 0 || h == 0 {
        return;
    }
    let Some(win) = app.get_webview_window("main") else { return };
    if let Ok(hwnd) = win.hwnd() {
        if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsIconic(hwnd.0 as _) } != 0 {
            return;
        }
    }
    let cur = win.outer_size().ok();
    let resized = cur.map(|s| s.width != w || s.height != h).unwrap_or(true);
    if resized {
        log(format!("fit_window {w}x{h} (was {cur:?})"));
        let _ = win.set_size(tauri::PhysicalSize::new(w, h));
    }
    window_ctl::place_window(&win);
    if resized {
        window_ctl::schedule_mem_pulse(&state.ws, 1500);
    }
}

#[tauri::command]
fn log_js(msg: String) {
    log(format!("js: {msg}"));
}

#[tauri::command]
fn quit(app: AppHandle) {
    speed::stop();
    app.exit(0);
}

/// Builds the collapsed-pill snapshot without deep-cloning `conns`, `listen`, `pids`, `alerts`, or `rules`.
fn slim_snapshot(full: &Snapshot) -> Snapshot {
    Snapshot {
        ts: full.ts,
        slim: true,
        admin: full.admin,
        speed_ok: full.speed_ok,
        speed_error: full.speed_error.clone(),
        geo_source: full.geo_source.clone(),
        conn_count: full.conn_count,
        listen_count: full.listen_count,
        hidden_loopback: full.hidden_loopback,
        up: full.up,
        down: full.down,
        apps: full
            .apps
            .iter()
            .map(|a| AppRow {
                key: a.key.clone(),
                name: a.name.clone(),
                exe: a.exe.clone(),
                path: a.path.clone(),
                icon: a.icon.clone(),
                pids: Vec::new(),
                up: a.up,
                down: a.down,
                is_proxy: a.is_proxy,
                via_proxy: a.via_proxy,
                conns: Vec::new(),
                listen: Vec::new(),
            })
            .collect(),
        proxy: full.proxy.clone(),
        exit: full.exit.clone(),
        control: control::ControlState {
            status: full.control.status.clone(),
            message: full.control.message.clone(),
            client: full.control.client.clone(),
            rules: Vec::new(),
            node: full.control.node.clone(),
        },
        guard: GuardView {
            settings: full.guard.settings.clone(),
            sources: full.guard.sources.clone(),
            total: full.guard.total,
            alerts: Vec::new(),
            unacked: full.guard.unacked,
            tun: full.guard.tun,
        },
        sys: full.sys.clone(),
        latency: full.latency.clone(),
        hotkey_ok: full.hotkey_ok,
        maximized: full.maximized,
        animations: full.animations,
    }
}

fn single_instance() -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name = procinfo::wide("Local\\MicaBar-single-instance");
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if h.is_null() {
            return true;
        }
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// Data of the app's earlier name: %LOCALAPPDATA%\NetIsland -> %LOCALAPPDATA%\MicaBar (settings,
/// rules, traffic, guard library), once, before anything writes the new folder.
fn migrate_old_data() {
    let Ok(base) = std::env::var("LOCALAPPDATA") else { return };
    let (old, new) = (std::path::Path::new(&base).join("NetIsland"), std::path::Path::new(&base).join("MicaBar"));
    if old.is_dir() && !new.exists() {
        let ok = std::fs::rename(&old, &new).is_ok();
        if ok {
            let _ = std::fs::rename(new.join("netisland.log"), new.join("micabar.log"));
        }
        log(format!("migrated data from {} ({})", old.display(), if ok { "ok" } else { "failed" }));
    }
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let th = std::thread::current();
        let name = th.name().unwrap_or("unnamed");
        let loc = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_else(|| "unknown".into());
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("panic");
        log(format!("PANIC on thread {name} at {loc}: {msg}"));
    }));
    if !single_instance() {
        return;
    }
    migrate_old_data();
    i18n::set_lang(&i18n::wanted());
    let admin = is_admin();
    log(format!("---- MicaBar {} start, admin={admin}", env!("CARGO_PKG_VERSION")));
    // Start ETW session setup in parallel with disk/DB warm-up & Tauri window creation
    let etw_init = Mutex::new(admin.then(|| std::thread::spawn(speed::start)));
    let (geo, guard_settings, matcher, rules, traffic_book, ai_cached) = std::thread::scope(|s| {
        let g = s.spawn(Geo::load);
        let m = s.spawn(|| {
            let gs = guard::load_settings();
            let mb = guard::Matcher::build(&gs);
            (gs, mb)
        });
        let r = s.spawn(control::load_rules);
        let tb = s.spawn(traffic::Book::new);
        let ac = s.spawn(ai::load_cache);
        guard::register_toast_identity(include_bytes!("../icons/128x128.png"));
        let geo = g.join().unwrap_or_else(|_| Geo::load());
        let (gs, mb) = m.join().unwrap_or_else(|_| {
            let gs = guard::load_settings();
            let mb = guard::Matcher::build(&gs);
            (gs, mb)
        });
        (
            geo,
            gs,
            mb,
            r.join().unwrap_or_default(),
            tb.join().unwrap_or_default(),
            ac.join().unwrap_or_default(),
        )
    });
    log(format!("geo db: {:?}", geo.source));
    let shared = Arc::new(Shared {
        geo,
        proxy: Mutex::new(ProxyState::default()),
        exit: Mutex::new(ExitState::default()),
        last: Mutex::new(None),
        mihomo: Mutex::new(MihomoLink { ctl: None, secret: None, last_try: None }),
        ws: Arc::new(WinState::default()),
        glass: Arc::new(Mutex::new(None)),
        control: Mutex::new(control::ControlState { status: "unavailable".into(), ..Default::default() }),
        rules: Mutex::new(rules),
        core_home: Mutex::new(None),
        node_pending: AtomicBool::new(false),
        matcher: std::sync::RwLock::new(matcher),
        guard: Mutex::new(guard_settings),
        alerts: Mutex::new(AlertBook::default()),
        latency: Mutex::new(Latency::default()),
        latency_history: Mutex::new(std::collections::VecDeque::new()),
        traffic: Mutex::new(traffic_book),
        group_score: Mutex::new(HashMap::new()),
        ai: Mutex::new(ai_cached),
        ai_trigger: AtomicBool::new(false),
        media: Arc::new(Mutex::new(None)),
        exit_trigger: AtomicBool::new(false),
        admin,
    });
    let _ = GLOBAL_SHARED.set(shared.clone());

    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(shared.clone())
        .manage(shared.ws.clone())
        .invoke_handler(tauri::generate_handler![
            get_icon,
            set_hit_rects,
            pill_click,
            collapse_island,
            start_glass,
            control_enable,
            control_disable,
            control_set,
            control_remove,
            control_nodes,
            control_select,
            guard_options,
            guard_add,
            guard_remove,
            guard_update_gfw,
            guard_import,
            guard_import_set,
            guard_block,
            guard_unblock,
            alert_ack,
            alert_clear,
            settings_get,
            settings_set,
            ui_lang,
            settings_reset,
            monitors_list,
            hotkey_check,
            glass_off,
            set_zoom,
            fonts_list,
            app_info,
            update_check,
            update_install,
            open_url,
            traffic_query,
            exit_group_options,
            ai_get,
            ai_refresh,
            sysmon_sample,
            sysmon_end,
            startup_list,
            startup_set,
            audio_viz,
            media_get,
            media_control,
            open_manager,
            notify_list,
            notify_open,
            notify_dismiss,
            notify_read,
            notify_apps,
            notify_takeover,
            notify_hooks,
            notify_hook,
            notify_test,
            note_mode,
            layout_edit,
            tour_mode,
            tour_restart,
            agents_get,
            agents_focus,
            netdiag_get,
            netdiag_run,
            netdiag_fix,
            firstnet_list,
            firstnet_action,
            clip_list,
            clip_copy,
            clip_pin,
            clip_delete,
            clip_clear,
            grab_start,
            grab_free,
            grab_end,
            latency_history,
            latency_probe,
            refresh_exit,
            get_snapshot,
            fit_window,
            log_js,
            quit
        ])
        .setup(move |app| {
            let handle = app.handle().clone();

            let menu = tray_menu(app.handle())?;
            let sh_tray = shared.clone();
            let sh_tray2 = shared.clone();
            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("MicaBar")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(move |tray, ev| {
                    // left click: the manager window (right click opens the menu)
                    if let tauri::tray::TrayIconEvent::Click { button: tauri::tray::MouseButton::Left, button_state: tauri::tray::MouseButtonState::Up, .. } = ev {
                        sh_tray2.ws.collapse_req.store(true, Ordering::SeqCst);
                        open_manager_window(tray.app_handle(), "home");
                    }
                })
                .on_menu_event(move |app, ev| match ev.id.as_ref() {
                "open" => open_manager_window(app, "home"),
                "settings" => open_manager_window(app, "settings"),
                "layout" => {
                    sh_tray.ws.edit.store(true, Ordering::SeqCst);
                    window_ctl::set_mem_normal(app, &sh_tray.ws);
                    sh_tray.ws.collapse_req.store(true, Ordering::SeqCst);
                    let _ = app.emit_to("main", "layout-edit", true);
                }
                "refresh" => sh_tray.exit_trigger.store(true, Ordering::SeqCst),
                "log" => {
                    let _ = std::process::Command::new("notepad.exe").arg(log_path()).spawn();
                }
                "quit" => {
                    speed::stop();
                    app.exit(0);
                }
                id if id.starts_with("theme:") => {
                    let t = match id.trim_start_matches("theme:") {
                        "light" => "glass-light",
                        "dark" => "glass-dark",
                        _ => "auto",
                    };
                    let s = settings::update(&serde_json::json!({ "appearance": { "theme": t } }));
                    sync_manager_titlebar(app);
                    let _ = app.emit("settings", &s);
                }
                _ => {}
            });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            // icons left behind by an earlier MicaBar that was killed instead of quit
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(1500));
                log(format!("tray sweep: {} toolbar(s)", system::sweep_tray()));
            });
            ai_loop(handle.clone(), shared.clone());
            notify::start(handle.clone());
            clipboard::start(handle.clone());
            perf::start(handle.clone(), shared.ws.clone(), shared.glass.clone());
            let sh = shared.clone();
            netdiag::start(Arc::new(move || diag_info(&sh)));
            let sh = shared.clone();
            std::thread::spawn(move || {
                let history = traffic::all_programs(&sh.traffic.lock().unwrap());
                firstnet::init(history);
            });
            let h_media = handle.clone();
            media::start(shared.media.clone(), move |np| {
                let _ = h_media.emit("media", np);
            });

            if let Some(w) = app.get_webview_window("main") {
                let hwnd = w.hwnd().ok().map(|h| h.0 as isize);
                if let Some(h) = hwnd {
                    window_ctl::disable_dwm_transitions(h);
                }
                window_ctl::place_window(&w);
                let _ = w.show();
                if let Some(h) = hwnd {
                    window_ctl::configure_island_window_initial(h);
                    let ok = glass::exclude_from_dda(h);
                    log(format!("exclude window from desktop duplication: {ok}"));
                }
                log(format!(
                    "window outer={:?} inner={:?} pos={:?} scale={:?}",
                    w.outer_size().ok(),
                    w.inner_size().ok(),
                    w.outer_position().ok(),
                    w.scale_factor().ok()
                ));
            }

            // collector thread
            proxy::start_ip_watcher(on_network_change);
            let etw_handle = etw_init.lock().unwrap().take();
            let sh = shared.clone();
            let h = handle.clone();
            std::thread::spawn(move || {
                let mut etw_pending = etw_handle;
                let mut col = Collector::new_warming(sh.admin);
                fn attach_ready_etw(
                    etw_pending: &mut Option<std::thread::JoinHandle<Result<speed::Counters, String>>>,
                    col: &mut Collector,
                    wait: bool,
                ) {
                    let ready = etw_pending.as_ref().map(|jh| wait || jh.is_finished()).unwrap_or(false);
                    if ready {
                        if let Some(jh) = etw_pending.take() {
                            let res = jh.join().unwrap_or_else(|_| Err("ETW thread panicked".into()));
                            match &res {
                                Ok(_) => log("ETW kernel network session started"),
                                Err(e) => log(format!("ETW unavailable: {e}")),
                            }
                            col.attach_counters(res);
                        }
                    }
                }
                // Attach immediately if ETW already finished during Tauri setup; otherwise publish
                // the very first snapshot right away and attach ETW right after tick 1!
                attach_ready_etw(&mut etw_pending, &mut col, false);
                sh.exit_trigger.store(true, Ordering::SeqCst);
                let (wake_lock, wake_cvar) = collector_wake();
                loop {
                    let t = Instant::now();
                    crate::perf::tick("collector");
                    attach_ready_etw(&mut etw_pending, &mut col, false);
                    let snap = Arc::new(col.tick(&sh));
                    let mgr_visible = h
                        .get_webview_window("manager")
                        .map(|w| !w.is_minimized().unwrap_or(false))
                        .unwrap_or(false);
                    let expanded = sh.ws.expanded.load(Ordering::SeqCst);
                    let in_game = sh.ws.game.load(Ordering::SeqCst);
                    let hidden = sh.ws.hidden.load(Ordering::SeqCst);
                    let need_full = expanded || mgr_visible;
                    if need_full {
                        let _ = h.emit("snapshot", &*snap);
                    } else if !hidden {
                        let slim = slim_snapshot(&snap);
                        let _ = h.emit("snapshot", &slim);
                    }
                    *sh.last.lock().unwrap() = Some(snap);
                    crate::perf::count("snapshots", 1);
                    if etw_pending.is_some() {
                        attach_ready_etw(&mut etw_pending, &mut col, true);
                    }
                    let spent = t.elapsed();
                    let game_idle = in_game && !expanded && !mgr_visible;
                    let period = collector_period(settings::hot().profile, game_idle);
                    let wait = period.saturating_sub(spent);
                    if !wait.is_zero() {
                        let mut dirty = wake_lock.lock().unwrap();
                        if !*dirty {
                            let (g, _) = wake_cvar.wait_timeout(dirty, wait).unwrap();
                            dirty = g;
                        }
                        if *dirty {
                            *dirty = false;
                            drop(dirty);
                            // Coalesce bursty adapter/IP callbacks over 120ms so DHCP settles into one tick
                            std::thread::sleep(Duration::from_millis(120));
                        }
                    }
                }
            });

            // exit IP & background maintenance scheduler (fast boot, coalesced runtime triggers, smart retry)
            let sh = shared.clone();
            let h = handle.clone();
            tauri::async_runtime::spawn(async move {
                let started = Instant::now();
                let mut last: Option<Instant> = None;
                let mut retry_at: Option<Instant> = None;
                let mut fail_streak: u32 = 0;
                let mut gfw_try: Option<Instant> = None;
                let mut lat_at: Option<Instant> = None;
                let mut update_at: Option<Instant> = None;
                let update_busy = Arc::new(AtomicBool::new(false));
                let lat_busy = Arc::new(AtomicBool::new(false));
                let gfw_busy = Arc::new(AtomicBool::new(false));
                let exit_busy = Arc::new(AtomicBool::new(false));
                let exit_result_flag = Arc::new(Mutex::new(None::<bool>));
                loop {
                    tokio_sleep(Duration::from_millis(200)).await;
                    if let Some(all_ok) = exit_result_flag.lock().unwrap().take() {
                        if all_ok {
                            fail_streak = 0;
                            retry_at = None;
                        } else {
                            fail_streak = fail_streak.saturating_add(1);
                            let secs = match fail_streak {
                                1 => 5,
                                2 => 12,
                                3 => 25,
                                _ => 60,
                            };
                            retry_at = Some(Instant::now() + Duration::from_secs(secs));
                        }
                    }
                    let update_due = update_at.map(|t| t.elapsed() > Duration::from_secs(24 * 3600)).unwrap_or(started.elapsed() > Duration::from_secs(60));
                    if settings::bool_at("general.autoUpdate", true) && update_due && !update_busy.swap(true, Ordering::SeqCst) {
                        update_at = Some(Instant::now());
                        let h2 = h.clone();
                        let ub = update_busy.clone();
                        tauri::async_runtime::spawn(async move {
                            match check_update(&h2).await {
                                Ok(u) => {
                                    log(format!("auto update check: available={} {:?}", u.available, u.version));
                                    if u.available {
                                        let _ = h2.emit("update", &u);
                                    }
                                }
                                Err(e) => log(format!("auto update check failed: {e}")),
                            }
                            ub.store(false, Ordering::SeqCst);
                        });
                    }
                    let every = Duration::from_secs(profile_secs(60, 30, 15));
                    let exit_ready = sh.exit.lock().unwrap().last_check > 0;
                    if latency_wanted()
                        && lat_at.map(|t| t.elapsed() >= every).unwrap_or(exit_ready || started.elapsed() > Duration::from_secs(8))
                        && !lat_busy.swap(true, Ordering::SeqCst)
                    {
                        lat_at = Some(Instant::now());
                        let sh2 = sh.clone();
                        let lb = lat_busy.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = tauri::async_runtime::spawn_blocking(move || probe_latency(&sh2)).await;
                            lb.store(false, Ordering::SeqCst);
                        });
                    }
                    let (on, updated) = {
                        let s = sh.guard.lock().unwrap();
                        (s.gfw, s.gfw_updated)
                    };
                    let stale = now_ms().saturating_sub(updated) > 24 * 3600_000;
                    let may_try = gfw_try.map(|t| t.elapsed() > Duration::from_secs(1800)).unwrap_or(true);
                    if on && stale && may_try && started.elapsed() > Duration::from_secs(15) && !gfw_busy.swap(true, Ordering::SeqCst) {
                        gfw_try = Some(Instant::now());
                        let sh2 = sh.clone();
                        let gb = gfw_busy.clone();
                        tauri::async_runtime::spawn(async move {
                            if update_gfw(sh2.clone()).await.is_ok() {
                                let _ = blocking(move || apply_guard(&sh2)).await;
                            }
                            gb.store(false, Ordering::SeqCst);
                        });
                    }
                    // Wait for the very first collector snapshot (~20ms) so proxy/TUN state is populated before probing
                    if sh.last.lock().unwrap().is_none() {
                        continue;
                    }
                    if !exit_busy.load(Ordering::SeqCst) {
                        let initial = last.is_none();
                        let periodic = last.map(|t| t.elapsed() >= Duration::from_secs(300)).unwrap_or(true);
                        let retry_due = retry_at.map(|t| Instant::now() >= t).unwrap_or(false);
                        let triggered = sh.exit_trigger.swap(false, Ordering::SeqCst);
                        if (triggered || periodic || retry_due) && !exit_busy.swap(true, Ordering::SeqCst) {
                            last = Some(Instant::now());
                            retry_at = None;
                            let h2 = h.clone();
                            let sh2 = sh.clone();
                            let eb = exit_busy.clone();
                            let rf = exit_result_flag.clone();
                            tauri::async_runtime::spawn(async move {
                                // Zero extra wait on cold start; 350ms coalesce on runtime network/proxy switches
                                if !initial && triggered {
                                    tokio_sleep(Duration::from_millis(350)).await;
                                    sh2.exit_trigger.store(false, Ordering::SeqCst);
                                }
                                let ok = exit_check(h2, sh2).await;
                                *rf.lock().unwrap() = Some(ok);
                                eb.store(false, Ordering::SeqCst);
                            });
                        }
                    }
                }
            });

            // start with Windows: the task must point at this exe (after updates / moves)
            std::thread::spawn(|| system::sync_autostart(settings::bool_at("general.autostart", false)));

            // hover / click-through / topmost / fullscreen, hotkey, glass backdrop
            let ws = shared.ws.clone();
            let h = handle.clone();
            std::thread::spawn(move || window_ctl::hover_loop(h, ws));
            window_ctl::hotkey_thread(shared.ws.clone());

            // watchdog: monotonic Instant (immune to sleep/NTP jumps) + in-flight guard (no queue flood)
            let h = handle.clone();
            std::thread::spawn(move || {
                let origin = Instant::now();
                let mono_ms = || origin.elapsed().as_millis() as u64;
                let beat = Arc::new(std::sync::atomic::AtomicU64::new(0));
                let inflight = Arc::new(AtomicBool::new(false));
                let mut since: Option<u64> = None;
                loop {
                    std::thread::sleep(Duration::from_millis(1000));
                    if !inflight.swap(true, Ordering::SeqCst) {
                        let b = beat.clone();
                        let inf = inflight.clone();
                        let o = origin;
                        if h.run_on_main_thread(move || {
                            b.store(o.elapsed().as_millis() as u64, Ordering::SeqCst);
                            inf.store(false, Ordering::SeqCst);
                        })
                        .is_err()
                        {
                            inflight.store(false, Ordering::SeqCst);
                        }
                    }
                    let now = mono_ms();
                    let last = beat.load(Ordering::SeqCst);
                    let gap = now.saturating_sub(last);
                    if inflight.load(Ordering::SeqCst) && gap > 2000 && since.is_none() {
                        since = Some(last);
                        log(format!("main thread: no response for {gap} ms"));
                    } else if !inflight.load(Ordering::SeqCst) {
                        if let Some(s) = since.take() {
                            log(format!("main thread: responsive again after ~{} ms", last.saturating_sub(s)));
                        }
                    }
                }
            });
            let ws = shared.ws.clone();
            let chan = shared.glass.clone();
            std::thread::spawn(move || glass::glass_loop(ws, chan));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building MicaBar")
        .run(|app, ev| {
            if let tauri::RunEvent::Exit = ev {
                speed::stop();
                if let Some(sh) = app.try_state::<Arc<Shared>>() {
                    sh.traffic.lock().unwrap().flush();
                }
            }
        });
}

async fn tokio_sleep(d: Duration) {
    tokio::time::sleep(d).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_jump_avoids_mid_game_false_positives() {
        assert!(!is_clock_jump(1_000, 1_000));
        assert!(!is_clock_jump(4_000, 4_000));
        assert!(!is_clock_jump(20_000, 20_000));
        assert!(!is_clock_jump(45_000, 45_000));
        // Real monotonic stall > 45s (hibernate/VM pause)
        assert!(is_clock_jump(46_000, 46_000));
        // Real wall-clock jump > mono + 15s (S3/S4 sleep where Instant paused)
        assert!(is_clock_jump(20_000, 1_000));
    }

    #[test]
    fn collector_period_throttles_during_idle_games() {
        assert_eq!(collector_period(0, false), Duration::from_millis(2000));
        assert_eq!(collector_period(1, false), Duration::from_millis(1000));
        assert_eq!(collector_period(2, false), Duration::from_millis(1000));
        assert_eq!(collector_period(0, true), Duration::from_millis(4000));
        assert_eq!(collector_period(1, true), Duration::from_millis(4000));
        assert_eq!(collector_period(2, true), Duration::from_millis(4000));
    }
}
