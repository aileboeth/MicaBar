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
            "monitor": "primary",
            // the first-run tour on the island was finished or skipped
            "tourDone": false
        },
        "interaction": {
            // hover | click
            "expand": "hover",
            "hoverDelay": 500,
            // how long a press has to be held before the island is grabbed (fling it along the edge)
            "holdMs": 260,
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
            // where along that edge: start (left / top) | center | end (right / bottom); set by flinging
            "anchor": "center",
            "lineWhenMaximized": false
        },
        "pill": {
            // shown side by side in this order; what doesn't fit the width takes turns in one slot at the end
            "items": [
                { "id": "exits", "on": true },
                { "id": "speed", "on": true },
                { "id": "ai-claude", "on": true },
                { "id": "ai-codex", "on": true },
                { "id": "ai-antigravity", "on": true },
                { "id": "ai-grok", "on": true },
                { "id": "media", "on": true },
                { "id": "leaks", "on": true },
                { "id": "conns", "on": false },
                { "id": "latency", "on": false },
                { "id": "cpu", "on": false },
                { "id": "mem", "on": false },
                { "id": "clock", "on": false },
                { "id": "exit", "on": false },
                { "id": "rotate", "on": false },
                { "id": "notes", "on": true },
                { "id": "clip", "on": true }
            ],
            // simple | digital | serif
            "clockStyle": "simple",
            "clockSeconds": false
        },
        "size": {
            // collapsed pill: height, and width (0 = fit the content, up to pillMaxW)
            "pillH": 44,
            "pillW": 0,
            "pillMaxW": 760,
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
            // running Claude Code sessions in the pill (started by the hooks)
            "live": true,
            // minutes between queries (Claude: at least 10)
            "interval": 10,
            // proxy port the quota queries go through (0 = the proxy client's own port, found
            // automatically); there is no direct fallback
            "proxyPort": 0,
            "claude": true,
            "codex": true,
            "antigravity": true,
            "grok": true,
            // quota rings: which usage window each tool's ring shows (both | first | second | max),
            // colour steps by share used, and the three colours
            "ring": {
                "mode": { "claude": "both", "codex": "both", "antigravity": "both", "grok": "first" },
                "warn": 60,
                "crit": 85,
                "colors": { "ok": "#34d399", "warn": "#fbbf24", "crit": "#f87171" }
            }
        },
        "media": {
            "on": true,
            "cover": true,
            // the song in the pill: viz (cover + bars moving with the music) | title
            "pillStyle": "viz"
        },
        "exit": {
            // exit control (the Clash Party override: MicaBar group, per-group ports, guard rules);
            // false = never offered, the UI hides it (turning it off removes the override first)
            "control": true,
            // auto = the fallback (MATCH) group + the busiest proxy groups; manual = `groups`
            "mode": "auto",
            "groups": [],
            "max": 4,
            // registered organisation + type (home / hosting / mobile / proxy) of each exit IP,
            // looked up through the proxy port (ipapi.is + ip-api.com), cached 24 h per IP
            "ipOrg": true
        },
        // network diagnosis: quiet probes, a full check when they fail, system proxy watch
        "diag": { "on": true },
        // a note when a program reaches the internet for the first time (system programs under
        // the Windows folder only with "system")
        "firstNet": { "on": true, "system": false },
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
            ],
            // pages of the open island, switched by the icons at its top
            "pages": [
                { "id": "net", "on": true },
                { "id": "sys", "on": true },
                { "id": "music", "on": true },
                { "id": "ai", "on": true },
                { "id": "notes", "on": true },
                { "id": "clip", "on": true }
            ]
        },
        "notify": {
            "on": true,
            // sources: Windows toasts (notification database), taskbar flashes (WeChat…)
            "toast": true,
            "flash": true,
            // how a new note shows: icon (small app icon at the pill's right end) | inline (one
            // line in the pill for a few seconds) | card (the pill springs into a card)
            "style": "icon",
            // seconds a note shows (inline / card; longer while the cursor is on a card)
            "duration": 5,
            // source (app + sender, never the message text) | smart (text hidden while fullscreen /
            // recording) | full
            "privacy": "source",
            "followDnd": true,
            "quiet": { "on": false, "from": 23, "to": 8 },
            // local endpoint for the Claude Code / Codex hooks
            "port": 47900,
            // per app key: { "island": bool, "mute": bool }
            "apps": {}
        },
        // clipboard history: text (+ pictures), kept on disk encrypted for this Windows user or only
        // in memory, removed after days days (0 = never by age) / beyond max entries; pinned stay;
        // content a password manager marks private is skipped
        "clip": { "on": true, "images": true, "store": "disk", "days": 7, "max": 200, "skipPrivate": true },
        "meta": {
            // 2 = big pill with items side by side (0.2.4)
            "pillV": 2,
            // 1 = Antigravity ring shows 5 h + weekly of its busier model group (0.2.6)
            "ringV": 1,
            // 1 = notifications: the page and the pill's unread count switched on once;
            // 2 = message text hidden by default ("所有的通知都不要显示内容")
            "notifyV": 2,
            // 1 = the clipboard page and its pill icon switched on once (0.2.21)
            "clipV": 1
        },
        "appearance": {
            // auto | glass-dark | glass-light | <preset id> | custom
            "theme": "auto",
            // live blurred backdrop; off = solid background (and no screen capture)
            "glass": true,
            // 20..100, 50 = default look
            "opacity": 50,
            // keeps text readable over any backdrop: off | normal | strong (white glass lifts dark
            // areas behind it, black glass dims bright ones, pixel by pixel)
            "legible": "normal",
            // the collapsed pill as a whole, text included (percent; the open panel is always opaque)
            "pillOpacity": 100,
            "font": "",
            // percent
            "scale": 100,
            // animations: auto (follow Windows "animation effects") | on | off
            "motion": "auto",
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
        let fresh = v.as_object().map(|o| o.is_empty()).unwrap_or(true);
        let old_pill = v.pointer("/meta/pillV").and_then(|x| x.as_u64()).unwrap_or(0) < 2;
        let old_ring = v.pointer("/meta/ringV").and_then(|x| x.as_u64()).unwrap_or(0) < 1;
        let notify_v = v.pointer("/meta/notifyV").and_then(|x| x.as_u64()).unwrap_or(0);
        let old_notify = notify_v < 1;
        let old_clip = v.pointer("/meta/clipV").and_then(|x| x.as_u64()).unwrap_or(0) < 1;
        fill(&mut v, &defaults());
        if notify_v < 2 && !fresh {
            // the first notification builds showed the text: now hidden unless chosen again
            if v.pointer("/notify/privacy").and_then(|x| x.as_str()) == Some("smart") {
                v["notify"]["privacy"] = json!("source");
            }
            v["meta"]["notifyV"] = json!(2);
            if let Ok(t) = serde_json::to_string_pretty(&v) {
                let _ = std::fs::write(path(), t);
            }
        }
        if old_notify && !fresh {
            // new lists items arrive switched off; the notification page and count start on
            for (list, id) in [("/panel/pages", "notes"), ("/pill/items", "notes")] {
                if let Some(a) = v.pointer_mut(list).and_then(|x| x.as_array_mut()) {
                    for i in a.iter_mut().filter(|i| i.get("id").and_then(|x| x.as_str()) == Some(id)) {
                        i["on"] = json!(true);
                    }
                }
            }
            v["meta"]["notifyV"] = json!(2);
            if let Ok(t) = serde_json::to_string_pretty(&v) {
                let _ = std::fs::write(path(), t);
            }
        }
        if old_clip && !fresh {
            // new list items arrive switched off; the clipboard page and pill icon start on (asked for)
            for list in ["/panel/pages", "/pill/items"] {
                if let Some(a) = v.pointer_mut(list).and_then(|x| x.as_array_mut()) {
                    for i in a.iter_mut().filter(|i| i.get("id").and_then(|x| x.as_str()) == Some("clip")) {
                        i["on"] = json!(true);
                    }
                }
            }
            v["meta"]["clipV"] = json!(1);
            if let Ok(t) = serde_json::to_string_pretty(&v) {
                let _ = std::fs::write(path(), t);
            }
        }
        if old_ring && !fresh {
            // 0.2.6: Antigravity now reports 5-hour + weekly pools; "max" was the old default
            if v.pointer("/ai/ring/mode/antigravity").and_then(|x| x.as_str()) == Some("max") {
                v["ai"]["ring"]["mode"]["antigravity"] = json!("both");
            }
            v["meta"]["ringV"] = json!(1);
            if let Ok(t) = serde_json::to_string_pretty(&v) {
                let _ = std::fs::write(path(), t);
            }
        }
        if old_pill && !fresh {
            // 0.2.4: bigger pill, items side by side instead of one rotating slot
            let d = defaults();
            let h = v.pointer("/size/pillH").and_then(|x| x.as_f64()).unwrap_or(36.0).max(44.0);
            let w = v.pointer("/size/pillMaxW").and_then(|x| x.as_f64()).unwrap_or(520.0).max(760.0);
            v["size"]["pillH"] = json!(h);
            v["size"]["pillMaxW"] = json!(w);
            v["pill"]["items"] = d["pill"]["items"].clone();
            v["meta"]["pillV"] = json!(2);
            if let Ok(t) = serde_json::to_string_pretty(&v) {
                let _ = std::fs::write(path(), t);
            }
        }
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
    // "restore defaults" is about the look and behaviour: the tour doesn't come back
    let tour_done = s.pointer("/general/tourDone").cloned();
    *s = defaults();
    if let Some(v) = tour_done {
        s["general"]["tourDone"] = v;
    }
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
