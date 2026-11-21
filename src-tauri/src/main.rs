#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod audio;
mod conn;
mod control;
mod dns;
mod exitip;
mod geo;
mod glass;
mod guard;
mod icon;
mod media;
mod mihomo;
mod notify;
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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

fn log_path() -> std::path::PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    let dir = std::path::PathBuf::from(base).join("MicaBar");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("micabar.log")
}

pub fn log(msg: impl AsRef<str>) {
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock();
    let p = log_path();
    if std::fs::metadata(&p).map(|m| m.len() > 2_000_000).unwrap_or(false) {
        let _ = std::fs::rename(&p, p.with_extension("log.old"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let t = now_ms();
        let _ = writeln!(f, "[{}] {}", fmt_time(t), msg.as_ref());
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn fmt_time(ms: u64) -> String {
    // UTC+8 wall clock without a date library.
    let s = ms / 1000 + 8 * 3600;
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
    last: Mutex<Option<Snapshot>>,
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

// ---------- collector ----------

struct Collector {
    procs: procinfo::ProcCache,
    counters: Option<speed::Counters>,
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
    net_sig: String,
    proxy_sig: String,
    control_at: Option<Instant>,
    sampler: sysinfo::Sampler,
    traffic_at: Option<Instant>,
    traffic_flush: Option<Instant>,
    animations: bool,
    animations_at: Option<Instant>,
}

fn network_signature() -> String {
    let Ok(ads) = ipconfig::get_adapters() else { return String::new() };
    // physical uplinks only: TUN adapters come and go with the proxy and churn their addresses
    let mut parts: Vec<String> = ads
        .iter()
        .filter(|a| a.oper_status() == ipconfig::OperStatus::IfOperStatusUp && !a.gateways().is_empty() && !proxy::looks_like_tun(a))
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
    fn new(counters: Result<speed::Counters, String>) -> Collector {
        let (counters, speed_error) = match counters {
            Ok(c) => (Some(c), None),
            Err(e) => (None, Some(e)),
        };
        Collector {
            procs: Default::default(),
            counters,
            speed_error,
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
            net_sig: String::new(),
            proxy_sig: String::new(),
            control_at: None,
            sampler: Default::default(),
            traffic_at: None,
            traffic_flush: Some(Instant::now()),
            animations: true,
            animations_at: None,
        }
    }

    fn due(at: &Option<Instant>, every: Duration) -> bool {
        at.map(|t| t.elapsed() >= every).unwrap_or(true)
    }

    fn tick(&mut self, sh: &Shared) -> Snapshot {
        let list = procinfo::list_processes();
        self.procs.refresh(&list);
        let sockets = conn::snapshot();

        // --- proxy detection ---
        if Self::due(&self.cfg_at, Duration::from_secs(10)) {
            let cfg = mihomo::read_config();
            if cfg.file != self.cfg.file {
                log(format!("core config: {:?}", cfg.file));
            }
            self.cfg = cfg;
            self.cfg_at = Some(Instant::now());
        }
        if Self::due(&self.tun_at, Duration::from_secs(5)) {
            self.tun = proxy::tun_adapter();
            self.tun_at = Some(Instant::now());
            let sig = network_signature();
            if !self.net_sig.is_empty() && sig != self.net_sig {
                log("network changed; re-checking exit IP");
                sh.exit_trigger.store(true, Ordering::SeqCst);
            }
            self.net_sig = sig;
        }
        let (client, core_pids, core_name) = proxy::client_name(&list);
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
                .filter(|(pid, exe)| core_pids.contains(pid) || proxy::client_name(&[(*pid, exe.clone())]).0.is_some())
                .map(|(pid, _)| *pid)
                .collect();
            local_port = proxy::KNOWN_PORTS.iter().copied().find(|p| {
                sockets.iter().any(|s| s.proto == Proto::Tcp && s.state == "LISTEN" && s.local_port == *p && owners.contains(&s.pid))
            });
        }
        // only mihomo-type cores have a controller worth looking for (xray / sing-box cores don't)
        let mihomo_core = list.iter().any(|(pid, exe)| core_pids.contains(pid) && proxy::is_mihomo_core(exe));

        // --- mihomo controller ---
        let mut mconns: Vec<MConn> = Vec::new();
        let mut mihomo_err = None;
        {
            let mut link = sh.mihomo.lock().unwrap();
            if !mihomo_core {
                link.ctl = None;
            } else if link.ctl.is_none() && Self::due(&link.last_try, Duration::from_secs(5)) {
                link.last_try = Some(Instant::now());
                match mihomo::discover(&self.cfg) {
                    Some((c, ver)) => {
                        log(format!("mihomo controller: {} (version {ver})", c.describe()));
                        let mut p = sh.proxy.lock().unwrap();
                        p.core_version = non_empty(&ver);
                        link.ctl = Some(c);
                        link.secret = self.cfg.secret.clone();
                    }
                    None => mihomo_err = Some("找不到 mihomo 控制接口".to_string()),
                }
            }
            if let Some(c) = link.ctl.clone() {
                match mihomo::connections(&c, link.secret.as_deref()) {
                    Ok(v) => mconns = v,
                    Err(e) => {
                        log(format!("mihomo /connections failed: {e}"));
                        mihomo_err = Some(e);
                        link.ctl = None;
                    }
                }
            }
        }

        let client_label = match (&client, &core_name) {
            (Some(c), Some(k)) => Some(format!("{c} ({k})")),
            (Some(c), None) => Some(c.clone()),
            (None, Some(k)) => Some(k.clone()),
            (None, None) => None,
        };
        let state = {
            let mut p = sh.proxy.lock().unwrap();
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
            p.mihomo_ok = !mconns.is_empty() || (core_running && mihomo_err.is_none() && sh.mihomo.lock().unwrap().ctl.is_some());
            p.mihomo_error = if mihomo_core { mihomo_err } else { None };
            p.controller = sh.mihomo.lock().unwrap().ctl.as_ref().map(|c| c.describe());
            if !core_running {
                p.core_version = None;
            }
            p.clone()
        };
        let sig = format!("{}|{:?}|{:?}|{:?}|{:?}", state.detected, state.client, state.system_proxy, state.tun, state.local_port);
        if sig != self.proxy_sig {
            if !self.proxy_sig.is_empty() {
                log(format!("proxy state changed: {sig}"));
                sh.exit_trigger.store(true, Ordering::SeqCst);
            }
            self.proxy_sig = sig;
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
                self.dns.clear();
            }
            self.dns.extend(fresh);
            self.dns_at = Some(Instant::now());
        }
        // live resolutions from ETW (works when the cache is empty, e.g. TUN DNS with tiny TTLs)
        let live: Vec<(IpAddr, String)> = std::mem::take(&mut *speed::dns_log().lock().unwrap());
        self.dns.extend(live);

        // --- speeds ---
        let mut pid_rate: HashMap<u32, (f64, f64)> = HashMap::new();
        if let Some(c) = &self.counters {
            let cur = c.lock().unwrap().clone();
            let dt = self.prev_counter_at.elapsed().as_secs_f64().max(0.2);
            for (pid, (s, r)) in &cur {
                let (ps, pr) = self.prev_counters.get(pid).copied().unwrap_or((*s, *r));
                pid_rate.insert(*pid, ((s.saturating_sub(ps)) as f64 / dt, (r.saturating_sub(pr)) as f64 / dt));
            }
            self.prev_counters = cur;
            self.prev_counter_at = Instant::now();
        }
        let mdt = self.prev_mihomo_at.elapsed().as_secs_f64().max(0.2);
        let mut conn_rate: HashMap<String, (f64, f64)> = HashMap::new();
        let mut next_prev = HashMap::with_capacity(mconns.len());
        for c in &mconns {
            if let Some((pu, pd)) = self.prev_mihomo.get(&c.id) {
                conn_rate.insert(c.id.clone(), (c.upload.saturating_sub(*pu) as f64 / mdt, c.download.saturating_sub(*pd) as f64 / mdt));
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
        let mut hidden_loopback = 0usize;
        let core_set: HashSet<u32> = core_pids.iter().copied().collect();
        let proxy_ports: HashSet<u16> = [self.cfg.mixed_port, self.cfg.http_port, self.cfg.socks_port, local_port].into_iter().flatten().collect();

        let app_key = |path: &Option<String>, exe: &str| -> String {
            path.as_ref().map(|p| p.to_lowercase()).unwrap_or_else(|| exe.to_lowercase())
        };

        for s in &sockets {
            if s.pid == 0 {
                continue;
            }
            let info = self.procs.get(s.pid).cloned().unwrap_or(procinfo::ProcInfo {
                exe: format!("pid {}", s.pid),
                path: None,
                display: format!("进程 {}", s.pid),
                icon: None,
            });
            let key = app_key(&info.path, &info.exe);
            let app = apps.entry(key.clone()).or_insert_with(|| AppRow {
                key: key.clone(),
                name: info.display.clone(),
                exe: info.exe.clone(),
                path: info.path.clone(),
                icon: info.icon.clone(),
                is_proxy: core_set.contains(&s.pid) || proxy::client_name(&[(s.pid, info.exe.clone())]).0.is_some(),
                ..Default::default()
            });
            if !app.pids.contains(&s.pid) {
                app.pids.push(s.pid);
            }
            let net = if s.proto == Proto::Tcp { "tcp" } else { "udp" };
            let matched = by_port.get(&(net, s.local_port)).and_then(|ids| {
                ids.iter().copied().find(|i| {
                    let m = &mconns[*i].metadata;
                    m.process_path.is_empty()
                        || info.path.as_ref().map(|p| p.eq_ignore_ascii_case(&m.process_path)).unwrap_or(true)
                })
            });
            if let Some(i) = matched {
                used.insert(i);
                let row = self.row_from_mihomo(&mconns[i], s.proto, &fmt_ip(&s.local), s.local_port, s.state, &sh.geo, &conn_rate, &proxy_ports, s.remote.as_ref());
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
                    // public destination, mihomo running, yet the connection never reached it
                    let bypass = !mconns.is_empty() && !app.is_proxy && !geo::is_private(&rip) && s.state == "ESTABLISHED";
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
            let exe = non_empty(&c.metadata.process).unwrap_or_else(|| "未知进程".into());
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
            let row = self.row_from_mihomo(c, proto, &c.metadata.source_ip, lp, if proto == Proto::Tcp { "ESTABLISHED" } else { "ACTIVE" }, &sh.geo, &conn_rate, &proxy_ports, None);
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
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
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
                if new_busy && now - LAST.load(Ordering::Relaxed) > 60_000 {
                    LAST.store(now, Ordering::Relaxed);
                    sh.exit_trigger.store(true, Ordering::SeqCst);
                }
            }
        }
        let conn_count = out.iter().map(|a| a.conns.len()).sum();
        let listen_count = out.iter().map(|a| a.listen.len()).sum();
        self.account_traffic(sh, &out);
        let guard = self.detect_leaks(sh, &mut out, state.tun.is_some());
        let (want_cpu, want_mem) = (settings::item_on("pill.items", "cpu"), settings::item_on("pill.items", "mem"));
        let sys = (want_cpu || want_mem).then(|| self.sampler.sample(want_cpu, want_mem));

        Snapshot {
            ts: now_ms(),
            admin: sh.admin,
            speed_ok: self.counters.is_some(),
            speed_error: self.speed_error.clone(),
            geo_source: sh.geo.source.clone(),
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

    /// Adds this tick's bytes per app to today's traffic book: bytes on connections that left
    /// through a proxy node count as "proxy", the rest of the app's traffic as "direct".
    fn account_traffic(&mut self, sh: &Shared, apps: &[AppRow]) {
        let dt = self.traffic_at.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0).min(5.0);
        self.traffic_at = Some(Instant::now());
        if dt <= 0.0 {
            return;
        }
        let mut book = sh.traffic.lock().unwrap();
        for a in apps.iter().filter(|a| !a.is_proxy) {
            let via_node = |c: &&ConnRow| c.node.as_deref().map(|n| n != "DIRECT" && !n.starts_with("REJECT")).unwrap_or(false);
            let (pu, pd) = a.conns.iter().filter(via_node).fold((0.0, 0.0), |s, c| (s.0 + c.up, s.1 + c.down));
            let (du, dd) = if self.counters.is_some() {
                ((a.up - pu).max(0.0), (a.down - pd).max(0.0))
            } else {
                a.conns.iter().filter(|c| !via_node(c)).fold((0.0, 0.0), |s, c| (s.0 + c.up, s.1 + c.down))
            };
            book.add(&a.key, &a.name, a.path.as_ref(), (pu * dt, pd * dt), (du * dt, dd * dt));
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
                let recent = book.notified.get(&f.key).map(|t| now - t < 10 * 60_000).unwrap_or(false);
                if recent {
                    continue;
                }
                book.notified.insert(f.key.clone(), now);
                let what = f.host.clone().or_else(|| f.ip.clone()).unwrap_or_default();
                let why = if f.kind == "bypass" { "没经过代理软件，直接连了出去" } else { "被分流规则判成了直连" };
                guard::toast(&format!("代理泄漏：{}", f.app), &format!("{what} 本该走代理，{why}"), &format!("命中规则库：{}", f.hit));
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
                let msg = other.map(|c| format!("出口控制目前只支持 Clash Party / Mihomo Party（检测到 {c}）"));
                ("unavailable", msg, None)
            }
            Some(cp) => {
                let installed = control::override_installed(&cp.data_dir);
                let loaded = link.as_ref().map(|(c, s)| control::providers_loaded(c, s.as_deref()).unwrap_or(false)).unwrap_or(false);
                if installed && !control::override_current(&cp.data_dir) {
                    (
                        "available",
                        Some("出口控制更新了规则结构，需要重新写入覆写（会重启 Clash Party 一次）".to_string()),
                        Some("Clash Party".to_string()),
                    )
                } else if loaded {
                    ("active", None, Some("Clash Party".to_string()))
                } else if installed {
                    ("pending", Some("覆写已写入，等 Clash Party 重新加载内核后生效".to_string()), Some("Clash Party".to_string()))
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
        local: &str,
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
            local: local.to_string(),
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
    let res = match kind {
        Check::Direct => {
            let nic = proxy::physical_ipv4();
            r.via = nic.as_ref().map(|(n, ip)| format!("{n} {ip}"));
            exitip::probe_direct(nic.map(|(_, ip)| ip)).await
        }
        Check::Proxy(port) => match exitip::client(port) {
            Ok(c) => exitip::probe(&c, false).await,
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

async fn exit_check(app: AppHandle, sh: Arc<Shared>) {
    {
        sh.exit.lock().unwrap().checking = true;
    }
    let _ = app.emit("exit", sh.exit.lock().unwrap().clone());
    let p = sh.proxy.lock().unwrap().clone();
    let direct = check_one(sh.clone(), Check::Direct).await;
    let proxied = if p.detected {
        match p.local_port {
            Some(port) => Some(check_one(sh.clone(), Check::Proxy(Some(port))).await),
            None if p.tun.is_some() => Some(check_one(sh.clone(), Check::Proxy(None)).await),
            None => Some(ExitResult { error: Some("检测到代理软件，但没找到本地代理端口".into()), at: now_ms(), ..Default::default() }),
        }
    } else {
        None
    };
    log(format!(
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
    let groups = if p.detected { measure_groups(sh.clone()).await } else { Vec::new() };
    // every group failing usually means the core is still starting (e.g. right after the
    // override was installed): try once more shortly
    {
        static RETRIED: AtomicBool = AtomicBool::new(false);
        let all_failed = !groups.is_empty() && groups.iter().all(|g| g.r.ip.is_none());
        if all_failed && !RETRIED.swap(true, Ordering::SeqCst) {
            let sh2 = sh.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(15));
                sh2.exit_trigger.store(true, Ordering::SeqCst);
            });
        } else if !all_failed {
            RETRIED.store(false, Ordering::SeqCst);
        }
    }
    if !groups.is_empty() {
        log(format!(
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
fn set_hit_rects(state: tauri::State<'_, Arc<Shared>>, pill: [f64; 4], panel: [f64; 4]) {
    *state.ws.pill.lock().unwrap() = Rect { x: pill[0], y: pill[1], w: pill[2], h: pill[3] };
    *state.ws.panel.lock().unwrap() = Rect { x: panel[0], y: panel[1], w: panel[2], h: panel[3] };
}

// ---------- exit control commands ----------

fn ctl_link(sh: &Shared) -> Result<(Controller, Option<String>), String> {
    let l = sh.mihomo.lock().unwrap();
    l.ctl.clone().map(|c| (c, l.secret.clone())).ok_or_else(|| "mihomo 控制接口未连接".to_string())
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
    set_control(&sh, "working", Some("正在写入覆写并重启 Clash Party…".into()));
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
    set_control(&sh, "working", Some("正在移除覆写并重启 Clash Party…".into()));
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
        let home = sh.core_home.lock().unwrap().clone().ok_or("还不知道内核目录，稍等几秒再试")?;
        let rule = {
            let mut rules = sh.rules.lock().unwrap();
            let r = control::upsert(&mut rules, &kind, process, value, &target);
            control::persist(&home, &rules)?;
            r
        };
        control::reload_providers(&ctl, secret.as_deref())?;
        let n = control::close_matching(&ctl, secret.as_deref(), &rule);
        log(format!("control: {} {:?} {:?} -> {} (reset {n} connections)", rule.kind, rule.process, rule.value, target));
        sh.control.lock().unwrap().rules = sh.rules.lock().unwrap().clone();
        Ok(n)
    })
    .await
}

#[tauri::command]
async fn control_remove(state: tauri::State<'_, Arc<Shared>>, id: String) -> Result<(), String> {
    let sh = state.inner().clone();
    blocking(move || {
        let (ctl, secret) = ctl_link(&sh)?;
        let home = sh.core_home.lock().unwrap().clone().ok_or("还不知道内核目录")?;
        let removed = {
            let mut rules = sh.rules.lock().unwrap();
            let r = rules.iter().find(|r| r.id == id).cloned();
            rules.retain(|r| r.id != id);
            control::persist(&home, &rules)?;
            r
        };
        control::reload_providers(&ctl, secret.as_deref())?;
        if let Some(r) = removed {
            control::close_matching(&ctl, secret.as_deref(), &r);
        }
        sh.control.lock().unwrap().rules = sh.rules.lock().unwrap().clone();
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
    let nodes: Vec<String> = {
        let ex = sh.exit.lock().unwrap();
        let main = ex.groups.iter().find(|g| g.is_default).and_then(|g| g.r.node.clone()).or_else(|| ex.proxy.as_ref().and_then(|e| e.node.clone()));
        let mut v: Vec<String> = main.into_iter().collect();
        for g in &ex.groups {
            if let Some(n) = &g.r.node {
                if !v.contains(n) && !n.eq_ignore_ascii_case("DIRECT") && !n.eq_ignore_ascii_case("REJECT") {
                    v.push(n.clone());
                }
            }
        }
        v.truncate(4); // the chart has four series colours
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
            Err(e) => (None, Some(if e.contains("504") || e.to_lowercase().contains("timeout") { "超时".to_string() } else { e })),
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
fn exit_group_options() -> Vec<String> {
    mihomo::read_config()
        .file
        .map(|f| control::group_listeners(std::path::Path::new(&f)).into_iter().map(|(g, _)| g).collect())
        .unwrap_or_default()
}

#[tauri::command]
fn traffic_query(state: tauri::State<'_, Arc<Shared>>, days: i64, day: Option<String>) -> traffic::Report {
    let book = state.traffic.lock().unwrap();
    traffic::report(&book, days.clamp(1, traffic::KEEP_DAYS + 1), day.as_deref())
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
fn app_info(app: AppHandle) -> AppInfo {
    AppInfo {
        version: app.package_info().version.to_string(),
        installed: system::is_installed(),
        autostart: system::autostart_enabled(),
        log_dir: log_path().parent().map(|p| p.display().to_string()).unwrap_or_default(),
        exe: std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
    }
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
        Err(e) => Err(format!("检查更新失败：{e}")),
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
        return Err("这是便携版，不能自动更新；请到 GitHub 下载新版本".into());
    }
    let u = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?.ok_or("已经是最新版本")?;
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
            format!("下载失败：{e}")
        })?;
    log(format!("update: downloaded {} bytes, signature ok, installing", bytes.len()));
    let _ = app.emit("update-progress", serde_json::json!({ "phase": "install" }));
    // give the page a moment to show "正在安装" before the installer closes us
    tokio::time::sleep(Duration::from_millis(600)).await;
    state.traffic.lock().unwrap().flush();
    speed::stop();
    u.install(bytes).map_err(|e| {
        log(format!("update: install failed: {e}"));
        format!("安装失败：{e}")
    })?;
    app.restart();
}

// ---------- notifications ----------

#[tauri::command]
fn notify_list() -> Vec<notify::Note> {
    notify::list()
}

#[tauri::command]
fn notify_open(id: u64) -> Result<(), String> {
    notify::open(id)
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
fn notify_apps() -> Vec<NotifyApp> {
    notify::apps().into_iter().map(|a| NotifyApp { takeover: a.source == "toast" && notify::takeover_on(&a.key), seen: a }).collect()
}

#[tauri::command]
fn notify_takeover(key: String, on: bool) -> Result<(), String> {
    notify::set_takeover(&key, on)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NotifyHooks {
    port: u16,
    quiet: bool,
    hooks: Vec<notify::HookStatus>,
}

#[tauri::command]
fn notify_hooks() -> NotifyHooks {
    NotifyHooks { port: notify::port(), quiet: notify::quiet_now(), hooks: notify::hooks_status() }
}

#[tauri::command]
fn notify_hook(tool: String, on: bool) -> Result<String, String> {
    notify::set_hook(&tool, on)
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

/// Side effects of a settings change that live in the backend.
fn apply_settings_backend(app: &AppHandle, ws: &WinState) {
    window_ctl::reload_hotkey(ws);
    if let Some(w) = app.get_webview_window("main") {
        window_ctl::place_window(&w);
    }
}

#[tauri::command]
fn settings_set(app: AppHandle, state: tauri::State<'_, Arc<Shared>>, patch: serde_json::Value) -> Result<serde_json::Value, String> {
    // start with Windows: create / remove the logon task first, keep the setting only if it worked
    if let Some(on) = patch.pointer("/general/autostart").and_then(|v| v.as_bool()) {
        system::set_autostart(on).map_err(|e| format!("设置开机自启失败：{e}"))?;
    }
    let s = settings::update(&patch);
    apply_settings_backend(&app, &state.ws);
    let _ = app.emit("settings", &s);
    Ok(s)
}

#[tauri::command]
fn settings_reset(app: AppHandle, state: tauri::State<'_, Arc<Shared>>) -> serde_json::Value {
    let s = settings::reset();
    apply_settings_backend(&app, &state.ws);
    let _ = app.emit("settings", &s);
    s
}

/// Glass off: drop the frame channel, which pauses desktop duplication entirely.
#[tauri::command]
fn glass_off(state: tauri::State<'_, Arc<Shared>>) {
    *state.glass.lock().unwrap() = None;
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
fn fonts_list() -> Vec<String> {
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
            let mut e = guard::parse_list(v).into_iter().next().ok_or("不是有效的域名")?;
            e.kind = "exclude".into();
            e
        } else if let Some(v) = t.strip_prefix("keyword:").or_else(|| t.strip_prefix("关键词:")).or_else(|| t.strip_prefix("关键词：")) {
            guard::Entry { kind: "keyword".into(), value: v.trim().to_lowercase() }
        } else {
            guard::parse_list(t).into_iter().next().ok_or("没认出来：填域名、IP/CIDR、keyword:关键词，或 !域名 表示排除")?
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
            Err(format!("更新 gfw 列表失败：{e}"))
        }
    }
}

#[tauri::command]
async fn guard_update_gfw(state: tauri::State<'_, Arc<Shared>>) -> Result<usize, String> {
    let sh = state.inner().clone();
    let n = update_gfw(sh.clone()).await;
    let sh2 = sh.clone();
    blocking(move || apply_guard(&sh2)).await?;
    n
}

#[tauri::command]
async fn guard_import(state: tauri::State<'_, Arc<Shared>>, url: String) -> Result<usize, String> {
    let sh = state.inner().clone();
    let url = url.trim().to_string();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("填一个 http(s) 链接".into());
    }
    let r = guard::fetch_text(&[url.clone()], proxy_port(&sh)).await;
    let n = {
        let mut s = sh.guard.lock().unwrap();
        s.imports.retain(|i| i.url != url);
        match &r {
            Ok(t) => {
                let n = guard::save_import(&url, t);
                s.imports.push(guard::Import { url: url.clone(), enabled: true, count: n, updated: now_ms(), error: None });
                n
            }
            Err(e) => return Err(format!("下载失败：{e}")),
        }
    };
    if n == 0 {
        sh.guard.lock().unwrap().imports.retain(|i| i.url != url);
        guard::remove_import_file(&url);
        return Err("这个链接里没认出任何域名 / IP 规则".into());
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
async fn guard_block(state: tauri::State<'_, Arc<Shared>>, path: String, exe: String) -> Result<usize, String> {
    let sh = state.inner().clone();
    blocking(move || {
        if sh.proxy.lock().unwrap().tun.is_some() {
            return Err("TUN 模式下程序的流量也走虚拟网卡，用防火墙禁止会让它完全断网，所以不提供".into());
        }
        let name = guard::block(&path, &exe)?;
        {
            let mut s = sh.guard.lock().unwrap();
            s.blocked.retain(|b| !b.path.eq_ignore_ascii_case(&path));
            s.blocked.push(guard::Blocked { path: path.clone(), exe: exe.clone(), rule: name, at: now_ms() });
            guard::save_settings(&s);
        }
        let pids: HashSet<u32> = procinfo::list_processes()
            .into_iter()
            .filter(|(p, _)| procinfo::image_path(*p).map(|x| x.eq_ignore_ascii_case(&path)).unwrap_or(false))
            .map(|(p, _)| p)
            .collect();
        Ok(guard::reset_connections(&pids))
    })
    .await
}

#[tauri::command]
async fn guard_unblock(state: tauri::State<'_, Arc<Shared>>, path: String) -> Result<(), String> {
    let sh = state.inner().clone();
    blocking(move || {
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

/// Asks only the tools that are due (each has its own clock); the others keep their last numbers.
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
    let mut out = Vec::new();
    let mut asked = false;
    for (tool, _) in ai::TOOLS {
        if !settings::bool_at(&format!("ai.{tool}"), true) {
            continue;
        }
        // the last result, also when the tool (or the AI section) was switched off for a while
        let p = prev.iter().find(|p| p.tool == tool).cloned().or_else(|| ai::known_quota(tool));
        if !ai::due(tool, every, manual) {
            out.extend(p);
            continue;
        }
        asked = true;
        let q = ai::query(tool, proxy, p.as_ref()).await;
        // log failures once, not on every retry
        let same = p.as_ref().map(|p| p.status == q.status && p.message == q.message).unwrap_or(false);
        if q.status != "ok" && !same {
            log(format!("ai {tool}: {} {}", q.status, q.message.clone().unwrap_or_default()));
        }
        out.push(q);
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
            tokio::time::sleep(Duration::from_secs(2)).await;
            if !settings::bool_at("ai.on", true) {
                if !sh.ai.lock().unwrap().is_empty() {
                    sh.ai.lock().unwrap().clear();
                    let _ = app.emit("ai", Vec::<ai::Quota>::new());
                }
                continue;
            }
            // wait for the first snapshot: until then the proxy client isn't detected yet and the
            // services (blocked here without it) would be asked directly
            if sh.last.lock().unwrap().is_none() {
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

/// Music bars on or off (a page shows them while a song plays); bands arrive as "viz" events.
#[tauri::command]
fn audio_viz(app: AppHandle, window: tauri::Window, on: bool) {
    // the island and the desktop window each say whether they show bars; on while either does
    static WANT: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let any = {
        let mut w = WANT.lock().unwrap();
        w.retain(|l| l != window.label());
        if on {
            w.push(window.label().to_string());
        }
        !w.is_empty()
    };
    audio::set_wanted(any, move |b| {
        let _ = app.emit("viz", b);
    });
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
    blocking(move || {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        media::control(&action)
    })
    .await
}

// ---------- manager window / layout editing ----------

/// Opens (or focuses) the desktop manager window on a page.
fn open_manager_window(app: &AppHandle, page: &str) {
    if let Some(w) = app.get_webview_window("manager") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        let _ = app.emit_to("manager", "goto", page.to_string());
        return;
    }
    let s = settings::get();
    let g = s.pointer("/manager").cloned().unwrap_or_default();
    let num = |k: &str, d: f64| g.get(k).and_then(|v| v.as_f64()).unwrap_or(d);
    let url = tauri::WebviewUrl::App(format!("manager.html#{page}").into());
    let mut b = tauri::WebviewWindowBuilder::new(app, "manager", url)
        .title("MicaBar")
        .inner_size(num("w", 1120.0).max(820.0), num("h", 740.0).max(560.0))
        .min_inner_size(820.0, 560.0)
        .resizable(true)
        // same WebView2 environment as the island window (must match), and keep painting while
        // Chromium thinks the window is covered (the island window overlaps it)
        .additional_browser_args("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,CalculateNativeWinOcclusion")
        .decorations(true)
        // created hidden and shown below: showing an existing window also makes its webview visible,
        // otherwise the page only repaints after mouse input
        .visible(false);
    if let (Some(x), Some(y)) = (g.get("x").and_then(|v| v.as_f64()), g.get("y").and_then(|v| v.as_f64())) {
        // only when that spot is still on a monitor
        let on_screen = window_ctl::monitor_list().iter().any(|m| x >= m.x as f64 - 50.0 && y >= m.y as f64 - 10.0 && x < (m.x + m.w) as f64 - 100.0 && y < (m.y + m.h) as f64 - 100.0);
        if on_screen {
            b = b.position(x, y);
        }
    } else {
        b = b.center();
    }
    match b.build() {
        Ok(w) => {
            if let Some(icon) = app.default_window_icon() {
                let _ = w.set_icon(icon.clone());
            }
            let _ = w.show();
            let _ = w.set_focus();
            let w2 = w.clone();
            w.on_window_event(move |ev| {
                // remember size and position (logical px) for the next time
                if let tauri::WindowEvent::CloseRequested { .. } = ev {
                    let sf = w2.scale_factor().unwrap_or(1.0);
                    if let (Ok(p), Ok(sz), Ok(false)) = (w2.outer_position(), w2.inner_size(), w2.is_minimized()) {
                        settings::update(&serde_json::json!({ "manager": {
                            "x": p.x as f64 / sf, "y": p.y as f64 / sf,
                            "w": sz.width as f64 / sf, "h": sz.height as f64 / sf
                        } }));
                    }
                }
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
        state.ws.collapse_req.store(true, Ordering::SeqCst);
    }
    let _ = app.emit_to("main", "layout-edit", on);
    log(format!("layout edit {}", if on { "on" } else { "off" }));
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
    *state.glass.lock().unwrap() = Some(on_frame);
}

#[tauri::command]
fn refresh_exit(state: tauri::State<'_, Arc<Shared>>) {
    state.exit_trigger.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn get_snapshot(state: tauri::State<'_, Arc<Shared>>) -> Option<Snapshot> {
    state.last.lock().unwrap().clone()
}

/// The page asks for a window size in physical pixels (its CSS size × devicePixelRatio).
#[tauri::command]
fn fit_window(app: AppHandle, w: u32, h: u32) {
    let Some(win) = app.get_webview_window("main") else { return };
    let cur = win.outer_size().ok();
    if cur.map(|s| s.width != w || s.height != h).unwrap_or(true) {
        log(format!("fit_window {w}x{h} (was {cur:?})"));
        let _ = win.set_size(tauri::PhysicalSize::new(w, h));
    }
    window_ctl::place_window(&win);
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

fn single_instance() -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name = procinfo::wide("Global\\MicaBar-single-instance");
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        !(h.is_null() || GetLastError() == ERROR_ALREADY_EXISTS)
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
    if !single_instance() {
        return;
    }
    migrate_old_data();
    let admin = is_admin();
    log(format!("---- MicaBar {} start, admin={admin}", env!("CARGO_PKG_VERSION")));
    let geo = Geo::load();
    log(format!("geo db: {:?}", geo.source));
    let guard_settings = guard::load_settings();
    guard::register_toast_identity(include_bytes!("../icons/128x128.png"));
    let shared = Arc::new(Shared {
        geo,
        proxy: Mutex::new(ProxyState::default()),
        exit: Mutex::new(ExitState::default()),
        last: Mutex::new(None),
        mihomo: Mutex::new(MihomoLink { ctl: None, secret: None, last_try: None }),
        ws: Arc::new(WinState::default()),
        glass: Arc::new(Mutex::new(None)),
        control: Mutex::new(control::ControlState { status: "unavailable".into(), ..Default::default() }),
        rules: Mutex::new(control::load_rules()),
        core_home: Mutex::new(None),
        node_pending: AtomicBool::new(false),
        matcher: std::sync::RwLock::new(guard::Matcher::build(&guard_settings)),
        guard: Mutex::new(guard_settings),
        alerts: Mutex::new(AlertBook::default()),
        latency: Mutex::new(Latency::default()),
        latency_history: Mutex::new(std::collections::VecDeque::new()),
        traffic: Mutex::new(traffic::Book::new()),
        group_score: Mutex::new(HashMap::new()),
        ai: Mutex::new(ai::load_cache()),
        ai_trigger: AtomicBool::new(false),
        media: Arc::new(Mutex::new(None)),
        exit_trigger: AtomicBool::new(false),
        admin,
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(shared.clone())
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

            let open_item = MenuItem::with_id(app, "open", "打开 MicaBar", true, None::<&str>)?;
            let settings_item = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
            let layout_item = MenuItem::with_id(app, "layout", "调整岛的大小…", true, None::<&str>)?;
            let refresh = MenuItem::with_id(app, "refresh", "重新检测出口 IP", true, None::<&str>)?;
            let log_item = MenuItem::with_id(app, "log", "打开日志", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let theme_auto = MenuItem::with_id(app, "theme:auto", "外观：跟随系统", true, None::<&str>)?;
            let theme_light = MenuItem::with_id(app, "theme:light", "外观：白色磨砂", true, None::<&str>)?;
            let theme_dark = MenuItem::with_id(app, "theme:dark", "外观：黑色磨砂", true, None::<&str>)?;
            let sep = tauri::menu::PredefinedMenuItem::separator(app)?;
            let sep0 = tauri::menu::PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(
                app,
                &[&open_item, &settings_item, &layout_item, &sep0, &refresh, &theme_auto, &theme_light, &theme_dark, &sep, &log_item, &quit_item],
            )?;
            let sh_tray = shared.clone();
            let sh_tray2 = shared.clone();
            let mut tray = TrayIconBuilder::new()
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
            let h_media = handle.clone();
            media::start(shared.media.clone(), move |np| {
                let _ = h_media.emit("media", np);
            });

            if let Some(w) = app.get_webview_window("main") {
                window_ctl::place_window(&w);
                let _ = w.show();
                if let Ok(hwnd) = w.hwnd() {
                    let ok = glass::exclude_from_dda(hwnd.0 as isize);
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
            let sh = shared.clone();
            let h = handle.clone();
            std::thread::spawn(move || {
                let counters = if sh.admin { speed::start() } else { Err("需要管理员权限".into()) };
                match &counters {
                    Ok(_) => log("ETW kernel network session started"),
                    Err(e) => log(format!("ETW unavailable: {e}")),
                }
                let mut col = Collector::new(counters);
                sh.exit_trigger.store(true, Ordering::SeqCst);
                loop {
                    let t = Instant::now();
                    let snap = col.tick(&sh);
                    let _ = h.emit("snapshot", &snap);
                    *sh.last.lock().unwrap() = Some(snap);
                    let spent = t.elapsed();
                    let period = if settings::str_at("general.profile", "balanced") == "saver" { 2000 } else { 1000 };
                    std::thread::sleep(Duration::from_millis(period).saturating_sub(spent));
                }
            });

            // exit IP scheduler
            let sh = shared.clone();
            let h = handle.clone();
            tauri::async_runtime::spawn(async move {
                let mut last = Instant::now() - Duration::from_secs(3600);
                let started = Instant::now();
                let mut gfw_try: Option<Instant> = None;
                let mut lat_at: Option<Instant> = None;
                let mut update_at: Option<Instant> = None;
                loop {
                    tokio_sleep(Duration::from_millis(500)).await;
                    // updates: first check a minute after start, then daily
                    let update_due = update_at.map(|t| t.elapsed() > Duration::from_secs(24 * 3600)).unwrap_or(started.elapsed() > Duration::from_secs(60));
                    if settings::bool_at("general.autoUpdate", true) && update_due {
                        update_at = Some(Instant::now());
                        match check_update(&h).await {
                            Ok(u) => {
                                log(format!("auto update check: available={} {:?}", u.available, u.version));
                                if u.available {
                                    let _ = h.emit("update", &u);
                                }
                            }
                            Err(e) => log(format!("auto update check failed: {e}")),
                        }
                    }
                    let every = Duration::from_secs(profile_secs(60, 30, 15));
                    if latency_wanted() && lat_at.map(|t| t.elapsed() >= every).unwrap_or(started.elapsed() > Duration::from_secs(8)) {
                        lat_at = Some(Instant::now());
                        let sh2 = sh.clone();
                        let _ = tauri::async_runtime::spawn_blocking(move || probe_latency(&sh2)).await;
                    }
                    // gfw list: fetch once the proxy is known, then daily
                    let (on, updated) = {
                        let s = sh.guard.lock().unwrap();
                        (s.gfw, s.gfw_updated)
                    };
                    let stale = now_ms().saturating_sub(updated) > 24 * 3600_000;
                    let may_try = gfw_try.map(|t| t.elapsed() > Duration::from_secs(1800)).unwrap_or(true);
                    if on && stale && may_try && started.elapsed() > Duration::from_secs(15) {
                        gfw_try = Some(Instant::now());
                        if update_gfw(sh.clone()).await.is_ok() {
                            let sh2 = sh.clone();
                            let _ = blocking(move || apply_guard(&sh2)).await;
                        }
                    }
                    let due = last.elapsed() >= Duration::from_secs(300);
                    if sh.exit_trigger.swap(false, Ordering::SeqCst) || due {
                        // let the proxy state settle (e.g. right after switching nodes)
                        tokio_sleep(Duration::from_millis(1500)).await;
                        sh.exit_trigger.store(false, Ordering::SeqCst);
                        exit_check(h.clone(), sh.clone()).await;
                        last = Instant::now();
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
