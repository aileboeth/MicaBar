//! Live AI sessions (Claude Code), shown in the pill while they work.
//!
//! The hooks tell the start (UserPromptSubmit), a permission prompt (Notification) and the end
//! (Stop). What they don't tell is followed through the session's transcript (JSON lines):
//! - new lines after a permission prompt: the user answered, the session runs again;
//! - "[Request interrupted by user]" as the last user line: stopped with Esc (no Stop hook);
//! - nothing written for a long time, or its terminal window gone: the session is over.

use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::notify::{self, Target};
use crate::{log, now_ms};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub tool: String,
    /// last folder of the working directory
    pub project: String,
    /// run | ask
    pub state: String,
    pub started: u64,
    pub state_at: u64,
    /// what it waits for ("使用 Bash")
    pub ask: String,
    #[serde(skip)]
    target: Target,
    #[serde(skip)]
    transcript: String,
    #[serde(skip)]
    mtime: Option<SystemTime>,
    /// last time the transcript grew (or a hook came)
    #[serde(skip)]
    active_at: u64,
}

static LIST: Mutex<Vec<Session>> = Mutex::new(Vec::new());

/// A running session with no transcript activity for this long is taken as gone (closed
/// terminal, crash); one waiting for the user is kept longer. One long command (a build) writes
/// nothing until it ends, so this is generous.
const RUN_QUIET: u64 = 60 * 60_000;
/// Lines written right after a permission prompt belong to the prompt itself, not to an answer.
const ASK_SETTLE: u64 = 3000;
const ASK_QUIET: u64 = 3 * 3600_000;

fn on() -> bool {
    crate::settings::bool_at("ai.live", true)
}

fn mtime(path: &str) -> Option<SystemTime> {
    if path.is_empty() {
        return None;
    }
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn publish() {
    let list = list();
    notify::emit("agents", list);
}

pub fn list() -> Vec<Session> {
    if !on() {
        return Vec::new();
    }
    LIST.lock().unwrap().clone()
}

/// A prompt was sent: the session runs (a queued prompt while running keeps the start time).
pub fn started(id: &str, tool: &str, project: &str, transcript: &str, target: Target) {
    if id.is_empty() {
        return;
    }
    let now = now_ms();
    {
        let mut l = LIST.lock().unwrap();
        match l.iter_mut().find(|s| s.id == id) {
            Some(s) => {
                s.state = "run".into();
                s.state_at = now;
                s.ask.clear();
                s.active_at = now;
                if !transcript.is_empty() {
                    s.transcript = transcript.into();
                }
                if !matches!(target, Target::None) {
                    s.target = target;
                }
            }
            None => l.push(Session {
                id: id.into(),
                tool: tool.into(),
                project: project.into(),
                state: "run".into(),
                started: now,
                state_at: now,
                ask: String::new(),
                target,
                transcript: transcript.into(),
                mtime: mtime(transcript),
                active_at: now,
            }),
        }
    }
    publish();
}

/// A permission prompt: the session waits for the user.
pub fn asking(id: &str, tool: &str, project: &str, what: &str, transcript: &str, target: Target) {
    if id.is_empty() {
        return;
    }
    let now = now_ms();
    {
        let mut l = LIST.lock().unwrap();
        if !l.iter().any(|s| s.id == id) {
            // MicaBar started after the prompt: the session is known from here on
            l.push(Session {
                id: id.into(),
                tool: tool.into(),
                project: project.into(),
                state: "ask".into(),
                started: now,
                state_at: now,
                ask: String::new(),
                target: Target::None,
                transcript: String::new(),
                mtime: None,
                active_at: now,
            });
        }
        let s = l.iter_mut().find(|s| s.id == id).unwrap();
        s.state = "ask".into();
        s.state_at = now;
        s.ask = what.into();
        s.active_at = now;
        if !transcript.is_empty() {
            s.transcript = transcript.into();
        }
        // the baseline: lines written after this mean the user answered
        s.mtime = mtime(&s.transcript);
        if !matches!(target, Target::None) {
            s.target = target;
        }
    }
    publish();
}

pub fn stopped(id: &str) {
    let removed = {
        let mut l = LIST.lock().unwrap();
        let n = l.len();
        l.retain(|s| s.id != id);
        n != l.len()
    };
    if removed {
        publish();
    }
}

/// Brings the session's terminal / editor window to the front.
pub fn focus(id: &str) -> Result<(), String> {
    let t = LIST.lock().unwrap().iter().find(|s| s.id == id).map(|s| s.target.clone()).ok_or("这个任务已经结束了")?;
    if matches!(t, Target::None) {
        return Err("不知道它在哪个窗口里".into());
    }
    notify::focus_target(&t);
    Ok(())
}

/// The last user / assistant line of a transcript says the user pressed Esc.
fn interrupted(path: &str) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(24 * 1024)));
    let mut raw = Vec::new();
    let _ = f.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw);
    for line in text.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("user") => return line.contains("[Request interrupted by user"),
            Some("assistant") => return false,
            _ => continue,
        }
    }
    false
}

fn watch() {
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let now = now_ms();
        let mut changed = false;
        {
            let mut l = LIST.lock().unwrap();
            let before = l.len();
            l.retain_mut(|s| {
                if !notify::target_alive(&s.target) {
                    log(format!("agents: {} {} window gone", s.tool, s.project));
                    return false;
                }
                let m = mtime(&s.transcript);
                if m.is_some() && m != s.mtime {
                    s.mtime = m;
                    s.active_at = now;
                    if interrupted(&s.transcript) {
                        log(format!("agents: {} {} interrupted", s.tool, s.project));
                        return false;
                    }
                    if s.state == "ask" && now.saturating_sub(s.state_at) >= ASK_SETTLE {
                        s.state = "run".into();
                        s.state_at = now;
                        s.ask.clear();
                        changed = true;
                    }
                }
                let quiet = if s.state == "ask" { ASK_QUIET } else { RUN_QUIET };
                now.saturating_sub(s.active_at) < quiet
            });
            changed |= l.len() != before;
        }
        if changed {
            publish();
        }
    }
}

pub fn start() {
    std::thread::spawn(watch);
}
