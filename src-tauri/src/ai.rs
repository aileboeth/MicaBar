//! AI subscription quota: Claude Code, Codex, Antigravity and Grok, read with the sign-in each tool
//! already keeps on this PC. Credentials are read from disk / the running app at query time and
//! sent only to that tool's own service (Antigravity: to its local language server); nothing is
//! stored, logged or refreshed — an expired sign-in is reported, the tool renews it itself.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Window {
    /// "5 小时", "每周", a model name…
    pub label: String,
    /// percent used, 0..100
    pub used: f64,
    /// epoch ms
    pub resets_at: Option<u64>,
    /// pool the window belongs to when a tool has several (Antigravity: "Gemini", "Claude / GPT")
    pub group: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase", default)]
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
    /// proxy port the last query went through (none for Antigravity, which is asked locally)
    pub via: Option<u16>,
}

pub const TOOLS: [(&str, &str); 4] = [
    ("claude", "Claude"),
    ("codex", "Codex"),
    ("antigravity", "Antigravity"),
    ("grok", "Grok"),
];

fn home() -> PathBuf {
    PathBuf::from(std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into()))
}

/// Error markers: "soon:" = never reached the service (look again in a minute); NO_PROXY is
/// replaced by its sentence before it is shown.
pub const NO_PROXY: &str = "soon:noproxy";
fn no_proxy_text() -> String {
    crate::i18n::t("连不上：没找到代理软件的本地端口，额度只经代理查询，不会直连（代理软件开着就会自动接上，也可以在设置 → AI 额度里填端口）")
}

/// Cached HTTP/1.1 clients keyed by local proxy port so TLS root stores and pools are reused.
/// The account services are only ever asked through the proxy client's local port: never
/// directly (direct requests from China fail or get the account flagged).
fn client(proxy_port: Option<u16>) -> Result<reqwest::Client, String> {
    static CLIENTS: OnceLock<Mutex<HashMap<u16, reqwest::Client>>> = OnceLock::new();
    let p = proxy_port.ok_or_else(|| NO_PROXY.to_string())?;
    let map = CLIENTS.get_or_init(Default::default);
    if let Some(c) = map.lock().unwrap().get(&p).cloned() {
        return Ok(c);
    }
    // HTTP/1.1 only: these account endpoints are known to be picky about HTTP/2 clients
    let built = reqwest::Client::builder()
        .http1_only()
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(8))
        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{p}")).map_err(|e| e.to_string())?)
        .build()
        .map_err(|e| e.to_string())?;
    let mut g = map.lock().unwrap();
    if g.len() >= 8 {
        g.clear();
    }
    g.insert(p, built.clone());
    Ok(built)
}

/// Cached local no-proxy client for Antigravity's self-signed loopback language server.
fn local_client() -> Result<reqwest::Client, String> {
    static LOCAL: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = LOCAL.get() {
        return Ok(c.clone());
    }
    let built = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .timeout(Duration::from_secs(6))
        .build()
        .map_err(|e| e.to_string())?;
    let _ = LOCAL.set(built.clone());
    Ok(built)
}

/// Converts epoch ms to wall-clock `(hour, minute, second)` in the system's local timezone.
fn local_hms(epoch_ms: u64) -> (u16, u16, u16) {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    #[link(name = "kernel32")]
    extern "system" {
        fn FileTimeToSystemTime(lpfiletime: *const FILETIME, lpsystemtime: *mut SYSTEMTIME) -> i32;
        fn SystemTimeToTzSpecificLocalTime(
            lptimezoneinformation: *const core::ffi::c_void,
            lpuniversaltime: *const SYSTEMTIME,
            lplocaltime: *mut SYSTEMTIME,
        ) -> i32;
    }
    // 1601-01-01 -> 1970-01-01 is 11_644_473_600_000 ms, in 100-ns FILETIME ticks
    let ticks = (epoch_ms as u128 + 11_644_473_600_000u128).saturating_mul(10_000);
    let ft = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    unsafe {
        let mut utc: SYSTEMTIME = std::mem::zeroed();
        let mut loc: SYSTEMTIME = std::mem::zeroed();
        if FileTimeToSystemTime(&ft, &mut utc) != 0
            && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut loc) != 0
        {
            return (loc.wHour, loc.wMinute, loc.wSecond);
        }
    }
    let s = (epoch_ms / 1000) % 86400;
    ((s / 3600) as u16, ((s % 3600) / 60) as u16, (s % 60) as u16)
}

fn fmt_local_hms(epoch_ms: u64) -> String {
    let (h, m, s) = local_hms(epoch_ms);
    format!("{h:02}:{m:02}:{s:02}")
}

fn fmt_local_hm(epoch_ms: u64) -> String {
    let (h, m, _) = local_hms(epoch_ms);
    format!("{h:02}:{m:02}")
}

fn iso_ms(s: &str) -> Option<u64> {
    // "2026-11-19T12:20:00.253518+00:00" / "+0800" / "...Z" -> epoch ms (UTC; offset honoured)
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let b = s.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let n = |a: usize, e: usize| s.get(a..e)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (n(0, 4)?, n(5, 7)?, n(8, 10)?, n(11, 13)?, n(14, 16)?, n(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || !(0..=23).contains(&h) || !(0..=59).contains(&mi) || !(0..=60).contains(&se) {
        return None;
    }
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
    let tz = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit()).trim();
    if let Some(sign) = tz.chars().next().filter(|c| *c == '+' || *c == '-') {
        let digits = &tz[1..];
        let oh = digits.get(0..2).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
        let om = if digits.as_bytes().get(2) == Some(&b':') {
            digits.get(3..5).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0)
        } else {
            digits.get(2..4).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0)
        };
        let off = oh * 3600 + om * 60;
        secs += if sign == '+' { -off } else { off };
    }
    (secs > 0).then_some(secs as u64 * 1000)
}

/// epoch seconds / ms number or ISO string -> epoch ms
fn time_ms(v: &Value) -> Option<u64> {
    let from_num = |x: f64| {
        if !x.is_finite() || x <= 0.0 {
            None
        } else if x > 1e12 {
            Some(x as u64)
        } else {
            Some((x * 1000.0) as u64)
        }
    };
    match v {
        Value::Number(n) => n.as_f64().and_then(from_num),
        Value::String(s) => {
            let s = s.trim();
            iso_ms(s).or_else(|| s.parse::<f64>().ok().and_then(from_num))
        }
        _ => None,
    }
}

fn window_label(secs: f64) -> String {
    let h = secs / 3600.0;
    if (h - 5.0).abs() < 0.5 {
        crate::i18n::t("5 小时").into()
    } else if (h - 24.0 * 7.0).abs() < 2.0 {
        crate::i18n::t("每周").into()
    } else if h >= 24.0 * 27.0 {
        crate::i18n::t("每月").into()
    } else if h >= 24.0 {
        crate::tr!("{0} 天", (h / 24.0).round())
    } else {
        crate::tr!("{0} 小时", h.round())
    }
}

/// Retry-After of a response in seconds (capped at an hour), when the service sends one: either a
/// number of seconds or an HTTP date ("Wed, 21 Oct 2015 07:28:00 GMT").
fn retry_after(r: &reqwest::Response) -> Option<u64> {
    let s = r.headers().get("retry-after")?.to_str().ok()?.trim().to_string();
    retry_after_secs(&s, crate::now_ms()).map(|x| x.min(3600))
}

fn retry_after_secs(s: &str, now_ms: u64) -> Option<u64> {
    if let Ok(n) = s.parse::<u64>() {
        return Some(n);
    }
    let p: Vec<&str> = s.split_whitespace().collect();
    if p.len() < 5 {
        return None;
    }
    const MON: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mo = MON.iter().position(|m| m.eq_ignore_ascii_case(p[2]))? + 1;
    let d: u32 = p[1].parse().ok()?;
    let at = iso_ms(&format!("{}-{:02}-{:02}T{}Z", p[3], mo, d, p[4]))?;
    Some(at.saturating_sub(now_ms) / 1000)
}

/// 429 comes back as "429:<retry-after seconds or 0>" so query can back off.
fn http_err(tool: &str, status: reqwest::StatusCode, body: &str, retry: Option<u64>) -> String {
    if status.as_u16() == 429 {
        return format!("429:{}", retry.unwrap_or(0));
    }
    match status.as_u16() {
        401 | 403 => crate::tr!("{0} 的登录已失效，打开 {1} 用一下就会自动续期", tool, tool),
        _ => crate::tr!("{0} 返回 {1}：{2}", tool, status.as_u16(), body.chars().take(80).collect::<String>()),
    }
}

fn mark_sent(tool: &str) {
    last_sent().lock().unwrap().insert(tool.to_string(), crate::now_ms());
}

// ---------- Claude Code ----------

async fn claude(proxy: Option<u16>) -> Result<Quota, String> {
    let dir = std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".claude"));
    let text = std::fs::read_to_string(dir.join(".credentials.json"))
        .map_err(|_| format!("absent:{}", crate::i18n::t("没找到 Claude Code 的登录（~/.claude/.credentials.json）")))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let o = v
        .get("claudeAiOauth")
        .ok_or_else(|| format!("absent:{}", crate::i18n::t("Claude Code 没有用订阅账号登录")))?;
    let token = o
        .get("accessToken")
        .and_then(|t| t.as_str())
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| format!("absent:{}", crate::i18n::t("Claude Code 没有用订阅账号登录")))?;
    if o.get("expiresAt").and_then(time_ms).map(|e| e < crate::now_ms()).unwrap_or(false) {
        return Err(format!("soon:{}", crate::i18n::t("Claude Code 的登录已过期，打开 Claude Code 用一下就会自动续期")));
    }
    let plan = o.get("subscriptionType").and_then(|s| s.as_str()).map(|s| {
        let mut c = s.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    });
    let c = client(proxy)?;
    mark_sent("claude");
    let r = c
        .get("https://api.anthropic.com/api/oauth/usage")
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("anthropic-version", "2023-06-01")
        .header("User-Agent", "MicaBar")
        .send()
        .await
        .map_err(|e| format!("soon:{}", crate::tr!("连不上 Anthropic：{0}", err_chain(&e))))?;
    let status = r.status();
    let retry = retry_after(&r);
    let body = r.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(http_err("Claude Code", status, &body, retry));
    }
    let u: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let mut windows = Vec::new();
    for (key, label) in [
        ("five_hour", crate::i18n::t("5 小时")),
        ("seven_day", crate::i18n::t("每周")),
        ("seven_day_opus", crate::i18n::t("每周 · Opus")),
        ("seven_day_sonnet", crate::i18n::t("每周 · Sonnet")),
    ] {
        if let Some(w) = u.get(key).filter(|w| w.is_object()) {
            if let Some(used) = w.get("utilization").and_then(|x| x.as_f64()) {
                windows.push(Window {
                    label,
                    used,
                    resets_at: w.get("resets_at").and_then(time_ms),
                    group: None,
                });
            }
        }
    }
    if windows.is_empty() {
        return Err(crate::tr!(
            "{0} 返回 {1}：{2}",
            "Claude Code",
            status.as_u16(),
            body.chars().take(80).collect::<String>()
        ));
    }
    Ok(Quota {
        plan,
        windows,
        source: Some(crate::i18n::t("Anthropic 账户用量")),
        ..Default::default()
    })
}

// ---------- Codex ----------

fn codex_home() -> PathBuf {
    std::env::var("CODEX_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".codex"))
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
        .or_else(|| {
            w.get("reset_after_seconds")
                .or_else(|| w.get("resets_in_seconds"))
                .and_then(|x| x.as_f64())
                .filter(|&s| s.is_finite() && s > 0.0)
                .map(|s| now.saturating_add((s * 1000.0) as u64))
        });
    Some(Window {
        label: if secs > 0.0 { window_label(secs) } else { crate::i18n::t("额度") },
        used,
        resets_at,
        group: None,
    })
}

/// Newest non-expired rate-limit snapshot Codex wrote into its session logs (offline fallback).
async fn codex_from_sessions() -> Option<(Vec<Window>, u64)> {
    tokio::task::spawn_blocking(codex_from_sessions_sync).await.ok().flatten()
}

fn codex_from_sessions_sync() -> Option<(Vec<Window>, u64)> {
    let now = crate::now_ms();
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let mut stack = vec![codex_home().join("sessions")];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file() && p.extension().map(|x| x.eq_ignore_ascii_case("jsonl")).unwrap_or(false) {
                if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                    files.push((m, p));
                }
            }
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (mtime, p) in files.into_iter().take(8) {
        let Ok(mut f) = std::fs::File::open(&p) else { continue };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let truncated = if len > 262_144 {
            let _ = f.seek(SeekFrom::End(-262_144));
            true
        } else {
            false
        };
        let mut raw = Vec::with_capacity(len.min(262_144) as usize);
        if f.read_to_end(&mut raw).is_err() {
            continue;
        }
        let text = String::from_utf8_lossy(&raw);
        let slice = if truncated {
            match text.find('\n') {
                Some(i) => &text[i + 1..],
                None => &text[..],
            }
        } else {
            &text[..]
        };
        let file_at = mtime
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        for line in slice.lines().rev().filter(|l| l.contains("\"rate_limits\"")) {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let Some(rl) = find_key(&v, "rate_limits") else { continue };
            let at = v
                .get("timestamp")
                .or_else(|| find_key(&v, "timestamp"))
                .and_then(time_ms)
                .unwrap_or(file_at);
            let ws: Vec<Window> = ["primary", "secondary"]
                .iter()
                .filter_map(|k| rl.get(*k).and_then(|w| codex_window(w, at)))
                .collect();
            if !ws.is_empty() && ws.iter().any(|w| w.resets_at.map(|r| r > now).unwrap_or(false)) {
                return Some((ws, at));
            }
        }
    }
    None
}

fn strip_toml_comment(line: &str) -> &str {
    let mut in_double = false;
    let mut in_single = false;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if in_double {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            }
        } else if in_single {
            if ch == '\'' {
                in_single = false;
            }
        } else {
            match ch {
                '"' => in_double = true,
                '\'' => in_single = true,
                '#' => return &line[..idx],
                _ => {}
            }
        }
    }
    line
}

fn parse_toml_top_key(cfg: &str, want_key: &str) -> Option<String> {
    let cfg = cfg.trim_start_matches('\u{feff}');
    for raw_line in cfg.lines() {
        let l = strip_toml_comment(raw_line).trim();
        if l.is_empty() {
            continue;
        }
        if l.starts_with('[') {
            break;
        }
        let Some((k, v)) = l.split_once('=') else { continue };
        let k = k.trim().trim_matches('"').trim_matches('\'');
        if k != want_key {
            continue;
        }
        let v = v.trim();
        let unquoted = if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
            || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
        {
            &v[1..v.len() - 1]
        } else {
            v
        };
        return Some(unquoted.trim().to_string());
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
    let home_dir = codex_home();
    let auth = std::fs::read_to_string(home_dir.join("auth.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let tokens = auth.as_ref().and_then(|a| a.get("tokens")).filter(|t| {
        t.get("access_token")
            .and_then(|x| x.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    });
    if let Some(t) = tokens {
        let token = t.get("access_token").and_then(|x| x.as_str()).unwrap_or_default();
        let c = client(proxy)?;
        let mut req = c
            .get("https://chatgpt.com/backend-api/wham/usage")
            .bearer_auth(token)
            .header("User-Agent", "codex_cli_rs")
            .header("Accept", "application/json");
        if let Some(acc) = t.get("account_id").and_then(|x| x.as_str()) {
            req = req.header("ChatGPT-Account-Id", acc);
        }
        mark_sent("codex");
        match req.send().await {
            Ok(r) => {
                let status = r.status();
                let retry = retry_after(&r);
                let body = r.text().await.unwrap_or_default();
                if status.is_success() {
                    let u: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
                    let now = crate::now_ms();
                    let rl = u.get("rate_limit").cloned().unwrap_or(Value::Null);
                    let windows: Vec<Window> = ["primary_window", "secondary_window"]
                        .iter()
                        .filter_map(|k| rl.get(*k).and_then(|w| codex_window(w, now)))
                        .collect();
                    let plan = u.get("plan_type").and_then(|p| p.as_str()).map(|p| p.to_string());
                    return Ok(Quota {
                        plan,
                        windows,
                        source: Some(crate::i18n::t("ChatGPT 账户用量")),
                        ..Default::default()
                    });
                }
                // Never mask 429 / 401 / 403; only fall back to recent non-expired session logs on 404
                if status.as_u16() == 404 {
                    if let Some((windows, at)) = codex_from_sessions().await {
                        return Ok(Quota {
                            windows,
                            source: Some(crate::tr!("最近一次 Codex 会话（{0}）", fmt_local_hms(at))),
                            ..Default::default()
                        });
                    }
                }
                return Err(http_err("Codex", status, &body, retry));
            }
            Err(e) => {
                if let Some((windows, at)) = codex_from_sessions().await {
                    return Ok(Quota {
                        windows,
                        source: Some(crate::tr!("最近一次 Codex 会话（{0}）", fmt_local_hms(at))),
                        ..Default::default()
                    });
                }
                return Err(format!("soon:{}", crate::tr!("连不上 chatgpt.com：{0}", err_chain(&e))));
            }
        }
    }
    if let Some((windows, at)) = codex_from_sessions().await {
        return Ok(Quota {
            windows,
            source: Some(crate::tr!("最近一次 Codex 会话（{0}）", fmt_local_hms(at))),
            ..Default::default()
        });
    }
    // a custom model provider (API key / relay) has no subscription quota
    let cfg = std::fs::read_to_string(home_dir.join("config.toml")).unwrap_or_default();
    let provider = parse_toml_top_key(&cfg, "model_provider");
    match provider {
        Some(p) if !p.is_empty() && p != "openai" => {
            Err(format!("absent:{}", crate::i18n::t("Codex 用的是自定义服务商（按 API 计费），没有订阅额度")))
        }
        _ if home_dir.exists() => Err(format!("absent:{}", crate::i18n::t("Codex 没有用 ChatGPT 账号登录（没有 auth.json）"))),
        _ => Err(format!("absent:{}", crate::i18n::t("没找到 Codex"))),
    }
}

// ---------- Antigravity ----------

fn command_line(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *const u16,
    }
    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryInformationProcess(
            h: *mut core::ffi::c_void,
            class: u32,
            buf: *mut core::ffi::c_void,
            len: u32,
            ret: *mut u32,
        ) -> i32;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut words = vec![0u64; 8 * 1024]; // 64 KB, 8-byte aligned
        let cap = words.len() * std::mem::size_of::<u64>();
        let mut ret = 0u32;
        // ProcessCommandLineInformation = 60: a UNICODE_STRING followed by its characters
        let mut st = NtQueryInformationProcess(h, 60, words.as_mut_ptr() as _, cap as u32, &mut ret);
        if st < 0 && (ret as usize) > cap && (ret as usize) <= 512 * 1024 {
            words.resize((ret as usize).div_ceil(std::mem::size_of::<u64>()), 0);
            let new_cap = words.len() * std::mem::size_of::<u64>();
            st = NtQueryInformationProcess(h, 60, words.as_mut_ptr() as _, new_cap as u32, &mut ret);
        }
        CloseHandle(h);
        let total = words.len() * std::mem::size_of::<u64>();
        let valid = if ret > 0 { (ret as usize).min(total) } else { total };
        let hdr_size = std::mem::size_of::<UnicodeString>();
        if st < 0 || valid < hdr_size {
            return None;
        }
        let us = std::ptr::read_unaligned(words.as_ptr() as *const UnicodeString);
        let bytes = (us.length as usize) & !1;
        if bytes == 0 || us.buffer.is_null() || (us.buffer as usize) % std::mem::align_of::<u16>() != 0 {
            return None;
        }
        let buf_start = words.as_ptr() as usize;
        let buf_end = buf_start.checked_add(valid)?;
        let ptr_addr = us.buffer as usize;
        let ptr_end = ptr_addr.checked_add(bytes)?;
        if ptr_addr < buf_start + hdr_size || ptr_end > buf_end {
            return None;
        }
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(us.buffer, bytes / 2)))
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

fn is_ag_marker(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.contains("antigravity") || l.contains("gemini")
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
                        out.push(Window {
                            label: label.to_string(),
                            used: ((1.0 - rem) * 100.0).clamp(0.0, 100.0),
                            resets_at: reset,
                            group: None,
                        });
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

/// RetrieveUserQuotaSummary: {response: {groups: [{displayName: "Gemini Models", buckets: [{window:
/// "5h" | "weekly", remainingFraction, resetTime}]}]}} -> "Gemini · 5 小时", "Gemini · 每周", …
fn ag_summary(v: &Value) -> Vec<Window> {
    let mut out = Vec::new();
    let Some(groups) = find_key(v, "groups").and_then(|g| g.as_array()) else { return out };
    for g in groups {
        let name = g
            .get("displayName")
            .and_then(|x| x.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| crate::i18n::t("模型"));
        let short = match name.trim_end_matches(" models").trim_end_matches(" Models") {
            "Claude and GPT" => "Claude / GPT".to_string(),
            s => s.to_string(),
        };
        let mut buckets: Vec<&Value> = g
            .get("buckets")
            .and_then(|b| b.as_array())
            .map(|b| b.iter().collect())
            .unwrap_or_default();
        // 5-hour first, then weekly
        buckets.sort_by_key(|b| if b.get("window").and_then(|w| w.as_str()) == Some("5h") { 0 } else { 1 });
        for b in buckets {
            let Some(rem) = b.get("remainingFraction").and_then(|x| x.as_f64()) else { continue };
            let kind = match b.get("window").and_then(|w| w.as_str()) {
                Some("5h") => crate::i18n::t("5 小时"),
                Some("weekly") => crate::i18n::t("每周"),
                Some(o) => o.to_string(),
                None => b
                    .get("displayName")
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| crate::i18n::t("额度")),
            };
            out.push(Window {
                label: format!("{short} · {kind}"),
                used: ((1.0 - rem) * 100.0).clamp(0.0, 100.0),
                resets_at: b.get("resetTime").and_then(time_ms),
                group: Some(short.clone()),
            });
        }
    }
    out
}

/// Newer Antigravity (2.3x) starts one standalone "hub" language server that reads its token from
/// stdin, so the command line has no --csrf_token. It announces itself for local clients in
/// ~/.gemini/antigravity/daemon/hub_<port>.json ({pid, httpsPort, httpPort, csrfToken}); files of
/// servers that have exited stay behind, so only one whose pid is a running Antigravity server counts.
fn ag_discovery(running: &[u32]) -> Option<(u32, String, Vec<(&'static str, u16)>)> {
    let dir = home().join(".gemini").join("antigravity").join("daemon");
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    files.into_iter().find_map(|(_, p)| {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
        let pid = u32::try_from(v.get("pid")?.as_u64()?).ok()?;
        if !running.contains(&pid) {
            return None;
        }
        let csrf = v.get("csrfToken")?.as_str()?.trim().to_string();
        if csrf.is_empty() {
            return None;
        }
        let mut endpoints: Vec<(&'static str, u16)> = Vec::new();
        for (key, scheme) in [("httpsPort", "https"), ("httpPort", "http")] {
            if let Some(port) = v
                .get(key)
                .and_then(|x| x.as_u64())
                .and_then(|p| u16::try_from(p).ok())
                .filter(|&p| p >= 1)
            {
                if !endpoints.contains(&(scheme, port)) {
                    endpoints.push((scheme, port));
                }
            }
        }
        (!endpoints.is_empty()).then_some((pid, csrf, endpoints))
    })
}

fn ag_extract_plan(v: &Value) -> Option<String> {
    find_key(v, "planName")
        .or_else(|| v.pointer("/userStatus/userTier/name"))
        .and_then(|p| p.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

async fn antigravity() -> Result<Quota, String> {
    static AG_PLAN: OnceLock<Mutex<Option<(u32, u64, String)>>> = OnceLock::new();
    // Filter language_server* processes to genuine Antigravity / Gemini binaries (never Windsurf/Codeium)
    let mut ag_procs: Vec<(u32, Option<String>)> = Vec::new();
    for (p, n) in crate::procinfo::list_processes() {
        if !n.to_ascii_lowercase().starts_with("language_server") {
            continue;
        }
        let path_ok = crate::procinfo::image_path(p).as_deref().map(is_ag_marker).unwrap_or(false);
        let cmd = command_line(p);
        let cmd_ok = cmd.as_deref().map(is_ag_marker).unwrap_or(false);
        if path_ok || cmd_ok {
            ag_procs.push((p, cmd));
        }
    }
    let running_pids: Vec<u32> = ag_procs.iter().map(|(p, _)| *p).collect();
    let installed = std::env::var("LOCALAPPDATA")
        .map(|l| PathBuf::from(l).join("Programs").join("Antigravity").exists())
        .unwrap_or(false)
        || home().join(".gemini").join("antigravity").exists();
    // the hub's discovery file first; older versions: a workspace server with --csrf_token
    let (pid, csrf, mut endpoints) = match ag_discovery(&running_pids) {
        Some(found) => found,
        None => {
            let server = ag_procs.iter().find_map(|(p, cmd)| {
                let csrf = arg_value(cmd.as_deref()?, "--csrf_token")?;
                Some((*p, csrf))
            });
            let Some((pid, csrf)) = server else {
                return Err(if installed || !running_pids.is_empty() {
                    format!("absent:{}", crate::i18n::t("Antigravity 没在运行（打开它后才能读到额度）"))
                } else {
                    format!("absent:{}", crate::i18n::t("没找到 Antigravity"))
                });
            };
            (pid, csrf, Vec::new())
        }
    };
    // any other port the server listens on, after the announced ones
    for s in crate::conn::snapshot() {
        if s.pid == pid
            && s.local_port >= 1
            && (s.remote.is_none() || s.state == "LISTEN")
            && matches!(s.proto, crate::conn::Proto::Tcp)
            && !endpoints.iter().any(|&(_, p)| p == s.local_port)
        {
            endpoints.push(("https", s.local_port));
            endpoints.push(("http", s.local_port));
        }
    }
    let c = local_client()?;
    let locale = crate::i18n::lang();
    let body = serde_json::json!({
        "metadata": {
            "ideName": "antigravity",
            "extensionName": "antigravity",
            "locale": locale,
        }
    });
    if !endpoints.is_empty() {
        mark_sent("antigravity");
    }
    let now = crate::now_ms();
    let cached_plan = AG_PLAN
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .clone()
        .filter(|(p, at, _)| *p == pid && now.saturating_sub(*at) < 3_600_000)
        .map(|(_, _, plan)| plan);
    let mut last_err = crate::i18n::t("语言服务器没有响应");
    for (scheme, port) in endpoints {
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
                last_err = crate::tr!("语言服务器返回 {0}", r.status().as_u16());
                continue;
            }
            let Ok(v) = r.json::<Value>().await else { continue };
            let mut windows = ag_summary(&v);
            if windows.is_empty() {
                ag_windows(&v, &mut windows);
            }
            if windows.is_empty() {
                continue;
            }
            let mut plan = ag_extract_plan(&v).or_else(|| cached_plan.clone());
            // RetrieveUserQuotaSummary omits planName: fetch GetUserStatus once per hour on the working endpoint
            if plan.is_none() && method == "RetrieveUserQuotaSummary" {
                let status_url = format!("{scheme}://127.0.0.1:{port}/exa.language_server_pb.LanguageServerService/GetUserStatus");
                if let Ok(sr) = c
                    .post(&status_url)
                    .header("X-Codeium-Csrf-Token", &csrf)
                    .header("Connect-Protocol-Version", "1")
                    .json(&body)
                    .send()
                    .await
                {
                    if sr.status().is_success() {
                        if let Ok(sv) = sr.json::<Value>().await {
                            plan = ag_extract_plan(&sv);
                        }
                    }
                }
            }
            if let Some(ref p) = plan {
                *AG_PLAN.get_or_init(Default::default).lock().unwrap() = Some((pid, now, p.clone()));
            } else {
                plan = known_quota("antigravity").and_then(|q| q.plan);
            }
            return Ok(Quota {
                plan,
                windows,
                source: Some(crate::i18n::t("Antigravity 本机语言服务器")),
                ..Default::default()
            });
        }
    }
    Err(last_err)
}

// ---------- Grok ----------

fn fnv1a_str(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in s.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

async fn grok(proxy: Option<u16>) -> Result<Quota, String> {
    static GROK_PLAN: OnceLock<Mutex<Option<(u64, u64, String)>>> = OnceLock::new();
    let dir = std::env::var("GROK_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".grok"));
    let text = std::fs::read_to_string(dir.join("auth.json"))
        .map_err(|_| format!("absent:{}", crate::i18n::t("没找到 Grok 的登录（需要先用 grok CLI 登录一次）")))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    // one entry per sign-in scope; take the first with a non-empty bearer key that hasn't expired
    let now = crate::now_ms();
    let mut entries: Vec<&Value> = match &v {
        Value::Object(m) if m.contains_key("key") => vec![&v],
        Value::Object(m) => m.values().collect(),
        Value::Array(a) => a.iter().collect(),
        _ => vec![],
    };
    entries.retain(|e| {
        e.get("key")
            .and_then(|k| k.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    });
    if entries.is_empty() {
        return Err(format!("absent:{}", crate::i18n::t("没找到 Grok 的登录（需要先用 grok CLI 登录一次）")));
    }
    let entry = entries
        .iter()
        .find(|e| e.get("expires_at").and_then(time_ms).map(|t| t > now).unwrap_or(true))
        .ok_or_else(|| format!("soon:{}", crate::i18n::t("Grok 的登录已过期，运行一次 grok 就会自动续期")))?;
    let key = entry.get("key").and_then(|k| k.as_str()).unwrap_or_default().trim();
    let c = client(proxy)?;
    let get = |path: &str| {
        c.get(format!("https://cli-chat-proxy.grok.com{path}"))
            .bearer_auth(key)
            .header("x-xai-token-auth", "xai-grok-cli")
            .header("Accept", "application/json")
            .send()
    };
    mark_sent("grok");
    let r = get("/v1/billing?format=credits")
        .await
        .map_err(|e| format!("soon:{}", crate::tr!("连不上 grok.com：{0}", err_chain(&e))))?;
    let status = r.status();
    let retry = retry_after(&r);
    let body = r.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(http_err("Grok", status, &body, retry));
    }
    let b: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let cfg = b.get("config").unwrap_or(&b);
    let end = cfg
        .pointer("/currentPeriod/end")
        .or_else(|| cfg.get("billingPeriodEnd"))
        .and_then(time_ms);
    let mut windows = Vec::new();
    if let Some(p) = cfg.get("creditUsagePercent").and_then(|x| x.as_f64()) {
        windows.push(Window {
            label: crate::i18n::t("本期额度"),
            used: p,
            resets_at: end,
            group: None,
        });
    }
    if let Some(list) = cfg.get("productUsage").and_then(|x| x.as_array()) {
        for p in list {
            if let (Some(name), Some(u)) = (
                p.get("product").and_then(|x| x.as_str()),
                p.get("usagePercent").and_then(|x| x.as_f64()),
            ) {
                windows.push(Window {
                    label: name.to_string(),
                    used: u,
                    resets_at: end,
                    group: None,
                });
            }
        }
    }
    if windows.is_empty() {
        return Err(crate::i18n::t("Grok 没返回额度信息"));
    }
    let key_hash = fnv1a_str(key);
    let cached = GROK_PLAN.get_or_init(Default::default).lock().unwrap().clone();
    let plan = match &cached {
        Some((kh, at, p)) if *kh == key_hash && now.saturating_sub(*at) < 3_600_000 => Some(p.clone()),
        _ => {
            let fetched = match get("/v1/settings").await {
                Ok(r) if r.status().is_success() => r
                    .json::<Value>()
                    .await
                    .ok()
                    .and_then(|s| s.get("subscription_tier_display").and_then(|x| x.as_str()).map(|x| x.to_string())),
                _ => None,
            };
            if let Some(ref p) = fetched {
                *GROK_PLAN.get_or_init(Default::default).lock().unwrap() = Some((key_hash, now, p.clone()));
            }
            fetched.or_else(|| cached.filter(|(kh, _, _)| *kh == key_hash).map(|(_, _, p)| p))
        }
    };
    Ok(Quota {
        plan,
        windows,
        source: Some(crate::i18n::t("Grok 账户用量")),
        ..Default::default()
    })
}

type Map<T> = Mutex<HashMap<String, T>>;

/// Per tool: (429s in a row, no request before this epoch ms).
fn backoff() -> &'static Map<(u32, u64)> {
    static B: OnceLock<Map<(u32, u64)>> = OnceLock::new();
    B.get_or_init(Default::default)
}

/// Per tool: epoch ms of the last request actually sent to its service.
fn last_sent() -> &'static Map<u64> {
    static L: OnceLock<Map<u64>> = OnceLock::new();
    L.get_or_init(Default::default)
}

/// Per tool: ask again at this epoch ms when the last try never reached the service or lacked
/// local sign-in — never allowed to bypass `last_sent + floor_ms` or `backoff`.
fn retry_at() -> &'static Map<u64> {
    static R: OnceLock<Map<u64>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// Last result per tool, also for tools switched off for a while, so switching one back on (or
/// the whole AI section) shows its numbers again instead of asking the service at once.
fn known() -> &'static Map<Quota> {
    static K: OnceLock<Map<Quota>> = OnceLock::new();
    K.get_or_init(Default::default)
}

pub fn known_quota(tool: &str) -> Option<Quota> {
    known().lock().unwrap().get(tool).cloned()
}

/// Shortest gap between two queries of one tool. Anthropic's usage endpoint answers 429 within
/// minutes (seen 2026-11-20: a manual refresh 5 minutes after a good query), and the limit may be
/// shared with Claude Code itself, so Claude waits at least 10 minutes even for a manual refresh.
pub fn floor_ms(tool: &str, manual: bool) -> u64 {
    match (tool, manual) {
        ("claude", _) => 10 * 60_000,
        (_, true) => 60_000,
        _ => 60_000,
    }
}

/// When `tool` may be asked next (epoch ms; 0 = now).
pub fn next_at(tool: &str, every_ms: u64, manual: bool) -> u64 {
    let backoff_until = backoff().lock().unwrap().get(tool).map(|b| b.1).unwrap_or(0);
    let last = last_sent().lock().unwrap().get(tool).copied();
    let floor_until = last.map(|l| l.saturating_add(floor_ms(tool, manual))).unwrap_or(0);
    if let Some(r) = retry_at().lock().unwrap().get(tool).copied() {
        let retry_until = if manual { 0 } else { r };
        return floor_until.max(backoff_until).max(retry_until);
    }
    let interval_until = match last {
        Some(l) if manual => l.saturating_add(floor_ms(tool, true)),
        Some(l) => l.saturating_add(every_ms.max(floor_ms(tool, false))),
        None => 0,
    };
    interval_until.max(backoff_until)
}

/// Whether `tool` should be asked again now.
pub fn due(tool: &str, every_ms: u64, manual: bool) -> bool {
    crate::now_ms() >= next_at(tool, every_ms, manual)
}

/// Results, backoff and request times survive restarts (an update or a restart used to fire a
/// fresh round at once, which is what ran into Anthropic's 429). Only percentages, plan names and
/// times are kept here, never credentials.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Cache {
    quotas: Vec<Quota>,
    backoff: HashMap<String, (u32, u64)>,
    last_sent: HashMap<String, u64>,
}

fn cache_path() -> PathBuf {
    crate::log_path().with_file_name("ai-cache.json")
}

pub fn load_cache() -> Vec<Quota> {
    let Some(c) = std::fs::read_to_string(cache_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Cache>(&t).ok())
    else {
        return Vec::new();
    };
    *backoff().lock().unwrap() = c.backoff;
    *last_sent().lock().unwrap() = c.last_sent;
    *known().lock().unwrap() = c.quotas.iter().map(|q| (q.tool.clone(), q.clone())).collect();
    c.quotas
}

pub fn save_cache() {
    let mut quotas: Vec<Quota> = known().lock().unwrap().values().cloned().collect();
    quotas.sort_by_key(|q| TOOLS.iter().position(|(t, _)| *t == q.tool).unwrap_or(99));
    let backoff_map = backoff().lock().unwrap().clone();
    let sent_map = last_sent().lock().unwrap().clone();
    let c = Cache {
        quotas,
        backoff: backoff_map,
        last_sent: sent_map,
    };
    if let Ok(t) = serde_json::to_string(&c) {
        let _ = crate::settings::atomic_write(&cache_path(), t.as_bytes());
    }
}

/// Queries one tool. `prev` keeps the last good numbers when a query fails.
pub async fn query(tool: &str, proxy: Option<u16>, prev: Option<&Quota>) -> Quota {
    let name = TOOLS
        .iter()
        .find(|(t, _)| *t == tool)
        .map(|(_, n)| *n)
        .unwrap_or(tool)
        .to_string();
    let at = crate::now_ms();
    // backing off after a 429: no request until the pause is over
    let until = backoff().lock().unwrap().get(tool).map(|b| b.1).unwrap_or(0);
    retry_at().lock().unwrap().remove(tool);
    let r = if at < until {
        Err(crate::tr!("官方接口限流，{0} 后再查", fmt_local_hm(until)))
    } else {
        // Each provider only records `last_sent` right before dispatching a real HTTP request
        match tool {
            "claude" => claude(proxy).await,
            "codex" => codex(proxy).await,
            "antigravity" => antigravity().await,
            "grok" => grok(proxy).await,
            _ => Err(format!("absent:{}", crate::i18n::t("未知工具"))),
        }
    };
    // 429 -> pause: the service's Retry-After, else 10, 20, 40 then 60 minutes
    let r = match r {
        Err(e) if e.starts_with("429:") => {
            let retry: u64 = e[4..].parse().unwrap_or(0);
            let until_ms = {
                let mut map = backoff().lock().unwrap();
                let entry = map.entry(tool.to_string()).or_insert((0, 0));
                entry.0 = entry.0.saturating_add(1);
                let step = (10u64 << (entry.0 - 1).min(3)).min(60) * 60_000;
                entry.1 = at.saturating_add(step.max(retry.saturating_mul(1000)));
                entry.1
            };
            crate::log(format!("ai {tool}: 429, next query after {}", fmt_local_hms(until_ms)));
            Err(crate::tr!("官方接口限流，{0} 后再查", fmt_local_hm(until_ms)))
        }
        Ok(q) => {
            backoff().lock().unwrap().remove(tool);
            Ok(q)
        }
        other => other,
    };
    // tries that never reached the service (no connection yet, sign-in expired, tool not running):
    // look again in a minute (still bounded by floor_ms & backoff in next_at)
    if let Err(e) = &r {
        if e.starts_with("soon:") || e.starts_with("absent:") {
            retry_at().lock().unwrap().insert(tool.to_string(), at.saturating_add(60_000));
        }
    }
    let r = r.map_err(|e| {
        if e == NO_PROXY {
            no_proxy_text()
        } else {
            e.strip_prefix("soon:").map(str::to_string).unwrap_or(e)
        }
    });
    let q = match r {
        Ok(mut q) => {
            q.tool = tool.into();
            q.name = name;
            q.status = "ok".into();
            q.at = at;
            q.via = if tool == "antigravity" { None } else { proxy };
            q
        }
        Err(e) if e.starts_with("absent:") => Quota {
            tool: tool.into(),
            name,
            status: "absent".into(),
            message: Some(e[7..].to_string()),
            at,
            ..Default::default()
        },
        Err(e) => match prev.filter(|p| !p.windows.is_empty()) {
            Some(p) => Quota {
                status: "stale".into(),
                message: Some(e),
                ..p.clone()
            },
            None => Quota {
                tool: tool.into(),
                name,
                status: "error".into(),
                message: Some(e),
                at,
                ..Default::default()
            },
        },
    };
    known().lock().unwrap().insert(tool.to_string(), q.clone());
    q
}

/// "error sending request" alone says nothing: add the causes (connect refused, TLS, proxy…).
pub fn err_chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(x) = src {
        let t = x.to_string();
        if !s.contains(&t) {
            s.push_str(&crate::i18n::t("："));
            s.push_str(&t);
        }
        src = x.source();
    }
    s
}
