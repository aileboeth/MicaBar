//! Who an exit IP belongs to and what kind of line it is (home broadband, business, data
//! centre, mobile), plus a "flagged as proxy" mark. Two free lookups per IP, both through the
//! local proxy port: proxycheck.io (HTTPS, keyless 100 / day: type, provider, organisation,
//! reverse host) and ip-api.com (HTTP only on the free tier, lang=zh-CN: Chinese region names,
//! proxy flag). Chinese carriers get their Chinese name locally (ASN / name). Cached per IP for
//! 24 h, also on disk, so a check normally costs nothing.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Poll, Waker};
use std::time::Duration;

use serde::{Deserialize, Serialize};

const TTL_MS: u64 = 24 * 3600 * 1000;
/// a failed lookup is retried after this long, not on every exit check
const RETRY_MS: u64 = 10 * 60 * 1000;
/// bump when the meaning of the fields changes so old cache entries are looked up again
const VERSION: u32 = 4;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IpOrg {
    pub v: u32,
    /// interface language the names were looked up in (region, carrier)
    pub lang: String,
    /// 2-letter ISO country code ("CN", "US", …), also backfills ExitResult.cc when MMDB has none
    pub cc: Option<String>,
    /// registered organisation, e.g. "Ipxo LLC" (often the subscriber's name for home lines)
    pub org: Option<String>,
    /// the access provider, e.g. "AT&T Enterprises, LLC"
    pub isp: Option<String>,
    /// Chinese carrier name for Chinese IPs: 中国联通 / 中国电信 / 中国移动 …
    pub carrier: Option<String>,
    /// region in Chinese when ip-api has it (江西, 加州)
    pub region: Option<String>,
    /// reverse DNS name (adsl-pool.jx.chinaunicom.com says a lot)
    pub host: Option<String>,
    /// home | business | hosting | mobile; None = unknown
    pub kind: Option<String>,
    /// some lookup lists it as a proxy / VPN exit
    pub proxy: bool,
    /// which lookups answered: "proxycheck.io", "ip-api.com"
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
        let c = std::fs::read_to_string(cache_path())
            .ok()
            .and_then(|t| serde_json::from_str::<Cache>(t.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default();
        Mutex::new(c)
    })
}

fn cache_path() -> PathBuf {
    crate::log_path().with_file_name("iporg-cache.json")
}

fn norm_ip(ip: &str) -> String {
    match ip.trim().parse::<IpAddr>() {
        Ok(IpAddr::V6(v6)) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => v6.to_string(),
        },
        Ok(v4) => v4.to_string(),
        Err(_) => ip.trim().to_string(),
    }
}

fn save() {
    let now = crate::now_ms();
    let serialized = {
        let mut c = cache().lock().unwrap();
        c.ips.retain(|_, v| v.v == VERSION && now.saturating_sub(v.at) < TTL_MS);
        serde_json::to_string(&*c).ok()
    };
    if let Some(t) = serialized {
        let p = cache_path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = crate::settings::atomic_write(&p, t.as_bytes());
    }
}

/// Cached answer that is still fresh (a failed one only for RETRY_MS).
pub fn cached(ip: &str) -> Option<IpOrg> {
    let key = norm_ip(ip);
    let now = crate::now_ms();
    let c = cache().lock().unwrap();
    let lang = crate::i18n::lang();
    let v = c.ips.get(&key).or_else(|| c.ips.get(ip)).filter(|v| v.v == VERSION && v.lang == lang)?;
    let ttl = if v.src.is_empty() { RETRY_MS } else { TTL_MS };
    (now.saturating_sub(v.at) < ttl).then(|| v.clone())
}

fn client(proxy_port: Option<u16>) -> Result<reqwest::Client, String> {
    static CLIENTS: OnceLock<Mutex<HashMap<Option<u16>, reqwest::Client>>> = OnceLock::new();
    let pool = CLIENTS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(g) = pool.lock() {
        if let Some(c) = g.get(&proxy_port) {
            return Ok(c.clone());
        }
    }
    let b = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .connect_timeout(Duration::from_secs(5))
        .user_agent("curl/8.9.1")
        .pool_idle_timeout(Duration::from_secs(25));
    let b = match proxy_port {
        Some(p) => b.proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{p}")).map_err(|e| e.to_string())?),
        None => b.no_proxy(),
    };
    let c = b.build().map_err(|e| e.to_string())?;
    if let Ok(mut g) = pool.lock() {
        g.insert(proxy_port, c.clone());
    }
    Ok(c)
}

fn text(v: &serde_json::Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty() && *s != "unknown").map(str::to_string)
}

async fn get_json(c: &reqwest::Client, url: &str) -> Result<serde_json::Value, String> {
    let r = c.get(url).send().await.map_err(|e| if e.is_timeout() { crate::i18n::t("超时").to_string() } else { crate::i18n::t("连接失败").to_string() })?;
    if !r.status().is_success() {
        return Err(format!("HTTP {}", r.status().as_u16()));
    }
    r.json::<serde_json::Value>().await.map_err(|_| crate::i18n::t("返回格式不对").to_string())
}

/// Chinese name of a mainland carrier from its ASN or English name.
fn cn_carrier(asn: Option<u32>, names: &str) -> Option<&'static str> {
    if let Some(a) = asn {
        match a {
            4837 | 4808 | 9929 | 10099 | 17621 | 17622 | 17623 | 17816 | 9800 | 135061 => return Some("中国联通"),
            4134 | 4809 | 4811 | 4812 | 4813 | 4816 | 23764 | 58466 | 134774 | 136190 | 140292 => return Some("中国电信"),
            9808 | 24400 | 24444 | 24445 | 24547 | 56040..=56048 | 58453 | 58807 | 134810 => return Some("中国移动"),
            4538 | 4565 | 24348 => return Some("中国教育网"),
            9394 => return Some("中国铁通"),
            7497 => return Some("中国科技网"),
            _ => {}
        }
    }
    let n = names.to_ascii_lowercase();
    let has = |k: &str| n.contains(k);
    if has("unicom") || has("china169") || has("cnc group") || has("cncgroup") {
        Some("中国联通")
    } else if has("chinanet") || has("telecom") {
        Some("中国电信")
    } else if has("china mobile") || has("cmnet") || has("chinamobile") {
        Some("中国移动")
    } else if has("tietong") || has("railcom") {
        Some("中国铁通")
    } else if has("cernet") || has("education") {
        Some("中国教育网")
    } else if has("broadcast") || has("cable") || has("cbn") {
        Some("中国广电")
    } else if has("great wall") || has("gwbn") {
        Some("长城宽带")
    } else if has("dr.peng") || has("pengboshi") {
        Some("鹏博士")
    } else {
        None
    }
}

struct InFlightSlot {
    done: Option<IpOrg>,
    wakers: Vec<Waker>,
}

fn in_flight() -> &'static Mutex<HashMap<String, Arc<Mutex<InFlightSlot>>>> {
    static MAP: OnceLock<Mutex<HashMap<String, Arc<Mutex<InFlightSlot>>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

struct InFlightGuard {
    key: String,
    slot: Arc<Mutex<InFlightSlot>>,
}

impl InFlightGuard {
    fn finish(self, result: IpOrg) {
        in_flight().lock().unwrap().remove(&self.key);
        let wakers = {
            let mut s = self.slot.lock().unwrap();
            s.done = Some(result);
            std::mem::take(&mut s.wakers)
        };
        for w in wakers {
            w.wake();
        }
        std::mem::forget(self);
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        in_flight().lock().unwrap().remove(&self.key);
        let wakers = {
            let mut s = self.slot.lock().unwrap();
            if s.done.is_none() {
                s.done = Some(IpOrg {
                    v: VERSION,
                    lang: crate::i18n::lang(),
                    error: Some(crate::i18n::t("查询中断").into()),
                    at: crate::now_ms(),
                    ..Default::default()
                });
            }
            std::mem::take(&mut s.wakers)
        };
        for w in wakers {
            w.wake();
        }
    }
}

/// Queries ip-api.com directly over the physical NIC (never through the proxy node in plaintext!).
async fn direct_ipapi(url: &str) -> Result<serde_json::Value, String> {
    let Some((_, bind)) = crate::proxy::physical_ipv4() else {
        return Err("skipped plaintext ip-api without physical NIC".into());
    };
    let Some(resolved) = crate::exitip::resolve_direct("ip-api.com", bind).await else {
        return Err(crate::i18n::t("连接失败").into());
    };
    let c = reqwest::Client::builder()
        .no_proxy()
        .local_address(bind)
        .resolve("ip-api.com", SocketAddr::new(resolved, 80))
        .timeout(Duration::from_secs(6))
        .connect_timeout(Duration::from_secs(4))
        .user_agent("curl/8.9.1")
        .build()
        .map_err(|e| e.to_string())?;
    get_json(&c, url).await
}

/// Looks one IP up (cache first). `proxy_port` = the proxy client's local port.
#[allow(dead_code)]
pub async fn lookup(ip: &str, proxy_port: Option<u16>) -> IpOrg {
    lookup_ex(ip, proxy_port, false).await
}

/// Looks one IP up; when `is_direct` is true (the user's own domestic/physical exit IP),
/// plaintext `http://ip-api.com` is NEVER sent through the proxy node.
pub async fn lookup_ex(ip: &str, proxy_port: Option<u16>, is_direct: bool) -> IpOrg {
    let key = norm_ip(ip);
    if let Some(v) = cached(&key) {
        return v;
    }
    let lang = crate::i18n::lang();
    let flight_key = format!("{key}|{lang}|{is_direct}");
    let existing = {
        let mut map = in_flight().lock().unwrap();
        if let Some(slot) = map.get(&flight_key).cloned() {
            Ok(slot)
        } else {
            let slot = Arc::new(Mutex::new(InFlightSlot { done: None, wakers: Vec::new() }));
            map.insert(flight_key.clone(), slot.clone());
            Err(InFlightGuard { key: flight_key, slot })
        }
    };
    let guard = match existing {
        Ok(slot) => {
            return std::future::poll_fn(move |cx| {
                let mut s = slot.lock().unwrap();
                if let Some(res) = &s.done {
                    Poll::Ready(res.clone())
                } else {
                    if !s.wakers.iter().any(|w| w.will_wake(cx.waker())) {
                        s.wakers.push(cx.waker().clone());
                    }
                    Poll::Pending
                }
            })
            .await;
        }
        Err(g) => g,
    };

    let mut out = IpOrg { v: VERSION, lang: lang.clone(), at: crate::now_ms(), ..Default::default() };
    // ip-api region names: en de es pt-BR fr ja zh-CN ru (Korean gets English, zh-TW Chinese)
    let api_lang = match lang.as_str() {
        "zh-CN" | "zh-TW" => "zh-CN",
        "ko" => "en",
        l => l,
    };
    let c = match client(proxy_port) {
        Ok(c) => c,
        Err(e) => {
            out.error = Some(e);
            guard.finish(out.clone());
            return out;
        }
    };
    let url_b = format!("http://ip-api.com/json/{key}?lang={api_lang}&fields=status,message,countryCode,regionName,isp,org,as,proxy,hosting");
    let b = if is_direct {
        tauri::async_runtime::spawn(async move { direct_ipapi(&url_b).await })
    } else {
        let c2 = c.clone();
        tauri::async_runtime::spawn(async move { get_json(&c2, &url_b).await })
    };
    let a = get_json(&c, &format!("https://proxycheck.io/v2/{key}?vpn=1&asn=1")).await;
    let b = b.await.unwrap_or_else(|_| Err(crate::i18n::t("查询中断").into()));
    let mut errs = Vec::new();
    let mut asn: Option<u32> = None;
    let mut cc: Option<String> = None;
    let mut names = String::new();
    let mut ipapi_hosting = false;
    match a {
        Ok(v) => {
            let entry = v
                .get(&key)
                .or_else(|| v.get(ip))
                .or_else(|| {
                    v.as_object().and_then(|obj| {
                        obj.iter()
                            .find(|(k, val)| val.is_object() && *k != "status" && *k != "message")
                            .map(|(_, val)| val)
                    })
                });
            if let Some(r) = entry {
                asn = text(r, "asn").and_then(|s| s.trim_start_matches("AS").parse().ok());
                cc = text(r, "isocode");
                out.isp = text(r, "provider");
                out.org = text(r, "organisation");
                out.host = text(r, "hostname");
                out.proxy |= text(r, "proxy").as_deref() == Some("yes");
                let rtype = text(r, "type").unwrap_or_default().to_ascii_lowercase();
                if matches!(rtype.as_str(), "vpn" | "tor") {
                    out.proxy = true;
                }
                out.kind = match rtype.as_str() {
                    "residential" => Some("home".into()),
                    "business" => Some("business".into()),
                    "wireless" => Some("mobile".into()),
                    "hosting" => Some("hosting".into()),
                    // "ISP" / "Fixed Line ISP" / "Education" / "Government" / "" are NOT proxies
                    _ => None,
                };
                out.src.push("proxycheck.io".into());
            } else {
                errs.push(format!(
                    "proxycheck.io: {}",
                    text(&v, "message").or_else(|| text(&v, "status")).unwrap_or_else(|| crate::i18n::t("没有结果").into())
                ));
            }
        }
        Err(e) => errs.push(format!("proxycheck.io: {e}")),
    }
    match b {
        Ok(v) if v.get("status").and_then(|s| s.as_str()) == Some("success") => {
            // its mobile flag is wrong for whole Chinese fixed-line ranges, so it is not used
            out.proxy |= v.get("proxy").and_then(|x| x.as_bool()).unwrap_or(false);
            ipapi_hosting = v.get("hosting").and_then(|x| x.as_bool()).unwrap_or(false);
            out.region = text(&v, "regionName");
            cc = cc.or_else(|| text(&v, "countryCode"));
            if asn.is_none() {
                asn = text(&v, "as")
                    .and_then(|s| s.split_whitespace().next().map(str::to_string))
                    .and_then(|s| s.trim_start_matches("AS").parse().ok());
            }
            if out.isp.is_none() {
                out.isp = text(&v, "isp");
            }
            if out.org.is_none() {
                out.org = text(&v, "org");
            }
            names.push_str(&format!(
                " {} {} {}",
                text(&v, "isp").unwrap_or_default(),
                text(&v, "org").unwrap_or_default(),
                text(&v, "as").unwrap_or_default()
            ));
            out.src.push("ip-api.com".into());
        }
        Ok(v) => {
            if !is_direct || out.src.is_empty() {
                errs.push(format!("ip-api.com: {}", text(&v, "message").unwrap_or_else(|| crate::i18n::t("查询失败").into())));
            }
        }
        Err(e) => {
            if !is_direct || out.src.is_empty() {
                errs.push(format!("ip-api.com: {e}"));
            }
        }
    }
    if out.kind.is_none() && ipapi_hosting {
        out.kind = Some("hosting".into());
    }
    if matches!(cc.as_deref(), Some("CN") | None) {
        names.push_str(&format!(" {} {}", out.isp.clone().unwrap_or_default(), out.org.clone().unwrap_or_default()));
        if cc.as_deref() == Some("CN") || (is_direct && cc.is_none()) {
            out.carrier = cn_carrier(asn, &names).map(crate::i18n::t);
            if cc.is_none() && out.carrier.is_some() {
                cc = Some("CN".into());
            }
        }
    }
    out.cc = cc
        .as_deref()
        .map(str::trim)
        .filter(|s| s.len() == 2 && s.chars().all(|c| c.is_ascii_alphabetic()))
        .map(|s| s.to_ascii_uppercase());
    if !errs.is_empty() {
        out.error = Some(errs.join("; "));
    }
    crate::log(format!(
        "ip org: {key} -> kind={:?} proxy={} isp={:?} org={:?} carrier={:?} region={:?} err={:?}",
        out.kind, out.proxy, out.isp, out.org, out.carrier, out.region, out.error
    ));
    cache().lock().unwrap().ips.insert(key, out.clone());
    save();
    guard.finish(out.clone());
    out
}
