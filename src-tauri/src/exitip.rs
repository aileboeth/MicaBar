//! Exit IP checks: one request through the local proxy port (foreign echo services, normally
//! routed to the proxy node) and one without a proxy to domestic echo services (normally DIRECT).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Mutex, OnceLock};
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
    /// registered organisation + network type (iporg.rs), filled in after the check
    pub reg: Option<crate::iporg::IpOrg>,
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

fn public_ip(s: &str) -> Option<IpAddr> {
    let ip: IpAddr = s.trim().parse().ok()?;
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    };
    (!crate::geo::is_private(&ip) && !ip.is_loopback()).then_some(ip)
}

fn find_ip(text: &str) -> Option<IpAddr> {
    // First token that parses as a public IP address.
    text.split(|c: char| !(c.is_ascii_hexdigit() || c == '.' || c == ':'))
        .filter(|t| t.len() >= 7)
        .find_map(public_ip)
}

fn builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .connect_timeout(Duration::from_secs(5))
        .user_agent("curl/8.9.1")
        .pool_idle_timeout(Duration::from_secs(25))
}

fn cached_client(proxy_port: Option<u16>, local_v4: Option<Ipv4Addr>) -> Result<reqwest::Client, String> {
    static POOL: OnceLock<Mutex<HashMap<(Option<u16>, Option<Ipv4Addr>), reqwest::Client>>> = OnceLock::new();
    let pool = POOL.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (proxy_port, local_v4);
    if let Ok(g) = pool.lock() {
        if let Some(c) = g.get(&key) {
            return Ok(c.clone());
        }
    }
    let mut b = match proxy_port {
        Some(p) => builder().proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{p}")).map_err(|e| e.to_string())?),
        None => builder().no_proxy(),
    };
    if let Some(ip4) = local_v4 {
        b = b.local_address(IpAddr::V4(ip4));
    }
    let c = b.build().map_err(|e| e.to_string())?;
    if let Ok(mut g) = pool.lock() {
        g.insert(key, c.clone());
    }
    Ok(c)
}

pub fn client(proxy_port: Option<u16>) -> Result<reqwest::Client, String> {
    cached_client(proxy_port, None)
}

fn extract_doh_a(v: &serde_json::Value) -> Option<IpAddr> {
    v.get("Answer")
        .and_then(|a| a.as_array())?
        .iter()
        .filter(|a| a.get("type").and_then(|t| t.as_u64()) == Some(1))
        .find_map(|a| a.get("data").and_then(|d| d.as_str()).and_then(public_ip))
}

/// Resolves a host over public DNS-over-HTTPS / HTTPDNS by IP literal, leaving through `bind`,
/// so neither the TUN DNS hijack nor fake-ip answers get involved.
pub(crate) async fn resolve_direct(host: &str, bind: IpAddr) -> Option<IpAddr> {
    let c = match bind {
        IpAddr::V4(v4) => cached_client(None, Some(v4)).ok()?,
        _ => builder().no_proxy().local_address(bind).timeout(Duration::from_secs(4)).build().ok()?,
    };
    let short = Duration::from_secs(4);
    if let Ok(r) = c.get(format!("https://223.5.5.5/resolve?name={host}&type=1")).timeout(short).send().await {
        if r.status().is_success() {
            if let Ok(v) = r.json::<serde_json::Value>().await {
                if let Some(ip) = extract_doh_a(&v) {
                    return Some(ip);
                }
            }
        }
    }
    if let Ok(r) = c.get(format!("http://119.29.29.29/d?dn={host}")).timeout(short).send().await {
        if r.status().is_success() {
            if let Ok(t) = r.text().await {
                if let Some(ip) = t.split(';').find_map(public_ip) {
                    return Some(ip);
                }
            }
        }
    }
    if let Ok(r) = c
        .get(format!("https://1.1.1.1/dns-query?name={host}&type=A"))
        .header("Accept", "application/dns-json")
        .timeout(short)
        .send()
        .await
    {
        if r.status().is_success() {
            if let Ok(v) = r.json::<serde_json::Value>().await {
                if let Some(ip) = extract_doh_a(&v) {
                    return Some(ip);
                }
            }
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
    for url in DOMESTIC.iter().chain(&["https://api.ipify.org?format=json", "https://ifconfig.me/ip"]) {
        let host = host_of(url);
        let Some(ip) = resolve_direct(&host, bind).await else {
            errors.push(crate::tr!("{0}: DNS 解析失败", host));
            continue;
        };
        let port = if url.starts_with("http://") { 80 } else { 443 };
        let c = builder()
            .no_proxy()
            .local_address(bind)
            .resolve(&host, std::net::SocketAddr::new(ip, port))
            .build()
            .map_err(|e| e.to_string())?;
        match fetch(&c, url).await {
            Ok(p) => return Ok(p),
            Err(e) => errors.push(e),
        }
    }
    Err(errors.join("; "))
}

async fn fetch(client: &reqwest::Client, url: &str) -> Result<Probe, String> {
    let resp = client.get(url).send().await.map_err(|e| format!("{}: {}", host_of(url), short_err(&e)))?;
    if !resp.status().is_success() {
        return Err(format!("{}: HTTP {}", host_of(url), resp.status().as_u16()));
    }
    let text = resp.text().await.map_err(|e| format!("{}: {e}", host_of(url)))?;
    let (mut cc, mut org) = (None, None);
    let ip = if text.trim_start().starts_with('{') {
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        cc = v.get("country").and_then(|x| x.as_str()).map(|s| s.to_string());
        org = v.get("org").and_then(|x| x.as_str()).map(|s| s.to_string());
        v.get("ip").and_then(|x| x.as_str()).and_then(public_ip)
    } else {
        find_ip(&text)
    };
    match ip {
        Some(ip) => Ok(Probe { ip, host: host_of(url), cc, org }),
        None => Err(crate::tr!("{0}: 返回里没有 IP", host_of(url))),
    }
}

pub async fn probe(client: &reqwest::Client, domestic: bool) -> Result<Probe, String> {
    let mut errors = Vec::new();
    let list = if domestic { DOMESTIC } else { FOREIGN };
    for url in list {
        match fetch(client, url).await {
            Ok(p) => return Ok(p),
            Err(e) => errors.push(e),
        }
    }
    if domestic {
        for url in &["https://api.ipify.org?format=json", "https://ifconfig.me/ip"] {
            if let Ok(p) = fetch(client, url).await {
                return Ok(p);
            }
        }
    }
    Err(errors.join("; "))
}

fn short_err(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        crate::i18n::t("超时").into()
    } else if e.is_connect() {
        crate::i18n::t("连接失败").into()
    } else {
        let s = e.to_string();
        s.chars().take(80).collect()
    }
}
