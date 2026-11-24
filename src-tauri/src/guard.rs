//! Leak guard: a library of "should go through the proxy" destinations, used to
//!  - detect leaks: a connection to such a destination that left DIRECT (mihomo rule) or never
//!    went through mihomo at all (app ignores the system proxy),
//!  - prevent rule leaks: guard rule-providers placed in the override right after MicaBar's own
//!    rules force those destinations to the MicaBar group,
//!  - stop bypass leaks: a Windows Firewall rule that blocks a program's direct traffic to public
//!    addresses (loopback is never filtered, so the program can still use the local proxy port).

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::log;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const GFW_URLS: [&str; 2] = [
    "https://cdn.jsdelivr.net/gh/Loyalsoldier/v2ray-rules-dat@release/gfw.txt",
    "https://raw.githubusercontent.com/Loyalsoldier/v2ray-rules-dat/release/gfw.txt",
];

/// Common services that are unreachable or degraded without a proxy in mainland China.
const BUILTIN: &[&str] = &[
    // Google
    "google.com", "googleapis.com", "gstatic.com", "googleusercontent.com", "googlevideo.com", "ggpht.com",
    "google.com.hk", "googlesyndication.com", "googletagmanager.com", "google-analytics.com", "gmail.com",
    "googledrive.com", "googlesource.com", "android.com", "gvt1.com", "gvt2.com", "withgoogle.com", "goo.gl",
    "youtube.com", "youtu.be", "ytimg.com", "youtube-nocookie.com", "blogger.com", "blogspot.com",
    // dev
    "github.com", "githubusercontent.com", "githubassets.com", "github.io", "githubcopilot.com", "ghcr.io",
    "gitlab.com", "docker.com", "docker.io", "npmjs.com", "npmjs.org", "pypi.org", "pythonhosted.org",
    "stackoverflow.com", "stackexchange.com", "sourceforge.net", "huggingface.co", "hf.co", "kaggle.com",
    "jsdelivr.net", "unpkg.com", "crates.io", "rust-lang.org", "golang.org", "go.dev", "medium.com",
    // AI
    "openai.com", "chatgpt.com", "oaistatic.com", "oaiusercontent.com", "anthropic.com", "claude.ai",
    "claude.com", "claudeusercontent.com", "gemini.google.com", "perplexity.ai", "poe.com", "x.ai", "grok.com",
    "midjourney.com", "character.ai", "cursor.sh", "cursor.com", "statsig.com", "sentry.io",
    // social / messaging
    "twitter.com", "x.com", "twimg.com", "t.co", "facebook.com", "fbcdn.net", "instagram.com",
    "cdninstagram.com", "whatsapp.com", "whatsapp.net", "messenger.com", "threads.net", "telegram.org",
    "t.me", "telegram.me", "telegra.ph", "discord.com", "discord.gg", "discordapp.com", "discordapp.net",
    "reddit.com", "redd.it", "redditmedia.com", "redditstatic.com", "linkedin.com", "pinterest.com",
    "tumblr.com", "line.me", "signal.org", "quora.com", "mastodon.social", "bsky.app",
    // media
    "netflix.com", "nflxvideo.net", "nflximg.net", "spotify.com", "scdn.co", "twitch.tv", "ttvnw.net",
    "vimeo.com", "dailymotion.com", "soundcloud.com", "pixiv.net", "pximg.net", "hulu.com", "disneyplus.com",
    // knowledge / news
    "wikipedia.org", "wikimedia.org", "wikiwand.com", "archive.org", "nytimes.com", "bbc.com", "bbc.co.uk",
    "reuters.com", "bloomberg.com", "wsj.com", "theguardian.com", "economist.com", "dw.com", "voanews.com",
    // misc
    "dropbox.com", "dropboxusercontent.com", "proton.me", "protonmail.com", "duckduckgo.com", "notion.so",
    "notion.site", "steamcommunity.com", "patreon.com", "imgur.com", "flickr.com", "wordpress.com",
];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Entry {
    /// suffix | keyword | cidr | exclude
    pub kind: String,
    pub value: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Import {
    pub url: String,
    pub enabled: bool,
    pub count: usize,
    pub updated: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Blocked {
    pub path: String,
    pub exe: String,
    pub rule: String,
    pub at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub builtin: bool,
    pub gfw: bool,
    pub gfw_count: usize,
    pub gfw_updated: u64,
    pub gfw_error: Option<String>,
    /// Put the guard providers into mihomo (rule leaks are routed to the proxy).
    pub force_proxy: bool,
    pub notify: bool,
    pub custom: Vec<Entry>,
    pub imports: Vec<Import>,
    pub blocked: Vec<Blocked>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            builtin: true,
            gfw: true,
            gfw_count: 0,
            gfw_updated: 0,
            gfw_error: None,
            // off by default: it sends built-in / gfw domains to the MicaBar group, which changes the
            // subscription's own routing (a friend's complaint, 2026-11-22); turned on in the alerts tab
            force_proxy: false,
            notify: true,
            custom: Vec::new(),
            imports: Vec::new(),
            blocked: Vec::new(),
        }
    }
}

fn dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    let d = PathBuf::from(base).join("MicaBar").join("guard");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn load_settings() -> Settings {
    // tolerate a UTF-8 BOM (Notepad / PowerShell 5 write one)
    std::fs::read_to_string(dir().join("guard.json"))
        .ok()
        .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default()
}

pub fn save_settings(s: &Settings) {
    if let Ok(t) = serde_json::to_string_pretty(s) {
        let _ = crate::settings::atomic_write(&dir().join("guard.json"), t.as_bytes());
    }
}

fn import_file(url: &str) -> PathBuf {
    let mut h: u64 = 1469598103934665603;
    for b in url.bytes() {
        h = (h ^ b as u64).wrapping_mul(1099511628211);
    }
    dir().join(format!("import-{h:016x}.txt"))
}

// ---------- parsing ----------

fn clean_domain(s: &str) -> Option<String> {
    let d = s
        .trim()
        .trim_matches(|c| c == '\'' || c == '"')
        .trim_start_matches("+.")
        .trim_start_matches("*.")
        .trim_start_matches('.');
    let d = d.trim_end_matches('.').to_lowercase();
    let ok = !d.is_empty() && d.contains('.') && d.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_');
    ok.then_some(d)
}

/// Sanitizes a domain keyword, rejecting empty strings and YAML / Mihomo delimiter characters.
pub fn clean_keyword(s: &str) -> Option<String> {
    let base = s.split('@').next().unwrap_or("");
    let cleaned: String = base
        .trim()
        .trim_matches(|c| c == '\'' || c == '"')
        .chars()
        .filter(|c| !matches!(c, '\r' | '\n' | '\0' | ',' | '(' | ')' | '\'' | '"') && !c.is_control() && !c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Strips `- `, outer quotes, and inline `# ...` comments outside quotes.
fn strip_line_comment_and_quotes(raw: &str) -> String {
    let mut l = raw.trim();
    if l.is_empty() || l.starts_with('#') || l.starts_with("//") || l.starts_with('!') {
        return String::new();
    }
    if let Some(rest) = l.strip_prefix("- ") {
        l = rest.trim();
    } else if l == "-" {
        return String::new();
    }
    let mut out = String::with_capacity(l.len());
    let mut quote: Option<char> = None;
    let mut chars = l.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    if q == '\'' && chars.peek() == Some(&'\'') {
                        out.push('\'');
                        chars.next();
                    } else {
                        quote = None;
                    }
                } else {
                    out.push(c);
                }
            }
            None => {
                if c == '\'' || c == '"' {
                    quote = Some(c);
                } else if c == '#' {
                    break;
                } else {
                    out.push(c);
                }
            }
        }
    }
    out.trim().to_string()
}

fn has_cn_attr(s: &str) -> bool {
    s.split('@')
        .skip(1)
        .any(|attr| attr.split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_').next() == Some("cn"))
}

/// Parses plain domain lists, v2fly (domain:/full:/keyword:), Clash classical lines
/// (DOMAIN-SUFFIX, DOMAIN, DOMAIN-KEYWORD, IP-CIDR, IP-CIDR6), Clash domain sets (+.x.com)
/// and YAML payload lists of any of those.
pub fn parse_list(text: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let l = strip_line_comment_and_quotes(raw);
        if l.is_empty() || l.eq_ignore_ascii_case("payload:") {
            continue;
        }
        let lower = l.to_lowercase();
        if has_cn_attr(&lower) {
            continue;
        }
        let entry = |kind: &str, v: &str| Entry { kind: kind.into(), value: v.to_string() };
        if let Some((k, rest)) = lower.split_once(',') {
            let v = rest.split(',').next().unwrap_or("").trim();
            match k.trim() {
                "domain-suffix" | "domain" => {
                    if let Some(d) = clean_domain(v) {
                        out.push(entry("suffix", &d));
                    }
                }
                "domain-keyword" => {
                    if let Some(kw) = clean_keyword(v) {
                        out.push(entry("keyword", &kw));
                    }
                }
                "ip-cidr" | "ip-cidr6" => {
                    if parse_cidr(v).is_some() {
                        out.push(entry("cidr", v));
                    }
                }
                _ => {}
            }
            continue;
        }
        if let Some((k, v)) = lower.split_once(':') {
            match k.trim() {
                "domain" | "full" => {
                    if let Some(d) = clean_domain(v.split('@').next().unwrap_or("")) {
                        out.push(entry("suffix", &d));
                    }
                    continue;
                }
                "keyword" => {
                    if let Some(kw) = clean_keyword(v) {
                        out.push(entry("keyword", &kw));
                    }
                    continue;
                }
                "regexp" | "include" => continue,
                _ => {}
            }
        }
        let bare = lower.split('@').next().unwrap_or("").trim();
        if parse_cidr(bare).is_some() {
            out.push(entry("cidr", bare));
        } else if let Some(d) = clean_domain(bare) {
            out.push(entry("suffix", &d));
        }
    }
    out
}

pub fn parse_cidr(s: &str) -> Option<(IpAddr, u8)> {
    let (ip, len) = match s.split_once('/') {
        Some((a, b)) => (a.trim().parse::<IpAddr>().ok()?, b.trim().parse::<u8>().ok()?),
        None => {
            let ip: IpAddr = s.trim().parse().ok()?;
            (ip, if ip.is_ipv4() { 32 } else { 128 })
        }
    };
    let max = if ip.is_ipv4() { 32 } else { 128 };
    (len <= max).then_some((ip, len))
}

fn in_cidr(ip: &IpAddr, net: &(IpAddr, u8)) -> bool {
    match (ip, net.0) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let mask = if net.1 == 0 { 0 } else { u32::MAX << (32 - net.1) };
            (u32::from(*a) & mask) == (u32::from(b) & mask)
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let mask = if net.1 == 0 { 0 } else { u128::MAX << (128 - net.1) };
            (u128::from(*a) & mask) == (u128::from(b) & mask)
        }
        _ => false,
    }
}

// ---------- matcher ----------

#[derive(Default)]
pub struct Matcher {
    suffixes: HashSet<String>,
    keywords: Vec<String>,
    cidrs: Vec<(IpAddr, u8)>,
    excludes: HashSet<String>,
    pub sources: Vec<(String, usize)>,
}

impl Matcher {
    pub fn build(s: &Settings) -> Matcher {
        let mut m = Matcher::default();
        let add = |m: &mut Matcher, list: Vec<Entry>, name: String| {
            let mut n = 0usize;
            for e in list {
                match e.kind.as_str() {
                    "suffix" => {
                        if let Some(d) = clean_domain(&e.value) {
                            m.suffixes.insert(d);
                            n += 1;
                        }
                    }
                    "keyword" => {
                        if let Some(kw) = clean_keyword(&e.value) {
                            m.keywords.push(kw);
                            n += 1;
                        }
                    }
                    "cidr" => {
                        if let Some(c) = parse_cidr(&e.value) {
                            m.cidrs.push(c);
                            n += 1;
                        }
                    }
                    "exclude" => {
                        if let Some(d) = clean_domain(&e.value) {
                            m.excludes.insert(d);
                            n += 1;
                        }
                    }
                    _ => {}
                }
            }
            m.sources.push((name, n));
        };
        if s.builtin {
            add(&mut m, BUILTIN.iter().map(|d| Entry { kind: "suffix".into(), value: d.to_string() }).collect(), crate::i18n::t("内置").into());
        }
        if s.gfw {
            let t = std::fs::read_to_string(dir().join("gfw.txt")).unwrap_or_default();
            add(&mut m, parse_list(&t), "gfw".into());
        }
        for im in s.imports.iter().filter(|i| i.enabled) {
            let t = std::fs::read_to_string(import_file(&im.url)).unwrap_or_default();
            add(&mut m, parse_list(&t), im.url.clone());
        }
        add(&mut m, s.custom.clone(), crate::i18n::t("自定义").into());
        m.keywords.sort();
        m.keywords.dedup();
        m
    }

    fn excluded(&self, host: &str) -> bool {
        let mut h = host;
        loop {
            if self.excludes.contains(h) {
                return true;
            }
            match h.find('.') {
                Some(i) => h = &h[i + 1..],
                None => return false,
            }
        }
    }

    /// The library entry that covers this host / IP, if any.
    pub fn hit(&self, host: Option<&str>, ip: Option<IpAddr>) -> Option<String> {
        if let Some(host) = host {
            let host = host.trim_end_matches('.').to_lowercase();
            if !host.is_empty() {
                if self.excluded(&host) {
                    return None;
                }
                let mut h = host.as_str();
                loop {
                    if self.suffixes.contains(h) {
                        return Some(h.to_string());
                    }
                    match h.find('.') {
                        Some(i) => h = &h[i + 1..],
                        None => break,
                    }
                }
                if let Some(k) = self.keywords.iter().find(|k| !k.is_empty() && host.contains(k.as_str())) {
                    return Some(crate::tr!("关键词 {0}", k));
                }
            }
        }
        if let Some(ip) = ip {
            let ip = match ip {
                IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
                v4 => v4,
            };
            if let Some(c) = self.cidrs.iter().find(|c| in_cidr(&ip, c)) {
                return Some(format!("{}/{}", c.0, c.1));
            }
        }
        None
    }

    pub fn len(&self) -> usize {
        self.suffixes.len() + self.keywords.len() + self.cidrs.len()
    }

    /// mihomo provider files: guard-domain (domain behaviour), guard-ip (ipcidr), guard-kw (classical).
    pub fn write_providers(&self, home: &Path, enabled: bool) -> Result<(), String> {
        let d = home.join("micabar");
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        let head = "# 由 MicaBar 生成（预警规则库），请在 MicaBar 里修改\n";
        let list = |items: Vec<String>| {
            if items.is_empty() || !enabled {
                format!("{head}payload: []\n")
            } else {
                format!(
                    "{head}payload:\n{}",
                    items
                        .iter()
                        .filter(|i| !i.contains('\r') && !i.contains('\n'))
                        .map(|i| format!("  - '{}'\n", i.replace('\'', "''")))
                        .collect::<String>()
                )
            }
        };
        // Drop suffixes that are themselves excluded OR have an excluded subdomain under them
        // (since a `+.parent.com` rule in guard-domain would also force `sub.parent.com`).
        let has_excluded_child = |s: &str| {
            let dot_s = format!(".{s}");
            self.excludes.iter().any(|ex| ex.ends_with(&dot_s))
        };
        let mut doms: Vec<String> = self
            .suffixes
            .iter()
            .filter_map(|s| clean_domain(s))
            .filter(|s| !self.excluded(s) && !has_excluded_child(s))
            .map(|s| format!("+.{s}"))
            .collect();
        doms.sort();
        doms.dedup();
        let mut ips: Vec<String> = self.cidrs.iter().map(|(ip, l)| format!("{ip}/{l}")).collect();
        ips.sort();
        ips.dedup();
        let mut kws: Vec<String> = self
            .keywords
            .iter()
            .filter_map(|k| clean_keyword(k))
            .filter(|k| !self.excludes.iter().any(|ex| ex.contains(k.as_str())))
            .map(|k| format!("DOMAIN-KEYWORD,{k}"))
            .collect();
        kws.sort();
        kws.dedup();
        crate::settings::atomic_write(&d.join("guard-domain.yaml"), list(doms).as_bytes()).map_err(|e| e.to_string())?;
        crate::settings::atomic_write(&d.join("guard-ip.yaml"), list(ips).as_bytes()).map_err(|e| e.to_string())?;
        crate::settings::atomic_write(&d.join("guard-kw.yaml"), list(kws).as_bytes()).map_err(|e| e.to_string())?;
        Ok(())
    }
}

// ---------- downloads ----------

const MAX_DOWNLOAD_BYTES: usize = 16 * 1024 * 1024;

fn download_client(proxy_port: Option<u16>) -> Result<reqwest::Client, String> {
    static CLIENTS: OnceLock<Mutex<HashMap<Option<u16>, reqwest::Client>>> = OnceLock::new();
    let map = CLIENTS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(g) = map.lock() {
        if let Some(c) = g.get(&proxy_port) {
            return Ok(c.clone());
        }
    }
    let b = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(25))
        .connect_timeout(std::time::Duration::from_secs(8))
        .user_agent("curl/8.9.1")
        .pool_idle_timeout(std::time::Duration::from_secs(25));
    let b = match proxy_port {
        Some(p) => b.proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{p}")).map_err(|e| e.to_string())?),
        None => b.no_proxy(),
    };
    let c = b.build().map_err(|e| e.to_string())?;
    if let Ok(mut g) = map.lock() {
        g.insert(proxy_port, c.clone());
    }
    Ok(c)
}

async fn read_body_capped(mut resp: reqwest::Response) -> Result<String, String> {
    if let Some(len) = resp.content_length() {
        if len > MAX_DOWNLOAD_BYTES as u64 {
            return Err("response exceeds 16 MB limit".into());
        }
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if buf.len().saturating_add(chunk.len()) > MAX_DOWNLOAD_BYTES {
            return Err("response exceeds 16 MB limit".into());
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

pub async fn fetch_text(urls: &[String], proxy_port: Option<u16>) -> Result<String, String> {
    let mut errs = Vec::new();
    for port in [proxy_port, None] {
        let Ok(c) = download_client(port) else { continue };
        for u in urls {
            match c.get(u).send().await {
                Ok(r) if r.status().is_success() => match read_body_capped(r).await {
                    Ok(t) => return Ok(t),
                    Err(e) => errs.push(e),
                },
                Ok(r) => errs.push(format!("HTTP {}", r.status())),
                Err(e) => errs.push(e.to_string().chars().take(80).collect()),
            }
        }
        if proxy_port.is_none() {
            break;
        }
    }
    Err(errs.join("; "))
}

pub fn save_gfw(text: &str) -> usize {
    let _ = crate::settings::atomic_write(&dir().join("gfw.txt"), text.as_bytes());
    parse_list(text).len()
}

pub fn save_import(url: &str, text: &str) -> usize {
    let _ = crate::settings::atomic_write(&import_file(url), text.as_bytes());
    parse_list(text).len()
}

pub fn remove_import_file(url: &str) {
    let _ = std::fs::remove_file(import_file(url));
}

// ---------- firewall ----------

/// Public address space (everything except loopback / private / link-local / CGNAT / Mihomo fake-ip 198.18.0.0/15 / multicast).
const PUBLIC_V4: &str = "1.0.0.0-9.255.255.255,11.0.0.0-100.63.255.255,100.128.0.0-126.255.255.255,\
128.0.0.0-169.253.255.255,169.255.0.0-172.15.255.255,172.32.0.0-192.167.255.255,\
192.169.0.0-198.17.255.255,198.20.0.0-223.255.255.255";
const PUBLIC_V6: &str = "2000::-3fff:ffff:ffff:ffff:ffff:ffff:ffff:ffff";

pub(crate) fn decode_console(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn MultiByteToWideChar(cp: u32, flags: u32, src: *const u8, src_len: i32, dst: *mut u16, dst_len: i32) -> i32;
    }
    const CP_OEMCP: u32 = 1;
    unsafe {
        let len = bytes.len().min(i32::MAX as usize) as i32;
        let needed = MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), len, std::ptr::null_mut(), 0);
        if needed <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut wide = vec![0u16; needed as usize];
        let written = MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), len, wide.as_mut_ptr(), needed);
        if written <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        String::from_utf16_lossy(&wide[..written as usize])
    }
}

pub(crate) fn firewall_rule_exists(name: &str) -> bool {
    std::process::Command::new("netsh")
        .args(["advfirewall", "firewall", "show", "rule", &format!("name={name}")])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn netsh(args: &[String]) -> Result<String, String> {
    let out = std::process::Command::new("netsh").args(args).creation_flags(CREATE_NO_WINDOW).output().map_err(|e| e.to_string())?;
    let stdout = decode_console(&out.stdout);
    let stderr = decode_console(&out.stderr);
    let text = if stdout.trim().is_empty() { stderr } else { stdout };
    if out.status.success() {
        Ok(text)
    } else {
        Err(crate::tr!("netsh 失败（{0}）", text.trim()))
    }
}

pub fn rule_name(exe: &str) -> String {
    format!("MicaBar 禁止直连 - {exe}")
}

pub fn block(path: &str, exe: &str) -> Result<String, String> {
    let name = rule_name(exe);
    let _ = unblock(&name, path);
    netsh(&[
        "advfirewall".into(),
        "firewall".into(),
        "add".into(),
        "rule".into(),
        format!("name={name}"),
        "dir=out".into(),
        "action=block".into(),
        format!("program={path}"),
        "enable=yes".into(),
        "profile=any".into(),
        format!("remoteip={PUBLIC_V4},{PUBLIC_V6}"),
        "description=Added by MicaBar: this program bypassed the proxy. Delete to allow direct connections again.".into(),
    ])?;
    log(format!("guard: firewall block {path}"));
    Ok(name)
}

pub fn unblock(name: &str, path: &str) -> Result<(), String> {
    let r = netsh(&[
        "advfirewall".into(),
        "firewall".into(),
        "delete".into(),
        "rule".into(),
        format!("name={name}"),
        format!("program={path}"),
    ]);
    if r.is_ok() {
        return Ok(());
    }
    let r2 = netsh(&[
        "advfirewall".into(),
        "firewall".into(),
        "delete".into(),
        "rule".into(),
        format!("name={name}"),
    ]);
    if r2.is_ok() || !firewall_rule_exists(name) {
        Ok(())
    } else {
        r.map(|_| ())
    }
}

/// Drops the program's active/connecting IPv4 connections to public addresses (they would otherwise
/// keep flowing; the firewall only stops new ones).
pub fn reset_connections(pid_set: &HashSet<u32>) -> usize {
    #[repr(C)]
    struct Row {
        state: u32,
        local_addr: u32,
        local_port: u32,
        remote_addr: u32,
        remote_port: u32,
    }
    #[link(name = "iphlpapi")]
    extern "system" {
        fn SetTcpEntry(row: *mut Row) -> u32;
    }
    let mut n = 0;
    for s in crate::conn::snapshot_raw_v4() {
        // SYN_SENT (3) and ESTABLISHED (5)
        if !pid_set.contains(&s.pid) || !matches!(s.state, 3 | 5) {
            continue;
        }
        let rip = IpAddr::V4(std::net::Ipv4Addr::from(s.remote_addr.to_ne_bytes()));
        if crate::geo::is_private(&rip) {
            continue;
        }
        let mut row = Row { state: 12, local_addr: s.local_addr, local_port: s.local_port, remote_addr: s.remote_addr, remote_port: s.remote_port };
        match unsafe { SetTcpEntry(&mut row) } {
            0 => n += 1,
            rc => log(format!("guard: reset {rip} (pid {}) failed: error {rc}", s.pid)),
        }
    }
    n
}

// ---------- notifications ----------

pub const APP_ID: &str = "com.aile.micabar";

/// Registers an AppUserModelID so an unpackaged exe can show toasts under its own name.
/// Skips disk sync and registry writes when already up-to-date to keep cold start fast.
pub fn register_toast_identity(icon_png: &[u8]) {
    use winreg::enums::HKEY_CURRENT_USER;
    let icon = dir().join("toast-icon.png");
    let same_icon = std::fs::read(&icon).map(|b| b.as_slice() == icon_png).unwrap_or(false);
    if !same_icon {
        let _ = crate::settings::atomic_write(&icon, icon_png);
    }
    let want_uri = icon.display().to_string();
    let subkey = format!(r"Software\Classes\AppUserModelId\{APP_ID}");
    let hk = winreg::RegKey::predef(HKEY_CURRENT_USER);
    let already_ok = hk
        .open_subkey(&subkey)
        .ok()
        .map(|k| {
            k.get_value::<String, _>("DisplayName").ok().as_deref() == Some("MicaBar")
                && k.get_value::<String, _>("IconUri").ok().as_deref() == Some(want_uri.as_str())
        })
        .unwrap_or(false);
    if !already_ok {
        if let Ok((k, _)) = hk.create_subkey(&subkey) {
            let _ = k.set_value("DisplayName", &"MicaBar");
            let _ = k.set_value("IconUri", &want_uri);
        }
    }
}

pub fn toast(title: &str, line1: &str, line2: &str) {
    use tauri_winrt_notification::{Duration, Toast};
    let r = Toast::new(APP_ID).title(title).text1(line1).text2(line2).duration(Duration::Short).show();
    if let Err(e) = r {
        log(format!("toast failed: {e:?}"));
    }
}
