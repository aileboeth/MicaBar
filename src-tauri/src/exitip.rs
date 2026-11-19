//! Exit IP checks: one request through the local proxy port (foreign echo services, normally
//! routed to the proxy node) and one without a proxy to domestic echo services (normally DIRECT).

use std::net::IpAddr;
use std::time::Duration;

use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize)]
pub struct ExitResult {
    pub ip: Option<String>,
    pub cc: Option<String>,
    pub asn: Option<u32>,
    pub org: Option<String>,
    /// Final outbound (node) and rule group reported by mihomo for the check connection.
    pub node: Option<String>,
    pub group: Option<String>,
    pub rule: Option<String>,
    pub service: Option<String>,
    /// For the direct check: the physical adapter it was bound to.
    pub via: Option<String>,
    pub error: Option<String>,
    pub at: u64,
}

pub struct Probe {
    pub ip: IpAddr,
    pub host: String,
    pub cc: Option<String>,
    pub org: Option<String>,
}

const FOREIGN: &[&str] = &[
    "https://ipinfo.io/json",
    "https://api.ipify.org?format=json",
    "https://ifconfig.me/ip",
];

const DOMESTIC: &[&str] = &["https://4.ipw.cn", "https://myip.ipip.net", "https://ip.3322.net"];

fn host_of(url: &str) -> String {
    url.trim_start_matches("https://").trim_start_matches("http://").split(['/', '?']).next().unwrap_or("").to_string()
}

fn find_ip(text: &str) -> Option<IpAddr> {
    // First token that parses as an IP address.
    text.split(|c: char| !(c.is_ascii_hexdigit() || c == '.' || c == ':'))
        .filter(|t| t.len() >= 7)
        .find_map(|t| t.parse::<IpAddr>().ok())
}

fn builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .connect_timeout(Duration::from_secs(5))
        .user_agent("curl/8.9.1")
        .pool_idle_timeout(Duration::from_secs(20))
}

pub fn client(proxy_port: Option<u16>) -> Result<reqwest::Client, String> {
    let b = match proxy_port {
        Some(p) => builder().proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{p}")).map_err(|e| e.to_string())?),
        None => builder().no_proxy(),
    };
    b.build().map_err(|e| e.to_string())
}

/// Resolves a host over public DNS-over-HTTPS / HTTPDNS by IP literal, leaving through `bind`,
/// so neither the TUN DNS hijack nor fake-ip answers get involved.
async fn resolve_direct(host: &str, bind: IpAddr) -> Option<IpAddr> {
    let c = builder().no_proxy().local_address(bind).timeout(Duration::from_secs(4)).build().ok()?;
    if let Ok(r) = c.get(format!("https://223.5.5.5/resolve?name={host}&type=1")).send().await {
        if let Ok(v) = r.json::<serde_json::Value>().await {
            if let Some(ans) = v.get("Answer").and_then(|a| a.as_array()) {
                if let Some(ip) = ans
                    .iter()
                    .filter(|a| a.get("type").and_then(|t| t.as_u64()) == Some(1))
                    .find_map(|a| a.get("data").and_then(|d| d.as_str()).and_then(|s| s.parse().ok()))
                {
                    return Some(ip);
                }
            }
        }
    }
    if let Ok(r) = c.get(format!("http://119.29.29.29/d?dn={host}")).send().await {
        if let Ok(t) = r.text().await {
            return t.split(';').find_map(|s| s.trim().parse().ok());
        }
    }
    None
}

/// Direct exit check bound to the physical NIC (bypasses TUN and proxy rules).
pub async fn probe_direct(bind: Option<IpAddr>) -> Result<Probe, String> {
    let Some(bind) = bind else {
        let c = client(None)?;
        return probe(&c, true).await;
    };
    let mut errors = Vec::new();
    for url in DOMESTIC {
        let host = host_of(url);
        let mut b = builder().no_proxy().local_address(bind);
        match resolve_direct(&host, bind).await {
            Some(ip) => b = b.resolve(&host, std::net::SocketAddr::new(ip, 443)),
            None => errors.push(format!("{host}: DNS 解析失败")),
        }
        let c = b.build().map_err(|e| e.to_string())?;
        match fetch(&c, url).await {
            Ok(p) => return Ok(p),
            Err(e) => errors.push(e),
        }
    }
    Err(errors.join("; "))
}

async fn fetch(client: &reqwest::Client, url: &str) -> Result<Probe, String> {
    let resp = client.get(url).send().await.map_err(|e| format!("{}: {}", host_of(url), short_err(&e)))?;
    let text = resp.text().await.map_err(|e| format!("{}: {e}", host_of(url)))?;
    let (mut cc, mut org) = (None, None);
    let ip = if text.trim_start().starts_with('{') {
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        cc = v.get("country").and_then(|x| x.as_str()).map(|s| s.to_string());
        org = v.get("org").and_then(|x| x.as_str()).map(|s| s.to_string());
        v.get("ip").and_then(|x| x.as_str()).and_then(|s| s.parse().ok())
    } else {
        find_ip(&text)
    };
    match ip {
        Some(ip) => Ok(Probe { ip, host: host_of(url), cc, org }),
        None => Err(format!("{}: 返回里没有 IP", host_of(url))),
    }
}

pub async fn probe(client: &reqwest::Client, domestic: bool) -> Result<Probe, String> {
    let list = if domestic { DOMESTIC } else { FOREIGN };
    let mut errors = Vec::new();
    for url in list {
        match fetch(client, url).await {
            Ok(p) => return Ok(p),
            Err(e) => errors.push(e),
        }
    }
    Err(errors.join("; "))
}

fn short_err(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "超时".into()
    } else if e.is_connect() {
        "连接失败".into()
    } else {
        let s = e.to_string();
        s.chars().take(80).collect()
    }
}
