//! AI subscription quota: Claude Code, Codex, Antigravity and Grok, read with the sign-in each tool
//! already keeps on this PC. Credentials are read from disk / the running app at query time and
//! sent only to that tool's own service (Antigravity: to its local language server); nothing is
//! stored, logged or refreshed — an expired sign-in is reported, the tool renews it itself.

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    /// "5 小时", "每周", a model name…
    pub label: String,
    /// percent used, 0..100
    pub used: f64,
    /// epoch ms
    pub resets_at: Option<u64>,
}

#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    /// claude | codex | antigravity | grok
    pub tool: String,
    pub name: String,
    pub plan: Option<String>,
    pub windows: Vec<Window>,
    /// ok | stale (last good values, latest query failed) | absent (not installed / signed in) | error
    pub status: String,
    pub message: Option<String>,
    /// where the numbers came from (shown as a hint)
    pub source: Option<String>,
    pub at: u64,
}

pub const TOOLS: [(&str, &str); 4] = [("claude", "Claude"), ("codex", "Codex"), ("antigravity", "Antigravity"), ("grok", "Grok")];

fn home() -> PathBuf {
    PathBuf::from(std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into()))
}

fn client(proxy_port: Option<u16>) -> Result<reqwest::Client, String> {
    let mut b = reqwest::Client::builder().timeout(Duration::from_secs(15)).connect_timeout(Duration::from_secs(8));
    if let Some(p) = proxy_port {
        b = b.proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{p}")).map_err(|e| e.to_string())?);
    }
    b.build().map_err(|e| e.to_string())
}

fn iso_ms(s: &str) -> Option<u64> {
    // "2026-11-19T12:20:00.253518+00:00" / "...Z" -> epoch ms (UTC; offset honoured)
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let n = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (n(0, 4)?, n(5, 7)?, n(8, 10)?, n(11, 13)?, n(14, 16)?, n(17, 19)?);
    // days from civil (Howard Hinnant)
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let mut secs = days * 86400 + h * 3600 + mi * 60 + se;
    let rest = &s[19..];
    let tz = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    if let Some(sign) = tz.chars().next().filter(|c| *c == '+' || *c == '-') {
        let oh = tz.get(1..3).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
        let om = tz.get(4..6).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
        let off = oh * 3600 + om * 60;
        secs += if sign == '+' { -off } else { off };
    }
    (secs > 0).then_some(secs as u64 * 1000)
}

/// epoch seconds / ms number or ISO string -> epoch ms
fn time_ms(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => {
            let x = n.as_f64()?;
            Some(if x > 1e12 { x as u64 } else { (x * 1000.0) as u64 })
        }
        Value::String(s) => iso_ms(s).or_else(|| s.parse::<f64>().ok().map(|x| if x > 1e12 { x as u64 } else { (x * 1000.0) as u64 })),
        _ => None,
    }
}

fn window_label(secs: f64) -> String {
    let h = secs / 3600.0;
    if (h - 5.0).abs() < 0.5 {
        "5 小时".into()
    } else if (h - 24.0 * 7.0).abs() < 2.0 {
        "每周".into()
    } else if h >= 24.0 * 27.0 {
        "每月".into()
    } else if h >= 24.0 {
        format!("{} 天", (h / 24.0).round())
    } else {
        format!("{} 小时", h.round())
    }
}

fn http_err(tool: &str, status: reqwest::StatusCode, body: &str) -> String {
    match status.as_u16() {
        401 | 403 => format!("{tool} 的登录已失效，打开 {tool} 用一下就会自动续期"),
        429 => "查询太频繁，稍后再试".into(),
        _ => format!("{tool} 返回 {}：{}", status.as_u16(), body.chars().take(80).collect::<String>()),
    }
}

// ---------- Claude Code ----------

async fn claude(proxy: Option<u16>) -> Result<Quota, String> {
    let dir = std::env::var("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|_| home().join(".claude"));
    let text = std::fs::read_to_string(dir.join(".credentials.json")).map_err(|_| "absent:没找到 Claude Code 的登录（~/.claude/.credentials.json）".to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let o = v.get("claudeAiOauth").ok_or("absent:Claude Code 没有用订阅账号登录")?;
    let token = o.get("accessToken").and_then(|t| t.as_str()).ok_or("absent:Claude Code 没有用订阅账号登录")?;
    if o.get("expiresAt").and_then(|e| e.as_u64()).map(|e| e < crate::now_ms()).unwrap_or(false) {
        return Err("Claude Code 的登录已过期，打开 Claude Code 用一下就会自动续期".into());
    }
    let plan = o.get("subscriptionType").and_then(|s| s.as_str()).map(|s| {
        let mut c = s.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    });
    let r = client(proxy)?
        .get("https://api.anthropic.com/api/oauth/usage")
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", "NetIsland")
        .send()
        .await
        .map_err(|e| format!("连不上 Anthropic：{e}"))?;
    let status = r.status();
    let body = r.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(http_err("Claude Code", status, &body));
    }
    let u: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let mut windows = Vec::new();
    for (key, label) in [("five_hour", "5 小时"), ("seven_day", "每周"), ("seven_day_opus", "每周 · Opus"), ("seven_day_sonnet", "每周 · Sonnet")] {
        if let Some(w) = u.get(key).filter(|w| w.is_object()) {
            if let Some(used) = w.get("utilization").and_then(|x| x.as_f64()) {
                windows.push(Window { label: label.into(), used, resets_at: w.get("resets_at").and_then(time_ms) });
            }
        }
    }
    Ok(Quota { plan, windows, source: Some("Anthropic 账户用量".into()), ..Default::default() })
}

// ---------- Codex ----------

fn codex_home() -> PathBuf {
    std::env::var("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".codex"))
}

fn codex_window(w: &Value, now: u64) -> Option<Window> {
    let used = w.get("used_percent").and_then(|x| x.as_f64())?;
    let secs = w
        .get("limit_window_seconds")
        .and_then(|x| x.as_f64())
        .or_else(|| w.get("window_minutes").and_then(|x| x.as_f64()).map(|m| m * 60.0))
        .unwrap_or(0.0);
    let resets_at = w
        .get("reset_at")
        .or_else(|| w.get("resets_at"))
        .and_then(time_ms)
        .or_else(|| w.get("reset_after_seconds").or_else(|| w.get("resets_in_seconds")).and_then(|x| x.as_f64()).map(|s| now + (s * 1000.0) as u64));
    Some(Window { label: if secs > 0.0 { window_label(secs) } else { "额度".into() }, used, resets_at })
}

/// Newest rate-limit snapshot Codex wrote into its session logs (offline fallback).
fn codex_from_sessions() -> Option<(Vec<Window>, u64)> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let mut stack = vec![codex_home().join("sessions")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().map(|x| x == "jsonl").unwrap_or(false) {
                if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                    files.push((m, p));
                }
            }
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (mtime, p) in files.into_iter().take(8) {
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        for line in text.lines().rev().filter(|l| l.contains("\"rate_limits\"")) {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let Some(rl) = find_key(&v, "rate_limits") else { continue };
            let at = mtime.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
            let ws: Vec<Window> = ["primary", "secondary"].iter().filter_map(|k| rl.get(*k).and_then(|w| codex_window(w, at))).collect();
            if !ws.is_empty() {
                return Some((ws, at));
            }
        }
    }
    None
}

fn find_key<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(m) => m.get(key).or_else(|| m.values().find_map(|x| find_key(x, key))),
        Value::Array(a) => a.iter().find_map(|x| find_key(x, key)),
        _ => None,
    }
}

async fn codex(proxy: Option<u16>) -> Result<Quota, String> {
    let auth = std::fs::read_to_string(codex_home().join("auth.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let tokens = auth.as_ref().and_then(|a| a.get("tokens")).filter(|t| t.get("access_token").and_then(|x| x.as_str()).is_some());
    if let Some(t) = tokens {
        let token = t.get("access_token").and_then(|x| x.as_str()).unwrap_or_default();
        let mut req = client(proxy)?
            .get("https://chatgpt.com/backend-api/wham/usage")
            .bearer_auth(token)
            .header("User-Agent", "codex_cli_rs")
            .header("Accept", "application/json");
        if let Some(acc) = t.get("account_id").and_then(|x| x.as_str()) {
            req = req.header("ChatGPT-Account-Id", acc);
        }
        match req.send().await {
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                if status.is_success() {
                    let u: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
                    let now = crate::now_ms();
                    let rl = u.get("rate_limit").cloned().unwrap_or(Value::Null);
                    let windows: Vec<Window> = ["primary_window", "secondary_window"].iter().filter_map(|k| rl.get(*k).and_then(|w| codex_window(w, now))).collect();
                    let plan = u.get("plan_type").and_then(|p| p.as_str()).map(|p| p.to_string());
                    return Ok(Quota { plan, windows, source: Some("ChatGPT 账户用量".into()), ..Default::default() });
                }
                if let Some((windows, at)) = codex_from_sessions() {
                    return Ok(Quota { windows, source: Some(format!("最近一次 Codex 会话（{}）", crate::fmt_time(at))), ..Default::default() });
                }
                return Err(http_err("Codex", status, &body));
            }
            Err(e) => {
                if let Some((windows, at)) = codex_from_sessions() {
                    return Ok(Quota { windows, source: Some(format!("最近一次 Codex 会话（{}）", crate::fmt_time(at))), ..Default::default() });
                }
                return Err(format!("连不上 chatgpt.com：{e}"));
            }
        }
    }
    if let Some((windows, at)) = codex_from_sessions() {
        return Ok(Quota { windows, source: Some(format!("最近一次 Codex 会话（{}）", crate::fmt_time(at))), ..Default::default() });
    }
    // a custom model provider (API key / relay) has no subscription quota
    let cfg = std::fs::read_to_string(codex_home().join("config.toml")).unwrap_or_default();
    let provider = cfg.lines().find_map(|l| {
        let l = l.trim();
        l.strip_prefix("model_provider").map(|r| r.trim_start().trim_start_matches('=').trim().trim_matches('"').to_string())
    });
    match provider {
        Some(p) if !p.is_empty() && p != "openai" => Err("absent:Codex 用的是自定义服务商（按 API 计费），没有订阅额度".into()),
        _ if codex_home().exists() => Err("absent:Codex 没有用 ChatGPT 账号登录（没有 auth.json）".into()),
        _ => Err("absent:没找到 Codex".into()),
    }
}

// ---------- Antigravity ----------

fn command_line(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryInformationProcess(h: *mut core::ffi::c_void, class: u32, buf: *mut core::ffi::c_void, len: u32, ret: *mut u32) -> i32;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = vec![0u8; 64 * 1024];
        let mut ret = 0u32;
        // ProcessCommandLineInformation = 60: a UNICODE_STRING followed by its characters
        let st = NtQueryInformationProcess(h, 60, buf.as_mut_ptr() as _, buf.len() as u32, &mut ret);
        CloseHandle(h);
        if st < 0 {
            return None;
        }
        let len = u16::from_le_bytes([buf[0], buf[1]]) as usize / 2;
        let ptr = usize::from_le_bytes(buf[8..16].try_into().ok()?) as *const u16;
        if ptr.is_null() || len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len)))
    }
}

fn arg_value(cmd: &str, name: &str) -> Option<String> {
    let mut it = cmd.split_whitespace();
    while let Some(a) = it.next() {
        if let Some(v) = a.strip_prefix(&format!("{name}=")) {
            return Some(v.trim_matches('"').to_string());
        }
        if a == name {
            return it.next().map(|v| v.trim_matches('"').to_string());
        }
    }
    None
}

/// Objects anywhere in the reply that carry a remaining fraction: model configs
/// ({label, quotaInfo: {remainingFraction, resetTime}}) or quota buckets
/// ({displayName, remaining: {remainingFraction}, resetTime}).
fn ag_windows(v: &Value, out: &mut Vec<Window>) {
    match v {
        Value::Object(m) => {
            let label = m.get("label").or_else(|| m.get("displayName")).and_then(|x| x.as_str());
            let q = m.get("quotaInfo").or_else(|| m.get("remaining"));
            if let (Some(label), Some(q)) = (label, q) {
                if let Some(rem) = q.get("remainingFraction").and_then(|x| x.as_f64()) {
                    let reset = q.get("resetTime").or_else(|| m.get("resetTime")).and_then(time_ms);
                    if !out.iter().any(|w| w.label == label) {
                        out.push(Window { label: label.to_string(), used: ((1.0 - rem) * 100.0).clamp(0.0, 100.0), resets_at: reset });
                    }
                    return;
                }
            }
            m.values().for_each(|x| ag_windows(x, out));
        }
        Value::Array(a) => a.iter().for_each(|x| ag_windows(x, out)),
        _ => {}
    }
}

async fn antigravity() -> Result<Quota, String> {
    let procs: Vec<u32> = crate::procinfo::list_processes()
        .into_iter()
        .filter(|(_, n)| n.to_ascii_lowercase().starts_with("language_server"))
        .map(|(p, _)| p)
        .collect();
    let installed = std::env::var("LOCALAPPDATA").map(|l| PathBuf::from(l).join("Programs").join("Antigravity").exists()).unwrap_or(false);
    let server = procs.iter().find_map(|&p| {
        let cmd = command_line(p)?;
        let csrf = arg_value(&cmd, "--csrf_token")?;
        Some((p, csrf))
    });
    let Some((pid, csrf)) = server else {
        return Err(if installed || !procs.is_empty() { "absent:Antigravity 没在运行（打开它并进入一个工作区后才能读到额度）".into() } else { "absent:没找到 Antigravity".into() });
    };
    let ports: Vec<u16> = crate::conn::snapshot()
        .into_iter()
        .filter(|s| s.pid == pid && (s.remote.is_none() || s.state == "LISTEN") && matches!(s.proto, crate::conn::Proto::Tcp))
        .map(|s| s.local_port)
        .collect();
    let c = reqwest::Client::builder()
        .danger_accept_invalid_certs(true) // the language server uses a self-signed loopback certificate
        .no_proxy()
        .timeout(Duration::from_secs(6))
        .build()
        .map_err(|e| e.to_string())?;
    let body = serde_json::json!({ "metadata": { "ideName": "antigravity", "extensionName": "antigravity", "locale": "zh-CN" } });
    let mut last_err = String::from("语言服务器没有响应");
    for port in ports {
        for scheme in ["https", "http"] {
            for method in ["RetrieveUserQuotaSummary", "GetUserStatus"] {
                let url = format!("{scheme}://127.0.0.1:{port}/exa.language_server_pb.LanguageServerService/{method}");
                let r = c
                    .post(&url)
                    .header("X-Codeium-Csrf-Token", &csrf)
                    .header("Connect-Protocol-Version", "1")
                    .json(&body)
                    .send()
                    .await;
                let Ok(r) = r else { continue };
                if !r.status().is_success() {
                    last_err = format!("语言服务器返回 {}", r.status().as_u16());
                    continue;
                }
                let Ok(v) = r.json::<Value>().await else { continue };
                let mut windows = Vec::new();
                ag_windows(&v, &mut windows);
                if windows.is_empty() {
                    continue;
                }
                let plan = find_key(&v, "planName").and_then(|p| p.as_str()).map(|s| s.to_string());
                return Ok(Quota { plan, windows, source: Some("Antigravity 本机语言服务器".into()), ..Default::default() });
            }
        }
    }
    Err(last_err)
}

// ---------- Grok ----------

async fn grok(proxy: Option<u16>) -> Result<Quota, String> {
    let dir = std::env::var("GROK_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".grok"));
    let text = std::fs::read_to_string(dir.join("auth.json")).map_err(|_| "absent:没找到 Grok 的登录（需要先用 grok CLI 登录一次）".to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    // one entry per sign-in scope; take the first with a bearer key that hasn't expired
    let now = crate::now_ms();
    let mut entries: Vec<&Value> = match &v {
        Value::Object(m) if m.contains_key("key") => vec![&v],
        Value::Object(m) => m.values().collect(),
        Value::Array(a) => a.iter().collect(),
        _ => vec![],
    };
    entries.retain(|e| e.get("key").and_then(|k| k.as_str()).is_some());
    let entry = entries
        .iter()
        .find(|e| e.get("expires_at").and_then(time_ms).map(|t| t > now).unwrap_or(true))
        .ok_or("Grok 的登录已过期，运行一次 grok 就会自动续期")?;
    let key = entry.get("key").and_then(|k| k.as_str()).unwrap_or_default();
    let c = client(proxy)?;
    let get = |path: &str| {
        c.get(format!("https://cli-chat-proxy.grok.com{path}"))
            .bearer_auth(key)
            .header("x-xai-token-auth", "xai-grok-cli")
            .header("Accept", "application/json")
            .send()
    };
    let r = get("/v1/billing?format=credits").await.map_err(|e| format!("连不上 grok.com：{e}"))?;
    let status = r.status();
    let body = r.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(http_err("Grok", status, &body));
    }
    let b: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let cfg = b.get("config").unwrap_or(&b);
    let end = cfg.pointer("/currentPeriod/end").or_else(|| cfg.get("billingPeriodEnd")).and_then(time_ms);
    let mut windows = Vec::new();
    if let Some(p) = cfg.get("creditUsagePercent").and_then(|x| x.as_f64()) {
        windows.push(Window { label: "本期额度".into(), used: p, resets_at: end });
    }
    if let Some(list) = cfg.get("productUsage").and_then(|x| x.as_array()) {
        for p in list {
            if let (Some(name), Some(u)) = (p.get("product").and_then(|x| x.as_str()), p.get("usagePercent").and_then(|x| x.as_f64())) {
                windows.push(Window { label: name.to_string(), used: u, resets_at: end });
            }
        }
    }
    let plan = match get("/v1/settings").await {
        Ok(r) => r.json::<Value>().await.ok().and_then(|s| s.get("subscription_tier_display").and_then(|x| x.as_str()).map(|x| x.to_string())),
        Err(_) => None,
    };
    if windows.is_empty() {
        return Err("Grok 没返回额度信息".into());
    }
    Ok(Quota { plan, windows, source: Some("Grok 账户用量".into()), ..Default::default() })
}

/// Queries one tool. `prev` keeps the last good numbers when a query fails.
pub async fn query(tool: &str, proxy: Option<u16>, prev: Option<&Quota>) -> Quota {
    let name = TOOLS.iter().find(|(t, _)| *t == tool).map(|(_, n)| *n).unwrap_or(tool).to_string();
    let r = match tool {
        "claude" => claude(proxy).await,
        "codex" => codex(proxy).await,
        "antigravity" => antigravity().await,
        "grok" => grok(proxy).await,
        _ => Err("absent:未知工具".into()),
    };
    let at = crate::now_ms();
    match r {
        Ok(mut q) => {
            q.tool = tool.into();
            q.name = name;
            q.status = "ok".into();
            q.at = at;
            q
        }
        Err(e) if e.starts_with("absent:") => Quota { tool: tool.into(), name, status: "absent".into(), message: Some(e[7..].to_string()), at, ..Default::default() },
        Err(e) => match prev.filter(|p| !p.windows.is_empty()) {
            Some(p) => Quota { status: "stale".into(), message: Some(e), ..p.clone() },
            None => Quota { tool: tool.into(), name, status: "error".into(), message: Some(e), at, ..Default::default() },
        },
    }
}
