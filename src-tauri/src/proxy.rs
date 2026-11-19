//! Detects proxy software: known client/core processes, the WinINet system proxy and TUN adapters.

use std::net::IpAddr;

use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize)]
pub struct ProxyState {
    pub detected: bool,
    /// Friendly client name, e.g. "Clash Party (mihomo)".
    pub client: Option<String>,
    pub core_pids: Vec<u32>,
    pub system_proxy: Option<String>,
    pub tun: Option<String>,
    /// Local HTTP/mixed port used to send exit-IP checks through the proxy.
    pub local_port: Option<u16>,
    pub controller: Option<String>,
    pub core_version: Option<String>,
    pub mihomo_ok: bool,
    pub mihomo_error: Option<String>,
}

const CLIENTS: &[(&str, &str)] = &[
    ("clash party.exe", "Clash Party"),
    ("mihomo party.exe", "Mihomo Party"),
    ("mihomo-party.exe", "Mihomo Party"),
    ("clash-verge.exe", "Clash Verge"),
    ("clash verge.exe", "Clash Verge"),
    ("clash for windows.exe", "Clash for Windows"),
    ("clashn.exe", "ClashN"),
    ("v2rayn.exe", "v2rayN"),
    ("nekoray.exe", "NekoRay"),
    ("throne.exe", "Throne"),
    ("hiddify.exe", "Hiddify"),
    ("flclash.exe", "FlClash"),
    ("shadowsocks.exe", "Shadowsocks"),
    ("karing.exe", "Karing"),
];

const CORES: &[&str] = &[
    "mihomo", "verge-mihomo", "clash-meta", "clash", "xray", "v2ray", "sing-box", "hysteria", "tuic",
    "naive", "trojan-go", "ss-local", "sslocal",
];

pub fn is_core_exe(exe: &str) -> bool {
    let l = exe.to_lowercase();
    let stem = l.trim_end_matches(".exe");
    CORES.iter().any(|c| stem == *c || stem.starts_with(&format!("{c}-")))
}

pub fn client_name(procs: &[(u32, String)]) -> (Option<String>, Vec<u32>, Option<String>) {
    let mut client = None;
    let mut cores = Vec::new();
    let mut core_name = None;
    for (pid, exe) in procs {
        let l = exe.to_lowercase();
        if client.is_none() {
            if let Some((_, n)) = CLIENTS.iter().find(|(e, _)| *e == l) {
                client = Some(n.to_string());
            }
        }
        if is_core_exe(exe) {
            cores.push(*pid);
            core_name.get_or_insert_with(|| l.trim_end_matches(".exe").to_string());
        }
    }
    (client, cores, core_name)
}

pub fn system_proxy() -> Option<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    let key = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        .ok()?;
    let enabled: u32 = key.get_value("ProxyEnable").unwrap_or(0);
    if enabled == 0 {
        return None;
    }
    let server: String = key.get_value("ProxyServer").ok()?;
    let s = server.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Port of an http proxy in a WinINet ProxyServer string ("127.0.0.1:7890" or "http=...;https=...").
pub fn port_from_proxy_server(s: &str) -> Option<u16> {
    let part = s
        .split(';')
        .find(|p| p.starts_with("http=") || !p.contains('='))
        .unwrap_or(s);
    let addr = part.split('=').last().unwrap_or(part);
    let host = addr.split('/').find(|x| x.contains(':')).unwrap_or(addr);
    let local = host.starts_with("127.") || host.starts_with("localhost") || host.starts_with("[::1]");
    if !local {
        return None;
    }
    host.rsplit(':').next()?.parse().ok()
}

pub fn looks_like_tun(a: &ipconfig::Adapter) -> bool {
    let both = format!("{} {}", a.friendly_name(), a.description()).to_lowercase();
    ["mihomo", "meta", "wintun", "sing-box", "singbox", "clash", "v2ray", "xray", "tun"]
        .iter()
        .any(|k| both.contains(k))
        && !both.contains("teredo")
        && !both.contains("tap-windows")
}

/// The real uplink: an up, non-TUN adapter with an IPv4 default gateway (lowest metric wins).
/// Binding the direct exit-IP check to its address makes it leave through this NIC even when a
/// TUN adapter owns the default route (Windows picks the interface from the source address).
pub fn physical_ipv4() -> Option<(String, IpAddr)> {
    let adapters = ipconfig::get_adapters().ok()?;
    let mut best: Option<(u32, String, IpAddr)> = None;
    for a in &adapters {
        if a.oper_status() != ipconfig::OperStatus::IfOperStatusUp || looks_like_tun(a) {
            continue;
        }
        if !a.gateways().iter().any(|g| g.is_ipv4() && !g.is_unspecified()) {
            continue;
        }
        let Some(ip) = a.ip_addresses().iter().find(|i| i.is_ipv4() && !i.is_loopback()).copied() else { continue };
        let metric = a.ipv4_metric();
        if best.as_ref().map(|b| metric < b.0).unwrap_or(true) {
            best = Some((metric, a.friendly_name().to_string(), ip));
        }
    }
    best.map(|(_, n, ip)| (n, ip))
}

pub fn tun_adapter() -> Option<String> {
    let adapters = ipconfig::get_adapters().ok()?;
    for a in adapters {
        if a.oper_status() != ipconfig::OperStatus::IfOperStatusUp {
            continue;
        }
        let name = a.friendly_name().to_string();
        let desc = a.description().to_string();
        if looks_like_tun(&a) {
            return Some(if desc.is_empty() || desc == name { name } else { format!("{name} ({desc})") });
        }
    }
    None
}
