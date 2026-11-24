//! Now playing: the Windows media session manager (GSMTC) — works with most players (NetEase
//! Cloud Music, QQ Music, Spotify, browsers, the Windows media player). Polled on a worker thread
//! with an MTA apartment; the cover is read when the track changes and again for a few polls after.

use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use serde::Serialize;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::{DataReader, IRandomAccessStreamReference};

#[derive(Serialize, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// player's app id, e.g. "cloudmusic.exe" or "Spotify.exe"
    pub app: String,
    pub playing: bool,
    /// data: URL of the cover, when the player provides one
    pub cover: Option<String>,
    pub position: f64,
    pub duration: f64,
    pub at: u64,
}

pub type Shared = Arc<Mutex<Option<NowPlaying>>>;

type ControlMsg = (String, SyncSender<Result<(), String>>);

fn control_bus() -> &'static (Sender<ControlMsg>, Mutex<Option<Receiver<ControlMsg>>>) {
    static BUS: OnceLock<(Sender<ControlMsg>, Mutex<Option<Receiver<ControlMsg>>>)> = OnceLock::new();
    BUS.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        (tx, Mutex::new(Some(rx)))
    })
}

fn raster_mime(declared: &str, buf: &[u8]) -> Option<&'static str> {
    let m = declared.trim().split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match m.as_str() {
        "image/png" => return Some("image/png"),
        "image/jpeg" | "image/jpg" => return Some("image/jpeg"),
        "image/webp" => return Some("image/webp"),
        "image/gif" => return Some("image/gif"),
        "image/bmp" | "image/x-bmp" | "image/x-ms-bmp" => return Some("image/bmp"),
        _ => {}
    }
    if buf.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if buf.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if buf.len() >= 12 && &buf[0..4] == b"RIFF" && &buf[8..12] == b"WEBP" {
        Some("image/webp")
    } else if buf.starts_with(b"GIF87a") || buf.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if buf.starts_with(b"BM") {
        Some("image/bmp")
    } else {
        None
    }
}

fn read_cover(r: &IRandomAccessStreamReference) -> Option<String> {
    let stream = r.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > 4 * 1024 * 1024 {
        return None;
    }
    let declared = stream.ContentType().map(|s| s.to_string()).unwrap_or_default();
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    let n = reader.LoadAsync(size as u32).ok()?.join().ok()?;
    if n == 0 || (n as u64) > 4 * 1024 * 1024 {
        return None;
    }
    let mut buf = vec![0u8; n as usize];
    reader.ReadBytes(&mut buf).ok()?;
    let mime = raster_mime(&declared, &buf)?;
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&buf)))
}

fn app_label(aumid: &str) -> String {
    // "Spotify.exe", "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic", "308046B0AF4A39CB" (browsers)
    let a = aumid.rsplit('!').next().unwrap_or(aumid);
    a.trim_end_matches(".exe").rsplit('\\').next().unwrap_or(a).to_string()
}

/// Polls (1.5 s each) after a track change during which the cover is read again: players publish
/// the new title first and the new thumbnail a moment later (NetEase Cloud Music, QQ Music), and
/// browsers (YouTube Music) only after downloading the artwork, so a single read at the change kept
/// the previous song's cover (or none). Up to ~30 s; once a new cover arrived, two more reads.
const COVER_REREADS: u32 = 20;
const COVER_SETTLE: u32 = 2;
const WIN_EPOCH_100NS: i64 = 116_444_736_000_000_000;

fn is_playing(s: &Session) -> bool {
    s.GetPlaybackInfo()
        .and_then(|p| p.PlaybackStatus())
        .map(|st| st == Status::Playing)
        .unwrap_or(false)
}

fn pick_session(mgr: &Manager) -> windows::core::Result<Option<Session>> {
    let sessions = mgr.GetSessions()?;
    let mut pick = mgr.GetCurrentSession().ok();
    if pick.as_ref().map(|s| !is_playing(s)).unwrap_or(true) {
        if let Some(p) = (0..sessions.Size().unwrap_or(0)).filter_map(|i| sessions.GetAt(i).ok()).find(is_playing) {
            pick = Some(p);
        }
    }
    Ok(pick)
}

struct PollCtx {
    last_key: String,
    last_cover: Option<String>,
    prev_cover: Option<String>,
    rereads: u32,
    had_cover_pref: bool,
    session: Option<Session>,
}

impl PollCtx {
    fn reset(&mut self) {
        self.last_key.clear();
        self.last_cover = None;
        self.prev_cover = None;
        self.rereads = 0;
        self.session = None;
    }
}

/// One poll: the session the system considers current, else the first one that is playing.
/// Returns `Err` when the GSMTC manager itself is broken (e.g. Explorer/WinRT restarted).
fn poll(mgr: &Manager, ctx: &mut PollCtx) -> windows::core::Result<Option<(NowPlaying, bool)>> {
    let Some(s) = pick_session(mgr)? else {
        ctx.reset();
        return Ok(None);
    };
    let Some(props) = s.TryGetMediaPropertiesAsync().ok().and_then(|op| op.join().ok()) else {
        ctx.reset();
        return Ok(None);
    };
    let title = props.Title().map(|h| h.to_string()).unwrap_or_default();
    if title.is_empty() {
        ctx.reset();
        return Ok(None);
    }
    ctx.session = Some(s.clone());
    let artist = props.Artist().map(|h| h.to_string()).unwrap_or_default();
    let album = props.AlbumTitle().map(|h| h.to_string()).unwrap_or_default();
    let app = app_label(&s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default());
    let key = format!("{app}|{title}|{artist}|{album}");

    let want_cover = crate::settings::bool_at("media.cover", true);
    if !want_cover {
        ctx.last_key = key;
        ctx.last_cover = None;
        ctx.prev_cover = None;
        ctx.rereads = 0;
    } else {
        let thumb = props.Thumbnail().ok();
        if thumb.is_none() {
            ctx.last_cover = None;
        }
        if key != ctx.last_key || !ctx.had_cover_pref {
            let same_track = key == ctx.last_key;
            ctx.last_key = key;
            let previous = if same_track { None } else { ctx.last_cover.take() };
            let first = thumb.as_ref().and_then(read_cover);
            ctx.rereads = if first.is_some() && first != previous { COVER_SETTLE } else { COVER_REREADS };
            ctx.prev_cover = previous;
            ctx.last_cover = first;
        } else if ctx.rereads > 0 {
            ctx.rereads -= 1;
            match thumb.as_ref().and_then(read_cover) {
                Some(c) => {
                    if ctx.last_cover.as_ref() != Some(&c) {
                        if ctx.prev_cover.as_ref() != Some(&c) {
                            ctx.rereads = ctx.rereads.min(COVER_SETTLE);
                        }
                        ctx.last_cover = Some(c);
                    }
                }
                None if thumb.is_none() || (ctx.rereads == 0 && ctx.last_cover == ctx.prev_cover) => {
                    ctx.last_cover = None;
                }
                None => {}
            }
        }
    }
    ctx.had_cover_pref = want_cover;

    let playing = is_playing(&s);
    let now_ms = crate::now_ms();
    let (mut position, mut duration, mut has_updated_ts) = (0.0, 0.0, false);
    if let Ok(tl) = s.GetTimelineProperties() {
        // TimeSpan = 100 ns ticks
        let start = tl.StartTime().map(|t| t.Duration as f64 / 1e7).unwrap_or(0.0);
        let raw_pos = tl.Position().map(|t| (t.Duration as f64 / 1e7) - start).unwrap_or(0.0).max(0.0);
        duration = tl.EndTime().map(|t| (t.Duration as f64 / 1e7) - start).unwrap_or(0.0).max(0.0);
        position = raw_pos;
        if let Ok(dt) = tl.LastUpdatedTime() {
            if dt.UniversalTime > WIN_EPOCH_100NS {
                has_updated_ts = true;
                let updated_ms = ((dt.UniversalTime - WIN_EPOCH_100NS) / 10_000) as u64;
                if playing && now_ms > updated_ms {
                    let elapsed = (now_ms - updated_ms) as f64 / 1000.0;
                    if elapsed < 86_400.0 {
                        position = raw_pos + elapsed;
                        if duration > 0.0 {
                            position = position.min(duration);
                        }
                    }
                }
            }
        }
    }
    Ok(Some((
        NowPlaying {
            title,
            artist,
            album,
            app,
            playing,
            cover: ctx.last_cover.clone(),
            position,
            duration,
            at: now_ms,
        },
        has_updated_ts,
    )))
}

fn run_control(mgr: &mut Option<Manager>, cached: &Option<Session>, action: &str) -> Result<(), String> {
    if mgr.is_none() {
        *mgr = Some(Manager::RequestAsync().and_then(|op| op.join()).map_err(|e| e.to_string())?);
    }
    let session = match cached.clone() {
        Some(s) => Some(s),
        None => match pick_session(mgr.as_ref().unwrap()) {
            Ok(s) => s,
            Err(_) => {
                *mgr = Some(Manager::RequestAsync().and_then(|op| op.join()).map_err(|e| e.to_string())?);
                pick_session(mgr.as_ref().unwrap()).map_err(|e| e.to_string())?
            }
        },
    }
    .ok_or_else(|| crate::i18n::t("没有正在播放的程序"))?;

    let op = match action {
        "next" => session.TrySkipNextAsync(),
        "previous" => session.TrySkipPreviousAsync(),
        _ => session.TryTogglePlayPauseAsync(),
    }
    .map_err(|e| e.to_string())?;
    op.join().map_err(|e| e.to_string())?;
    Ok(())
}

/// Starts the poller; `on_change` runs whenever the track, play state, seek or cover changes.
pub fn start(state: Shared, on_change: impl Fn(Option<NowPlaying>) + Send + 'static) {
    let rx = control_bus()
        .1
        .lock()
        .unwrap()
        .take()
        .unwrap_or_else(|| mpsc::channel().1);
    std::thread::spawn(move || {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let mut mgr: Option<Manager> = None;
        let mut ctx = PollCtx {
            last_key: String::new(),
            last_cover: None,
            prev_cover: None,
            rereads: 0,
            had_cover_pref: true,
            session: None,
        };
        let mut fails = 0u32;

        let wait_or_handle = |rx: &Receiver<ControlMsg>, mgr: &mut Option<Manager>, session: &Option<Session>, timeout: Duration| {
            if let Ok((act, reply)) = rx.recv_timeout(timeout) {
                let _ = reply.send(run_control(mgr, session, &act));
                while let Ok((act2, reply2)) = rx.try_recv() {
                    let _ = reply2.send(run_control(mgr, session, &act2));
                }
                // Give the player a brief moment to publish its updated PlaybackStatus/track.
                std::thread::sleep(Duration::from_millis(120));
            }
        };

        loop {
            crate::perf::tick("media");
            let on = crate::settings::bool_at("media.on", true);
            if !on {
                ctx.reset();
                if state.lock().unwrap().take().is_some() {
                    on_change(None);
                }
                wait_or_handle(&rx, &mut mgr, &ctx.session, Duration::from_secs(3));
                continue;
            }
            if mgr.is_none() {
                match Manager::RequestAsync().and_then(|op| op.join()) {
                    Ok(m) => {
                        mgr = Some(m);
                        fails = 0;
                    }
                    Err(e) => {
                        if fails == 0 {
                            crate::log(format!("media: session manager unavailable: {e}"));
                        }
                        fails += 1;
                        wait_or_handle(&rx, &mut mgr, &ctx.session, Duration::from_secs(30));
                        continue;
                    }
                }
            }
            match poll(mgr.as_ref().unwrap(), &mut ctx) {
                Ok( polled ) => {
                    let (now, has_updated_ts) = match polled {
                        Some((np, ts)) => (Some(np), ts),
                        None => (None, false),
                    };
                    let emit_val = {
                        let mut cur = state.lock().unwrap();
                        let differs = match (&*cur, &now) {
                            (Some(a), Some(b)) => {
                                let expected_pos = if a.playing {
                                    let p = a.position + b.at.saturating_sub(a.at) as f64 / 1000.0;
                                    if a.duration > 0.0 { p.min(a.duration) } else { p }
                                } else {
                                    a.position
                                };
                                let seeked = (b.position - expected_pos).abs() > 3.0;
                                let dur_changed = (b.duration - a.duration).abs() > 0.5;
                                a.title != b.title
                                    || a.artist != b.artist
                                    || a.album != b.album
                                    || a.playing != b.playing
                                    || a.cover != b.cover
                                    || a.app != b.app
                                    || dur_changed
                                    || seeked
                            }
                            (None, None) => false,
                            _ => true,
                        };
                        if differs {
                            *cur = now.clone();
                            Some(now)
                        } else {
                            if has_updated_ts {
                                *cur = now;
                            } else if let (Some(a), Some(b)) = (&mut *cur, &now) {
                                if (b.position - a.position).abs() > 0.01 {
                                    *a = b.clone();
                                }
                            }
                            None
                        }
                    };
                    if let Some(v) = emit_val {
                        on_change(v);
                    }
                }
                Err(e) => {
                    crate::log(format!("media: session query failed ({e}); reconnecting"));
                    mgr = None;
                    ctx.reset();
                    wait_or_handle(&rx, &mut mgr, &ctx.session, Duration::from_secs(2));
                    continue;
                }
            }
            wait_or_handle(&rx, &mut mgr, &ctx.session, Duration::from_millis(1500));
        }
    });
}

/// play-pause | next | previous on the active session (dispatched to the media MTA thread).
pub fn control(action: &str) -> Result<(), String> {
    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    control_bus()
        .0
        .send((action.to_string(), reply_tx))
        .map_err(|_| crate::i18n::t("没有正在播放的程序"))?;
    reply_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| crate::i18n::t("没有正在播放的程序"))?
}
