//! Network diagnosis: when the internet goes away, tell which link of the chain broke
//! (network adapter → router → DNS → internet → proxy software → node) and offer a fix.
//!
//! A watcher probes quietly (one small HTTP request to a public DNS server's web API every
//! 10 s, one request through the proxy every 30 s); two failures in a row run the full check.
//! It also keeps an eye on the system proxy: switched off or pointed elsewhere while the proxy
//! client still runs (another program changed it) is reported with a one-click restore.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::os::windows::process::CommandExt;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::{log, now_ms, proxy};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// What the diagnosis needs to know about the proxy (filled in by main from its shared state).
#[derive(Clone, Debug, Default)]
pub struct Info {
    pub detected: bool,
    pub client: Option<String>,
    pub client_running: bool,
    /// the client's exe (to start it again)
    pub client_exe: Option<String>,
    pub core_running: bool,
    pub local_port: Option<u16>,
    pub tun: bool,
    pub system_proxy: Option<String>,
    /// node of the main exit
    pub node: Option<String>,
    /// the mihomo controller answers (node switching possible)
    pub controller: bool,
    pub clash_party: bool,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub id: &'static str,
    pub label: &'static str,
    /// wait | run | ok | warn | fail | skip
    pub status: &'static str,
    pub detail: String,
    pub ms: Option<u32>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Fix {
    pub id: &'static str,
    pub label: String,
    pub primary: bool,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub at: u64,
    pub running: bool,
    /// started by the watcher (not the button)
    pub auto: bool,
    pub ok: bool,
    /// short text for the pill ("DNS 坏了"); None = nothing wrong
    pub problem: Option<String>,
    /// net | sysproxy (what kind of problem)
    pub kind: String,
    pub verdict: String,
    pub advice: String,
    pub steps: Vec<Step>,
    pub fixes: Vec<Fix>,
    /// when the trouble began (for "断了 3 分钟")
    pub down_since: Option<u64>,
    /// set once on the report that clears a problem: how long it lasted
    pub recovered_ms: Option<u64>,
    pub took_ms: u32,
}

type InfoFn = Arc<dyn Fn() -> Info + Send + Sync>;

struct Watch {
    /// the last finished check (the sheet shows it)
    last: Option<Report>,
    /// the problem showing in the pill (kind net or sysproxy), until it clears
    problem: Option<Report>,
    /// the system proxy value seen while it pointed at the client's port
    good_proxy: Option<String>,
    proxy_off_since: Option<Instant>,
    /// the user said they switched it off themselves: don't nag until it is on again
    proxy_ignored: bool,
    running: bool,
}

static W: Mutex<Watch> = Mutex::new(Watch { last: None, problem: None, good_proxy: None, proxy_off_since: None, proxy_ignored: false, running: false });
static INFO: Mutex<Option<InfoFn>> = Mutex::new(None);
static CLIENT_EXE: Mutex<Option<String>> = Mutex::new(None);

/// The proxy client's exe as last seen running.
pub fn remember_client(exe: &str) {
    *CLIENT_EXE.lock().unwrap() = Some(exe.to_string());
}

pub fn client_exe() -> Option<String> {
    CLIENT_EXE.lock().unwrap().clone()
}

fn info() -> Info {
    let f = INFO.lock().unwrap().clone();
    let mut i = f.map(|f| f()).unwrap_or_default();
    i.client_running = i.client.is_some();
    i.client_exe = client_exe();
    if let Some(e) = &i.client_exe {
        let l = e.to_lowercase();
        i.clash_party = l.ends_with("clash party.exe") || l.ends_with("mihomo party.exe") || l.ends_with("mihomo-party.exe");
        if i.client.is_none() {
            // not running any more: name it after its exe
            i.client = Some(e.rsplit(['\\', '/']).next().unwrap_or(e).trim_end_matches(".exe").trim_end_matches(".EXE").to_string());
        }
    }
    i
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub last: Option<Report>,
    pub problem: Option<Report>,
    pub running: bool,
}

pub fn state() -> State {
    let w = W.lock().unwrap();
    State { last: w.last.clone(), problem: w.problem.clone(), running: w.running }
}

fn publish() {
    crate::notify::emit("netdiag", state());
}

fn problem_kind(w: &Watch) -> Option<String> {
    w.problem.as_ref().map(|p| p.kind.clone())
}

// ---------- probes ----------

/// The real uplink: up, not a TUN adapter, with an IPv4 gateway (lowest metric).
fn uplink() -> Option<(String, Ipv4Addr)> {
    let ads = ipconfig::get_adapters().ok()?;
    let mut best: Option<(u32, String, Ipv4Addr)> = None;
    for a in &ads {
        if a.oper_status() != ipconfig::OperStatus::IfOperStatusUp || proxy::looks_like_tun(a) {
            continue;
        }
        let Some(gw) = a.gateways().iter().find_map(|g| match g {
            IpAddr::V4(v) if !v.is_unspecified() => Some(*v),
            _ => None,
        }) else {
            continue;
        };
        let m = a.ipv4_metric();
        if best.as_ref().map(|b| m < b.0).unwrap_or(true) {
            best = Some((m, a.friendly_name().to_string(), gw));
        }
    }
    best.map(|(_, n, g)| (n, g))
}

/// One ICMP echo; round trip in ms.
fn ping(ip: Ipv4Addr, timeout_ms: u32) -> Option<u32> {
    #[repr(C)]
    struct OptionInfo {
        ttl: u8,
        tos: u8,
        flags: u8,
        size: u8,
        data: *mut u8,
    }
    #[repr(C)]
    struct EchoReply {
        address: u32,
        status: u32,
        rtt: u32,
        data_size: u16,
        reserved: u16,
        data: *mut u8,
        options: OptionInfo,
    }
    #[link(name = "iphlpapi")]
    extern "system" {
        fn IcmpCreateFile() -> isize;
        fn IcmpCloseHandle(h: isize) -> i32;
        fn IcmpSendEcho(h: isize, dest: u32, req: *const u8, req_size: u16, opts: *const u8, reply: *mut u8, reply_size: u32, timeout: u32) -> u32;
    }
    unsafe {
        let h = IcmpCreateFile();
        if h == -1 || h == 0 {
            return None;
        }
        let payload = *b"micabar-diag";
        let mut buf = vec![0u8; std::mem::size_of::<EchoReply>() + payload.len() + 16];
        let n = IcmpSendEcho(h, u32::from_ne_bytes(ip.octets()), payload.as_ptr(), payload.len() as u16, std::ptr::null(), buf.as_mut_ptr(), buf.len() as u32, timeout_ms);
        IcmpCloseHandle(h);
        if n == 0 {
            return None;
        }
        let r = &*(buf.as_ptr() as *const EchoReply);
        (r.status == 0).then_some(r.rtt.max(1))
    }
}

/// A fresh DNS lookup (the resolver cache bypassed), through the system's DNS settings.
fn resolve(host: &str, timeout: Duration) -> Result<u32, String> {
    let (tx, rx) = mpsc::channel();
    let t = Instant::now();
    let host = host.to_string();
    std::thread::spawn(move || {
        let _ = tx.send(crate::dns::query_fresh(&host));
    });
    match rx.recv_timeout(timeout) {
        Ok(0) => Ok(t.elapsed().as_millis() as u32),
        Ok(9003) => Err("域名不存在（DNS 返回 NXDOMAIN）".into()),
        Ok(1460) => Err("DNS 服务器没有回应".into()),
        Ok(rc) => Err(format!("解析失败（错误 {rc}）")),
        Err(_) => Err("DNS 服务器没有回应".into()),
    }
}

/// Sends one HTTP request and returns the status code and the time to the first byte.
fn http(addr: SocketAddr, req: &str, timeout: Duration) -> Result<(u16, u32), String> {
    let t = Instant::now();
    let mut s = TcpStream::connect_timeout(&addr, timeout).map_err(|e| short_err(&e))?;
    let left = timeout.saturating_sub(t.elapsed()).max(Duration::from_millis(300));
    let _ = s.set_read_timeout(Some(left));
    let _ = s.set_write_timeout(Some(left));
    s.write_all(req.as_bytes()).map_err(|e| short_err(&e))?;
    let mut buf = [0u8; 64];
    let mut got = 0;
    while got < 12 {
        match s.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) => return Err(short_err(&e)),
        }
    }
    let head = String::from_utf8_lossy(&buf[..got]).to_string();
    let code = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| if got == 0 { "连接被对方关掉了".to_string() } else { "回应不是 HTTP".to_string() })?;
    Ok((code, t.elapsed().as_millis() as u32))
}

fn short_err(e: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match e.kind() {
        TimedOut | WouldBlock => "超时".into(),
        ConnectionRefused => "连接被拒绝".into(),
        ConnectionReset | ConnectionAborted => "连接被重置".into(),
        _ => match e.raw_os_error() {
            Some(10051) | Some(10065) => "网络不可达".into(),
            _ => e.to_string(),
        },
    }
}

/// The internet without the proxy: the web APIs of two public DNS services (plain IPs, so DNS
/// itself is not involved). Either answering is enough.
fn direct(timeout: Duration) -> Result<u32, String> {
    let targets = [
        ("119.29.29.29:80", "GET /d?dn=qq.com HTTP/1.1\r\nHost: 119.29.29.29\r\nConnection: close\r\n\r\n"),
        ("223.5.5.5:80", "GET /resolve?name=qq.com HTTP/1.1\r\nHost: 223.5.5.5\r\nConnection: close\r\n\r\n"),
    ];
    let (tx, rx) = mpsc::channel();
    for (a, req) in targets {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let r = http(a.parse().unwrap(), req, timeout);
            let _ = tx.send(r);
        });
    }
    drop(tx);
    let mut last = Err("超时".to_string());
    for _ in 0..2 {
        match rx.recv_timeout(timeout + Duration::from_millis(500)) {
            Ok(Ok((_, ms))) => return Ok(ms),
            Ok(Err(e)) => last = Err(e),
            Err(_) => break,
        }
    }
    last
}

/// A request through the proxy (its local port, or the TUN adapter when there is no port).
fn via_proxy(port: Option<u16>, timeout: Duration) -> Result<u32, String> {
    let r = match port {
        Some(p) => http(
            SocketAddr::from(([127, 0, 0, 1], p)),
            "GET http://www.gstatic.com/generate_204 HTTP/1.1\r\nHost: www.gstatic.com\r\nProxy-Connection: close\r\nConnection: close\r\n\r\n",
            timeout,
        ),
        None => {
            use std::net::ToSocketAddrs;
            let a = ("www.gstatic.com", 80).to_socket_addrs().map_err(|_| "解析 www.gstatic.com 失败".to_string())?.next().ok_or("解析 www.gstatic.com 失败")?;
            http(a, "GET /generate_204 HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: close\r\n\r\n", timeout)
        }
    }?;
    match r.0 {
        204 | 200 => Ok(r.1),
        502 | 503 | 504 => Err("代理连不上节点".into()),
        c => Err(format!("代理返回 {c}")),
    }
}

fn port_open(port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(800)).is_ok()
}

// ---------- the full check ----------

fn step(id: &'static str, label: &'static str) -> Step {
    Step { id, label, status: "wait", ..Default::default() }
}

fn fix(id: &'static str, label: impl Into<String>, primary: bool) -> Fix {
    Fix { id, label: label.into(), primary }
}

/// Runs every check in order; `progress` gets the report after each step.
pub fn diagnose(auto: bool, progress: &dyn Fn(&Report)) -> Report {
    let t0 = Instant::now();
    let inf = info();
    // a client, its core, a TUN adapter or a system proxy setting
    let proxied = inf.detected;
    let mut r = Report {
        at: now_ms(),
        running: true,
        auto,
        steps: vec![
            step("nic", "网卡"),
            step("gw", "路由器"),
            step("dns", "DNS"),
            step("net", "互联网"),
            step("core", "代理软件"),
            step("node", "节点"),
        ],
        ..Default::default()
    };
    if !proxied {
        r.steps[4].status = "skip";
        r.steps[4].detail = "没有检测到代理软件".into();
        r.steps[5].status = "skip";
    }
    let set = |r: &mut Report, i: usize, status: &'static str, detail: String, ms: Option<u32>| {
        r.steps[i].status = status;
        r.steps[i].detail = detail;
        r.steps[i].ms = ms;
    };
    progress(&r);

    // 1 adapter
    r.steps[0].status = "run";
    progress(&r);
    let up = uplink();
    match &up {
        Some((name, _)) => set(&mut r, 0, "ok", name.clone(), None),
        None => set(&mut r, 0, "fail", "没有连上的网卡".into(), None),
    }
    progress(&r);

    // 2 router
    if let Some((_, gw)) = up {
        r.steps[1].status = "run";
        progress(&r);
        match ping(gw, 900).or_else(|| ping(gw, 900)) {
            Some(ms) => set(&mut r, 1, "ok", gw.to_string(), Some(ms)),
            None => set(&mut r, 1, "warn", format!("{gw} 不回应 ping"), None),
        }
    } else {
        set(&mut r, 1, "skip", String::new(), None);
    }
    progress(&r);

    // 3 + 4 DNS and the internet (side by side: a dead line makes both wait)
    r.steps[2].status = "run";
    r.steps[3].status = "run";
    progress(&r);
    let dns_t = std::thread::spawn(|| resolve("www.baidu.com", Duration::from_secs(4)).or_else(|_| resolve("www.qq.com", Duration::from_secs(3))));
    let net = direct(Duration::from_secs(4));
    let dns = dns_t.join().unwrap_or_else(|_| Err("出错".into()));
    match &dns {
        Ok(ms) => set(&mut r, 2, "ok", "解析正常".into(), Some(*ms)),
        Err(e) => set(&mut r, 2, "fail", e.clone(), None),
    }
    match &net {
        Ok(ms) => set(&mut r, 3, "ok", "直连畅通".into(), Some(*ms)),
        Err(e) => set(&mut r, 3, "fail", e.clone(), None),
    }
    progress(&r);

    // 5 + 6 proxy
    let mut core_ok = true;
    if proxied {
        r.steps[4].status = "run";
        progress(&r);
        let name = inf.client.clone().unwrap_or_else(|| "代理".into());
        let port = inf.local_port.or_else(|| inf.system_proxy.as_deref().and_then(proxy::port_from_proxy_server));
        match port {
            Some(p) if port_open(p) => set(&mut r, 4, "ok", format!("{name} · 端口 {p}"), None),
            Some(p) => {
                core_ok = false;
                let why = if inf.client_running { String::new() } else { format!("（{name} 没在运行）") };
                set(&mut r, 4, "fail", format!("端口 {p} 没有程序在监听{why}"), None);
            }
            None if inf.core_running => set(&mut r, 4, "ok", format!("{name} · TUN"), None),
            None => {
                core_ok = false;
                set(&mut r, 4, "fail", format!("{name} 的内核没在运行"), None);
            }
        }
        progress(&r);
        if core_ok {
            r.steps[5].status = "run";
            progress(&r);
            let node = inf.node.clone().unwrap_or_default();
            match via_proxy(if inf.tun && inf.local_port.is_none() { None } else { port }, Duration::from_secs(6)) {
                Ok(ms) => set(&mut r, 5, "ok", node, Some(ms)),
                Err(e) => set(&mut r, 5, "fail", if node.is_empty() { e } else { format!("{node} · {e}") }, None),
            }
        } else {
            set(&mut r, 5, "skip", String::new(), None);
        }
        progress(&r);
    }

    // verdict: the first broken link explains the rest
    let st = |i: usize| r.steps[i].status;
    let (problem, verdict, advice, fixes): (Option<&str>, String, String, Vec<Fix>) = if st(0) == "fail" {
        (Some("没连网"), "电脑没有连上网络".into(), "检查网线是否插好、Wi-Fi 是否连上，飞行模式有没有打开。".into(), vec![fix("netsettings", "打开网络设置", true)])
    } else if proxied && inf.tun && !core_ok {
        (Some("代理挂了"), "TUN 模式开着，但代理软件没在工作".into(), "所有流量都要经过代理软件，它一停就全断了。重启代理软件，或者先把 TUN 关掉。".into(), proxy_fixes(&inf))
    } else if st(3) == "fail" && st(1) == "warn" {
        (Some("路由器连不上"), "连不上路由器，外网也不通".into(), "看看路由器 / 光猫的灯是否正常，重启一下路由器通常能好；用 Wi-Fi 的话试试重新连接。".into(), vec![fix("netsettings", "打开网络设置", true), fix("retry", "再测一次", false)])
    } else if st(3) == "fail" {
        (Some("宽带断了"), "路由器是通的，但外网不通".into(), "多半是宽带这一段的问题：光猫掉线、欠费或运营商故障。重启光猫试试，不行就联系运营商。".into(), vec![fix("retry", "再测一次", true)])
    } else if st(2) == "fail" {
        (Some("DNS 坏了"), "能连外网，但域名解析失败".into(), "网址变不成 IP，所以网页打不开。先刷新 DNS 缓存；还不行就把网卡的 DNS 改成 223.5.5.5 / 119.29.29.29。".into(), vec![fix("flushdns", "刷新 DNS 缓存", true), fix("netsettings", "打开网络设置", false)])
    } else if proxied && !core_ok {
        (Some("代理挂了"), format!("{} 没在工作", inf.client.clone().unwrap_or_else(|| "代理软件".into())), "直连是好的，只有走代理的网站打不开。重启一下代理软件。".into(), proxy_fixes(&inf))
    } else if proxied && st(5) == "fail" {
        let mut f = Vec::new();
        if inf.controller {
            f.push(fix("switch-node", "换一个能用的节点", true));
        }
        f.push(fix("retry", "再测一次", !inf.controller));
        (Some("节点挂了"), format!("节点 {} 连不上", inf.node.clone().unwrap_or_default()).replace("节点  ", "节点 "), "直连正常、代理软件也在运行，是当前节点不通。换一个节点，或者在代理软件里更新订阅。".into(), f)
    } else {
        (None, "网络正常".into(), String::new(), Vec::new())
    };
    r.ok = problem.is_none();
    r.problem = problem.map(|s| s.to_string());
    r.kind = if r.ok { String::new() } else { "net".into() };
    r.verdict = verdict;
    r.advice = advice;
    r.fixes = fixes;
    r.running = false;
    r.took_ms = t0.elapsed().as_millis() as u32;
    r.at = now_ms();
    log(format!("netdiag: {} ({}) {}", if r.ok { "ok" } else { r.problem.as_deref().unwrap_or("") }, if auto { "auto" } else { "manual" }, r.steps.iter().map(|s| format!("{}={}", s.id, s.status)).collect::<Vec<_>>().join(" ")));
    r
}

fn proxy_fixes(inf: &Info) -> Vec<Fix> {
    let name = inf.client.clone().unwrap_or_else(|| "代理软件".into());
    let mut f = Vec::new();
    if inf.clash_party {
        f.push(fix("restart-proxy", format!("重启 {name}"), true));
    } else if inf.client_exe.is_some() {
        f.push(fix("start-proxy", format!("打开 {name}"), true));
    }
    f.push(fix("retry", "再测一次", f.is_empty()));
    f
}

/// The system proxy problem as a report of its own (no chain checks needed).
fn sysproxy_report(good: &str, now_value: Option<&str>, client: &str) -> Report {
    let verdict = match now_value {
        None => "系统代理被关掉了".to_string(),
        Some(v) => format!("系统代理被改成了 {v}"),
    };
    Report {
        at: now_ms(),
        auto: true,
        ok: false,
        problem: Some(if now_value.is_none() { "系统代理被关了".into() } else { "系统代理被改了".into() }),
        kind: "sysproxy".into(),
        verdict,
        advice: format!("{client} 还在运行，但浏览器等程序已经不走它了（原来是 {good}）。常见原因是别的软件改了系统代理。"),
        fixes: vec![fix("restore-sysproxy", format!("恢复成 {good}"), true), fix("ignore-sysproxy", "是我关的", false)],
        ..Default::default()
    }
}

// ---------- fixes that need no proxy access ----------

pub fn flush_dns() -> Result<(), String> {
    let out = std::process::Command::new("ipconfig").arg("/flushdns").creation_flags(CREATE_NO_WINDOW).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        log("netdiag: dns cache flushed");
        Ok(())
    } else {
        Err("刷新 DNS 缓存失败".into())
    }
}

pub fn open_net_settings() {
    let _ = std::process::Command::new("explorer.exe").arg("ms-settings:network-status").spawn();
}

/// Points the WinINet system proxy back at `server` and tells running programs.
pub fn set_system_proxy(server: &str) -> Result<(), String> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
    let k = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings", KEY_SET_VALUE)
        .map_err(|e| format!("打不开系统代理设置：{e}"))?;
    k.set_value("ProxyServer", &server.to_string()).map_err(|e| e.to_string())?;
    k.set_value("ProxyEnable", &1u32).map_err(|e| e.to_string())?;
    #[link(name = "wininet")]
    extern "system" {
        fn InternetSetOptionW(h: *mut u8, option: u32, buf: *mut u8, len: u32) -> i32;
    }
    unsafe {
        // INTERNET_OPTION_SETTINGS_CHANGED, INTERNET_OPTION_REFRESH
        InternetSetOptionW(std::ptr::null_mut(), 39, std::ptr::null_mut(), 0);
        InternetSetOptionW(std::ptr::null_mut(), 37, std::ptr::null_mut(), 0);
    }
    log(format!("netdiag: system proxy restored to {server}"));
    Ok(())
}

pub fn restore_system_proxy() -> Result<(), String> {
    let good = W.lock().unwrap().good_proxy.clone().ok_or("不知道原来的系统代理是什么")?;
    set_system_proxy(&good)?;
    clear_sysproxy();
    Ok(())
}

/// "是我关的": no more warnings until the system proxy points at the client again.
pub fn ignore_system_proxy() {
    let mut w = W.lock().unwrap();
    w.proxy_ignored = true;
    w.proxy_off_since = None;
    drop(w);
    clear_sysproxy();
}

fn clear_sysproxy() {
    let cleared = {
        let mut w = W.lock().unwrap();
        if problem_kind(&w).as_deref() == Some("sysproxy") {
            w.problem = None;
            true
        } else {
            false
        }
    };
    if cleared {
        publish();
    }
}

// ---------- running it ----------

/// The button: runs the full check now (progress goes to the page as it happens).
pub fn run_manual() -> Report {
    {
        let mut w = W.lock().unwrap();
        if w.running {
            return w.last.clone().unwrap_or_default();
        }
        w.running = true;
    }
    publish();
    let r = diagnose(false, &|p| crate::notify::emit("netdiag-progress", p.clone()));
    finish(r)
}

/// Stores a finished check: a network problem starts / continues, or a showing one clears.
fn finish(mut r: Report) -> Report {
    let mut w = W.lock().unwrap();
    w.running = false;
    let now = now_ms();
    let net_showing = problem_kind(&w).as_deref() == Some("net");
    if r.ok {
        if net_showing {
            r.recovered_ms = w.problem.as_ref().and_then(|p| p.down_since).map(|d| now.saturating_sub(d));
            w.problem = None;
        }
    } else {
        r.down_since = if net_showing { w.problem.as_ref().and_then(|p| p.down_since) } else { None }.or(Some(now));
        // a network outage outranks a changed system proxy
        w.problem = Some(r.clone());
    }
    w.last = Some(r.clone());
    drop(w);
    publish();
    r
}

fn watch_loop() {
    let started = Instant::now();
    let mut next_direct = Instant::now() + Duration::from_secs(20);
    let mut next_proxy = Instant::now() + Duration::from_secs(30);
    let mut direct_fail = 0u32;
    let mut proxy_fail = 0u32;
    let mut good_streak = 0u32;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        if !crate::settings::bool_at("diag.on", true) {
            continue;
        }
        let inf = info();

        // --- the system proxy ---
        system_proxy_watch(&inf);

        // --- quiet probes ---
        let showing = problem_kind(&W.lock().unwrap()).as_deref() == Some("net");
        let saver = crate::settings::str_at("general.profile", "balanced") == "saver";
        let every = Duration::from_secs(if showing { 5 } else if saver { 20 } else { 10 });
        let now = Instant::now();
        let mut probed = false;
        let mut failed = false;
        if now >= next_direct {
            probed = true;
            if direct(Duration::from_secs(3)).is_err() {
                direct_fail += 1;
                failed = true;
            } else {
                direct_fail = 0;
            }
            // a first failure is checked again soon
            next_direct = Instant::now() + if direct_fail == 1 { Duration::from_secs(4) } else { every };
        }
        let proxied = inf.detected && inf.local_port.is_some() && !inf.tun;
        // the system proxy still points at a local port nobody listens on (the client quit or
        // crashed without clearing it): browsers can't load anything
        let sys_port = if inf.tun || proxied { None } else { inf.system_proxy.as_deref().and_then(proxy::port_from_proxy_server) };
        if (proxied || sys_port.is_some()) && Instant::now() >= next_proxy {
            probed = true;
            let bad = match sys_port {
                Some(p) => !port_open(p),
                None => via_proxy(inf.local_port, Duration::from_secs(6)).is_err(),
            };
            if bad {
                proxy_fail += 1;
                failed = true;
            } else {
                proxy_fail = 0;
            }
            next_proxy = Instant::now() + if proxy_fail == 1 { Duration::from_secs(5) } else if showing { every } else { Duration::from_secs(if saver { 60 } else { 30 }) };
        } else if !proxied && sys_port.is_none() {
            proxy_fail = 0;
        }
        if !probed || started.elapsed() < Duration::from_secs(20) {
            continue;
        }
        if showing {
            // back to normal after two good rounds in a row: confirm with the full check
            good_streak = if failed { 0 } else { good_streak + 1 };
            if good_streak >= 2 && claim() {
                good_streak = 0;
                finish(diagnose(true, &|_| {}));
            }
        } else if (direct_fail >= 2 || proxy_fail >= 2) && claim() {
            direct_fail = 0;
            proxy_fail = 0;
            let r = diagnose(true, &|_| {});
            if r.ok {
                // a blip: nothing to report
                W.lock().unwrap().running = false;
            } else {
                finish(r);
            }
        }
    }
}

/// Takes the "a check is running" flag; false when one already runs.
fn claim() -> bool {
    let mut w = W.lock().unwrap();
    if w.running {
        return false;
    }
    w.running = true;
    true
}

fn system_proxy_watch(inf: &Info) {
    let mut emit = false;
    {
        let mut w = W.lock().unwrap();
        let watching = inf.detected && !inf.tun && inf.client_running && inf.local_port.is_some();
        if !watching {
            w.proxy_off_since = None;
            if !inf.client_running {
                w.good_proxy = None;
                w.proxy_ignored = false;
            }
        } else {
            let port = inf.local_port.unwrap();
            let points_here = inf.system_proxy.as_deref().and_then(proxy::port_from_proxy_server) == Some(port);
            let sys_problem = problem_kind(&w).as_deref() == Some("sysproxy");
            if points_here {
                w.good_proxy = inf.system_proxy.clone();
                w.proxy_off_since = None;
                w.proxy_ignored = false;
                if sys_problem {
                    drop(w);
                    clear_sysproxy();
                    return;
                }
            } else if let Some(good) = w.good_proxy.clone() {
                if !w.proxy_ignored && !sys_problem {
                    let since = *w.proxy_off_since.get_or_insert_with(Instant::now);
                    // a client switching modes passes through "off" for a moment
                    if since.elapsed() >= Duration::from_secs(6) && w.problem.is_none() {
                        let mut r = sysproxy_report(&good, inf.system_proxy.as_deref(), inf.client.as_deref().unwrap_or("代理软件"));
                        r.down_since = Some(now_ms());
                        log(format!("netdiag: system proxy changed: {:?} (was {good})", inf.system_proxy));
                        w.problem = Some(r);
                        emit = true;
                    }
                }
            }
        }
    }
    if emit {
        publish();
    }
}

pub fn start(info: InfoFn) {
    *INFO.lock().unwrap() = Some(info);
    std::thread::spawn(watch_loop);
}
