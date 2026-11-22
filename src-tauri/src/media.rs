//! Now playing: the Windows media session manager (GSMTC) — works with most players (NetEase
//! Cloud Music, QQ Music, Spotify, browsers, the Windows media player). Polled on a worker thread
//! with an MTA apartment; the cover is read only when the track changes.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use serde::Serialize;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager as Manager, GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
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

fn read_cover(r: &IRandomAccessStreamReference) -> Option<String> {
    let stream = r.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > 4 * 1024 * 1024 {
        return None;
    }
    let mime = stream.ContentType().map(|s| s.to_string()).unwrap_or_default();
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    let n = reader.LoadAsync(size as u32).ok()?.join().ok()?;
    let mut buf = vec![0u8; n as usize];
    reader.ReadBytes(&mut buf).ok()?;
    let mime = if mime.starts_with("image/") { mime } else if buf.starts_with(&[0x89, b'P']) { "image/png".into() } else { "image/jpeg".into() };
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&buf)))
}

fn app_label(aumid: &str) -> String {
    // "Spotify.exe", "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic", "308046B0AF4A39CB" (browsers)
    let a = aumid.rsplit('!').next().unwrap_or(aumid);
    a.trim_end_matches(".exe").rsplit('\\').next().unwrap_or(a).to_string()
}

/// One poll: the session the system considers current, else the first one that is playing.
fn poll(mgr: &Manager, last_key: &mut String, last_cover: &mut Option<String>) -> Option<NowPlaying> {
    let sessions = mgr.GetSessions().ok()?;
    let mut pick = mgr.GetCurrentSession().ok();
    let playing = |s: &windows::Media::Control::GlobalSystemMediaTransportControlsSession| {
        s.GetPlaybackInfo().and_then(|p| p.PlaybackStatus()).map(|st| st == Status::Playing).unwrap_or(false)
    };
    if pick.as_ref().map(|s| !playing(s)).unwrap_or(true) {
        if let Some(p) = (0..sessions.Size().unwrap_or(0)).filter_map(|i| sessions.GetAt(i).ok()).find(|s| playing(s)) {
            pick = Some(p);
        }
    }
    let s = pick?;
    let props = s.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
    let title = props.Title().map(|h| h.to_string()).unwrap_or_default();
    if title.is_empty() {
        return None;
    }
    let artist = props.Artist().map(|h| h.to_string()).unwrap_or_default();
    let album = props.AlbumTitle().map(|h| h.to_string()).unwrap_or_default();
    let app = app_label(&s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default());
    let key = format!("{app}|{title}|{artist}|{album}");
    if key != *last_key {
        *last_key = key;
        *last_cover = props.Thumbnail().ok().and_then(|t| read_cover(&t));
    }
    let (mut position, mut duration) = (0.0, 0.0);
    if let Ok(tl) = s.GetTimelineProperties() {
        // TimeSpan = 100 ns ticks
        position = tl.Position().map(|t| t.Duration as f64 / 1e7).unwrap_or(0.0);
        duration = tl.EndTime().map(|t| t.Duration as f64 / 1e7).unwrap_or(0.0);
    }
    Some(NowPlaying { title, artist, album, app, playing: playing(&s), cover: last_cover.clone(), position, duration, at: crate::now_ms() })
}

/// Starts the poller; `on_change` runs whenever the track, play state or cover changes.
pub fn start(state: Shared, on_change: impl Fn(Option<NowPlaying>) + Send + 'static) {
    std::thread::spawn(move || {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let mut mgr: Option<Manager> = None;
        let (mut last_key, mut last_cover) = (String::new(), None);
        let mut fails = 0u32;
        loop {
            let on = crate::settings::bool_at("media.on", true);
            if !on {
                if state.lock().unwrap().take().is_some() {
                    on_change(None);
                }
                std::thread::sleep(Duration::from_secs(3));
                continue;
            }
            if mgr.is_none() {
                match Manager::RequestAsync().and_then(|op| op.join()) {
                    Ok(m) => mgr = Some(m),
                    Err(e) => {
                        if fails == 0 {
                            crate::log(format!("media: session manager unavailable: {e}"));
                        }
                        fails += 1;
                        std::thread::sleep(Duration::from_secs(30));
                        continue;
                    }
                }
            }
            let now = poll(mgr.as_ref().unwrap(), &mut last_key, &mut last_cover);
            let changed = {
                let mut cur = state.lock().unwrap();
                let differs = match (&*cur, &now) {
                    (Some(a), Some(b)) => a.title != b.title || a.artist != b.artist || a.playing != b.playing || a.cover.is_some() != b.cover.is_some() || a.app != b.app,
                    (None, None) => false,
                    _ => true,
                };
                *cur = now.clone();
                differs
            };
            if changed {
                on_change(now);
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
    });
}

/// play-pause | next | previous on the current session.
pub fn control(action: &str) -> Result<(), String> {
    let mgr = Manager::RequestAsync().and_then(|op| op.join()).map_err(|e| e.to_string())?;
    let s = mgr.GetCurrentSession().map_err(|_| crate::i18n::t("没有正在播放的程序").to_string())?;
    let op = match action {
        "next" => s.TrySkipNextAsync(),
        "previous" => s.TrySkipPreviousAsync(),
        _ => s.TryTogglePlayPauseAsync(),
    }
    .map_err(|e| e.to_string())?;
    op.join().map_err(|e| e.to_string())?;
    Ok(())
}
