//! Live AI sessions (Claude Code), shown in the pill while they work.
//!
//! The hooks tell the start (UserPromptSubmit), a permission prompt (Notification) and the end
//! (Stop). What they don't tell is followed through the session's transcript (JSON lines) and its
//! live CLI process:
//! - new lines after a permission prompt: the user answered, the session runs again;
//! - "[Request interrupted by user]" as the last user text line: stopped with Esc (no Stop hook);
//! - its CLI process exited or its terminal window closed, or nothing written for a long time: the
//!   session is over.

use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json::Value;

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
    /// whether the post-settle transcript end-of-turn check already ran for the current mtime
    #[serde(skip)]
    settled_checked: bool,
    /// (pid, process_create_time_100ns) of the live CLI process so tab close / Ctrl+C ends it at once
    #[serde(skip)]
    tool_pid: Option<(u32, u64)>,
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
#[allow(dead_code)]
pub fn started(id: &str, tool: &str, project: &str, transcript: &str, target: Target) {
    started_with_pid(id, tool, project, transcript, target, None);
}

pub(crate) fn started_with_pid(
    id: &str,
    tool: &str,
    project: &str,
    transcript: &str,
    target: Target,
    tool_pid: Option<(u32, u64)>,
) {
    if id.is_empty() || !on() {
        return;
    }
    let resolved_path = if transcript.is_empty() {
        LIST.lock()
            .unwrap()
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.transcript.clone())
            .unwrap_or_default()
    } else {
        transcript.to_string()
    };
    let cur_mtime = mtime(&resolved_path);
    let now = now_ms();
    {
        let mut l = LIST.lock().unwrap();
        match l.iter_mut().find(|s| s.id == id) {
            Some(s) => {
                s.state = "run".into();
                s.state_at = now;
                s.ask.clear();
                s.active_at = now;
                s.settled_checked = false;
                if !transcript.is_empty() {
                    s.transcript = transcript.into();
                }
                s.mtime = cur_mtime;
                if !matches!(target, Target::None) {
                    s.target = target;
                }
                if tool_pid.is_some() {
                    s.tool_pid = tool_pid;
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
                mtime: cur_mtime,
                active_at: now,
                settled_checked: false,
                tool_pid,
            }),
        }
    }
    publish();
}

/// A permission prompt: the session waits for the user.
#[allow(dead_code)]
pub fn asking(id: &str, tool: &str, project: &str, what: &str, transcript: &str, target: Target) {
    asking_with_pid(id, tool, project, what, transcript, target, None);
}

pub(crate) fn asking_with_pid(
    id: &str,
    tool: &str,
    project: &str,
    what: &str,
    transcript: &str,
    target: Target,
    tool_pid: Option<(u32, u64)>,
) {
    if id.is_empty() || !on() {
        return;
    }
    let resolved_path = if transcript.is_empty() {
        LIST.lock()
            .unwrap()
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.transcript.clone())
            .unwrap_or_default()
    } else {
        transcript.to_string()
    };
    // the baseline: lines written after this mean the user answered
    let cur_mtime = mtime(&resolved_path);
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
                settled_checked: false,
                tool_pid: None,
            });
        }
        let s = l.iter_mut().find(|s| s.id == id).unwrap();
        s.state = "ask".into();
        s.state_at = now;
        s.ask = what.into();
        s.active_at = now;
        s.settled_checked = false;
        if !transcript.is_empty() {
            s.transcript = transcript.into();
        }
        s.mtime = cur_mtime;
        if !matches!(target, Target::None) {
            s.target = target;
        }
        if tool_pid.is_some() {
            s.tool_pid = tool_pid;
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
    let t = LIST
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.target.clone())
        .ok_or_else(|| crate::i18n::t("这个任务已经结束了"))?;
    if matches!(t, Target::None) {
        return Err(crate::i18n::t("不知道它在哪个窗口里"));
    }
    notify::focus_target(&t);
    Ok(())
}

const TURN_SETTLE: Duration = Duration::from_millis(4500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TranscriptState {
    Running,
    Interrupted,
    TurnEnded,
}

fn parse_transcript_tail(slice: &str) -> TranscriptState {
    for line in slice.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("isSidechain") == Some(&Value::Bool(true)) {
            continue;
        }
        match v.get("type").and_then(|t| t.as_str()) {
            Some("assistant") => {
                let end_turn = v.pointer("/message/stop_reason").and_then(|s| s.as_str()) == Some("end_turn");
                let content = v.pointer("/message/content").and_then(|c| c.as_array());
                let has_tool_use = content
                    .map(|arr| arr.iter().any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use")))
                    .unwrap_or(false);
                let non_empty = content.map(|arr| !arr.is_empty()).unwrap_or(false);
                return if end_turn && non_empty && !has_tool_use {
                    TranscriptState::TurnEnded
                } else {
                    TranscriptState::Running
                };
            }
            Some("user") => match v.pointer("/message/content") {
                Some(Value::String(s)) => {
                    let t = s.trim();
                    return if t == "[Request interrupted by user]" || t == "[Request interrupted by user for tool use]" {
                        TranscriptState::Interrupted
                    } else {
                        TranscriptState::Running
                    };
                }
                Some(Value::Array(arr)) => {
                    if arr.iter().any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result")) {
                        // A tool just returned its result and Claude is continuing the turn
                        return TranscriptState::Running;
                    }
                    let is_int = arr.iter().any(|b| {
                        b.get("type").and_then(|t| t.as_str()) == Some("text")
                            && b.get("text")
                                .and_then(|t| t.as_str())
                                .map(|s| {
                                    let t = s.trim();
                                    t == "[Request interrupted by user]" || t == "[Request interrupted by user for tool use]"
                                })
                                .unwrap_or(false)
                    });
                    return if is_int { TranscriptState::Interrupted } else { TranscriptState::Running };
                }
                _ => return TranscriptState::Running,
            },
            _ => continue,
        }
    }
    TranscriptState::Running
}

/// Inspects the tail of a Claude Code JSONL transcript for user Esc interruption or clean turn end
/// (so sessions still finish cleanly when the user intentionally removed `/hooks/Stop`).
fn transcript_state(path: &str) -> TranscriptState {
    let Ok(mut f) = std::fs::File::open(path) else { return TranscriptState::Running };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let truncated = if len > 262_144 {
        let _ = f.seek(SeekFrom::End(-262_144));
        true
    } else {
        false
    };
    let mut raw = Vec::with_capacity(len.min(262_144) as usize);
    if f.read_to_end(&mut raw).is_err() {
        return TranscriptState::Running;
    }
    let text = String::from_utf8_lossy(&raw);
    let slice = if truncated {
        match text.find('\n') {
            Some(i) => &text[i + 1..],
            None => return TranscriptState::Running,
        }
    } else {
        &text[..]
    };
    parse_transcript_tail(slice)
}

enum CheckOutcome {
    ProcessExited,
    WindowGone,
    TranscriptGrew { mtime: Option<SystemTime>, state: TranscriptState, settled: bool },
    SettledCheck(TranscriptState),
    Unchanged,
}

fn watch() {
    loop {
        crate::perf::tick("agents");
        std::thread::sleep(Duration::from_secs(2));
        if !on() {
            let had = {
                let mut l = LIST.lock().unwrap();
                let non_empty = !l.is_empty();
                l.clear();
                non_empty
            };
            if had {
                notify::emit("agents", Vec::<Session>::new());
            }
            continue;
        }
        // Snapshot sessions outside LIST.lock() so file I/O and Win32 queries never hold the mutex
        let snap: Vec<Session> = LIST.lock().unwrap().clone();
        if snap.is_empty() {
            continue;
        }
        let now_sys = SystemTime::now();
        let now_epoch = now_ms();
        let mut outcomes: Vec<(String, u64, Option<(u32, u64)>, Target, CheckOutcome)> = Vec::with_capacity(snap.len());
        for s in snap {
            let alive_ok = if let Some((pid, ct)) = s.tool_pid {
                if !notify::process_alive(pid, ct) {
                    Err(CheckOutcome::ProcessExited)
                } else if !notify::target_alive(&s.target) {
                    Err(CheckOutcome::WindowGone)
                } else {
                    Ok(())
                }
            } else if !notify::target_alive(&s.target) {
                Err(CheckOutcome::WindowGone)
            } else {
                Ok(())
            };
            let outcome = match alive_ok {
                Err(dead) => dead,
                Ok(()) => {
                    let m = mtime(&s.transcript);
                    let mtime_settled = m
                        .and_then(|mt| now_sys.duration_since(mt).ok())
                        .map(|d| d >= TURN_SETTLE)
                        .unwrap_or(false)
                        && now_epoch.saturating_sub(s.state_at) >= TURN_SETTLE.as_millis() as u64;
                    if m.is_some() && m != s.mtime {
                        CheckOutcome::TranscriptGrew {
                            mtime: m,
                            state: transcript_state(&s.transcript),
                            settled: mtime_settled,
                        }
                    } else if s.state == "run" && !s.settled_checked && mtime_settled {
                        CheckOutcome::SettledCheck(transcript_state(&s.transcript))
                    } else {
                        CheckOutcome::Unchanged
                    }
                }
            };
            outcomes.push((s.id, s.state_at, s.tool_pid, s.target, outcome));
        }
        let now = now_ms();
        let mut changed = false;
        let mut logs: Vec<String> = Vec::new();
        {
            let mut l = LIST.lock().unwrap();
            let before = l.len();
            l.retain_mut(|s| {
                if let Some((_, snap_state_at, snap_pid, snap_target, outcome)) =
                    outcomes.iter().find(|(id, _, _, _, _)| *id == s.id)
                {
                    match outcome {
                        CheckOutcome::ProcessExited if s.tool_pid == *snap_pid => {
                            logs.push(format!("agents: {} {} process exited", s.tool, s.project));
                            return false;
                        }
                        CheckOutcome::WindowGone if s.target == *snap_target => {
                            logs.push(format!("agents: {} {} window gone", s.tool, s.project));
                            return false;
                        }
                        CheckOutcome::TranscriptGrew { mtime: m, state: st, settled }
                            if s.state_at == *snap_state_at =>
                        {
                            s.mtime = *m;
                            s.active_at = now;
                            s.settled_checked = *settled;
                            if *st == TranscriptState::Interrupted {
                                logs.push(format!("agents: {} {} interrupted", s.tool, s.project));
                                return false;
                            }
                            if *settled && s.state == "run" && *st == TranscriptState::TurnEnded {
                                logs.push(format!("agents: {} {} turn finished (transcript)", s.tool, s.project));
                                return false;
                            }
                            if s.state == "ask" && now.saturating_sub(s.state_at) >= ASK_SETTLE {
                                s.state = "run".into();
                                s.state_at = now;
                                s.ask.clear();
                                s.settled_checked = false;
                                changed = true;
                            }
                        }
                        CheckOutcome::SettledCheck(st) if s.state_at == *snap_state_at => {
                            s.settled_checked = true;
                            if *st == TranscriptState::Interrupted {
                                logs.push(format!("agents: {} {} interrupted", s.tool, s.project));
                                return false;
                            }
                            if s.state == "run" && *st == TranscriptState::TurnEnded {
                                logs.push(format!("agents: {} {} turn finished (transcript)", s.tool, s.project));
                                return false;
                            }
                        }
                        _ => {}
                    }
                }
                let quiet = if s.state == "ask" { ASK_QUIET } else { RUN_QUIET };
                now.saturating_sub(s.active_at) < quiet
            });
            changed |= l.len() != before;
        }
        for msg in logs {
            log(msg);
        }
        if changed {
            publish();
        }
    }
}

pub fn start() {
    std::thread::spawn(watch);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_turn_end_and_tool_loop() {
        let finished = concat!(
            r#"{"type":"user","message":{"content":"hello"}}"#, "\n",
            r#"{"type":"assistant","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"Hi!"}]}}"#, "\n"
        );
        assert_eq!(parse_transcript_tail(finished), TranscriptState::TurnEnded);

        let in_tool = concat!(
            r#"{"type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"1"}]}}"#, "\n",
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"1","content":"[Request interrupted by user] in file"}]}}"#, "\n"
        );
        assert_eq!(parse_transcript_tail(in_tool), TranscriptState::Running);

        let sidechain_end = concat!(
            r#"{"type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"1"}]}}"#, "\n",
            r#"{"type":"assistant","isSidechain":true,"message":{"stop_reason":"end_turn","content":[{"type":"text","text":"subagent done"}]}}"#, "\n"
        );
        assert_eq!(parse_transcript_tail(sidechain_end), TranscriptState::Running);

        let esc = concat!(
            r#"{"type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"1"}]}}"#, "\n",
            r#"{"type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#, "\n"
        );
        assert_eq!(parse_transcript_tail(esc), TranscriptState::Interrupted);
    }
}
