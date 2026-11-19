//! Exit control for Clash Party / Mihomo Party: per-process / per-domain "proxy / direct / reject".
//!
//! One-time setup adds a global override to Clash Party that declares file rule-providers
//! netisland-<level>-<target> (files inside the core's home dir) placed at the top of the rules,
//! plus a select group "NetIsland" containing every node. Levels order rules by specificity, so a
//! narrower rule wins over a broader one whatever their targets:
//!   1 app + exact domain/IP, 2 app + main domain, 3 exact domain/IP, 4 main domain, 5 whole app.
//! After setup, every change only rewrites those files and asks mihomo to reload the providers
//! (no core restart), then closes the affected connections so they reconnect under the new rule.
//!
//! Clash Party caches override.yaml in memory and writes the cache back on its next save, so the
//! setup closes Clash Party first, writes the files, and starts it again with the same elevation.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::mihomo::{self, Controller, CoreConfig};
use crate::{log, procinfo};

pub const OVERRIDE_ID: &str = "netisland";
pub const GROUP: &str = "NetIsland";
const TARGETS: [&str; 3] = ["reject", "direct", "proxy"];
const LEVELS: [u8; 5] = [1, 2, 3, 4, 5];

fn provider_names() -> impl Iterator<Item = (u8, &'static str)> {
    LEVELS.into_iter().flat_map(|l| TARGETS.into_iter().map(move |t| (l, t)))
}
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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

    fn ip_cidr(v: &str) -> String {
        if v.contains(':') {
            format!("IP-CIDR6,{v}/128,no-resolve")
        } else {
            format!("IP-CIDR,{v}/32,no-resolve")
        }
    }

    /// The mihomo classical rule (without the target).
    pub fn line(&self) -> Option<String> {
        let p = self.process.as_deref().unwrap_or("");
        let v = self.value.as_deref().unwrap_or("");
        Some(match self.kind.as_str() {
            "process" => format!("PROCESS-NAME,{p}"),
            "domain" => format!("DOMAIN,{v}"),
            "suffix" => format!("DOMAIN-SUFFIX,{v}"),
            "ip" => Self::ip_cidr(v),
            "process_domain" => format!("AND,((PROCESS-NAME,{p}),(DOMAIN,{v}))"),
            "process_suffix" => format!("AND,((PROCESS-NAME,{p}),(DOMAIN-SUFFIX,{v}))"),
            "process_ip" => format!("AND,((PROCESS-NAME,{p}),({}))", Self::ip_cidr(v)),
            _ => return None,
        })
    }

    /// Whether an active mihomo connection is covered by this rule (to close it after a change).
    fn matches(&self, c: &mihomo::MConn) -> bool {
        let m = &c.metadata;
        let proc_ok = |p: &Option<String>| p.as_ref().map(|p| m.process.eq_ignore_ascii_case(p)).unwrap_or(true);
        let host = if m.host.is_empty() { &m.sniff_host } else { &m.host };
        let v = self.value.clone().unwrap_or_default().to_lowercase();
        let h = host.to_lowercase();
        let dom = h == v;
        let suf = h == v || h.ends_with(&format!(".{v}"));
        let ip = m.destination_ip == v;
        match self.kind.as_str() {
            "process" => proc_ok(&self.process),
            "domain" => dom,
            "suffix" => suf,
            "ip" => ip,
            "process_domain" => proc_ok(&self.process) && dom,
            "process_suffix" => proc_ok(&self.process) && suf,
            "process_ip" => proc_ok(&self.process) && ip,
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
    let d = PathBuf::from(base).join("NetIsland");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn load_rules() -> Vec<Rule> {
    std::fs::read_to_string(state_dir().join("rules.json"))
        .ok()
        .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default()
}

fn save_rules(rules: &[Rule]) -> Result<(), String> {
    let t = serde_json::to_string_pretty(rules).map_err(|e| e.to_string())?;
    std::fs::write(state_dir().join("rules.json"), t).map_err(|e| e.to_string())
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

fn data_dir_for(exe: Option<&str>) -> PathBuf {
    if let Some(exe) = exe {
        if let Some(dir) = Path::new(exe).parent() {
            if dir.join("PORTABLE").exists() {
                return dir.join("data");
            }
        }
    }
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    PathBuf::from(appdata).join("mihomo-party")
}

pub fn find_clash_party(procs: &[(u32, String)]) -> Option<ClashParty> {
    let pids: Vec<u32> = procs.iter().filter(|(_, e)| PARTY_EXES.contains(&e.to_lowercase().as_str())).map(|(p, _)| *p).collect();
    let exe = pids.iter().find_map(|p| procinfo::image_path(*p))?;
    let elevated = pids.iter().any(|p| is_elevated(*p));
    Some(ClashParty { data_dir: data_dir_for(Some(&exe)), exe, pids, elevated })
}

/// The core's home directory (its `-d` argument), where provider files must live
/// (mihomo only accepts provider paths inside its home dir).
pub fn core_home(core_pid: u32) -> Option<PathBuf> {
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

/// Per-group inbounds of the running config: (group name, port), read from the generated
/// config file (the override names them netisland-g<i>).
pub fn group_listeners(config_file: &Path) -> Vec<(String, u16)> {
    let Ok(text) = std::fs::read_to_string(config_file) else { return Vec::new() };
    let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&text) else { return Vec::new() };
    doc.get("listeners")
        .and_then(|l| l.as_sequence())
        .map(|s| {
            s.iter()
                .filter(|l| l.get("name").and_then(|n| n.as_str()).map(|n| n.starts_with("netisland-g")).unwrap_or(false))
                .filter_map(|l| {
                    let proxy = l.get("proxy")?.as_str()?.to_string();
                    let port = l.get("port")?.as_u64()? as u16;
                    Some((proxy, port))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The subscription's fallback group (target of the MATCH rule) in the running config.
pub fn match_group(config_file: &Path) -> Option<String> {
    let text = std::fs::read_to_string(config_file).ok()?;
    let doc: serde_yaml::Value = serde_yaml::from_str(&text).ok()?;
    doc.get("rules")?.as_sequence()?.iter().rev().find_map(|r| {
        let s = r.as_str()?;
        s.strip_prefix("MATCH,").map(|g| g.split(',').next().unwrap_or(g).trim().to_string())
    })
}

pub const GUARD_PROVIDERS: [(&str, &str, &str); 3] = [
    ("netisland-guard-domain", "domain", "guard-domain"),
    ("netisland-guard-kw", "classical", "guard-kw"),
    ("netisland-guard-ip", "ipcidr", "guard-ip"),
];

/// Clash Party JavaScript override. Besides NetIsland's own rules (prepended, by specificity) it
/// inserts the leak-guard rule sets after the subscription's last proxy-group rule, right before
/// the next rule that would send traffic DIRECT (literally DIRECT, or a group whose default
/// choice is DIRECT). Service rules keep their groups; guard domains that would otherwise fall
/// through to the broad direct rules are sent to the NetIsland group instead.
pub fn override_js() -> String {
    let own: Vec<String> = provider_names()
        .map(|(l, t)| format!("[\"netisland-{l}-{t}\", \"{l}-{t}\", \"{}\"]", target_policy(t)))
        .collect();
    let guard: Vec<String> = GUARD_PROVIDERS.iter().map(|(n, b, f)| format!("[\"{n}\", \"{b}\", \"{f}\"]")).collect();
    format!(
        r#"// NetIsland 出口控制 + 泄漏守卫：由 NetIsland 自动添加。删除这个覆写即可停用。
function main(config) {{
  const G = "{GROUP}";
  const own = [{own}];
  const guard = [{guard}];
  const providers = config["rule-providers"] = config["rule-providers"] || {{}};
  for (const [name, file] of own) {{
    providers[name] = {{ type: "file", behavior: "classical", format: "yaml", path: "./netisland/" + file + ".yaml" }};
  }}
  for (const [name, behavior, file] of guard) {{
    providers[name] = {{ type: "file", behavior: behavior, format: "yaml", path: "./netisland/" + file + ".yaml" }};
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
  const rest = (config.rules || []).filter((r) => !String(r).includes("netisland-"));
  // insert after the last rule that routes to a proxy group, before the next direct rule, so
  // every service rule of the subscription keeps its group and only the broad direct rules
  // (cn_domain, GEOIP CN, a direct MATCH...) are guarded
  // only destination rules count (protocol / port / process / inbound rules such as
  // NETWORK,UDP or DST-PORT don't decide where a domain goes)
  const notProxy = new Set(["REJECT", "REJECT-DROP", "PASS"]);
  const destTypes = new Set(["RULE-SET", "DOMAIN", "DOMAIN-SUFFIX", "DOMAIN-KEYWORD", "DOMAIN-REGEX",
    "DOMAIN-WILDCARD", "GEOSITE", "GEOIP", "IP-CIDR", "IP-CIDR6", "IP-SUFFIX", "IP-ASN", "AND", "OR", "NOT"]);
  let lastProxy = -1;
  rest.forEach((r, i) => {{
    const type = String(r).split(",")[0].trim().toUpperCase();
    const p = policy(r);
    if (destTypes.has(type) && !directish.has(p) && !notProxy.has(p)) lastProxy = i;
  }});
  const at = rest.findIndex((r, i) => i > lastProxy && directish.has(policy(r)));
  if (at >= 0) {{
    rest.splice(at, 0,
      "RULE-SET,netisland-guard-domain," + G,
      "RULE-SET,netisland-guard-kw," + G,
      "RULE-SET,netisland-guard-ip," + G + ",no-resolve");
  }}
  config.rules = own.map(([name, , target]) => "RULE-SET," + name + "," + target).concat(rest);
  // one loopback-only inbound per proxy group: traffic entering it skips the rules and goes
  // straight to that group, so NetIsland can measure every group's exit IP
  const kinds = new Set(["select", "url-test", "fallback", "load-balance", "smart"]);
  const listeners = (config.listeners || []).filter((l) => !String(l && l.name).startsWith("netisland-g"));
  groups.filter((g) => g && kinds.has(String(g.type).toLowerCase())).slice(0, 80).forEach((g, i) => {{
    listeners.push({{ name: "netisland-g" + i, type: "mixed", listen: "127.0.0.1", port: {listen_base} + i, proxy: g.name, udp: false }});
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
        .and_then(|s| s.iter().find(|it| it.get("id").and_then(|v| v.as_str()) == Some(OVERRIDE_ID)).cloned());
    item.map(|it| it.get("global").and_then(|g| g.as_bool()).unwrap_or(false)).unwrap_or(false)
        && (data_dir.join("override").join(format!("{OVERRIDE_ID}.js")).exists()
            || data_dir.join("override").join(format!("{OVERRIDE_ID}.yaml")).exists())
}

fn write_override(data_dir: &Path, install: bool) -> Result<(), String> {
    let mut list = read_override_list(data_dir);
    if list.get("items").and_then(|v| v.as_sequence()).is_none() {
        list = serde_yaml::from_str("items: []").unwrap();
    }
    let items = list.get_mut("items").and_then(|v| v.as_sequence_mut()).ok_or("override.yaml 格式不对")?;
    items.retain(|it| it.get("id").and_then(|v| v.as_str()) != Some(OVERRIDE_ID));
    let file = data_dir.join("override").join(format!("{OVERRIDE_ID}.js"));
    // the first versions wrote a YAML override
    let _ = std::fs::remove_file(data_dir.join("override").join(format!("{OVERRIDE_ID}.yaml")));
    if install {
        let now = crate::now_ms();
        let item: serde_yaml::Value = serde_yaml::from_str(&format!(
            "id: {OVERRIDE_ID}\nname: NetIsland 出口控制\ntype: local\next: js\nglobal: true\nupdated: {now}\n"
        ))
        .map_err(|e| e.to_string())?;
        items.push(item);
        std::fs::create_dir_all(data_dir.join("override")).map_err(|e| e.to_string())?;
        std::fs::write(&file, override_js()).map_err(|e| format!("写入覆写失败: {e}"))?;
    } else {
        let _ = std::fs::remove_file(&file);
    }
    let text = serde_yaml::to_string(&list).map_err(|e| e.to_string())?;
    std::fs::write(data_dir.join("override.yaml"), text).map_err(|e| format!("写入 override.yaml 失败: {e}"))
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

fn kill(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !h.is_null() {
            TerminateProcess(h, 1);
            CloseHandle(h);
        }
    }
}

fn stop_party(cp: &ClashParty, core_pids: &[u32]) {
    for p in &cp.pids {
        kill(*p);
    }
    std::thread::sleep(Duration::from_millis(600));
    // the core is a child of the app; make sure the old one is gone too
    let dir = Path::new(&cp.exe).parent().map(|d| d.to_string_lossy().to_lowercase()).unwrap_or_default();
    for p in core_pids {
        if procinfo::image_path(*p).map(|x| x.to_lowercase().starts_with(&dir)).unwrap_or(false) {
            kill(*p);
        }
    }
    std::thread::sleep(Duration::from_millis(400));
    // the killed app couldn't remove its tray icon: clear the leftover
    crate::system::sweep_tray();
}

fn start_party(cp: &ClashParty) -> Result<(), String> {
    let r = if cp.elevated {
        std::process::Command::new(&cp.exe).spawn()
    } else {
        // we run elevated; explorer.exe starts it as the normal desktop user
        std::process::Command::new("explorer.exe").arg(&cp.exe).spawn()
    };
    r.map(|_| ()).map_err(|e| format!("启动 Clash Party 失败: {e}"))
}

/// Waits until a controller answers and reports our rule providers.
fn wait_providers(timeout: Duration) -> Result<(Controller, Option<String>), String> {
    let t0 = Instant::now();
    let mut last = String::from("Clash Party 没有启动内核");
    while t0.elapsed() < timeout {
        std::thread::sleep(Duration::from_millis(700));
        let cfg = mihomo::read_config();
        if let Some((ctl, _)) = mihomo::discover(&cfg) {
            match providers_loaded(&ctl, cfg.secret.as_deref()) {
                Ok(true) => return Ok((ctl, cfg.secret)),
                Ok(false) => last = "内核已启动，但没有加载 NetIsland 规则集".into(),
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
    Ok(provider_names().all(|(l, t)| p.and_then(|p| p.get(format!("netisland-{l}-{t}"))).is_some())
        && GUARD_PROVIDERS.iter().all(|(n, _, _)| p.and_then(|p| p.get(*n)).is_some()))
}

/// Installs (or removes) the override and restarts Clash Party once.
/// On install returns the controller of the restarted core.
pub fn set_installed(install: bool, procs: &[(u32, String)], core_pids: &[u32], cfg: &CoreConfig) -> Result<Option<(Controller, Option<String>)>, String> {
    let cp = find_clash_party(procs).ok_or("没有找到正在运行的 Clash Party")?;
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
    }
    stop_party(&cp, core_pids);
    if let Err(e) = write_override(&cp.data_dir, install) {
        restore(&cp, bak.as_deref());
        let _ = start_party(&cp);
        return Err(e);
    }
    start_party(&cp)?;
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
            stop_party(&cp2, &cores);
            restore(&cp2, bak.as_deref());
            let _ = std::fs::remove_file(cp2.data_dir.join("override").join(format!("{OVERRIDE_ID}.js")));
            let _ = start_party(&cp2);
            Err(format!("Clash Party 没能加载出口控制覆写，已还原原来的设置（{e}）"))
        }
    }
}

fn restore(cp: &ClashParty, bak: Option<&Path>) {
    if let Some(b) = bak {
        let _ = std::fs::copy(b, cp.data_dir.join("override.yaml"));
    }
}

// ---------- rules ----------

pub fn write_provider_files(home: &Path, rules: &[Rule]) -> Result<(), String> {
    let dir = home.join("netisland");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建规则目录失败: {e}"))?;
    for (lv, t) in provider_names() {
        let lines: Vec<String> = rules
            .iter()
            .filter(|r| r.target == t && r.level() == lv)
            .filter_map(|r| r.line())
            .map(|l| format!("  - '{}'\n", l.replace('\'', "''")))
            .collect();
        let s = if lines.is_empty() {
            String::from("# 由 NetIsland 生成，请在 NetIsland 里修改\npayload: []\n")
        } else {
            format!("# 由 NetIsland 生成，请在 NetIsland 里修改\npayload:\n{}", lines.concat())
        };
        std::fs::write(dir.join(format!("{lv}-{t}.yaml")), s).map_err(|e| format!("写入规则失败: {e}"))?;
    }
    // files of the first version (one provider per target), no longer referenced
    for t in TARGETS {
        let _ = std::fs::remove_file(dir.join(format!("{t}.yaml")));
    }
    Ok(())
}

/// mihomo refuses to load a config whose file provider is missing: create empty guard files.
pub fn ensure_guard_files(home: &Path) -> Result<(), String> {
    let dir = home.join("netisland");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for (_, _, f) in GUARD_PROVIDERS {
        let p = dir.join(format!("{f}.yaml"));
        if !p.exists() {
            std::fs::write(&p, "payload: []\n").map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn reload_guard(ctl: &Controller, secret: Option<&str>) -> Result<(), String> {
    for (n, _, _) in GUARD_PROVIDERS {
        mihomo::request(ctl, "PUT", &format!("/providers/rules/{n}"), None, secret).map_err(|e| format!("刷新规则集 {n} 失败: {e}"))?;
    }
    Ok(())
}

pub fn reload_providers(ctl: &Controller, secret: Option<&str>) -> Result<(), String> {
    for (l, t) in provider_names() {
        mihomo::request(ctl, "PUT", &format!("/providers/rules/netisland-{l}-{t}"), None, secret)
            .map_err(|e| format!("刷新规则集 netisland-{l}-{t} 失败: {e}"))?;
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

pub fn upsert(rules: &mut Vec<Rule>, kind: &str, process: Option<String>, value: Option<String>, target: &str) -> Rule {
    let probe = Rule { id: String::new(), kind: kind.into(), process, value, target: target.into(), created: crate::now_ms() };
    rules.retain(|r| r.key() != probe.key());
    let mut r = probe;
    r.id = format!("{:x}", crate::now_ms() ^ (rules.len() as u64) << 40);
    if target != "default" {
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
    let g = proxies.get(GROUP).ok_or("内核里没有 NetIsland 分组")?;
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
    // move existing NetIsland connections to the new node
    if let Ok(conns) = mihomo::connections(ctl, secret) {
        for c in conns.iter().filter(|c| c.chains.iter().any(|x| x == GROUP)) {
            let _ = mihomo::request(ctl, "DELETE", &format!("/connections/{}", c.id), None, secret);
        }
    }
    Ok(())
}
