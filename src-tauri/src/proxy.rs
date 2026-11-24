//! Detects proxy software: known client/core processes, the WinINet system proxy and TUN adapters.

use std::net::IpAddr;

use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
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
    ("clash-nyanpasu.exe", "Clash Nyanpasu"),
    ("clash nyanpasu.exe", "Clash Nyanpasu"),
    ("clash for windows.exe", "Clash for Windows"),
    ("clashn.exe", "ClashN"),
    ("v2rayn.exe", "v2rayN"),
    ("nekoray.exe", "NekoRay"),
    ("nekobox.exe", "NekoBox"),
    ("throne.exe", "Throne"),
    ("hiddify.exe", "Hiddify"),
    ("flclash.exe", "FlClash"),
    ("shadowsocks.exe", "Shadowsocks"),
    ("karing.exe", "Karing"),
];

const CORES: &[&str] = &[
    "mihomo", "verge-mihomo", "clash-meta", "clash.meta", "clash", "xray", "v2ray", "sing-box",
    "hysteria", "tuic", "naive", "trojan-go", "ss-local", "sslocal",
    // cores that clients ship under their own name
    "flclashcore", "thronecore", "hiddifycli",
];

/// Allocation-free check whether `exe` (filename or path) is a known proxy GUI client.
pub fn is_client_exe(exe: &str) -> bool {
    let base = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
    CLIENTS.iter().any(|(e, _)| base.eq_ignore_ascii_case(e))
}

pub fn is_core_exe(exe: &str) -> bool {
    let base = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
    if is_client_exe(base) {
        return false;
    }
    let l = base.to_ascii_lowercase();
    let stem = l.strip_suffix(".exe").unwrap_or(&l);
    // GUI binaries, services and helpers are not proxy cores
    if stem.starts_with("clash-verge")
        || stem.starts_with("clash verge")
        || stem.starts_with("clash-nyanpasu")
        || stem.starts_with("clash nyanpasu")
        || stem.starts_with("clash-party")
        || stem.starts_with("clash party")
        || stem.starts_with("mihomo-party")
        || stem.starts_with("mihomo party")
        || (stem.starts_with("flclash") && stem != "flclashcore" && !stem.starts_with("flclashcore-"))
    {
        return false;
    }
    // installers, updaters and auxiliary service/helper processes
    if ["setup", "install", "update", "uninst", "service", "helper"].iter().any(|w| stem.contains(w)) {
        return false;
    }
    CORES.iter().any(|c| stem == *c || stem.starts_with(&format!("{c}-")) || stem.starts_with(&format!("{c}_")))
}

/// A core that speaks the mihomo (Clash) controller API; only these are worth looking for one.
pub fn is_mihomo_core(exe: &str) -> bool {
    if !is_core_exe(exe) {
        return false;
    }
    let base = exe.rsplit(['\\', '/']).next().unwrap_or(exe).to_ascii_lowercase();
    let stem = base.strip_suffix(".exe").unwrap_or(&base);
    stem.starts_with("mihomo") || stem.starts_with("verge-mihomo") || stem.starts_with("clash")
}

/// Usual local mixed / http ports of the common clients, for when the system proxy is left alone:
/// Clash 7890, Clash Verge 7897, v2rayN 10808 (+10809 http in old versions), Throne / NekoRay 2080,
/// Hiddify 12334, plain 1080.
pub const KNOWN_PORTS: &[u16] = &[7890, 7897, 10808, 10809, 2080, 12334, 1080];

pub fn client_name(procs: &[(u32, String)]) -> (Option<String>, Vec<u32>, Option<String>) {
    let mut client = None;
    let mut cores = Vec::new();
    let mut core_name = None;
    for (pid, exe) in procs {
        let base = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
        if client.is_none() {
            if let Some((_, n)) = CLIENTS.iter().find(|(e, _)| base.eq_ignore_ascii_case(e)) {
                client = Some(n.to_string());
            }
        }
        if is_core_exe(base) {
            cores.push(*pid);
            core_name.get_or_insert_with(|| {
                let l = base.to_ascii_lowercase();
                l.strip_suffix(".exe").unwrap_or(&l).to_string()
            });
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

/// Port of an http proxy in a WinINet ProxyServer string ("127.0.0.1:7890", "http=...;https=...",
/// or with a scheme like Hiddify writes it: "http://127.0.0.1:12334"). Never returns a socks= port.
pub fn port_from_proxy_server(s: &str) -> Option<u16> {
    let mut http_cand: Option<&str> = None;
    let mut https_cand: Option<&str> = None;
    let mut bare_cand: Option<&str> = None;
    for raw in s.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        if let Some((k, v)) = raw.split_once('=') {
            let k = k.trim();
            let v = v.trim();
            if k.eq_ignore_ascii_case("http") {
                http_cand.get_or_insert(v);
            } else if k.eq_ignore_ascii_case("https") {
                https_cand.get_or_insert(v);
            }
        } else {
            if let Some((scheme, _)) = raw.split_once("://") {
                if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
                    continue;
                }
            }
            bare_cand.get_or_insert(raw);
        }
    }
    let addr = http_cand.or(https_cand).or(bare_cand)?;
    let addr = match addr.split_once("://") {
        Some((scheme, rest)) => {
            if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
                return None;
            }
            rest
        }
        None => addr,
    };
    let host_port = addr.split('/').next().unwrap_or(addr).trim();
    let (host, port_str) = if let Some(rest) = host_port.strip_prefix('[') {
        let (h, after) = rest.split_once(']')?;
        let p = after.strip_prefix(':')?;
        (h, p)
    } else {
        host_port.rsplit_once(':')?
    };
    let is_local = host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host.parse::<std::net::Ipv4Addr>().map(|ip| ip.is_loopback()).unwrap_or(false)
        || host.parse::<std::net::Ipv6Addr>().map(|ip| ip.is_loopback()).unwrap_or(false);
    if !is_local {
        return None;
    }
    let port: u16 = port_str.trim().parse().ok()?;
    (port > 0).then_some(port)
}

pub fn adapters() -> Vec<ipconfig::Adapter> {
    ipconfig::get_adapters().unwrap_or_default()
}

pub fn is_virtual_nic(a: &ipconfig::Adapter) -> bool {
    if matches!(a.if_type(), ipconfig::IfType::SoftwareLoopback) {
        return true;
    }
    let both = format!("{} {}", a.friendly_name(), a.description()).to_ascii_lowercase();
    [
        "vethernet",
        "wsl",
        "hyper-v",
        "default switch",
        "vmware",
        "vmnet",
        "virtualbox",
        "vbox",
        "npcap",
        "loopback",
    ]
    .iter()
    .any(|k| both.contains(k))
}

pub fn looks_like_tun(a: &ipconfig::Adapter) -> bool {
    let both = format!("{} {}", a.friendly_name(), a.description()).to_ascii_lowercase();
    let non_proxy_vpn = [
        "teredo",
        "tap-windows",
        "tailscale",
        "wireguard",
        "zerotier",
        "openvpn",
        "nordlynx",
        "proton",
        "fortinet",
        "cisco",
        "anyconnect",
        "sangfor",
        "radmin",
        "hamachi",
    ];
    if non_proxy_vpn.iter().any(|k| both.contains(k)) {
        return false;
    }
    let markers = [
        "mihomo",
        "clash",
        "sing-box",
        "singbox",
        "nekobox",
        "nekoray",
        "tun2socks",
        "xray",
        "v2ray",
        "hiddify",
        "flclash",
        "karing",
        "throne",
    ];
    markers.iter().any(|k| both.contains(k))
        || both.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "meta")
}

fn usable_ipv4(a: &ipconfig::Adapter) -> Option<IpAddr> {
    a.ip_addresses().iter().find_map(|i| match i {
        IpAddr::V4(v) if !v.is_loopback() && !v.is_link_local() && !v.is_unspecified() => {
            let o = v.octets();
            if o[0] == 198 && (o[1] == 18 || o[1] == 19) {
                None
            } else {
                Some(IpAddr::V4(*v))
            }
        }
        _ => None,
    })
}

fn uplink_without_gateway_in(ads: &[ipconfig::Adapter]) -> Option<(String, IpAddr)> {
    ads.iter()
        .filter(|a| a.oper_status() == ipconfig::OperStatus::IfOperStatusUp && !looks_like_tun(a) && !is_virtual_nic(a))
        .find_map(|a| Some((a.friendly_name().to_string(), usable_ipv4(a)?)))
}

/// The real uplink: an up, non-TUN, non-virtual adapter with an IPv4 default gateway (lowest metric wins).
/// Binding the direct exit-IP check to its address makes it leave through this NIC even when a
/// TUN adapter owns the default route (Windows picks the interface from the source address).
pub fn physical_ipv4() -> Option<(String, IpAddr)> {
    let ads = ipconfig::get_adapters().ok()?;
    let mut best: Option<(u32, String, IpAddr)> = None;
    for a in &ads {
        if a.oper_status() != ipconfig::OperStatus::IfOperStatusUp || looks_like_tun(a) || is_virtual_nic(a) {
            continue;
        }
        if !a.gateways().iter().any(|g| g.is_ipv4() && !g.is_unspecified()) {
            continue;
        }
        let Some(ip) = usable_ipv4(a) else { continue };
        let metric = a.ipv4_metric();
        if best.as_ref().map(|b| metric < b.0).unwrap_or(true) {
            best = Some((metric, a.friendly_name().to_string(), ip));
        }
    }
    best.map(|(_, n, ip)| (n, ip)).or_else(|| uplink_without_gateway_in(&ads))
}

/// Dial-up style uplinks list no IPv4 gateway (Dr.COM campus clients, PPPoE "宽带连接": the
/// gateway shows as 0.0.0.0): an up, non-TUN, non-virtual adapter with a real IPv4 address (not 169.254.x.x).
pub fn uplink_without_gateway() -> Option<(String, IpAddr)> {
    let ads = ipconfig::get_adapters().ok()?;
    uplink_without_gateway_in(&ads)
}

    #[allow(dead_code)]
pub fn tun_adapter() -> Option<String> {
    let ads = ipconfig::get_adapters().ok()?;
    for a in ads {
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

/// Subscribes once to Windows IP interface and unicast IP address change notifications
/// (`iphlpapi!NotifyIpInterfaceChange` + `NotifyUnicastIpAddressChange`) so physical Wi-Fi /
/// Ethernet / PPPoE switches and Clash/Mihomo TUN creation or teardown wake the collector within ms.
pub fn start_ip_watcher(on_change: fn()) {
    use std::ffi::c_void;
    use std::sync::OnceLock;

    type IpCallback = unsafe extern "system" fn(*const c_void, *const c_void, i32);

    #[link(name = "iphlpapi")]
    extern "system" {
        fn NotifyIpInterfaceChange(
            family: u16,
            callback: Option<IpCallback>,
            caller_context: *const c_void,
            initial_notification: u8,
            notification_handle: *mut isize,
        ) -> u32;
        fn NotifyUnicastIpAddressChange(
            family: u16,
            callback: Option<IpCallback>,
            caller_context: *const c_void,
            initial_notification: u8,
            notification_handle: *mut isize,
        ) -> u32;
    }

    static CB: OnceLock<fn()> = OnceLock::new();
    static HANDLES: OnceLock<(isize, isize)> = OnceLock::new();

    unsafe extern "system" fn notify_cb(_: *const c_void, _: *const c_void, _: i32) {
        if let Some(f) = CB.get() {
            f();
        }
    }

    let _ = CB.set(on_change);
    HANDLES.get_or_init(|| unsafe {
        let (mut h_if, mut h_ip) = (0isize, 0isize);
        // AF_UNSPEC = 0, InitialNotification = 0 (FALSE)
        let r1 = NotifyIpInterfaceChange(0, Some(notify_cb), std::ptr::null(), 0, &mut h_if);
        let r2 = NotifyUnicastIpAddressChange(0, Some(notify_cb), std::ptr::null(), 0, &mut h_ip);
        if r1 != 0 || r2 != 0 {
            crate::log(format!("ip watcher: if={r1} addr={r2}"));
        }
        (h_if, h_ip)
    });
}
