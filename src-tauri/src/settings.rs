//! User settings: one JSON document in %LOCALAPPDATA%\MicaBar\settings.json.
//! The page edits it by sending partial patches; missing keys fall back to the defaults, so new
//! settings added in later versions appear with their default values.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

fn path() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    let d = PathBuf::from(base).join("MicaBar");
    let _ = std::fs::create_dir_all(&d);
    d.join("settings.json")
}

pub fn defaults() -> Value {
    json!({
        "general": {
            "autostart": false,
            "autoUpdate": true,
            // saver | balanced | smooth
            "profile": "balanced",
            // primary | cursor | <monitor name>
            "monitor": "primary"
        },
        "interaction": {
            // hover | click
            "expand": "hover",
            "hoverDelay": 300,
            // mouse never opens the island; only the hotkey does, clicks pass through
            "passthrough": false,
            // hover mode: the collapsed pill turns see-through while the cursor rests on it
            "peek": true,
            "hotkey": "Ctrl+Alt+N",
            // fullscreen app in front: pill | hide
            "fullscreen": "pill"
        },
        "shape": {
            // capsule | notch | line
            "style": "capsule",
            // top | bottom | left | right
            "dock": "top",
            "lineWhenMaximized": false
        },
        "pill": {
            "items": [
                { "id": "rotate", "on": true },
                { "id": "exit", "on": false },
                { "id": "conns", "on": true },
                { "id": "speed", "on": false },
                { "id": "latency", "on": false },
                { "id": "leaks", "on": true },
                { "id": "cpu", "on": false },
                { "id": "mem", "on": false },
                { "id": "clock", "on": false }
            ],
            // simple | digital | serif
            "clockStyle": "simple",
            "clockSeconds": false
        },
        "size": {
            // collapsed pill: height, and width (0 = fit the content, up to pillMaxW)
            "pillH": 36,
            "pillW": 0,
            "pillMaxW": 520,
            // expanded panel
            "panelW": 600,
            "panelH": 540
        },
        "rotate": {
            // the pill's rotating slot: cards shown one after another
            "interval": 5,
            "items": [
                { "id": "exits", "on": true },
                { "id": "media", "on": true },
                { "id": "ai", "on": true },
                { "id": "speed", "on": false },
                { "id": "latency", "on": false }
            ]
        },
        "ai": {
            "on": true,
            // minutes between queries
            "interval": 5,
            "claude": true,
            "codex": true,
            "antigravity": true,
            "grok": true
        },
        "media": {
            "on": true,
            "cover": true
        },
        "exit": {
            // auto = the fallback (MATCH) group + the busiest proxy groups; manual = `groups`
            "mode": "auto",
            "groups": [],
            "max": 4
        },
        "flash": {
            "exitChange": true,
            "nodeDown": true,
            "latencyHigh": true,
            "latencyMs": 800,
            "speedSpike": true,
            "speedMBps": 5
        },
        "panel": {
            "tabs": [
                { "id": "conns", "on": true },
                { "id": "listen", "on": true },
                { "id": "rules", "on": true },
                { "id": "guard", "on": true },
                { "id": "speed", "on": true },
                { "id": "traffic", "on": true },
                { "id": "nodes", "on": true }
            ]
        },
        "appearance": {
            // auto | glass-dark | glass-light | <preset id> | custom
            "theme": "auto",
            // live blurred backdrop; off = solid background (and no screen capture)
            "glass": true,
            // 20..100, 50 = default look
            "opacity": 50,
            "font": "",
            // percent
            "scale": 100,
            "custom": { "base": "dark", "bg": "#16161c", "text": "#f2f2f5", "accent": "#60a5fa" }
        }
    })
}

/// Recursively fills keys missing in `v` from `d` (arrays are taken as a whole from `v`, except
/// ordered `{id, on}` lists: ids added in a later version are appended, switched off).
fn fill(v: &mut Value, d: &Value) {
    if let (Some(va), Some(da)) = (v.as_array_mut(), d.as_array()) {
        if da.iter().all(|x| x.get("id").is_some()) {
            for x in da {
                let id = x.get("id");
                if !va.iter().any(|y| y.get("id") == id) {
                    let mut add = x.clone();
                    add["on"] = Value::Bool(false);
                    va.push(add);
                }
            }
        }
        return;
    }
    if let (Some(vo), Some(dobj)) = (v.as_object_mut(), d.as_object()) {
        for (k, dv) in dobj {
            match vo.get_mut(k) {
                Some(x) => fill(x, dv),
                None => {
                    vo.insert(k.clone(), dv.clone());
                }
            }
        }
    }
}

/// Deep-merges `patch` into `v` (objects merge, everything else replaces).
fn merge(v: &mut Value, patch: &Value) {
    match (v.as_object_mut(), patch.as_object()) {
        (Some(vo), Some(po)) => {
            for (k, pv) in po {
                match vo.get_mut(k) {
                    Some(x) if x.is_object() && pv.is_object() => merge(x, pv),
                    _ => {
                        vo.insert(k.clone(), pv.clone());
                    }
                }
            }
        }
        _ => *v = patch.clone(),
    }
}

fn store() -> &'static Mutex<Value> {
    static S: OnceLock<Mutex<Value>> = OnceLock::new();
    S.get_or_init(|| {
        let mut v: Value = std::fs::read_to_string(path())
            .ok()
            .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_else(|| json!({}));
        if !v.is_object() {
            v = json!({});
        }
        fill(&mut v, &defaults());
        Mutex::new(v)
    })
}

pub fn get() -> Value {
    store().lock().unwrap().clone()
}

pub fn update(patch: &Value) -> Value {
    let mut s = store().lock().unwrap();
    merge(&mut s, patch);
    fill(&mut s, &defaults());
    if let Ok(t) = serde_json::to_string_pretty(&*s) {
        let _ = std::fs::write(path(), t);
    }
    s.clone()
}

pub fn reset() -> Value {
    let mut s = store().lock().unwrap();
    *s = defaults();
    if let Ok(t) = serde_json::to_string_pretty(&*s) {
        let _ = std::fs::write(path(), t);
    }
    s.clone()
}

/// Dotted path lookup, e.g. "interaction.expand".
pub fn str_at(path: &str, default: &str) -> String {
    let s = store().lock().unwrap();
    let mut v = &*s;
    for p in path.split('.') {
        match v.get(p) {
            Some(x) => v = x,
            None => return default.to_string(),
        }
    }
    v.as_str().unwrap_or(default).to_string()
}

pub fn bool_at(path: &str, default: bool) -> bool {
    let s = store().lock().unwrap();
    let mut v = &*s;
    for p in path.split('.') {
        match v.get(p) {
            Some(x) => v = x,
            None => return default,
        }
    }
    v.as_bool().unwrap_or(default)
}

pub fn num_at(path: &str, default: f64) -> f64 {
    let s = store().lock().unwrap();
    let mut v = &*s;
    for p in path.split('.') {
        match v.get(p) {
            Some(x) => v = x,
            None => return default,
        }
    }
    v.as_f64().unwrap_or(default)
}

/// Whether an item of an ordered `{id, on}` list is switched on.
pub fn item_on(list_path: &str, id: &str) -> bool {
    let s = store().lock().unwrap();
    let mut v = &*s;
    for p in list_path.split('.') {
        match v.get(p) {
            Some(x) => v = x,
            None => return false,
        }
    }
    v.as_array()
        .map(|a| a.iter().any(|i| i.get("id").and_then(|x| x.as_str()) == Some(id) && i.get("on").and_then(|x| x.as_bool()).unwrap_or(false)))
        .unwrap_or(false)
}
