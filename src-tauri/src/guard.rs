//! Leak guard: a library of "should go through the proxy" destinations, used to
//!  - detect leaks: a connection to such a destination that left DIRECT (mihomo rule) or never
//!    went through mihomo at all (app ignores the system proxy),
//!  - prevent rule leaks: guard rule-providers placed in the override right after NetIsland's own
//!    rules force those destinations to the NetIsland group,
//!  - stop bypass leaks: a Windows Firewall rule that blocks a program's direct traffic to public
//!    addresses (loopback is never filtered, so the program can still use the local proxy port).

use std::collections::HashSet;
use std::net::IpAddr;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

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
            force_proxy: true,
            notify: true,
            custom: Vec::new(),
            imports: Vec::new(),
            blocked: Vec::new(),
        }
    }
}

fn dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    let d = PathBuf::from(base).join("NetIsland").join("guard");
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
        let _ = std::fs::write(dir().join("guard.json"), t);
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
    let d = s.trim().trim_matches(|c| c == '\'' || c == '"').trim_start_matches("+.").trim_start_matches("*.").trim_start_matches('.');
    let d = d.trim_end_matches('.').to_lowercase();
    let ok = !d.is_empty() && d.contains('.') && d.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_');
    ok.then_some(d)
}

/// Parses plain domain lists, v2fly (domain:/full:/keyword:), Clash classical lines
/// (DOMAIN-SUFFIX, DOMAIN, DOMAIN-KEYWORD, IP-CIDR, IP-CIDR6), Clash domain sets (+.x.com)
/// and YAML payload lists of any of those.
pub fn parse_list(text: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let mut l = raw.trim();
        if l.is_empty() || l.starts_with('#') || l.starts_with("//") || l.starts_with('!') || l == "payload:" {
            continue;
        }
        if let Some(r) = l.strip_prefix("- ") {
            l = r.trim().trim_matches(|c| c == '\'' || c == '"');
        }
        let l = l.split('#').next().unwrap_or("").trim();
        let lower = l.to_lowercase();
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
                    if !v.is_empty() {
                        out.push(entry("keyword", v));
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
            match k {
                "domain" | "full" => {
                    if let Some(d) = clean_domain(v.split('@').next().unwrap_or("")) {
                        out.push(entry("suffix", &d));
                    }
                    continue;
                }
                "keyword" => {
                    out.push(entry("keyword", v.trim()));
                    continue;
                }
                "regexp" | "include" => continue,
                _ => {}
            }
        }
        if parse_cidr(&lower).is_some() {
            out.push(entry("cidr", &lower));
        } else if let Some(d) = clean_domain(&lower) {
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
            let n = list.len();
            for e in list {
                match e.kind.as_str() {
                    "suffix" => {
                        m.suffixes.insert(e.value);
                    }
                    "keyword" => m.keywords.push(e.value),
                    "cidr" => {
                        if let Some(c) = parse_cidr(&e.value) {
                            m.cidrs.push(c);
                        }
                    }
                    "exclude" => {
                        m.excludes.insert(e.value);
                    }
                    _ => {}
                }
            }
            m.sources.push((name, n));
        };
        if s.builtin {
            add(&mut m, BUILTIN.iter().map(|d| Entry { kind: "suffix".into(), value: d.to_string() }).collect(), "内置".into());
        }
        if s.gfw {
            let t = std::fs::read_to_string(dir().join("gfw.txt")).unwrap_or_default();
            add(&mut m, parse_list(&t), "gfw".into());
        }
        for im in s.imports.iter().filter(|i| i.enabled) {
            let t = std::fs::read_to_string(import_file(&im.url)).unwrap_or_default();
            add(&mut m, parse_list(&t), im.url.clone());
        }
        add(&mut m, s.custom.clone(), "自定义".into());
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
                if let Some(k) = self.keywords.iter().find(|k| host.contains(k.as_str())) {
                    return Some(format!("关键词 {k}"));
                }
            }
        }
        if let Some(ip) = ip {
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
        let d = home.join("netisland");
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        let head = "# 由 NetIsland 生成（预警规则库），请在 NetIsland 里修改\n";
        let list = |items: Vec<String>| {
            if items.is_empty() || !enabled {
                format!("{head}payload: []\n")
            } else {
                format!("{head}payload:\n{}", items.iter().map(|i| format!("  - '{}'\n", i.replace('\'', "''"))).collect::<String>())
            }
        };
        // excluded domains must not be forced either: drop exact excludes from the domain set
        let mut doms: Vec<String> = self.suffixes.iter().filter(|s| !self.excluded(s)).map(|s| format!("+.{s}")).collect();
        doms.sort();
        let ips: Vec<String> = self.cidrs.iter().map(|(ip, l)| format!("{ip}/{l}")).collect();
        let kws: Vec<String> = self.keywords.iter().map(|k| format!("DOMAIN-KEYWORD,{k}")).collect();
        std::fs::write(d.join("guard-domain.yaml"), list(doms)).map_err(|e| e.to_string())?;
        std::fs::write(d.join("guard-ip.yaml"), list(ips)).map_err(|e| e.to_string())?;
        std::fs::write(d.join("guard-kw.yaml"), list(kws)).map_err(|e| e.to_string())?;
        Ok(())
    }
}

// ---------- downloads ----------

pub async fn fetch_text(urls: &[String], proxy_port: Option<u16>) -> Result<String, String> {
    let mut errs = Vec::new();
    for port in [proxy_port, None] {
        let Ok(c) = crate::exitip::client(port) else { continue };
        for u in urls {
            match c.get(u).timeout(std::time::Duration::from_secs(25)).send().await {
                Ok(r) if r.status().is_success() => match r.text().await {
                    Ok(t) => return Ok(t),
                    Err(e) => errs.push(e.to_string()),
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
    let _ = std::fs::write(dir().join("gfw.txt"), text);
    parse_list(text).len()
}

pub fn save_import(url: &str, text: &str) -> usize {
    let _ = std::fs::write(import_file(url), text);
    parse_list(text).len()
}

pub fn remove_import_file(url: &str) {
    let _ = std::fs::remove_file(import_file(url));
}

// ---------- firewall ----------

/// Public address space (everything except loopback / private / link-local / CGNAT / multicast).
const PUBLIC_V4: &str = "1.0.0.0-9.255.255.255,11.0.0.0-100.63.255.255,100.128.0.0-126.255.255.255,\
128.0.0.0-169.253.255.255,169.255.0.0-172.15.255.255,172.32.0.0-192.167.255.255,192.169.0.0-223.255.255.255";
const PUBLIC_V6: &str = "2000::-3fff:ffff:ffff:ffff:ffff:ffff:ffff:ffff";

fn netsh(args: &[String]) -> Result<String, String> {
    let out = std::process::Command::new("netsh").args(args).creation_flags(CREATE_NO_WINDOW).output().map_err(|e| e.to_string())?;
    // netsh prints in the console code page (936 on zh-CN); keep it lossy, only used for logs
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if out.status.success() {
        Ok(text)
    } else {
        Err(format!("netsh 失败（{}）", text.trim()))
    }
}

pub fn rule_name(exe: &str) -> String {
    format!("NetIsland 禁止直连 - {exe}")
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
        "description=Added by NetIsland: this program bypassed the proxy. Delete to allow direct connections again.".into(),
    ])?;
    log(format!("guard: firewall block {path}"));
    Ok(name)
}

pub fn unblock(name: &str, path: &str) -> Result<(), String> {
    netsh(&[
        "advfirewall".into(),
        "firewall".into(),
        "delete".into(),
        "rule".into(),
        format!("name={name}"),
        format!("program={path}"),
    ])
    .map(|_| ())
}

/// Drops the program's established IPv4 connections to public addresses (they would otherwise
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
        if !pid_set.contains(&s.pid) || s.state != 5 {
            continue;
        }
        let rip = IpAddr::V4(std::net::Ipv4Addr::from(s.remote_addr.to_ne_bytes()));
        if crate::geo::is_private(&rip) {
            continue;
        }
        let mut row = Row { state: 12, local_addr: s.local_addr, local_port: s.local_port, remote_addr: s.remote_addr, remote_port: s.remote_port };
        if unsafe { SetTcpEntry(&mut row) } == 0 {
            n += 1;
        }
    }
    n
}

// ---------- notifications ----------

pub const APP_ID: &str = "com.aile.netisland";

/// Registers an AppUserModelID so an unpackaged exe can show toasts under its own name.
pub fn register_toast_identity(icon_png: &[u8]) {
    use winreg::enums::HKEY_CURRENT_USER;
    let icon = dir().join("toast-icon.png");
    let _ = std::fs::write(&icon, icon_png);
    if let Ok((k, _)) = winreg::RegKey::predef(HKEY_CURRENT_USER).create_subkey(format!(r"Software\Classes\AppUserModelId\{APP_ID}")) {
        let _ = k.set_value("DisplayName", &"NetIsland");
        let _ = k.set_value("IconUri", &icon.display().to_string());
    }
}

pub fn toast(title: &str, line1: &str, line2: &str) {
    use tauri_winrt_notification::{Duration, Toast};
    let r = Toast::new(APP_ID).title(title).text1(line1).text2(line2).duration(Duration::Short).show();
    if let Err(e) = r {
        log(format!("toast failed: {e:?}"));
    }
}
