//! Exit control for Clash Party / Mihomo Party: per-process / per-domain "proxy / direct / reject".
//!
//! One-time setup adds a global override to Clash Party that declares file rule-providers
//! micabar-<level>-<target> (files inside the core's home dir) placed at the top of the rules,
//! plus a select group "MicaBar" containing every node. Levels order rules by specificity, so a
//! narrower rule wins over a broader one whatever their targets:
//!   1 app + exact domain/IP, 2 app + main domain, 3 exact domain/IP, 4 main domain, 5 whole app.
//! After setup, every change only rewrites those files and asks mihomo to reload the providers
//! (no core restart), then closes the affected connections so they reconnect under the new rule.
//!
//! Clash Party caches override.yaml in memory and writes the cache back on its next save, so the
//! setup closes Clash Party first, writes the files, and starts it again with the same elevation.

use std::ffi::c_void;
use std::net::IpAddr;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::mihomo::{self, Controller, CoreConfig};
use crate::{log, procinfo};

pub const OVERRIDE_ID: &str = "micabar";
/// The override of the app's earlier name (NetIsland); replaced on the next install.
const LEGACY_OVERRIDE_ID: &str = "netisland";
pub const GROUP: &str = "MicaBar";
const TARGETS: [&str; 3] = ["reject", "direct", "proxy"];
const LEVELS: [u8; 5] = [1, 2, 3, 4, 5];

fn provider_names() -> impl Iterator<Item = (u8, &'static str)> {
    LEVELS.into_iter().flat_map(|l| TARGETS.into_iter().map(move |t| (l, t)))
}
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn clean_atom(s: &str) -> Option<String> {
    let cleaned: String = s
        .chars()
        .filter(|c| !matches!(c, '\r' | '\n' | '\0' | ',' | '(' | ')') && !c.is_control())
        .collect();
    let t = cleaned.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn canon_ip(s: &str) -> Option<IpAddr> {
    let ip: IpAddr = s.trim().trim_matches(['[', ']']).parse().ok()?;
    Some(match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    })
}

fn ip_in_cidr(ip: IpAddr, net_ip: IpAddr, prefix: u8) -> bool {
    match (ip, net_ip) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix.min(32)) };
            (u32::from(a) & mask) == (u32::from(b) & mask)
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let mask = if prefix == 0 { 0 } else { u128::MAX << (128 - prefix.min(128)) };
            (u128::from(a) & mask) == (u128::from(b) & mask)
        }
        _ => false,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub id: String,
    /// process | domain | suffix | ip | process_domain | process_suffix | process_ip
    pub kind: String,
    pub process: Option<String>,
    pub value: Option<String>,
    /// proxy | direct | reject
    pub target: String,
    pub created: u64,
}

impl Rule {
    fn key(&self) -> (String, String, String) {
        (
            self.kind.clone(),
            self.process.clone().unwrap_or_default().to_lowercase(),
            self.value.clone().unwrap_or_default().to_lowercase(),
        )
    }

    /// Specificity level (1 = narrowest, matched first).
    pub fn level(&self) -> u8 {
        match self.kind.as_str() {
            "process_domain" | "process_ip" => 1,
            "process_suffix" => 2,
            "domain" | "ip" => 3,
            "suffix" => 4,
            _ => 5,
        }
    }

    fn ip_cidr(v: &str) -> Option<String> {
        let v = clean_atom(v)?;
        let (ip, prefix) = match v.split_once('/') {
            Some((addr, pre)) => {
                let ip = canon_ip(addr)?;
                let len: u8 = pre.trim().parse().ok()?;
                let max = if ip.is_ipv4() { 32 } else { 128 };
                if len > max {
                    return None;
                }
                (ip, len)
            }
            None => {
                let ip = canon_ip(&v)?;
                let len = if ip.is_ipv4() { 32 } else { 128 };
                (ip, len)
            }
        };
        if ip.is_ipv4() {
            Some(format!("IP-CIDR,{ip}/{prefix},no-resolve"))
        } else {
            Some(format!("IP-CIDR6,{ip}/{prefix},no-resolve"))
        }
    }

    fn proc_clause(p: &str) -> Option<String> {
        let p = clean_atom(p)?;
        if p.contains('\\') || p.contains('/') {
            Some(format!("PROCESS-PATH,{p}"))
        } else {
            Some(format!("PROCESS-NAME,{p}"))
        }
    }

    /// The mihomo classical rule (without the target).
    pub fn line(&self) -> Option<String> {
        match self.kind.as_str() {
            "process" => Self::proc_clause(self.process.as_deref()?),
            "domain" => {
                let v = clean_atom(self.value.as_deref()?)?;
                Some(format!("DOMAIN,{v}"))
            }
            "suffix" => {
                let v = clean_atom(self.value.as_deref()?)?.trim_start_matches('.').to_string();
                (!v.is_empty()).then(|| format!("DOMAIN-SUFFIX,{v}"))
            }
            "ip" => Self::ip_cidr(self.value.as_deref()?),
            "process_domain" => {
                let pr = Self::proc_clause(self.process.as_deref()?)?;
                let v = clean_atom(self.value.as_deref()?)?;
                Some(format!("AND,(({pr}),(DOMAIN,{v}))"))
            }
            "process_suffix" => {
                let pr = Self::proc_clause(self.process.as_deref()?)?;
                let v = clean_atom(self.value.as_deref()?)?.trim_start_matches('.').to_string();
                (!v.is_empty()).then(|| format!("AND,(({pr}),(DOMAIN-SUFFIX,{v}))"))
            }
            "process_ip" => {
                let pr = Self::proc_clause(self.process.as_deref()?)?;
                let ip = Self::ip_cidr(self.value.as_deref()?)?;
                Some(format!("AND,(({pr}),({ip}))"))
            }
            _ => None,
        }
    }

    /// Whether an active mihomo connection is covered by this rule (to close it after a change).
    fn matches(&self, c: &mihomo::MConn) -> bool {
        let m = &c.metadata;
        let proc_ok = match self.process.as_deref().and_then(clean_atom) {
            Some(p) => {
                if p.contains('\\') || p.contains('/') {
                    let norm_p = p.replace('/', "\\");
                    let norm_mp = m.process_path.replace('/', "\\");
                    norm_mp.eq_ignore_ascii_case(&norm_p)
                } else if !m.process.is_empty() {
                    m.process.eq_ignore_ascii_case(&p)
                } else {
                    m.process_path
                        .rsplit(['\\', '/'])
                        .next()
                        .map(|base| base.eq_ignore_ascii_case(&p))
                        .unwrap_or(false)
                }
            }
            None => false,
        };

        let v = self
            .value
            .as_deref()
            .and_then(clean_atom)
            .unwrap_or_default()
            .trim_end_matches('.')
            .to_lowercase();
        let v_suf = v.trim_start_matches('.');

        let host_is_ip = canon_ip(&m.host).is_some();
        let h_primary = if m.host.is_empty() || host_is_ip { &m.sniff_host } else { &m.host };
        let h1 = h_primary.trim().trim_end_matches('.').to_lowercase();
        let h2 = m.sniff_host.trim().trim_end_matches('.').to_lowercase();

        let dom = !v.is_empty() && (h1 == v || (!h2.is_empty() && h2 == v));
        let suf_match = |h: &str| !h.is_empty() && (h == v_suf || h.ends_with(&format!(".{v_suf}")));
        let suf = !v_suf.is_empty() && (suf_match(&h1) || suf_match(&h2));

        let ip = match canon_ip(&m.destination_ip) {
            Some(dst_ip) => match crate::guard::parse_cidr(&v) {
                Some((net_ip, prefix)) => {
                    let net_canon = match net_ip {
                        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
                        v4 => v4,
                    };
                    let adj_prefix = if net_ip.is_ipv6() && net_canon.is_ipv4() {
                        prefix.saturating_sub(96)
                    } else {
                        prefix
                    };
                    ip_in_cidr(dst_ip, net_canon, adj_prefix)
                }
                None => false,
            },
            None => false,
        };

        match self.kind.as_str() {
            "process" => proc_ok,
            "domain" => dom,
            "suffix" => suf,
            "ip" => ip,
            "process_domain" => proc_ok && dom,
            "process_suffix" => proc_ok && suf,
            "process_ip" => proc_ok && ip,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlState {
    /// unavailable | available | pending | active | working | error
    pub status: String,
    pub message: Option<String>,
    pub client: Option<String>,
    pub rules: Vec<Rule>,
    pub node: Option<String>,
}

fn state_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    let d = PathBuf::from(base).join("MicaBar");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn load_rules() -> Vec<Rule> {
    std::fs::read_to_string(state_dir().join("rules.json"))
        .ok()
        .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default()
}

pub fn save_rules(rules: &[Rule]) -> Result<(), String> {
    let t = serde_json::to_string_pretty(rules).map_err(|e| e.to_string())?;
    crate::settings::atomic_write(&state_dir().join("rules.json"), t.as_bytes()).map_err(|e| e.to_string())
}

// ---------- Clash Party discovery ----------

pub struct ClashParty {
    pub exe: String,
    pub pids: Vec<u32>,
    pub data_dir: PathBuf,
    pub elevated: bool,
}

const PARTY_EXES: [&str; 3] = ["clash party.exe", "mihomo party.exe", "mihomo-party.exe"];

fn is_elevated(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows_sys::Win32::System::Threading::{OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let mut tok = std::ptr::null_mut();
        let mut out = false;
        if OpenProcessToken(h, TOKEN_QUERY, &mut tok) != 0 {
            let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
            let mut len = 0u32;
            if GetTokenInformation(tok, TokenElevation, &mut e as *mut _ as *mut _, std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut len) != 0 {
                out = e.TokenIsElevated != 0;
            }
            CloseHandle(tok);
        }
        CloseHandle(h);
        out
    }
}

fn dir_freshness(d: &Path) -> Option<SystemTime> {
    [
        d.join("work").join("config.yaml"),
        d.join("override.yaml"),
        d.join("config.yaml"),
    ]
    .into_iter()
    .filter_map(|p| std::fs::metadata(p).ok()?.modified().ok())
    .max()
}

fn data_dir_for(exe: Option<&str>) -> PathBuf {
    if let Some(exe) = exe {
        if let Some(dir) = Path::new(exe).parent() {
            let portable_data = dir.join("data");
            if dir.join("PORTABLE").exists()
                || portable_data.join("override.yaml").exists()
                || portable_data.join("config.yaml").exists()
                || portable_data.join("work").join("config.yaml").exists()
            {
                return portable_data;
            }
        }
    }
    let appdata = PathBuf::from(std::env::var("APPDATA").unwrap_or_default());
    let clash_dir = appdata.join("clash-party");
    let mihomo_dir = appdata.join("mihomo-party");
    let exe_prefers_clash = exe
        .map(|e| e.to_ascii_lowercase().contains("clash"))
        .unwrap_or(true);
    let ordered = if exe_prefers_clash {
        [&clash_dir, &mihomo_dir]
    } else {
        [&mihomo_dir, &clash_dir]
    };
    let mut best: Option<(&PathBuf, SystemTime)> = None;
    for cand in ordered {
        if let Some(mtime) = dir_freshness(cand) {
            if best.as_ref().map(|(_, bm)| mtime > *bm).unwrap_or(true) {
                best = Some((cand, mtime));
            }
        }
    }
    if let Some((chosen, _)) = best {
        return chosen.clone();
    }
    ordered[0].clone()
}

pub fn find_clash_party(procs: &[(u32, String)]) -> Option<ClashParty> {
    let pids: Vec<u32> = procs.iter().filter(|(_, e)| PARTY_EXES.contains(&e.to_lowercase().as_str())).map(|(p, _)| *p).collect();
    let exe = pids.iter().find_map(|p| procinfo::image_path(*p))?;
    let elevated = pids.iter().any(|p| is_elevated(*p));
    Some(ClashParty { data_dir: data_dir_for(Some(&exe)), exe, pids, elevated })
}

fn process_command_line_nt(pid: u32) -> Option<String> {
    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *const u16,
    }
    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryInformationProcess(
            process_handle: *mut c_void,
            process_information_class: u32,
            process_information: *mut c_void,
            process_information_length: u32,
            return_length: *mut u32,
        ) -> i32;
    }
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;
    const STATUS_INFO_LENGTH_MISMATCH: i32 = -0x3fff_fffc; // 0xC0000004
    const STATUS_BUFFER_TOO_SMALL: i32 = -0x3fff_ffdd; // 0xC0000023
    const STATUS_BUFFER_OVERFLOW: i32 = -0x7fff_fffb; // 0x80000005

    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = vec![0u64; 512]; // 4096 bytes, 8-byte aligned
        let mut ret_len = 0u32;
        let mut status = NtQueryInformationProcess(
            h,
            PROCESS_COMMAND_LINE_INFORMATION,
            buf.as_mut_ptr() as *mut c_void,
            (buf.len() * 8) as u32,
            &mut ret_len,
        );
        if matches!(status, STATUS_INFO_LENGTH_MISMATCH | STATUS_BUFFER_TOO_SMALL | STATUS_BUFFER_OVERFLOW)
            && ret_len > 0
            && ret_len <= 1_048_576
        {
            buf = vec![0u64; (ret_len as usize + 16) / 8 + 1];
            status = NtQueryInformationProcess(
                h,
                PROCESS_COMMAND_LINE_INFORMATION,
                buf.as_mut_ptr() as *mut c_void,
                (buf.len() * 8) as u32,
                &mut ret_len,
            );
        }
        CloseHandle(h);
        if status < 0 {
            return None;
        }
        let total_bytes = buf.len() * 8;
        if total_bytes < std::mem::size_of::<UnicodeString>() {
            return None;
        }
        let us = &*(buf.as_ptr() as *const UnicodeString);
        if us.length == 0 || us.buffer.is_null() {
            return None;
        }
        let bptr = us.buffer as usize;
        let start = buf.as_ptr() as usize;
        let end = start.checked_add(total_bytes)?;
        let byte_len = (us.length as usize) & !1usize;
        if bptr % 2 != 0 || bptr < start || bptr.checked_add(byte_len)? > end {
            return None;
        }
        let slice = std::slice::from_raw_parts(us.buffer, byte_len / 2);
        Some(String::from_utf16_lossy(slice))
    }
}

/// The core's home directory (its `-d` argument), where provider files must live
/// (mihomo only accepts provider paths inside its home dir).
pub fn core_home(core_pid: u32) -> Option<PathBuf> {
    if let Some(cmd) = process_command_line_nt(core_pid) {
        if let Some(p) = parse_home(&cmd) {
            return Some(p);
        }
    }
    let out = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("(Get-CimInstance Win32_Process -Filter 'ProcessId={core_pid}').CommandLine"),
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let cmd = String::from_utf8_lossy(&out.stdout).to_string();
    parse_home(&cmd)
}

fn parse_home(cmd: &str) -> Option<PathBuf> {
    let i = cmd.find(" -d ")? + 4;
    let rest = cmd[i..].trim_start();
    let dir = if let Some(r) = rest.strip_prefix('"') { r.split('"').next()? } else { rest.split_whitespace().next()? };
    let p = PathBuf::from(dir);
    p.is_dir().then_some(p)
}

// ---------- override install ----------

fn target_policy(t: &str) -> &str {
    match t {
        "reject" => "REJECT",
        "direct" => "DIRECT",
        _ => GROUP,
    }
}

/// First port of the per-group inbounds (127.0.0.1:47800 + index).
pub const LISTEN_BASE: u16 = 47800;

#[derive(Clone, Default)]
struct ParsedConfig {
    listeners: Vec<(String, u16)>,
    match_group: Option<String>,
    rule_groups: Vec<String>,
    guard_active: bool,
}

type ParsedCacheEntry = (PathBuf, SystemTime, u64, ParsedConfig);

fn parsed_cfg_cache() -> &'static Mutex<Option<ParsedCacheEntry>> {
    static CACHE: OnceLock<Mutex<Option<ParsedCacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn split_top_level_commas(rule: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (idx, ch) in rule.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            ',' if depth == 0 => {
                parts.push(rule[start..idx].trim());
                start = idx + 1;
            }
            _ => {}
        }
    }
    parts.push(rule[start..].trim());
    parts
}

fn parse_running_config(config_file: &Path) -> ParsedConfig {
    let Ok(meta) = std::fs::metadata(config_file) else {
        return ParsedConfig::default();
    };
    let Ok(mtime) = meta.modified() else {
        return ParsedConfig::default();
    };
    let len = meta.len();
    if let Ok(guard) = parsed_cfg_cache().lock() {
        if let Some((cp, cm, clen, cached)) = guard.as_ref() {
            if cp == config_file && *cm == mtime && *clen == len {
                return cached.clone();
            }
        }
    }
    let Ok(text) = std::fs::read_to_string(config_file) else {
        return ParsedConfig::default();
    };
    let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&text) else {
        return ParsedConfig::default();
    };
    let listeners = doc
        .get("listeners")
        .and_then(|l| l.as_sequence())
        .map(|s| {
            s.iter()
                .filter(|l| l.get("name").and_then(|n| n.as_str()).map(|n| n.starts_with("micabar-g")).unwrap_or(false))
                .filter_map(|l| {
                    let proxy = l.get("proxy")?.as_str()?.to_string();
                    let port = l.get("port")?.as_u64()? as u16;
                    Some((proxy, port))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut match_group = None;
    let mut rule_groups: Vec<String> = Vec::new();
    let mut guard_active = false;
    if let Some(seq) = doc.get("rules").and_then(|r| r.as_sequence()) {
        for r in seq {
            let Some(s) = r.as_str() else { continue };
            if s.contains("micabar-guard-domain") {
                guard_active = true;
            }
            let parts = split_top_level_commas(s);
            let is_match = parts.first().map(|p| p.eq_ignore_ascii_case("MATCH")).unwrap_or(false);
            if is_match {
                if let Some(g) = parts.get(1).copied().filter(|g| !g.is_empty()) {
                    match_group = Some(g.to_string());
                }
            }
            let target = if is_match {
                parts.get(1).copied()
            } else {
                parts
                    .iter()
                    .rev()
                    .copied()
                    .find(|p| !matches!(*p, "no-resolve" | "src" | "dst") && !p.is_empty())
            };
            let Some(t) = target else { continue };
            if matches!(t, "DIRECT" | "REJECT" | "REJECT-DROP" | "PASS" | "COMPATIBLE") || t.ends_with(')') {
                continue;
            }
            if !rule_groups.iter().any(|g| g == t) {
                rule_groups.push(t.to_string());
            }
        }
    }

    let parsed = ParsedConfig { listeners, match_group, rule_groups, guard_active };
    if let Ok(mut guard) = parsed_cfg_cache().lock() {
        *guard = Some((config_file.to_path_buf(), mtime, len, parsed.clone()));
    }
    parsed
}

/// Per-group inbounds of the running config: (group name, port), read from the generated
/// config file (the override names them micabar-g<i>).
pub fn group_listeners(config_file: &Path) -> Vec<(String, u16)> {
    parse_running_config(config_file).listeners
}

/// The subscription's fallback group (target of the MATCH rule) in the running config.
pub fn match_group(config_file: &Path) -> Option<String> {
    parse_running_config(config_file).match_group
}

/// Targets of the running config's rules, in rule order, each once ("DOMAIN-SUFFIX,x,G",
/// "RULE-SET,x,G,no-resolve", "AND,((…),(…)),G", "MATCH,G"); built-in policies are left out.
pub fn rule_groups(config_file: &Path) -> Vec<String> {
    parse_running_config(config_file).rule_groups
}

/// Whether the running config's `rules` list actually includes the MicaBar leak-guard rule-sets.
pub fn guard_rules_active(config_file: &Path) -> bool {
    parse_running_config(config_file).guard_active
}

pub const GUARD_PROVIDERS: [(&str, &str, &str); 3] = [
    ("micabar-guard-domain", "domain", "guard-domain"),
    ("micabar-guard-kw", "classical", "guard-kw"),
    ("micabar-guard-ip", "ipcidr", "guard-ip"),
];

/// Clash Party JavaScript override. Besides MicaBar's own rules (prepended, by specificity) it
/// inserts the leak-guard rule sets after the subscription's last proxy-group rule, right before
/// the next rule that would send traffic DIRECT (or before the first destination DIRECT rule /
/// MATCH rule when DIRECT rules precede proxy rules), so `force_proxy` works across all layouts.
pub fn override_js() -> String {
    let own: Vec<String> = provider_names()
        .map(|(l, t)| format!("[\"micabar-{l}-{t}\", \"{l}-{t}\", \"{}\"]", target_policy(t)))
        .collect();
    let guard: Vec<String> = GUARD_PROVIDERS.iter().map(|(n, b, f)| format!("[\"{n}\", \"{b}\", \"{f}\"]")).collect();
    format!(
        r#"// MicaBar 出口控制 + 泄漏守卫 (v2)：由 MicaBar 自动添加。删除这个覆写即可停用。
function main(config) {{
  const G = "{GROUP}";
  const own = [{own}];
  const guard = [{guard}];
  const providers = config["rule-providers"] = config["rule-providers"] || {{}};
  for (const [name, file] of own) {{
    providers[name] = {{ type: "file", behavior: "classical", format: "yaml", path: "./micabar/" + file + ".yaml" }};
  }}
  for (const [name, behavior, file] of guard) {{
    providers[name] = {{ type: "file", behavior: behavior, format: "yaml", path: "./micabar/" + file + ".yaml" }};
  }}
  const groups = config["proxy-groups"] = config["proxy-groups"] || [];
  if (!groups.some((g) => g && g.name === G)) groups.unshift({{ name: G, type: "select", "include-all": true }});
  const directish = new Set(["DIRECT"]);
  for (const g of groups) {{
    if (g && Array.isArray(g.proxies) && g.proxies[0] === "DIRECT") directish.add(g.name);
  }}
  // policy of a rule line: last top-level field, skipping options such as no-resolve / src
  const policy = (rule) => {{
    const parts = [];
    let depth = 0, cur = "";
    for (const ch of String(rule)) {{
      if (ch === "(") depth++;
      if (ch === ")") depth--;
      if (ch === "," && depth === 0) {{ parts.push(cur.trim()); cur = ""; }} else cur += ch;
    }}
    parts.push(cur.trim());
    while (parts.length > 2 && ["no-resolve", "src"].includes(parts[parts.length - 1])) parts.pop();
    return parts[parts.length - 1];
  }};
  const rest = (config.rules || []).filter((r) => !String(r).includes("micabar-"));
  const notProxy = new Set(["REJECT", "REJECT-DROP", "PASS"]);
  const destTypes = new Set(["RULE-SET", "DOMAIN", "DOMAIN-SUFFIX", "DOMAIN-KEYWORD", "DOMAIN-REGEX",
    "DOMAIN-WILDCARD", "GEOSITE", "GEOIP", "IP-CIDR", "IP-CIDR6", "IP-SUFFIX", "IP-ASN", "AND", "OR", "NOT"]);
  const ruleType = (r) => String(r).split(",")[0].trim().toUpperCase();
  const isPrivateRule = (r) => /^GEOIP,\s*(LAN|PRIVATE)\b/i.test(String(r).trim());
  let lastProxy = -1;
  rest.forEach((r, i) => {{
    const type = ruleType(r);
    const p = policy(r);
    if (destTypes.has(type) && !directish.has(p) && !notProxy.has(p)) lastProxy = i;
  }});
  let at = rest.findIndex((r, i) => {{
    const type = ruleType(r);
    return i > lastProxy && (destTypes.has(type) || type === "MATCH") && directish.has(policy(r));
  }});
  if (at < 0) {{
    // Subscriptions that put CN/DIRECT rules before proxy rules, or end with MATCH,Proxy
    at = rest.findIndex((r) => destTypes.has(ruleType(r)) && !isPrivateRule(r) && directish.has(policy(r)));
  }}
  if (at < 0) {{
    at = rest.findIndex((r) => destTypes.has(ruleType(r)) && directish.has(policy(r)));
  }}
  if (at < 0) {{
    at = rest.findIndex((r) => ruleType(r) === "MATCH");
  }}
  if (at < 0) at = rest.length;
  rest.splice(at, 0,
    "RULE-SET,micabar-guard-domain," + G,
    "RULE-SET,micabar-guard-kw," + G,
    "RULE-SET,micabar-guard-ip," + G + ",no-resolve");
  config.rules = own.map(([name, , target]) => "RULE-SET," + name + "," + target).concat(rest);
  // one loopback-only inbound per proxy group: traffic entering it skips the rules and goes
  // straight to that group, so MicaBar can measure every group's exit IP
  const kinds = new Set(["select", "url-test", "fallback", "load-balance", "smart"]);
  const listeners = (config.listeners || []).filter((l) => !String(l && l.name).startsWith("micabar-g"));
  groups.filter((g) => g && kinds.has(String(g.type).toLowerCase())).slice(0, 80).forEach((g, i) => {{
    listeners.push({{ name: "micabar-g" + i, type: "mixed", listen: "127.0.0.1", port: {listen_base} + i, proxy: g.name, udp: false }});
  }});
  config.listeners = listeners;
  return config;
}}
"#,
        own = own.join(", "),
        guard = guard.join(", "),
        listen_base = LISTEN_BASE
    )
}

/// Whether the installed override file is the one this version writes.
pub fn override_current(data_dir: &Path) -> bool {
    std::fs::read_to_string(data_dir.join("override").join(format!("{OVERRIDE_ID}.js")))
        .map(|t| t.replace("\r\n", "\n") == override_js())
        .unwrap_or(false)
}

fn read_override_list(data_dir: &Path) -> serde_yaml::Value {
    std::fs::read_to_string(data_dir.join("override.yaml"))
        .ok()
        .and_then(|t| serde_yaml::from_str(&t).ok())
        .unwrap_or_else(|| serde_yaml::from_str("items: []").unwrap())
}

pub fn override_installed(data_dir: &Path) -> bool {
    let list = read_override_list(data_dir);
    let item = list
        .get("items")
        .and_then(|i| i.as_sequence())
        .and_then(|s| s.iter().find(|it| matches!(it.get("id").and_then(|v| v.as_str()), Some(OVERRIDE_ID) | Some(LEGACY_OVERRIDE_ID))).cloned());
    item.map(|it| it.get("global").and_then(|g| g.as_bool()).unwrap_or(false)).unwrap_or(false)
        && (data_dir.join("override").join(format!("{OVERRIDE_ID}.js")).exists()
            || data_dir.join("override").join(format!("{LEGACY_OVERRIDE_ID}.js")).exists()
            || data_dir.join("override").join(format!("{OVERRIDE_ID}.yaml")).exists())
}

fn write_override(data_dir: &Path, install: bool) -> Result<(), String> {
    let mut list = read_override_list(data_dir);
    if list.get("items").and_then(|v| v.as_sequence()).is_none() {
        list = serde_yaml::from_str("items: []").unwrap();
    }
    let items = list.get_mut("items").and_then(|v| v.as_sequence_mut()).ok_or(crate::i18n::t("override.yaml 格式不对"))?;
    items.retain(|it| !matches!(it.get("id").and_then(|v| v.as_str()), Some(OVERRIDE_ID) | Some(LEGACY_OVERRIDE_ID)));
    for f in [format!("{LEGACY_OVERRIDE_ID}.js"), format!("{LEGACY_OVERRIDE_ID}.yaml")] {
        let _ = std::fs::remove_file(data_dir.join("override").join(f));
    }
    let file = data_dir.join("override").join(format!("{OVERRIDE_ID}.js"));
    // the first versions wrote a YAML override
    let _ = std::fs::remove_file(data_dir.join("override").join(format!("{OVERRIDE_ID}.yaml")));
    if install {
        let now = crate::now_ms();
        let item: serde_yaml::Value = serde_yaml::from_str(&format!(
            "id: {OVERRIDE_ID}\nname: MicaBar 出口控制\ntype: local\next: js\nglobal: true\nupdated: {now}\n"
        ))
        .map_err(|e| e.to_string())?;
        items.push(item);
        std::fs::create_dir_all(data_dir.join("override")).map_err(|e| e.to_string())?;
        crate::settings::atomic_write(&file, override_js().as_bytes()).map_err(|e| crate::tr!("写入覆写失败: {0}", e))?;
    } else {
        let _ = std::fs::remove_file(&file);
    }
    std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let text = serde_yaml::to_string(&list).map_err(|e| e.to_string())?;
    crate::settings::atomic_write(&data_dir.join("override.yaml"), text.as_bytes()).map_err(|e| crate::tr!("写入 override.yaml 失败: {0}", e))
}

fn backup(data_dir: &Path) -> Option<PathBuf> {
    let src = data_dir.join("override.yaml");
    if !src.exists() {
        return None;
    }
    let dir = state_dir().join("backup");
    let _ = std::fs::create_dir_all(&dir);
    let dst = dir.join(format!("override-{}.yaml", crate::now_ms()));
    std::fs::copy(&src, &dst).ok().map(|_| dst)
}

#[link(name = "kernel32")]
extern "system" {
    fn WaitForSingleObject(h: *mut c_void, ms: u32) -> u32;
    fn GetLastError() -> u32;
}

const SYNCHRONIZE_RIGHT: u32 = 0x0010_0000;
const ERROR_ACCESS_DENIED_CODE: u32 = 5;

fn open_terminable(pids: &[u32]) -> Result<Vec<*mut c_void>, String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE};
    let mut handles = Vec::with_capacity(pids.len());
    unsafe {
        for &pid in pids {
            let h = OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE_RIGHT, 0, pid);
            if h.is_null() {
                let err = GetLastError();
                if err == ERROR_ACCESS_DENIED_CODE {
                    for prev in handles {
                        CloseHandle(prev);
                    }
                    return Err(std::io::Error::from_raw_os_error(err as i32).to_string());
                }
                // Process already exited (ERROR_INVALID_PARAMETER = 87)
            } else {
                handles.push(h);
            }
        }
    }
    Ok(handles)
}

fn terminate_and_wait(handles: Vec<*mut c_void>, deadline: Instant) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::TerminateProcess;
    unsafe {
        for &h in &handles {
            let _ = TerminateProcess(h, 1);
        }
        for h in handles {
            let rem = deadline
                .checked_duration_since(Instant::now())
                .map(|d| d.as_millis().min(3000) as u32)
                .unwrap_or(0);
            if rem > 0 {
                let _ = WaitForSingleObject(h, rem);
            }
            CloseHandle(h);
        }
    }
}

/// Stops Clash Party with its core and starts it again (same elevation).
pub fn restart_party(cp: &ClashParty, core_pids: &[u32]) -> Result<(), String> {
    log(format!("control: restarting {}", cp.exe));
    stop_party(cp, core_pids)?;
    start_party(cp)
}

fn stop_party(cp: &ClashParty, core_pids: &[u32]) -> Result<(), String> {
    let dir = Path::new(&cp.exe).parent().map(|d| d.to_string_lossy().to_lowercase()).unwrap_or_default();
    let child_cores: Vec<u32> = core_pids
        .iter()
        .copied()
        .filter(|p| !dir.is_empty() && procinfo::image_path(*p).map(|x| x.to_lowercase().starts_with(&dir)).unwrap_or(false))
        .collect();

    // Verify we have permission to terminate BEFORE touching processes or override.yaml.
    let party_handles = open_terminable(&cp.pids)?;
    let core_handles = match open_terminable(&child_cores) {
        Ok(h) => h,
        Err(e) => {
            unsafe {
                for h in party_handles {
                    windows_sys::Win32::Foundation::CloseHandle(h);
                }
            }
            return Err(e);
        }
    };

    let deadline = Instant::now() + Duration::from_secs(3);
    terminate_and_wait(party_handles, deadline);
    terminate_and_wait(core_handles, deadline);
    // the killed app couldn't remove its tray icon: clear the leftover
    crate::system::sweep_tray();
    Ok(())
}

fn start_party(cp: &ClashParty) -> Result<(), String> {
    let r = if cp.elevated {
        std::process::Command::new(&cp.exe).spawn()
    } else {
        // we run elevated; explorer.exe starts it as the normal desktop user
        std::process::Command::new("explorer.exe").arg(&cp.exe).spawn()
    };
    r.map(|_| ()).map_err(|e| crate::tr!("启动 Clash Party 失败: {0}", e))
}

/// Waits until a controller answers and reports our rule providers.
fn wait_providers(timeout: Duration) -> Result<(Controller, Option<String>), String> {
    let t0 = Instant::now();
    let mut last = String::from(crate::i18n::t("Clash Party 没有启动内核"));
    while t0.elapsed() < timeout {
        std::thread::sleep(Duration::from_millis(700));
        let cfg = mihomo::read_config();
        if let Some((ctl, _)) = mihomo::discover(&cfg) {
            let guard_ok = cfg
                .file
                .as_deref()
                .map(|f| guard_rules_active(Path::new(f)))
                .unwrap_or(true);
            match providers_loaded(&ctl, cfg.secret.as_deref()) {
                Ok(true) if guard_ok => return Ok((ctl, cfg.secret)),
                Ok(_) => last = crate::i18n::t("内核已启动，但没有加载 MicaBar 规则集").into(),
                Err(e) => last = e,
            }
        }
    }
    Err(last)
}

pub fn providers_loaded(ctl: &Controller, secret: Option<&str>) -> Result<bool, String> {
    let body = mihomo::get(ctl, "/providers/rules", secret)?;
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let p = v.get("providers");
    Ok(provider_names().all(|(l, t)| p.and_then(|p| p.get(format!("micabar-{l}-{t}"))).is_some())
        && GUARD_PROVIDERS.iter().all(|(n, _, _)| p.and_then(|p| p.get(*n)).is_some()))
}

/// Installs (or removes) the override and restarts Clash Party once.
/// On install returns the controller of the restarted core.
pub fn set_installed(install: bool, procs: &[(u32, String)], core_pids: &[u32], cfg: &CoreConfig) -> Result<Option<(Controller, Option<String>)>, String> {
    let cp = find_clash_party(procs).ok_or(crate::i18n::t("没有找到正在运行的 Clash Party"))?;
    let home = core_pids.first().and_then(|p| core_home(*p)).or_else(|| {
        cfg.file.as_ref().and_then(|f| Path::new(f).parent().map(|p| p.to_path_buf()))
    });
    log(format!(
        "control: {} override; exe={} elevated={} data={} home={:?}",
        if install { "install" } else { "remove" },
        cp.exe,
        cp.elevated,
        cp.data_dir.display(),
        home
    ));
    let bak = backup(&cp.data_dir);
    if let Some(h) = &home {
        write_provider_files(h, &load_rules())?;
        ensure_guard_files(h)?;
        // rule files of the app's earlier name
        let _ = std::fs::remove_dir_all(h.join("netisland"));
    }
    stop_party(&cp, core_pids)?;
    if let Err(e) = write_override(&cp.data_dir, install) {
        restore(&cp, bak.as_deref());
        let _ = start_party(&cp);
        return Err(e);
    }
    if let Err(e) = start_party(&cp) {
        restore(&cp, bak.as_deref());
        if install {
            let _ = std::fs::remove_file(cp.data_dir.join("override").join(format!("{OVERRIDE_ID}.js")));
        }
        return Err(e);
    }
    if !install {
        log("control: override removed");
        return Ok(None);
    }
    match wait_providers(Duration::from_secs(30)) {
        Ok(v) => {
            log("control: override active");
            Ok(Some(v))
        }
        Err(e) => {
            log(format!("control: activation failed ({e}); rolling back"));
            let cp2 = find_clash_party(&procinfo::list_processes()).unwrap_or(cp);
            let cores: Vec<u32> = crate::proxy::client_name(&procinfo::list_processes()).1;
            let _ = stop_party(&cp2, &cores);
            restore(&cp2, bak.as_deref());
            let _ = std::fs::remove_file(cp2.data_dir.join("override").join(format!("{OVERRIDE_ID}.js")));
            let _ = start_party(&cp2);
            Err(crate::tr!("Clash Party 没能加载出口控制覆写，已还原原来的设置（{0}）", e))
        }
    }
}

fn restore(cp: &ClashParty, bak: Option<&Path>) {
    let dst = cp.data_dir.join("override.yaml");
    match bak {
        Some(b) => {
            if let Ok(bytes) = std::fs::read(b) {
                let _ = crate::settings::atomic_write(&dst, &bytes);
            } else {
                let _ = std::fs::copy(b, dst);
            }
        }
        None => {
            let _ = std::fs::remove_file(dst);
        }
    }
}

// ---------- rules ----------

pub fn write_provider_files(home: &Path, rules: &[Rule]) -> Result<(), String> {
    let dir = home.join("micabar");
    std::fs::create_dir_all(&dir).map_err(|e| crate::tr!("创建规则目录失败: {0}", e))?;
    for (lv, t) in provider_names() {
        let lines: Vec<String> = rules
            .iter()
            .filter(|r| r.target == t && r.level() == lv)
            .filter_map(|r| r.line())
            .filter(|l| !l.contains('\r') && !l.contains('\n'))
            .map(|l| format!("  - '{}'\n", l.replace('\'', "''")))
            .collect();
        let s = if lines.is_empty() {
            String::from("# 由 MicaBar 生成，请在 MicaBar 里修改\npayload: []\n")
        } else {
            format!("# 由 MicaBar 生成，请在 MicaBar 里修改\npayload:\n{}", lines.concat())
        };
        crate::settings::atomic_write(&dir.join(format!("{lv}-{t}.yaml")), s.as_bytes())
            .map_err(|e| crate::tr!("写入规则失败: {0}", e))?;
    }
    // files of the first version (one provider per target), no longer referenced
    for t in TARGETS {
        let _ = std::fs::remove_file(dir.join(format!("{t}.yaml")));
    }
    Ok(())
}

/// mihomo refuses to load a config whose file provider is missing: create empty guard files.
pub fn ensure_guard_files(home: &Path) -> Result<(), String> {
    let dir = home.join("micabar");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for (_, _, f) in GUARD_PROVIDERS {
        let p = dir.join(format!("{f}.yaml"));
        if !p.exists() {
            crate::settings::atomic_write(&p, b"payload: []\n").map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn reload_guard(ctl: &Controller, secret: Option<&str>) -> Result<(), String> {
    for (n, _, _) in GUARD_PROVIDERS {
        mihomo::request(ctl, "PUT", &format!("/providers/rules/{n}"), None, secret).map_err(|e| crate::tr!("刷新规则集 {0} 失败: {1}", n, e))?;
    }
    Ok(())
}

pub fn reload_providers(ctl: &Controller, secret: Option<&str>) -> Result<(), String> {
    for (l, t) in provider_names() {
        mihomo::request(ctl, "PUT", &format!("/providers/rules/micabar-{l}-{t}"), None, secret)
            .map_err(|e| crate::tr!("刷新规则集 micabar-{0}-{1} 失败: {2}", l, t, e))?;
    }
    Ok(())
}

/// Closes active connections the rule covers so the app reconnects under the new rule.
pub fn close_matching(ctl: &Controller, secret: Option<&str>, rule: &Rule) -> usize {
    let Ok(conns) = mihomo::connections(ctl, secret) else { return 0 };
    let mut n = 0;
    for c in conns.iter().filter(|c| rule.matches(c)) {
        if mihomo::request(ctl, "DELETE", &format!("/connections/{}", c.id), None, secret).is_ok() {
            n += 1;
        }
    }
    n
}

static RULE_SEQ: AtomicU64 = AtomicU64::new(1);

pub fn upsert(rules: &mut Vec<Rule>, kind: &str, process: Option<String>, value: Option<String>, target: &str) -> Rule {
    let process = process.as_deref().and_then(clean_atom);
    let value = value.as_deref().and_then(clean_atom);
    let now = crate::now_ms();
    let probe = Rule { id: String::new(), kind: kind.into(), process, value, target: target.into(), created: now };
    rules.retain(|r| r.key() != probe.key());
    let mut r = probe;
    let seq = RULE_SEQ.fetch_add(1, Ordering::Relaxed);
    r.id = format!("{now:x}{:04x}", seq & 0xffff);
    if target != "default" && r.line().is_some() {
        rules.push(r.clone());
    }
    r
}

pub fn persist(home: &Path, rules: &[Rule]) -> Result<(), String> {
    save_rules(rules)?;
    write_provider_files(home, rules)
}

// ---------- nodes ----------

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    pub name: String,
    pub kind: String,
    pub delay: Option<u32>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Nodes {
    pub now: Option<String>,
    pub nodes: Vec<NodeInfo>,
}

pub fn nodes(ctl: &Controller, secret: Option<&str>) -> Result<Nodes, String> {
    let body = mihomo::get(ctl, "/proxies", secret)?;
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let proxies = v.get("proxies").ok_or("no proxies")?;
    let g = proxies.get(GROUP).ok_or(crate::i18n::t("内核里没有 MicaBar 分组"))?;
    let now = g.get("now").and_then(|x| x.as_str()).map(|s| s.to_string());
    let all: Vec<String> = g
        .get("all")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    let nodes = all
        .into_iter()
        .map(|name| {
            let p = proxies.get(&name);
            let kind = p.and_then(|p| p.get("type")).and_then(|t| t.as_str()).unwrap_or("").to_string();
            let delay = p
                .and_then(|p| p.get("history"))
                .and_then(|h| h.as_array())
                .and_then(|h| h.last())
                .and_then(|h| h.get("delay"))
                .and_then(|d| d.as_u64())
                .filter(|d| *d > 0)
                .map(|d| d as u32);
            NodeInfo { name, kind, delay }
        })
        .collect();
    Ok(Nodes { now, nodes })
}

pub fn test_delays(ctl: &Controller, secret: Option<&str>) -> Result<(), String> {
    let url = "https%3A%2F%2Fwww.gstatic.com%2Fgenerate_204";
    mihomo::get(ctl, &format!("/group/{GROUP}/delay?url={url}&timeout=3000"), secret).map(|_| ())
}

pub fn select(ctl: &Controller, secret: Option<&str>, name: &str) -> Result<(), String> {
    let body = serde_json::json!({ "name": name }).to_string();
    mihomo::request(ctl, "PUT", &format!("/proxies/{GROUP}"), Some(&body), secret)?;
    // move existing MicaBar connections to the new node
    if let Ok(conns) = mihomo::connections(ctl, secret) {
        for c in conns.iter().filter(|c| c.chains.iter().any(|x| x == GROUP)) {
            let _ = mihomo::request(ctl, "DELETE", &format!("/connections/{}", c.id), None, secret);
        }
    }
    Ok(())
}
