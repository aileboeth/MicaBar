//! Talks to a running mihomo core (Mihomo Party / Clash Party, Clash Verge Rev, plain mihomo)
//! through its external controller: a named pipe or a TCP address.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

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
    if let Ok(appdata) = std::env::var("APPDATA") {
        let a = PathBuf::from(appdata);
        v.push(a.join("mihomo-party").join("work").join("config.yaml"));
        v.push(a.join("clash-party").join("work").join("config.yaml"));
        v.push(a.join("io.github.clash-verge-rev.clash-verge-rev").join("clash-verge.yaml"));
        v.push(a.join("io.github.clash-verge-rev.clash-verge-rev").join("config.yaml"));
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        v.push(PathBuf::from(home).join(".config").join("mihomo").join("config.yaml"));
    }
    v
}

fn as_port(v: Option<&serde_yaml::Value>) -> Option<u16> {
    v.and_then(|x| x.as_u64()).filter(|p| *p > 0 && *p < 65536).map(|p| p as u16)
}

/// Reads the newest existing core config among the known client locations.
pub fn read_config() -> CoreConfig {
    let mut files: Vec<(PathBuf, std::time::SystemTime)> = config_candidates()
        .into_iter()
        .filter_map(|p| {
            let m = std::fs::metadata(&p).ok()?.modified().ok()?;
            Some((p, m))
        })
        .collect();
    files.sort_by(|a, b| b.1.cmp(&a.1));
    for (p, _) in files {
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&text) else { continue };
        let get = |k: &str| doc.get(k);
        let ctl = get("external-controller")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| if s.starts_with(':') { format!("127.0.0.1{s}") } else { s.replace("0.0.0.0", "127.0.0.1") });
        return CoreConfig {
            file: Some(p.display().to_string()),
            secret: get("secret").and_then(|v| v.as_str()).map(|s| s.to_string()).filter(|s| !s.is_empty()),
            mixed_port: as_port(get("mixed-port")),
            http_port: as_port(get("port")),
            socks_port: as_port(get("socks-port")),
            tun_enabled: get("tun").and_then(|t| t.get("enable")).and_then(|e| e.as_bool()).unwrap_or(false),
            controller_tcp: ctl,
        };
    }
    CoreConfig::default()
}

/// Named pipes that look like a mihomo controller, e.g. \\.\pipe\MihomoParty\mihomo-admin-user-1234.
pub fn find_pipes() -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(r"\\.\pipe\") {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let l = name.to_lowercase();
            if l.contains("mihomo") || l.contains("verge-mihomo") || l.contains("clash") {
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

fn dechunk(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        let Some(nl) = body[i..].windows(2).position(|w| w == b"\r\n") else { break };
        let line = String::from_utf8_lossy(&body[i..i + nl]);
        let size = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16).unwrap_or(0);
        i += nl + 2;
        if size == 0 {
            break;
        }
        let end = (i + size).min(body.len());
        out.extend_from_slice(&body[i..end]);
        i = end + 2;
    }
    out
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
            Err(e) => {
                if data.is_empty() {
                    return Err(e.to_string());
                }
                break; // pipe closed after the response
            }
        }
    }
    let Some(hend) = data.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Err("bad http response".into());
    };
    let head = String::from_utf8_lossy(&data[..hend]).to_lowercase();
    let status = head.split_whitespace().nth(1).unwrap_or("").to_string();
    let raw = &data[hend + 4..];
    let body = if head.contains("transfer-encoding: chunked") { dechunk(raw) } else { raw.to_vec() };
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

pub fn get(ctl: &Controller, path: &str, secret: Option<&str>) -> Result<Vec<u8>, String> {
    request(ctl, "GET", path, None, secret)
}

pub fn request(ctl: &Controller, method: &str, path: &str, body: Option<&str>, secret: Option<&str>) -> Result<Vec<u8>, String> {
    match ctl {
        Controller::Pipe(p) => {
            let mut last = String::new();
            for _ in 0..5 {
                match std::fs::OpenOptions::new().read(true).write(true).open(p) {
                    Ok(f) => return exchange(f, method, path, body, secret),
                    Err(e) => {
                        last = e.to_string();
                        std::thread::sleep(Duration::from_millis(40)); // ERROR_PIPE_BUSY
                    }
                }
            }
            Err(last)
        }
        Controller::Tcp(a) => {
            let addr: std::net::SocketAddr = a
                .parse()
                .or_else(|_| std::net::ToSocketAddrs::to_socket_addrs(a.as_str()).map_err(|e| e.to_string()).and_then(|mut i| i.next().ok_or_else(|| "no addr".to_string())))
                .map_err(|e: String| e)?;
            let s = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(800)).map_err(|e| e.to_string())?;
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
