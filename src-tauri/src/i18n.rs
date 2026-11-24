//! Interface language for the text the backend makes (notes, diagnosis, errors, tray menu).
//! The Chinese text is the key, the translations are the UI's own files ui/i18n/<lang>.js
//! (compiled in), so the island and the backend always agree. setting general.lang:
//! auto (= the Windows display language) | zh-CN | zh-TW | en | ja | ko | ru | es | pt-BR | fr | de

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

pub const LANGS: &[&str] = &["zh-CN", "zh-TW", "en", "ja", "ko", "ru", "es", "pt-BR", "fr", "de"];

fn source(code: &str) -> Option<&'static str> {
    Some(match code {
        "zh-TW" => include_str!("../../ui/i18n/zh-TW.js"),
        "en" => include_str!("../../ui/i18n/en.js"),
        "ja" => include_str!("../../ui/i18n/ja.js"),
        "ko" => include_str!("../../ui/i18n/ko.js"),
        "ru" => include_str!("../../ui/i18n/ru.js"),
        "es" => include_str!("../../ui/i18n/es.js"),
        "pt-BR" => include_str!("../../ui/i18n/pt-BR.js"),
        "fr" => include_str!("../../ui/i18n/fr.js"),
        "de" => include_str!("../../ui/i18n/de.js"),
        _ => return None,
    })
}

struct State {
    lang: String,
    dict: HashMap<String, String>,
}

fn state() -> &'static RwLock<State> {
    static S: OnceLock<RwLock<State>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(State { lang: "zh-CN".into(), dict: HashMap::new() }))
}

/// Strips `//...` and `/*...*/` JS comments while preserving string literals (`"..."` / `'...'`).
fn strip_js_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' | '\'' => {
                let quote = c;
                out.push(quote);
                while let Some(sc) = chars.next() {
                    out.push(sc);
                    if sc == '\\' {
                        if let Some(esc) = chars.next() {
                            out.push(esc);
                        }
                    } else if sc == quote {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                while let Some(lc) = chars.next() {
                    if lc == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                while let Some(bc) = chars.next() {
                    if bc == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        out.push(' ');
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// The file is `I18N.add("en", { ... });`: the JSON object between the outer braces.
fn parse_js_dict(src: &str) -> HashMap<String, String> {
    let cleaned = strip_js_comments(src);
    let (Some(a), Some(b)) = (cleaned.find('{'), cleaned.rfind('}')) else { return HashMap::new() };
    if a > b {
        return HashMap::new();
    }
    serde_json::from_str(&cleaned[a..=b]).unwrap_or_else(|e| {
        crate::log(format!("i18n: bad dictionary: {e}"));
        HashMap::new()
    })
}

/// Windows display language mapped onto LANGS (English when it isn't one of them).
pub fn system_lang() -> String {
    use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
    let id = unsafe { GetUserDefaultUILanguage() } as u32;
    let (primary, sub) = (id & 0x3ff, id >> 10);
    match primary {
        0x04 => if matches!(sub, 0x01 | 0x03 | 0x05) || id == 0x7c04 { "zh-TW" } else { "zh-CN" },
        0x09 => "en",
        0x11 => "ja",
        0x12 => "ko",
        0x19 => "ru",
        0x0a => "es",
        0x16 => "pt-BR",
        0x0c => "fr",
        0x07 => "de",
        _ => "en",
    }
    .to_string()
}

/// The language the setting resolves to.
pub fn wanted() -> String {
    let s = crate::settings::str_at("general.lang", "auto");
    if LANGS.contains(&s.as_str()) { s } else { system_lang() }
}

/// Loads `code` (when it isn't the current one). Returns true when it changed.
pub fn set_lang(code: &str) -> bool {
    let code = if LANGS.contains(&code) { code } else { "zh-CN" };
    if state().read().unwrap().lang == code {
        return false;
    }
    let dict = source(code).map(parse_js_dict).unwrap_or_default();
    crate::log(format!("i18n: {code} ({} entries)", dict.len()));
    let mut s = state().write().unwrap();
    s.lang = code.to_string();
    s.dict = dict;
    true
}

pub fn lang() -> String {
    state().read().unwrap().lang.clone()
}

/// Translation of a Chinese key (the key itself when there is none).
pub fn t(key: &str) -> String {
    let s = state().read().unwrap();
    s.dict.get(key).cloned().unwrap_or_else(|| key.to_string())
}

/// Fills {0}, {1}… with the arguments.
pub fn fill(s: &str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    let mut rest = s;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        match tail.find('}').and_then(|j| tail[..j].parse::<usize>().ok().map(|n| (j, n))) {
            Some((j, n)) if n < args.len() => {
                out.push_str(&args[n].to_string());
                rest = &tail[j + 1..];
            }
            _ => {
                out.push('{');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `tr!("{0} 换到了 {1}", group, node)`: translated, then filled.
#[macro_export]
macro_rules! tr {
    ($k:expr) => {
        $crate::i18n::t($k)
    };
    ($k:expr, $($a:expr),+ $(,)?) => {
        $crate::i18n::fill(&$crate::i18n::t($k), &[$(&$a as &dyn std::fmt::Display),+])
    };
}
