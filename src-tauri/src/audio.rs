//! Music bars: the loudness of what the speakers play right now, in 12 frequency bands, read
//! through WASAPI loopback on the default output device. Only levels are computed (nothing is
//! recorded or stored) and only while a page shows the bars (`set_wanted`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

pub const BANDS: usize = 12;
const N: usize = 2048;

static WANTED: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);

/// Turns the reader on or off; `emit` gets the bands (0..1) about 25 times a second while on.
pub fn set_wanted(on: bool, emit: impl Fn([f32; BANDS]) + Send + 'static) {
    WANTED.store(on, Ordering::SeqCst);
    if on && !STARTED.swap(true, Ordering::SeqCst) {
        std::thread::spawn(move || run(emit));
    }
}

fn run(emit: impl Fn([f32; BANDS])) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    loop {
        while !WANTED.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(300));
        }
        if let Err(e) = capture(&emit) {
            crate::log(format!("music bars: {e}"));
            emit([0.0; BANDS]);
            std::thread::sleep(Duration::from_secs(3));
        }
    }
}

/// Goertzel power of one frequency over the windowed buffer.
fn goertzel(x: &[f32], freq: f32, rate: f32) -> f32 {
    let w = 2.0 * std::f32::consts::PI * freq / rate;
    let c = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for &v in x {
        let s = v + c * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    (s1 * s1 + s2 * s2 - c * s1 * s2).max(0.0)
}

fn capture(emit: &impl Fn([f32; BANDS])) -> windows::core::Result<()> {
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
        let fmt = client.GetMixFormat()?;
        let (channels, bits, rate) = ((*fmt).nChannels as usize, (*fmt).wBitsPerSample, (*fmt).nSamplesPerSec as f32);
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 2_000_000, 0, fmt, None)?;
        CoTaskMemFree(Some(fmt as *const _));
        let cap: IAudioCaptureClient = client.GetService()?;
        client.Start()?;

        // band centres, log-spaced 60 Hz .. 10 kHz; window precomputed
        let freqs: Vec<f32> = (0..BANDS).map(|i| 60.0 * (10_000.0f32 / 60.0).powf(i as f32 / (BANDS - 1) as f32)).collect();
        let hann: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (N - 1) as f32).cos()).collect();
        let mut ring = vec![0.0f32; N];
        let mut pos = 0usize;
        let mut peak = [1e-6f32; BANDS];
        let mut out = [0.0f32; BANDS];
        let mut last_emit = Instant::now();
        let mut last_data = Instant::now();

        while WANTED.load(Ordering::SeqCst) {
            let mut n = cap.GetNextPacketSize()?;
            while n > 0 {
                let mut data: *mut u8 = std::ptr::null_mut();
                let (mut frames, mut flags) = (0u32, 0u32);
                cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
                for f in 0..frames as usize {
                    let mut v = 0.0f32;
                    if !silent && !data.is_null() {
                        for ch in 0..channels {
                            let i = f * channels + ch;
                            v += match bits {
                                32 => *(data as *const f32).add(i),
                                16 => *(data as *const i16).add(i) as f32 / 32768.0,
                                _ => 0.0,
                            };
                        }
                        v /= channels.max(1) as f32;
                    }
                    ring[pos] = v;
                    pos = (pos + 1) % N;
                }
                cap.ReleaseBuffer(frames)?;
                last_data = Instant::now();
                n = cap.GetNextPacketSize()?;
            }
            if last_emit.elapsed() >= Duration::from_millis(40) {
                last_emit = Instant::now();
                // nothing played for a moment: loopback delivers no packets at all
                if last_data.elapsed() > Duration::from_millis(250) {
                    ring.iter_mut().for_each(|v| *v = 0.0);
                }
                let x: Vec<f32> = (0..N).map(|i| ring[(pos + i) % N] * hann[i]).collect();
                let rms = (x.iter().map(|v| v * v).sum::<f32>() / N as f32).sqrt();
                for b in 0..BANDS {
                    let f = freqs[b];
                    let p = goertzel(&x, f * 0.85, rate) + goertzel(&x, f, rate) + goertzel(&x, f * 1.18, rate);
                    let level = (1.0 + p).ln();
                    // per-band automatic gain: quiet songs still move, loud ones don't clip
                    peak[b] = (peak[b] * 0.996).max(level).max(0.5);
                    let target = if rms < 2e-4 { 0.0 } else { (level / peak[b]).clamp(0.0, 1.0).powf(1.6) };
                    // fast up, slower down
                    out[b] = if target > out[b] { target } else { out[b] * 0.78 + target * 0.22 };
                }
                emit(out);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        client.Stop()?;
        emit([0.0; BANDS]);
    }
    Ok(())
}
