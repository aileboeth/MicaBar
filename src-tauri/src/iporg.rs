//! Who an exit IP is registered to and what kind of network it is (home broadband, data
//! centre, mobile, known proxy). Two free lookups per IP, both through the local proxy port:
//! ipapi.is (HTTPS, keyless answers carry the registered company) and ip-api.com (HTTP only on
//! the free tier: org / isp + hosting / mobile / proxy flags). Cached per IP for 24 h, also on
//! disk, so a check normally costs nothing.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

const TTL_MS: u64 = 24 * 3600 * 1000;
/// a failed lookup is retried after this long, not on every exit check
const RETRY_MS: u64 = 10 * 60 * 1000;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IpOrg {
    /// registered organisation (whois company), e.g. "Internet Utilities NA LLC"
    pub org: Option<String>,
    /// the access provider ip-api names, e.g. "AT&T Enterprises, LLC"
    pub isp: Option<String>,
    /// home | hosting | mobile | proxy (most telling first: hosting > proxy > mobile > home)
    pub kind: Option<String>,
    pub hosting: bool,
    pub mobile: bool,
    pub proxy: bool,
    /// which lookups answered: "ipapi.is", "ip-api.com"
    pub src: Vec<String>,
    pub error: Option<String>,
    pub at: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    ips: HashMap<String, IpOrg>,
}

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| {
        let c = std::fs::read_to_string(cache_path()).ok().and_then(|t| serde_json::from_str::<Cache>(&t).ok()).unwrap_or_default();
        Mutex::new(c)
    })
}

fn cache_path() -> PathBuf {
    crate::log_path().with_file_name("iporg-cache.json")
}

fn save() {
    let now = crate::now_ms();
    let mut c = cache().lock().unwrap();
    c.ips.retain(|_, v| now.saturating_sub(v.at) < TTL_MS);
    if let Ok(t) = serde_json::to_string(&*c) {
        let _ = std::fs::write(cache_path(), t);
    }
}

/// Cached answer that is still fresh (a failed one only for RETRY_MS).
pub fn cached(ip: &str) -> Option<IpOrg> {
    let now = crate::now_ms();
    let c = cache().lock().unwrap();
    let v = c.ips.get(ip)?;
    let ttl = if v.error.is_some() && v.src.is_empty() { RETRY_MS } else { TTL_MS };
    (now.saturating_sub(v.at) < ttl).then(|| v.clone())
}

fn client(proxy_port: Option<u16>) -> Result<reqwest::Client, String> {
    let b = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .connect_timeout(Duration::from_secs(5))
        .user_agent("curl/8.9.1");
    let b = match proxy_port {
        Some(p) => b.proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{p}")).map_err(|e| e.to_string())?),
        None => b,
    };
    b.build().map_err(|e| e.to_string())
}

fn text(v: &serde_json::Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

async fn get_json(c: &reqwest::Client, url: &str) -> Result<serde_json::Value, String> {
    let r = c.get(url).send().await.map_err(|e| if e.is_timeout() { "超时".to_string() } else { "连接失败".to_string() })?;
    if !r.status().is_success() {
        return Err(format!("HTTP {}", r.status().as_u16()));
    }
    r.json::<serde_json::Value>().await.map_err(|_| "返回格式不对".to_string())
}

/// Looks one IP up (cache first). `proxy_port` = the proxy client's local port.
pub async fn lookup(ip: &str, proxy_port: Option<u16>) -> IpOrg {
    if let Some(v) = cached(ip) {
        return v;
    }
    let mut out = IpOrg { at: crate::now_ms(), ..Default::default() };
    let c = match client(proxy_port) {
        Ok(c) => c,
        Err(e) => {
            out.error = Some(e);
            return out;
        }
    };
    let (c2, url_b) = (c.clone(), format!("http://ip-api.com/json/{ip}?fields=status,message,isp,org,mobile,proxy,hosting"));
    let b = tauri::async_runtime::spawn(async move { get_json(&c2, &url_b).await });
    let a = get_json(&c, &format!("https://api.ipapi.is/?q={ip}")).await;
    let b = b.await.unwrap_or_else(|_| Err("查询中断".into()));
    let mut errs = Vec::new();
    match a {
        Ok(v) => {
            // keyless answers: "company" is a plain string (paid ones: an object with name/type)
            let company = text(&v, "company").or_else(|| v.get("company").and_then(|c| text(c, "name")));
            if let Some(t) = v.get("company").and_then(|c| text(c, "type")) {
                if t == "hosting" {
                    out.hosting = true;
                }
            }
            if v.get("is_datacenter").and_then(|x| x.as_bool()) == Some(true) {
                out.hosting = true;
            }
            if v.get("is_mobile").and_then(|x| x.as_bool()) == Some(true) {
                out.mobile = true;
            }
            if ["is_proxy", "is_vpn", "is_tor"].iter().any(|k| v.get(*k).and_then(|x| x.as_bool()) == Some(true)) {
                out.proxy = true;
            }
            out.org = company;
            out.src.push("ipapi.is".into());
        }
        Err(e) => errs.push(format!("ipapi.is: {e}")),
    }
    match b {
        Ok(v) if v.get("status").and_then(|s| s.as_str()) == Some("success") => {
            out.hosting |= v.get("hosting").and_then(|x| x.as_bool()).unwrap_or(false);
            out.mobile |= v.get("mobile").and_then(|x| x.as_bool()).unwrap_or(false);
            out.proxy |= v.get("proxy").and_then(|x| x.as_bool()).unwrap_or(false);
            out.isp = text(&v, "isp");
            if out.org.is_none() {
                out.org = text(&v, "org").or_else(|| out.isp.clone());
            }
            out.src.push("ip-api.com".into());
        }
        Ok(v) => errs.push(format!("ip-api.com: {}", text(&v, "message").unwrap_or_else(|| "查询失败".into()))),
        Err(e) => errs.push(format!("ip-api.com: {e}")),
    }
    // the type needs ip-api's flags; ipapi.is alone (keyless) can't tell home from hosting
    let flags_known = out.src.iter().any(|s| s == "ip-api.com") || out.hosting || out.mobile || out.proxy;
    out.kind = if out.hosting {
        Some("hosting".into())
    } else if out.proxy {
        Some("proxy".into())
    } else if out.mobile {
        Some("mobile".into())
    } else if flags_known {
        Some("home".into())
    } else {
        None
    };
    if !errs.is_empty() {
        out.error = Some(errs.join("; "));
    }
    crate::log(format!(
        "ip org: {ip} -> org={:?} isp={:?} kind={:?} err={:?}",
        out.org, out.isp, out.kind, out.error
    ));
    cache().lock().unwrap().ips.insert(ip.to_string(), out.clone());
    save();
    out
}
