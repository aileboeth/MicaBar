//! Network diagnosis: when the internet goes away, tell which link of the chain broke
//! (network adapter → router → DNS → internet → proxy software → node) and offer a fix.
//!
//! A watcher probes quietly (one small HTTP request to a public DNS server's web API every
//! 10 s, one request through the proxy every 30 s); two failures in a row run the full check.
//! It also keeps an eye on the system proxy: switched off or pointed elsewhere while the proxy
//! client still runs (another program changed it) is reported with a one-click restore.

use std::ffi::c_void;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::os::windows::io::{FromRawSocket, RawSocket};
use std::os::windows::process::CommandExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, Once, OnceLock};
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
    pub label: String,
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
static WAKE: OnceLock<(Mutex<u8>, Condvar)> = OnceLock::new();

const WAKE_NET: u8 = 1;
const WAKE_RESUME: u8 = 2;

fn wake_pair() -> &'static (Mutex<u8>, Condvar) {
    WAKE.get_or_init(|| (Mutex::new(0), Condvar::new()))
}

/// Signals that an adapter, IP address, route or proxy state just changed so probes run promptly.
pub fn notify_network_change() {
    let (lock, cvar) = wake_pair();
    if let Ok(mut f) = lock.lock() {
        *f |= WAKE_NET;
        cvar.notify_one();
    }
}

/// Signals that Windows just resumed from sleep / hibernate.
pub fn notify_resume() {
    let (lock, cvar) = wake_pair();
    if let Ok(mut f) = lock.lock() {
        *f |= WAKE_RESUME;
        cvar.notify_one();
    }
}

fn client_file() -> std::path::PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    let dir = std::path::PathBuf::from(base).join("MicaBar");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("proxy-client.txt")
}

/// The proxy client's exe as last seen running (kept on disk: after a restart of MicaBar the
/// client may be the one that is gone).
pub fn remember_client(exe: &str) {
    let mut g = CLIENT_EXE.lock().unwrap();
    if g.as_deref() != Some(exe) {
        *g = Some(exe.to_string());
        let _ = crate::settings::atomic_write(&client_file(), exe.as_bytes());
    }
}

pub fn client_exe() -> Option<String> {
    let mut g = CLIENT_EXE.lock().unwrap();
    if g.is_none() {
        *g = std::fs::read_to_string(client_file()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && std::path::Path::new(s).exists());
    }
    g.clone()
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

/// The real uplink: up, not a TUN or virtual adapter, with an IPv4 gateway (lowest metric).
fn uplink() -> Option<(String, Ipv4Addr)> {
    let ads = ipconfig::get_adapters().ok()?;
    let mut best: Option<(u32, String, Ipv4Addr)> = None;
    for a in &ads {
        if a.oper_status() != ipconfig::OperStatus::IfOperStatusUp || proxy::looks_like_tun(a) || proxy::is_virtual_nic(a) {
            continue;
        }
        let has_usable_v4 = a.ip_addresses().iter().any(|i| match i {
            IpAddr::V4(v) => !v.is_loopback() && !v.is_link_local() && !v.is_unspecified(),
            _ => false,
        });
        if !has_usable_v4 {
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

/// An up adapter with a usable IPv4 address but no gateway listed (Dr.COM / PPPoE dial-up, some
/// tethering): the line can still work, there is just no router to ping.
fn uplink_no_gateway() -> Option<String> {
    proxy::uplink_without_gateway().map(|(name, _)| name)
}

/// Every adapter for the log when none looked like the uplink (so the next report says why).
fn adapters_summary() -> String {
    let Ok(ads) = ipconfig::get_adapters() else { return "adapter list failed".into() };
    ads.iter()
        .map(|a| {
            let v4 = a.ip_addresses().iter().filter(|i| i.is_ipv4()).count();
            let gw4 = a.gateways().iter().filter(|g| g.is_ipv4() && !g.is_unspecified()).count();
            format!(
                "[{} | {} | {:?} | {:?} | v4={v4} gw4={gw4} tun={}]",
                a.friendly_name(),
                a.description(),
                a.if_type(),
                a.oper_status(),
                proxy::looks_like_tun(a)
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One ICMP echo; round trip in ms.
fn ping(ip: Ipv4Addr, timeout_ms: u32) -> Option<u32> {
    #[repr(C)]
    struct IcmpEchoReply32 {
        address: u32,
        status: u32,
        rtt: u32,
        data_size: u16,
        reserved: u16,
        data: u32,
        options: [u8; 12],
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
        // u64-aligned buffer large enough for both 32-bit and 64-bit ICMP_ECHO_REPLY + payload
        let mut buf = vec![0u64; 16];
        let reply_bytes = (buf.len() * 8) as u32;
        let n = IcmpSendEcho(
            h,
            u32::from_ne_bytes(ip.octets()),
            payload.as_ptr(),
            payload.len() as u16,
            std::ptr::null(),
            buf.as_mut_ptr() as *mut u8,
            reply_bytes,
            timeout_ms,
        );
        IcmpCloseHandle(h);
        if n == 0 {
            return None;
        }
        let r = &*(buf.as_ptr() as *const IcmpEchoReply32);
        (r.status == 0).then_some(r.rtt.max(1))
    }
}

static RESOLVE_WORKERS: AtomicUsize = AtomicUsize::new(0);

struct ResolveGuard;
impl Drop for ResolveGuard {
    fn drop(&mut self) {
        RESOLVE_WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A fresh DNS lookup (the resolver cache bypassed), through the system's DNS settings.
/// Capped at 2 concurrent background threads so DNS outages never leak dozens of threads.
fn resolve(host: &str, timeout: Duration) -> Result<u32, String> {
    if RESOLVE_WORKERS
        .try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < 2).then_some(n + 1))
        .is_err()
    {
        return Err(crate::i18n::t("DNS 服务器没有回应").into());
    }
    let (tx, rx) = mpsc::channel();
    let t = Instant::now();
    let host = host.to_string();
    std::thread::spawn(move || {
        let _guard = ResolveGuard;
        let _ = tx.send(crate::dns::query_fresh(&host));
    });
    match rx.recv_timeout(timeout) {
        Ok(0) => Ok(t.elapsed().as_millis() as u32),
        Ok(9003) => Err(crate::i18n::t("域名不存在（DNS 返回 NXDOMAIN）").into()),
        Ok(1460) => Err(crate::i18n::t("DNS 服务器没有回应").into()),
        Ok(rc) => Err(crate::tr!("解析失败（错误 {0}）", rc)),
        Err(_) => Err(crate::i18n::t("DNS 服务器没有回应").into()),
    }
}

fn probe_dns(timeout: Duration) -> Result<u32, String> {
    let zh = crate::i18n::lang().starts_with("zh");
    if zh {
        resolve("www.baidu.com", timeout)
            .or_else(|_| resolve("www.qq.com", timeout))
            .or_else(|_| resolve("cloudflare.com", timeout))
    } else {
        resolve("cloudflare.com", timeout)
            .or_else(|_| resolve("www.gstatic.com", timeout))
            .or_else(|_| resolve("www.baidu.com", timeout))
    }
}

fn resolve_socket_timed(host: &str, port: u16, timeout: Duration) -> Result<SocketAddr, String> {
    if RESOLVE_WORKERS
        .try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < 2).then_some(n + 1))
        .is_err()
    {
        return Err(crate::i18n::t("DNS 服务器没有回应").into());
    }
    let (tx, rx) = mpsc::channel();
    let host_owned = host.to_string();
    std::thread::spawn(move || {
        let _guard = ResolveGuard;
        use std::net::ToSocketAddrs;
        let r = (host_owned.as_str(), port)
            .to_socket_addrs()
            .ok()
            .and_then(|mut it| it.next());
        let _ = tx.send(r);
    });
    match rx.recv_timeout(timeout) {
        Ok(Some(addr)) => Ok(addr),
        _ => Err(crate::i18n::t("解析 www.gstatic.com 失败").into()),
    }
}

fn connect_bound_v4(remote: SocketAddrV4, local_ip: Option<Ipv4Addr>, timeout: Duration) -> io::Result<TcpStream> {
    let Some(lip) = local_ip else {
        return TcpStream::connect_timeout(&SocketAddr::V4(remote), timeout);
    };
    #[repr(C)]
    struct SockAddrIn {
        sin_family: i16,
        sin_port: u16,
        sin_addr: [u8; 4],
        sin_zero: [u8; 8],
    }
    #[repr(C)]
    struct FdSet1 {
        fd_count: u32,
        fd_array: [usize; 1],
    }
    #[repr(C)]
    struct TimeVal {
        tv_sec: i32,
        tv_usec: i32,
    }
    #[link(name = "ws2_32")]
    extern "system" {
        fn WSAStartup(ver: u16, data: *mut u8) -> i32;
        fn socket(af: i32, stype: i32, proto: i32) -> usize;
        fn bind(s: usize, addr: *const c_void, namelen: i32) -> i32;
        fn connect(s: usize, addr: *const c_void, namelen: i32) -> i32;
        fn ioctlsocket(s: usize, cmd: i32, argp: *mut u32) -> i32;
        fn select(nfds: i32, rfds: *mut FdSet1, wfds: *mut FdSet1, efds: *mut FdSet1, timeout: *const TimeVal) -> i32;
        fn getsockopt(s: usize, level: i32, optname: i32, optval: *mut u8, optlen: *mut i32) -> i32;
        fn closesocket(s: usize) -> i32;
        fn WSAGetLastError() -> i32;
    }
    static WSA_INIT: Once = Once::new();
    WSA_INIT.call_once(|| unsafe {
        let mut wsa = [0u64; 64];
        let _ = WSAStartup(0x0202, wsa.as_mut_ptr() as *mut u8);
    });

    const AF_INET_I32: i32 = 2;
    const SOCK_STREAM_I32: i32 = 1;
    const IPPROTO_TCP_I32: i32 = 6;
    const FIONBIO_CMD: i32 = 0x8004_667e_u32 as i32;
    const WSAEWOULDBLOCK_ERR: i32 = 10035;
    const SOL_SOCKET_LVL: i32 = 0xffff;
    const SO_ERROR_OPT: i32 = 0x1007;

    unsafe {
        let s = socket(AF_INET_I32, SOCK_STREAM_I32, IPPROTO_TCP_I32);
        if s == !0usize {
            return Err(io::Error::from_raw_os_error(WSAGetLastError()));
        }
        let local_sa = SockAddrIn {
            sin_family: AF_INET_I32 as i16,
            sin_port: 0,
            sin_addr: lip.octets(),
            sin_zero: [0; 8],
        };
        if bind(s, &local_sa as *const _ as *const c_void, std::mem::size_of::<SockAddrIn>() as i32) != 0 {
            closesocket(s);
            return TcpStream::connect_timeout(&SocketAddr::V4(remote), timeout);
        }
        let mut nonblock = 1u32;
        let _ = ioctlsocket(s, FIONBIO_CMD, &mut nonblock);

        let rem_sa = SockAddrIn {
            sin_family: AF_INET_I32 as i16,
            sin_port: remote.port().to_be(),
            sin_addr: remote.ip().octets(),
            sin_zero: [0; 8],
        };
        let rc = connect(s, &rem_sa as *const _ as *const c_void, std::mem::size_of::<SockAddrIn>() as i32);
        if rc != 0 {
            let err = WSAGetLastError();
            if err != WSAEWOULDBLOCK_ERR {
                closesocket(s);
                return Err(io::Error::from_raw_os_error(err));
            }
            let mut wfds = FdSet1 { fd_count: 1, fd_array: [s] };
            let mut efds = FdSet1 { fd_count: 1, fd_array: [s] };
            let tv = TimeVal {
                tv_sec: timeout.as_secs().min(60) as i32,
                tv_usec: timeout.subsec_micros() as i32,
            };
            let sel = select(0, std::ptr::null_mut(), &mut wfds, &mut efds, &tv);
            if sel <= 0 {
                let e = if sel == 0 {
                    io::Error::new(io::ErrorKind::TimedOut, "timed out")
                } else {
                    io::Error::from_raw_os_error(WSAGetLastError())
                };
                closesocket(s);
                return Err(e);
            }
            let mut so_err: i32 = 0;
            let mut optlen = std::mem::size_of::<i32>() as i32;
            if getsockopt(s, SOL_SOCKET_LVL, SO_ERROR_OPT, &mut so_err as *mut i32 as *mut u8, &mut optlen) == 0 && so_err != 0 {
                closesocket(s);
                return Err(io::Error::from_raw_os_error(so_err));
            }
            if efds.fd_count > 0 && wfds.fd_count == 0 {
                closesocket(s);
                return Err(io::Error::new(io::ErrorKind::ConnectionRefused, "connect failed"));
            }
        }
        let mut block = 0u32;
        let _ = ioctlsocket(s, FIONBIO_CMD, &mut block);
        Ok(TcpStream::from_raw_socket(s as RawSocket))
    }
}

fn http_with_stream(mut s: TcpStream, t: Instant, req: &str, timeout: Duration) -> Result<(u16, u32), String> {
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
    let code = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| {
            if got == 0 {
                crate::i18n::t("连接被对方关掉了").to_string()
            } else {
                crate::i18n::t("回应不是 HTTP").to_string()
            }
        })?;
    Ok((code, t.elapsed().as_millis() as u32))
}

/// Sends one HTTP request and returns the status code and the time to the first byte.
fn http(addr: SocketAddr, req: &str, timeout: Duration) -> Result<(u16, u32), String> {
    let t = Instant::now();
    let s = TcpStream::connect_timeout(&addr, timeout).map_err(|e| short_err(&e))?;
    http_with_stream(s, t, req, timeout)
}

fn http_bound_v4(addr: SocketAddrV4, local_ip: Option<Ipv4Addr>, req: &str, timeout: Duration) -> Result<(u16, u32), String> {
    let t = Instant::now();
    let s = connect_bound_v4(addr, local_ip, timeout).map_err(|e| short_err(&e))?;
    http_with_stream(s, t, req, timeout)
}

fn short_err(e: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match e.kind() {
        TimedOut | WouldBlock => crate::i18n::t("超时").into(),
        ConnectionRefused => crate::i18n::t("连接被拒绝").into(),
        ConnectionReset | ConnectionAborted => crate::i18n::t("连接被重置").into(),
        _ => match e.raw_os_error() {
            Some(10051) | Some(10065) => crate::i18n::t("网络不可达").into(),
            _ => e.to_string(),
        },
    }
}

/// The internet without the proxy: bound to the physical IPv4 NIC so TUN Global mode is bypassed.
/// Probes two domestic public DNS HTTP APIs plus 1.1.1.1:80 for overseas / non-CN networks.
fn direct(timeout: Duration) -> Result<u32, String> {
    let local_v4 = proxy::physical_ipv4().and_then(|(_, ip)| match ip {
        IpAddr::V4(v4) => Some(v4),
        _ => None,
    });
    let targets: [(SocketAddrV4, &str); 3] = [
        (
            SocketAddrV4::new(Ipv4Addr::new(119, 29, 29, 29), 80),
            "GET /d?dn=qq.com HTTP/1.1\r\nHost: 119.29.29.29\r\nConnection: close\r\n\r\n",
        ),
        (
            SocketAddrV4::new(Ipv4Addr::new(223, 5, 5, 5), 80),
            "GET /resolve?name=qq.com HTTP/1.1\r\nHost: 223.5.5.5\r\nConnection: close\r\n\r\n",
        ),
        (
            SocketAddrV4::new(Ipv4Addr::new(1, 1, 1, 1), 80),
            "HEAD / HTTP/1.1\r\nHost: one.one.one.one\r\nConnection: close\r\n\r\n",
        ),
    ];
    let (tx, rx) = mpsc::channel();
    for (a, req) in targets {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let r = http_bound_v4(a, local_v4, req, timeout);
            let _ = tx.send(r);
        });
    }
    drop(tx);
    let mut last = Err(crate::i18n::t("超时").to_string());
    for _ in 0..targets.len() {
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
            let t0 = Instant::now();
            let a = resolve_socket_timed("www.gstatic.com", 80, timeout.min(Duration::from_secs(3)))?;
            let rem = timeout.saturating_sub(t0.elapsed()).max(Duration::from_millis(500));
            http(a, "GET /generate_204 HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: close\r\n\r\n", rem)
        }
    }?;
    match r.0 {
        204 | 200 => Ok(r.1),
        502 | 503 | 504 => Err(crate::i18n::t("代理连不上节点").into()),
        c => Err(crate::tr!("代理返回 {0}", c)),
    }
}

fn port_open(port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(800)).is_ok()
}

// ---------- the full check ----------

fn step(id: &'static str, label: &'static str) -> Step {
    Step { id, label: crate::i18n::t(label), status: "wait", ..Default::default() }
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
        r.steps[4].detail = crate::i18n::t("没有检测到代理软件").into();
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
        None => {
            log(format!("netdiag: no adapter with an IPv4 gateway: {}", adapters_summary()));
            match uplink_no_gateway() {
                Some(name) => set(&mut r, 0, "ok", name, None),
                None => set(&mut r, 0, "fail", crate::i18n::t("没有连上的网卡").into(), None),
            }
        }
    }
    progress(&r);

    // 2 router
    if let Some((_, gw)) = up {
        r.steps[1].status = "run";
        progress(&r);
        match ping(gw, 900).or_else(|| ping(gw, 900)) {
            Some(ms) => set(&mut r, 1, "ok", gw.to_string(), Some(ms)),
            None => set(&mut r, 1, "warn", crate::tr!("{0} 不回应 ping", gw), None),
        }
    } else {
        set(&mut r, 1, "skip", String::new(), None);
    }
    progress(&r);

    // 3 + 4 DNS and the internet (side by side: a dead line makes both wait)
    r.steps[2].status = "run";
    r.steps[3].status = "run";
    progress(&r);
    let dns_t = std::thread::spawn(|| probe_dns(Duration::from_secs(3)));
    let net = direct(Duration::from_secs(4));
    let dns = dns_t.join().unwrap_or_else(|_| Err(crate::i18n::t("出错").into()));
    match &dns {
        Ok(ms) => set(&mut r, 2, "ok", crate::i18n::t("解析正常").into(), Some(*ms)),
        Err(e) => set(&mut r, 2, "fail", e.clone(), None),
    }
    match &net {
        Ok(ms) => set(&mut r, 3, "ok", crate::i18n::t("直连畅通").into(), Some(*ms)),
        Err(e) => set(&mut r, 3, "fail", e.clone(), None),
    }
    // the internet answers, so the adapter works even when it wasn't recognised
    if r.steps[0].status == "fail" && net.is_ok() {
        set(&mut r, 0, "warn", crate::i18n::t("没认出是哪块网卡，但能连上外网").into(), None);
    }
    progress(&r);

    // 5 + 6 proxy
    let mut core_ok = true;
    if proxied {
        r.steps[4].status = "run";
        progress(&r);
        let name = inf.client.clone().unwrap_or_else(|| crate::i18n::t("代理").into());
        let port = inf.local_port.or_else(|| inf.system_proxy.as_deref().and_then(proxy::port_from_proxy_server));
        match port {
            Some(p) if port_open(p) => set(&mut r, 4, "ok", crate::tr!("{0} · 端口 {1}", name, p), None),
            Some(p) => {
                core_ok = false;
                let why = if inf.client_running { String::new() } else { crate::tr!("（{0} 没在运行）", name) };
                set(&mut r, 4, "fail", crate::tr!("端口 {0} 没有程序在监听{1}", p, why), None);
            }
            None if inf.core_running => set(&mut r, 4, "ok", format!("{name} · TUN"), None),
            None => {
                core_ok = false;
                set(&mut r, 4, "fail", crate::tr!("{0} 的内核没在运行", name), None);
            }
        }
        progress(&r);
        if core_ok {
            r.steps[5].status = "run";
            progress(&r);
            let node = inf.node.clone().unwrap_or_default();
            match via_proxy(if inf.tun && inf.local_port.is_none() { None } else { port }, Duration::from_secs(6)) {
                Ok(ms) => {
                    set(&mut r, 5, "ok", node, Some(ms));
                    // If the proxy node over the internet answered, broadband/uplink is definitely NOT dead
                    // (direct HTTP port 80 may simply be blocked by an overseas/strict firewall).
                    if r.steps[3].status == "fail" {
                        r.steps[3].status = "warn";
                    }
                    if r.steps[0].status == "fail" {
                        set(&mut r, 0, "warn", crate::i18n::t("没认出是哪块网卡，但能连上外网").into(), None);
                    }
                }
                Err(e) => set(&mut r, 5, "fail", if node.is_empty() { e } else { format!("{node} · {e}") }, None),
            }
        } else {
            set(&mut r, 5, "skip", String::new(), None);
        }
        progress(&r);
    }

    // verdict: the first broken link explains the rest
    let st = |i: usize| r.steps[i].status;
    let (problem, verdict, advice, fixes): (Option<String>, String, String, Vec<Fix>) = if st(0) == "fail" {
        (Some(crate::i18n::t("没连网")), crate::i18n::t("电脑没有连上网络").into(), crate::i18n::t("检查网线是否插好、Wi-Fi 是否连上，飞行模式有没有打开。").into(), vec![fix("netsettings", crate::i18n::t("打开网络设置"), true)])
    } else if proxied && inf.tun && !core_ok {
        (Some(crate::i18n::t("代理挂了")), crate::i18n::t("TUN 模式开着，但代理软件没在工作").into(), crate::i18n::t("所有流量都要经过代理软件，它一停就全断了。重启代理软件，或者先把 TUN 关掉。").into(), proxy_fixes(&inf))
    } else if st(3) == "fail" && st(1) == "warn" {
        (Some(crate::i18n::t("路由器连不上")), crate::i18n::t("连不上路由器，外网也不通").into(), crate::i18n::t("看看路由器 / 光猫的灯是否正常，重启一下路由器通常能好；用 Wi-Fi 的话试试重新连接。").into(), vec![fix("netsettings", crate::i18n::t("打开网络设置"), true), fix("retry", crate::i18n::t("再测一次"), false)])
    } else if st(3) == "fail" {
        (Some(crate::i18n::t("宽带断了")), crate::i18n::t("路由器是通的，但外网不通").into(), crate::i18n::t("多半是宽带这一段的问题：光猫掉线、欠费或运营商故障。重启光猫试试，不行就联系运营商。").into(), vec![fix("retry", crate::i18n::t("再测一次"), true)])
    } else if st(2) == "fail" {
        (Some(crate::i18n::t("DNS 坏了")), crate::i18n::t("能连外网，但域名解析失败").into(), crate::i18n::t("网址变不成 IP，所以网页打不开。先刷新 DNS 缓存；还不行就把网卡的 DNS 改成 223.5.5.5 / 119.29.29.29。").into(), vec![fix("flushdns", crate::i18n::t("刷新 DNS 缓存"), true), fix("netsettings", crate::i18n::t("打开网络设置"), false)])
    } else if proxied && !core_ok {
        (
            Some(crate::i18n::t("代理挂了")),
            crate::tr!("{0} 没在工作", inf.client.clone().unwrap_or_else(|| "代理软件".into())),
            if inf.client_running {
                crate::i18n::t("直连是好的，只有走代理的网站打不开。重启一下代理软件。").into()
            } else {
                crate::i18n::t("直连是好的，但代理软件已经退出了，系统代理还指着它，所以浏览器打不开网页。把它打开就好。").into()
            },
            proxy_fixes(&inf),
        )
    } else if proxied && st(5) == "fail" {
        let mut f = Vec::new();
        if inf.controller {
            f.push(fix("switch-node", crate::i18n::t("换一个能用的节点"), true));
        }
        f.push(fix("retry", crate::i18n::t("再测一次"), !inf.controller));
        (Some(crate::i18n::t("节点挂了")), crate::tr!("节点 {0} 连不上", inf.node.clone().unwrap_or_default()).replace("  ", " "), crate::i18n::t("直连正常、代理软件也在运行，是当前节点不通。换一个节点，或者在代理软件里更新订阅。").into(), f)
    } else {
        (None, crate::i18n::t("网络正常").into(), String::new(), Vec::new())
    };
    r.ok = problem.is_none();
    r.problem = problem;
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
    let name = inf.client.clone().unwrap_or_else(|| crate::i18n::t("代理软件").into());
    let mut f = Vec::new();
    if inf.clash_party && inf.client_running {
        f.push(fix("restart-proxy", crate::tr!("重启 {0}", name), true));
    } else if inf.client_exe.is_some() {
        f.push(fix("start-proxy", crate::tr!("打开 {0}", name), true));
    }
    f.push(fix("retry", crate::i18n::t("再测一次"), f.is_empty()));
    f
}

/// The system proxy problem as a report of its own (no chain checks needed).
fn sysproxy_report(good: &str, now_value: Option<&str>, client: &str) -> Report {
    let verdict = match now_value {
        None => crate::i18n::t("系统代理被关掉了").to_string(),
        Some(v) => crate::tr!("系统代理被改成了 {0}", v),
    };
    Report {
        at: now_ms(),
        auto: true,
        ok: false,
        problem: Some(if now_value.is_none() { crate::i18n::t("系统代理被关了").into() } else { crate::i18n::t("系统代理被改了").into() }),
        kind: "sysproxy".into(),
        verdict,
        advice: crate::tr!("{0} 还在运行，但浏览器等程序已经不走它了（原来是 {1}）。常见原因是别的软件改了系统代理。", client, good),
        fixes: vec![fix("restore-sysproxy", crate::tr!("恢复成 {0}", good), true), fix("ignore-sysproxy", crate::i18n::t("是我关的"), false)],
        ..Default::default()
    }
}

// ---------- fixes that need no proxy access ----------

pub fn flush_dns() -> Result<(), String> {
    let out = std::process::Command::new("ipconfig").arg("/flushdns").creation_flags(CREATE_NO_WINDOW).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        crate::dns::invalidate_cache();
        log("netdiag: dns cache flushed");
        Ok(())
    } else {
        Err(crate::i18n::t("刷新 DNS 缓存失败").into())
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
        .map_err(|e| crate::tr!("打不开系统代理设置：{0}", e))?;
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
    let good = W.lock().unwrap().good_proxy.clone().ok_or(crate::i18n::t("不知道原来的系统代理是什么"))?;
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
    let mut next_direct = Instant::now() + Duration::from_secs(10);
    let mut next_proxy = Instant::now() + Duration::from_secs(15);
    // the two probes above avoid the system DNS (plain IPs / the core resolves): a broken DNS
    // server needs its own probe
    let mut next_dns = Instant::now() + Duration::from_secs(12);
    let mut direct_fail = 0u32;
    let mut proxy_fail = 0u32;
    let mut dns_fail = 0u32;
    let mut good_streak = 0u32;
    let (wake_lock, wake_cvar) = wake_pair();
    loop {
        crate::perf::tick("netdiag");
        let flags = {
            let g = wake_lock.lock().unwrap();
            let (mut g, _) = wake_cvar.wait_timeout(g, Duration::from_secs(1)).unwrap();
            let f = *g;
            *g = 0;
            f
        };
        if !crate::settings::bool_at("diag.on", true) {
            let cleared = {
                let mut w = W.lock().unwrap();
                w.proxy_off_since = None;
                w.problem.take().is_some()
            };
            if cleared {
                direct_fail = 0;
                proxy_fail = 0;
                dns_fail = 0;
                good_streak = 0;
                publish();
            }
            continue;
        }
        let inf = info();

        // --- the system proxy ---
        system_proxy_watch(&inf);

        // --- quiet probes ---
        let showing = problem_kind(&W.lock().unwrap()).as_deref() == Some("net");
        let now = Instant::now();
        if flags & WAKE_RESUME != 0 {
            // Wake from sleep: discard pre-sleep partial failures; give NIC ~4s grace unless an
            // outage banner is already showing (in which case re-check quickly to clear it).
            direct_fail = 0;
            proxy_fail = 0;
            dns_fail = 0;
            good_streak = 0;
            let delay = Duration::from_millis(if showing { 1500 } else { 4000 });
            next_direct = now + delay;
            next_proxy = now + delay + Duration::from_millis(500);
            next_dns = now + delay + Duration::from_millis(500);
        } else if flags & WAKE_NET != 0 {
            let soon = Duration::from_millis(if showing { 800 } else { 1800 });
            next_direct = next_direct.min(now + soon);
            next_proxy = next_proxy.min(now + soon);
            next_dns = next_dns.min(now + soon);
        }
        let saver = crate::settings::str_at("general.profile", "balanced") == "saver";
        let every = Duration::from_secs(if showing { 5 } else if saver { 20 } else { 10 });
        let mut probed = false;
        let mut failed = false;
        // Probe proxy node even when `inf.tun` is true so dead nodes in TUN mode are caught!
        let proxied = inf.detected && (inf.local_port.is_some() || (inf.tun && inf.core_running));
        if now >= next_direct {
            probed = true;
            if direct(Duration::from_secs(3)).is_err() {
                // If proxy traffic works fine, broadband is up (direct port 80 may be firewall-filtered).
                if proxied && via_proxy(inf.local_port, Duration::from_secs(4)).is_ok() {
                    direct_fail = 0;
                    proxy_fail = 0;
                } else {
                    direct_fail += 1;
                    failed = true;
                }
            } else {
                direct_fail = 0;
            }
            // a first failure is checked again soon
            next_direct = Instant::now() + if direct_fail == 1 { Duration::from_secs(4) } else { every };
        }
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
        if Instant::now() >= next_dns {
            probed = true;
            if probe_dns(Duration::from_secs(3)).is_err() {
                dns_fail += 1;
                failed = true;
            } else {
                dns_fail = 0;
            }
            next_dns = Instant::now() + if dns_fail == 1 { Duration::from_secs(5) } else if showing { every } else { Duration::from_secs(if saver { 60 } else { 30 }) };
        }
        if !probed || started.elapsed() < Duration::from_secs(8) {
            continue;
        }
        if showing {
            // back to normal after two good rounds in a row (every kind of probe passing, not just
            // the one that ran this round): confirm with the full check
            good_streak = if failed || direct_fail > 0 || proxy_fail > 0 || dns_fail > 0 { 0 } else { good_streak + 1 };
            if good_streak >= 2 && claim() {
                good_streak = 0;
                finish(diagnose(true, &|_| {}));
            }
        } else if (direct_fail >= 2 || proxy_fail >= 2 || dns_fail >= 2) && claim() {
            let r = diagnose(true, &|_| {});
            if r.ok {
                // a blip: nothing to report, start counting again
                direct_fail = 0;
                proxy_fail = 0;
                dns_fail = 0;
                W.lock().unwrap().running = false;
            } else {
                // the failure counts stay: recovery needs every kind of probe passing again;
                // pull all three timers onto the 5s recovery cadence so none waits 30-60s
                good_streak = 0;
                finish(r);
                let cap = Instant::now() + Duration::from_secs(5);
                next_direct = next_direct.min(cap);
                next_proxy = next_proxy.min(cap);
                next_dns = next_dns.min(cap);
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
            if problem_kind(&w).as_deref() == Some("sysproxy") {
                w.problem = None;
                emit = true;
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
                        let mut r = sysproxy_report(&good, inf.system_proxy.as_deref(), inf.client.as_deref().unwrap_or(&crate::i18n::t("代理软件")));
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
