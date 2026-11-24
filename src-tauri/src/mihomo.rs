//! Talks to a running mihomo core (Mihomo Party / Clash Party, Clash Verge Rev, plain mihomo)
//! through its external controller: a named pipe or a TCP address.

use std::ffi::c_void;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;

#[derive(Clone, Debug, Default)]
pub struct CoreConfig {
    pub file: Option<String>,
    pub secret: Option<String>,
    pub mixed_port: Option<u16>,
    pub http_port: Option<u16>,
    pub socks_port: Option<u16>,
    pub tun_enabled: bool,
    pub controller_tcp: Option<String>,
}

fn config_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(exe) = crate::netdiag::client_exe() {
        if let Some(dir) = Path::new(&exe).parent() {
            v.push(dir.join("data").join("work").join("config.yaml"));
            v.push(dir.join("data").join("config.yaml"));
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        let a = PathBuf::from(appdata);
        v.push(a.join("clash-party").join("work").join("config.yaml"));
        v.push(a.join("mihomo-party").join("work").join("config.yaml"));
        v.push(a.join("io.github.clash-verge-rev.clash-verge-rev").join("clash-verge.yaml"));
        v.push(a.join("io.github.clash-verge-rev.clash-verge-rev").join("config.yaml"));
        v.push(a.join("clash-verge").join("config.yaml"));
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        v.push(PathBuf::from(home).join(".config").join("mihomo").join("config.yaml"));
    }
    v
}

fn as_port(v: Option<&serde_yaml::Value>) -> Option<u16> {
    v.and_then(|x| x.as_u64()).filter(|p| *p > 0 && *p < 65536).map(|p| p as u16)
}

fn yaml_scalar(v: Option<&serde_yaml::Value>) -> Option<String> {
    let v = v?;
    if let Some(s) = v.as_str() {
        Some(s.to_string())
    } else if let Some(u) = v.as_u64() {
        Some(u.to_string())
    } else if let Some(i) = v.as_i64() {
        Some(i.to_string())
    } else {
        None
    }
}

fn normalize_controller_tcp(raw: &str) -> Option<String> {
    let s = raw
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/');
    if s.is_empty() {
        return None;
    }
    let rewritten = if s.starts_with(':') {
        format!("127.0.0.1{s}")
    } else if let Some(rest) = s.strip_prefix("[::]:") {
        format!("[::1]:{rest}")
    } else if let Some(rest) = s.strip_prefix("0.0.0.0:") {
        format!("127.0.0.1:{rest}")
    } else {
        s.to_string()
    };
    Some(rewritten)
}

type ConfigCacheEntry = (PathBuf, SystemTime, u64, CoreConfig);

fn config_cache() -> &'static Mutex<Option<ConfigCacheEntry>> {
    static CACHE: OnceLock<Mutex<Option<ConfigCacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Reads the newest existing core config among the known client locations, cached by (path, mtime, len).
pub fn read_config() -> CoreConfig {
    let mut files: Vec<(PathBuf, SystemTime, u64)> = config_candidates()
        .into_iter()
        .filter_map(|p| {
            let meta = std::fs::metadata(&p).ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((p, meta.modified().ok()?, meta.len()))
        })
        .collect();
    files.sort_by(|a, b| b.1.cmp(&a.1));
    for (p, mtime, len) in files {
        if let Ok(guard) = config_cache().lock() {
            if let Some((cp, cm, clen, cached)) = guard.as_ref() {
                if *cp == p && *cm == mtime && *clen == len {
                    return cached.clone();
                }
            }
        }
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&text) else { continue };
        let get = |k: &str| doc.get(k);
        let ctl = yaml_scalar(get("external-controller")).and_then(|s| normalize_controller_tcp(&s));
        let cfg = CoreConfig {
            file: Some(p.display().to_string()),
            secret: yaml_scalar(get("secret")).filter(|s| !s.is_empty()),
            mixed_port: as_port(get("mixed-port")),
            http_port: as_port(get("port")),
            socks_port: as_port(get("socks-port")),
            tun_enabled: get("tun").and_then(|t| t.get("enable")).and_then(|e| e.as_bool()).unwrap_or(false),
            controller_tcp: ctl,
        };
        if let Ok(mut guard) = config_cache().lock() {
            *guard = Some((p, mtime, len, cfg.clone()));
        }
        return cfg;
    }
    CoreConfig::default()
}

/// Named pipes that look like a mihomo controller, e.g. \\.\pipe\MihomoParty\mihomo-admin-user-1234.
pub fn find_pipes() -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(r"\\.\pipe\") {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let l = name.to_ascii_lowercase();
            if l.starts_with("flclash")
                || l.contains("flclash")
                || l.contains("service")
                || l.contains("helper")
                || l.contains("ipc")
            {
                continue;
            }
            let matched = l.starts_with("mihomo-party")
                || l.starts_with("mihomoparty")
                || l.starts_with("clash-party")
                || l.starts_with("clashparty")
                || l.starts_with("verge-mihomo")
                || l.starts_with("mihomo");
            if matched {
                v.push(format!(r"\\.\pipe\{name}"));
            }
        }
    }
    v
}

#[derive(Clone, Debug)]
pub enum Controller {
    Pipe(String),
    Tcp(String),
}

impl Controller {
    pub fn describe(&self) -> String {
        match self {
            Controller::Pipe(p) => p.clone(),
            Controller::Tcp(a) => format!("http://{a}"),
        }
    }
}

fn dechunk(body: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        let Some(nl) = body[i..].windows(2).position(|w| w == b"\r\n") else {
            return Err("incomplete chunk header".into());
        };
        let line = std::str::from_utf8(&body[i..i + nl]).map_err(|_| "invalid chunk header")?;
        let hex = line.split(';').next().unwrap_or("").trim();
        if hex.is_empty() {
            return Err("empty chunk size".into());
        }
        let size = usize::from_str_radix(hex, 16).map_err(|_| "invalid chunk size")?;
        i += nl + 2;
        if size == 0 {
            return Ok(out);
        }
        let end = i.checked_add(size).ok_or("chunk size overflow")?;
        if end > body.len() {
            return Err("truncated chunked response".into());
        }
        out.extend_from_slice(&body[i..end]);
        if body.get(end..end + 2) != Some(b"\r\n") {
            return Err("invalid chunk terminator".into());
        }
        i = end + 2;
    }
    Err("unterminated chunked response".into())
}

fn exchange<S: Read + Write>(mut s: S, method: &str, path: &str, body: Option<&str>, secret: Option<&str>) -> Result<Vec<u8>, String> {
    let auth = secret.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    let body = body.unwrap_or("");
    let ctype = if body.is_empty() { String::new() } else { "Content-Type: application/json\r\n".to_string() };
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}{ctype}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let _ = s.flush();
    let mut data = Vec::new();
    let mut buf = [0u8; 65536];
    loop {
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => data.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            // ERROR_BROKEN_PIPE (109) / ERROR_PIPE_NOT_CONNECTED (233): server closed pipe after writing
            Err(e) if matches!(e.raw_os_error(), Some(109 | 233)) && !data.is_empty() => break,
            Err(e) => return Err(e.to_string()),
        }
    }
    let Some(hend) = data.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Err("bad http response".into());
    };
    let head = String::from_utf8_lossy(&data[..hend]).to_lowercase();
    let status = head.split_whitespace().nth(1).unwrap_or("").to_string();
    let raw = &data[hend + 4..];
    let body = if head.contains("transfer-encoding: chunked") {
        dechunk(raw)?
    } else {
        if let Some(clen) = head
            .lines()
            .find_map(|l| l.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse::<usize>().ok())
        {
            if raw.len() < clen {
                return Err("truncated http response".into());
            }
        }
        raw.to_vec()
    };
    if !status.starts_with('2') {
        // mihomo explains errors as {"message": "..."}
        let msg = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(|s| s.to_string()))
            .unwrap_or_default();
        return Err(format!("HTTP {status} {msg}").trim().to_string());
    }
    Ok(body)
}

#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    h_event: *mut c_void,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *const c_void,
        disp: u32,
        flags: u32,
        template: *mut c_void,
    ) -> *mut c_void;
    fn CreateEventW(sa: *const c_void, manual_reset: i32, initial_state: i32, name: *const u16) -> *mut c_void;
    fn ResetEvent(h: *mut c_void) -> i32;
    fn ReadFile(h: *mut c_void, buf: *mut u8, len: u32, read: *mut u32, ov: *mut Overlapped) -> i32;
    fn WriteFile(h: *mut c_void, buf: *const u8, len: u32, written: *mut u32, ov: *mut Overlapped) -> i32;
    fn GetOverlappedResult(h: *mut c_void, ov: *const Overlapped, transferred: *mut u32, wait: i32) -> i32;
    fn CancelIoEx(h: *mut c_void, ov: *const Overlapped) -> i32;
    fn WaitForSingleObject(h: *mut c_void, ms: u32) -> u32;
    fn GetLastError() -> u32;
    fn CloseHandle(h: *mut c_void) -> i32;
}

const GENERIC_RDWR: u32 = 0x8000_0000 | 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
const ERROR_IO_PENDING: u32 = 997;
const ERROR_PIPE_BUSY: u32 = 231;
const ERROR_BROKEN_PIPE: u32 = 109;
const ERROR_PIPE_NOT_CONNECTED: u32 = 233;
const ERROR_HANDLE_EOF: u32 = 38;
const WAIT_OBJECT_0: u32 = 0;

struct OverlappedPipe {
    h: *mut c_void,
    ev: *mut c_void,
    deadline: Instant,
}

impl Drop for OverlappedPipe {
    fn drop(&mut self) {
        unsafe {
            if !self.ev.is_null() {
                CloseHandle(self.ev);
            }
            if !self.h.is_null() && self.h != (-1isize as *mut c_void) {
                CloseHandle(self.h);
            }
        }
    }
}

impl OverlappedPipe {
    fn open(path: &str, deadline: Instant) -> io::Result<Self> {
        let wpath: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut last_err = ERROR_PIPE_BUSY;
        for _ in 0..5 {
            if Instant::now() >= deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "pipe open timeout"));
            }
            unsafe {
                let h = CreateFileW(
                    wpath.as_ptr(),
                    GENERIC_RDWR,
                    0,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    std::ptr::null_mut(),
                );
                if !h.is_null() && h != (-1isize as *mut c_void) {
                    let ev = CreateEventW(std::ptr::null(), 1, 0, std::ptr::null());
                    if ev.is_null() {
                        let e = GetLastError();
                        CloseHandle(h);
                        return Err(io::Error::from_raw_os_error(e as i32));
                    }
                    return Ok(OverlappedPipe { h, ev, deadline });
                }
                last_err = GetLastError();
                if last_err == ERROR_PIPE_BUSY && Instant::now() + Duration::from_millis(40) < deadline {
                    std::thread::sleep(Duration::from_millis(40));
                    continue;
                }
                return Err(io::Error::from_raw_os_error(last_err as i32));
            }
        }
        Err(io::Error::from_raw_os_error(last_err as i32))
    }

    fn rem_ms(&self) -> io::Result<u32> {
        let rem = self
            .deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "pipe timeout"))?;
        Ok((rem.as_millis() as u64).min(u32::MAX as u64).max(1) as u32)
    }
}

impl Read for OverlappedPipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let ms = self.rem_ms()?;
        unsafe {
            ResetEvent(self.ev);
            let mut ov = Overlapped { internal: 0, internal_high: 0, offset: 0, offset_high: 0, h_event: self.ev };
            let mut got = 0u32;
            let len = buf.len().min(u32::MAX as usize) as u32;
            if ReadFile(self.h, buf.as_mut_ptr(), len, &mut got, &mut ov) != 0 {
                return Ok(got as usize);
            }
            let err = GetLastError();
            if matches!(err, ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_HANDLE_EOF) {
                return Ok(0);
            }
            if err != ERROR_IO_PENDING {
                return Err(io::Error::from_raw_os_error(err as i32));
            }
            if WaitForSingleObject(self.ev, ms) != WAIT_OBJECT_0 {
                CancelIoEx(self.h, &ov);
                let _ = GetOverlappedResult(self.h, &ov, &mut got, 1);
                return Err(io::Error::new(io::ErrorKind::TimedOut, "pipe read timeout"));
            }
            if GetOverlappedResult(self.h, &ov, &mut got, 0) != 0 {
                return Ok(got as usize);
            }
            let err2 = GetLastError();
            if matches!(err2, ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_HANDLE_EOF) {
                return Ok(0);
            }
            Err(io::Error::from_raw_os_error(err2 as i32))
        }
    }
}

impl Write for OverlappedPipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let ms = self.rem_ms()?;
        unsafe {
            ResetEvent(self.ev);
            let mut ov = Overlapped { internal: 0, internal_high: 0, offset: 0, offset_high: 0, h_event: self.ev };
            let mut sent = 0u32;
            let len = buf.len().min(u32::MAX as usize) as u32;
            if WriteFile(self.h, buf.as_ptr(), len, &mut sent, &mut ov) != 0 {
                return Ok(sent as usize);
            }
            let err = GetLastError();
            if err != ERROR_IO_PENDING {
                return Err(io::Error::from_raw_os_error(err as i32));
            }
            if WaitForSingleObject(self.ev, ms) != WAIT_OBJECT_0 {
                CancelIoEx(self.h, &ov);
                let _ = GetOverlappedResult(self.h, &ov, &mut sent, 1);
                return Err(io::Error::new(io::ErrorKind::TimedOut, "pipe write timeout"));
            }
            if GetOverlappedResult(self.h, &ov, &mut sent, 0) != 0 {
                return Ok(sent as usize);
            }
            Err(io::Error::from_raw_os_error(GetLastError() as i32))
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

static PIPE_WORKERS: AtomicUsize = AtomicUsize::new(0);

struct PipeWorkerGuard;
impl Drop for PipeWorkerGuard {
    fn drop(&mut self) {
        PIPE_WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

struct PipeJob {
    pipe_path: String,
    method: String,
    path: String,
    body: Option<String>,
    secret: Option<String>,
    reply: mpsc::SyncSender<Result<Vec<u8>, String>>,
}

fn pipe_pool() -> &'static Mutex<Vec<mpsc::SyncSender<PipeJob>>> {
    static POOL: OnceLock<Mutex<Vec<mpsc::SyncSender<PipeJob>>>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(Vec::new()))
}

fn spawn_pipe_worker(first_job: PipeJob) -> bool {
    if PIPE_WORKERS
        .try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < 4).then_some(n + 1))
        .is_err()
    {
        return false;
    }
    let (tx, rx) = mpsc::sync_channel::<PipeJob>(0);
    std::thread::spawn(move || {
        let _guard = PipeWorkerGuard;
        crate::perf::tick("mihomo");
        let run_one = |job: PipeJob| {
            let deadline = Instant::now() + Duration::from_millis(2400);
            let res = OverlappedPipe::open(&job.pipe_path, deadline)
                .map_err(|e| e.to_string())
                .and_then(|pipe| exchange(pipe, &job.method, &job.path, job.body.as_deref(), job.secret.as_deref()));
            let _ = job.reply.send(res);
        };
        run_one(first_job);
        loop {
            {
                let mut pool = pipe_pool().lock().unwrap();
                if pool.len() >= 2 {
                    break;
                }
                pool.push(tx.clone());
            }
            match rx.recv_timeout(Duration::from_secs(30)) {
                Ok(job) => run_one(job),
                Err(_) => break,
            }
        }
    });
    true
}

fn pipe_request(pipe_path: &str, method: &str, path: &str, body: Option<&str>, secret: Option<&str>) -> Result<Vec<u8>, String> {
    const TIMEOUT: Duration = Duration::from_millis(2500);
    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    let mut job = Some(PipeJob {
        pipe_path: pipe_path.to_string(),
        method: method.to_string(),
        path: path.to_string(),
        body: body.map(str::to_string),
        secret: secret.map(str::to_string),
        reply: reply_tx,
    });
    // Reuse an idle pipe worker thread first (avoids 3600+ CreateThread syscalls/hour on 1Hz ticks)
    loop {
        let tx = pipe_pool().lock().unwrap().pop();
        let Some(tx) = tx else { break };
        match tx.try_send(job.take().unwrap()) {
            Ok(()) => break,
            Err(mpsc::TrySendError::Full(j) | mpsc::TrySendError::Disconnected(j)) => job = Some(j),
        }
    }
    if let Some(j) = job {
        if !spawn_pipe_worker(j) {
            return Err("too many blocked pipe requests".into());
        }
    }
    match reply_rx.recv_timeout(TIMEOUT) {
        Ok(res) => res,
        Err(_) => Err("pipe timeout".into()),
    }
}

pub fn get(ctl: &Controller, path: &str, secret: Option<&str>) -> Result<Vec<u8>, String> {
    request(ctl, "GET", path, None, secret)
}

pub fn request(ctl: &Controller, method: &str, path: &str, body: Option<&str>, secret: Option<&str>) -> Result<Vec<u8>, String> {
    match ctl {
        Controller::Pipe(p) => pipe_request(p, method, path, body, secret),
        Controller::Tcp(a) => {
            let addr: std::net::SocketAddr = a
                .parse()
                .or_else(|_| {
                    std::net::ToSocketAddrs::to_socket_addrs(a.as_str())
                        .map_err(|e| e.to_string())
                        .and_then(|mut i| i.next().ok_or_else(|| "no addr".to_string()))
                })
                .map_err(|e: String| e)?;
            let s = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(800)).map_err(|e| e.to_string())?;
            let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
            let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
            exchange(s, method, path, body, secret)
        }
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Meta {
    pub network: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "sourceIP")]
    pub source_ip: String,
    #[serde(rename = "sourcePort")]
    pub source_port: String,
    #[serde(rename = "destinationIP")]
    pub destination_ip: String,
    #[serde(rename = "destinationPort")]
    pub destination_port: String,
    pub host: String,
    #[serde(rename = "sniffHost")]
    pub sniff_host: String,
    pub process: String,
    #[serde(rename = "processPath")]
    pub process_path: String,
    #[serde(rename = "remoteDestination")]
    pub remote_destination: String,
    #[serde(rename = "destinationIPASN")]
    pub destination_asn: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct MConn {
    pub id: String,
    pub metadata: Meta,
    pub upload: u64,
    pub download: u64,
    pub chains: Vec<String>,
    pub rule: String,
    #[serde(rename = "rulePayload")]
    pub rule_payload: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ConnsResp {
    connections: Option<Vec<MConn>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Version {
    pub version: String,
    pub meta: bool,
}

pub fn connections(ctl: &Controller, secret: Option<&str>) -> Result<Vec<MConn>, String> {
    let body = get(ctl, "/connections", secret)?;
    let r: ConnsResp = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    Ok(r.connections.unwrap_or_default())
}

pub fn version(ctl: &Controller, secret: Option<&str>) -> Result<Version, String> {
    let body = get(ctl, "/version", secret)?;
    serde_json::from_slice(&body).map_err(|e| e.to_string())
}

/// Finds a controller that answers /version with the configured secret.
pub fn discover(cfg: &CoreConfig) -> Option<(Controller, String)> {
    let mut cands: Vec<Controller> = find_pipes().into_iter().map(Controller::Pipe).collect();
    if let Some(t) = &cfg.controller_tcp {
        cands.push(Controller::Tcp(t.clone()));
    }
    for c in cands {
        if let Ok(v) = version(&c, cfg.secret.as_deref()) {
            return Some((c, v.version));
        }
    }
    None
}
