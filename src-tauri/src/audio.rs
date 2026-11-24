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

#[derive(Clone, Copy, PartialEq, Eq)]
enum SampleFmt {
    F32,
    I16,
    I24,
    I32,
}

unsafe fn inspect_format(fmt: *const WAVEFORMATEX) -> Option<(SampleFmt, usize)> {
    if fmt.is_null() {
        return None;
    }
    let tag = std::ptr::addr_of!((*fmt).wFormatTag).read_unaligned();
    let bits = std::ptr::addr_of!((*fmt).wBitsPerSample).read_unaligned();
    let cb_size = std::ptr::addr_of!((*fmt).cbSize).read_unaligned();
    let is_float = match tag {
        3 => true,  // WAVE_FORMAT_IEEE_FLOAT
        1 => false, // WAVE_FORMAT_PCM
        0xFFFE if cb_size >= 22 => {
            let ext = fmt as *const WAVEFORMATEXTENSIBLE;
            let sub = std::ptr::addr_of!((*ext).SubFormat).read_unaligned();
            const SUBTYPE_PCM: windows::core::GUID = windows::core::GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
            const SUBTYPE_FLOAT: windows::core::GUID = windows::core::GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
            if sub == SUBTYPE_FLOAT {
                true
            } else if sub == SUBTYPE_PCM {
                false
            } else {
                return None;
            }
        }
        _ => return None,
    };
    let kind = match (is_float, bits) {
        (true, 32) => SampleFmt::F32,
        (false, 16) => SampleFmt::I16,
        (false, 24) => SampleFmt::I24,
        (false, 32) => SampleFmt::I32,
        _ => return None,
    };
    Some((kind, (bits as usize) / 8))
}

unsafe fn endpoint_id(dev: &IMMDevice) -> Option<String> {
    let raw = dev.GetId().ok()?;
    if raw.0.is_null() {
        return None;
    }
    let mut len = 0usize;
    while *raw.0.add(len) != 0 {
        len += 1;
    }
    let id = String::from_utf16_lossy(std::slice::from_raw_parts(raw.0, len));
    CoTaskMemFree(Some(raw.0 as *const _));
    Some(id)
}

/// Goertzel power of one precomputed `2.0 * cos(w)` coefficient over the windowed buffer.
fn goertzel(x: &[f32], c: f32) -> f32 {
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for &v in x {
        let s = v + c * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    let p = s1 * s1 + s2 * s2 - c * s1 * s2;
    if p.is_finite() && p > 0.0 { p } else { 0.0 }
}

fn capture(emit: &impl Fn([f32; BANDS])) -> windows::core::Result<()> {
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let cur_id = endpoint_id(&dev);
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
        let fmt = client.GetMixFormat()?;
        let parsed = inspect_format(fmt);
        let channels = std::ptr::addr_of!((*fmt).nChannels).read_unaligned() as usize;
        let block_align = std::ptr::addr_of!((*fmt).nBlockAlign).read_unaligned() as usize;
        let rate = std::ptr::addr_of!((*fmt).nSamplesPerSec).read_unaligned() as f32;
        let init_res = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 2_000_000, 0, fmt, None);
        CoTaskMemFree(Some(fmt as *const _));
        init_res?;
        let Some((sample_fmt, bytes_per_ch)) = parsed.filter(|_| channels > 0 && rate > 0.0) else {
            return Err(windows::core::Error::from_hresult(windows::core::HRESULT(0x8889_0008u32 as i32)));
        };
        let frame_stride = block_align.max(channels * bytes_per_ch);
        let cap: IAudioCaptureClient = client.GetService()?;
        client.Start()?;

        // band centres, log-spaced 60 Hz .. 10 kHz; Goertzel 2*cos(w) and Hann window precomputed
        let coeffs: [[f32; 3]; BANDS] = std::array::from_fn(|b| {
            let f = 60.0 * (10_000.0f32 / 60.0).powf(b as f32 / (BANDS - 1) as f32);
            [f * 0.85, f, f * 1.18].map(|freq| {
                let w = 2.0 * std::f32::consts::PI * freq / rate;
                2.0 * w.cos()
            })
        });
        let hann: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (N - 1) as f32).cos()).collect();
        let mut ring = vec![0.0f32; N];
        let mut x = vec![0.0f32; N];
        let mut pos = 0usize;
        let mut peak = [1e-6f32; BANDS];
        let mut out = [0.0f32; BANDS];
        let mut zero_sent = false;
        let mut last_emit = Instant::now();
        let mut last_data = Instant::now();
        let mut last_dev_check = Instant::now();
        let inv_ch = 1.0 / (channels as f32);

        while WANTED.load(Ordering::SeqCst) {
            if last_dev_check.elapsed() >= Duration::from_secs(3) {
                last_dev_check = Instant::now();
                let now_id = en.GetDefaultAudioEndpoint(eRender, eConsole).ok().and_then(|d| endpoint_id(&d));
                if now_id != cur_id {
                    let _ = client.Stop();
                    if now_id.is_none() {
                        emit([0.0; BANDS]);
                    }
                    return Ok(());
                }
            }

            let mut n = cap.GetNextPacketSize()?;
            while n > 0 {
                let mut data: *mut u8 = std::ptr::null_mut();
                let (mut frames, mut flags) = (0u32, 0u32);
                cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
                for f in 0..frames as usize {
                    let mut v = 0.0f32;
                    if !silent && !data.is_null() {
                        let base = data.add(f * frame_stride);
                        for ch in 0..channels {
                            let p = base.add(ch * bytes_per_ch);
                            let s = match sample_fmt {
                                SampleFmt::F32 => {
                                    let fv = (p as *const f32).read_unaligned();
                                    if fv.is_finite() { fv.clamp(-4.0, 4.0) } else { 0.0 }
                                }
                                SampleFmt::I16 => ((p as *const i16).read_unaligned() as f32) / 32_768.0,
                                SampleFmt::I24 => {
                                    let s24 = (*p as i32) | ((*p.add(1) as i32) << 8) | (((*p.add(2) as i8) as i32) << 16);
                                    (s24 as f32) / 8_388_608.0
                                }
                                SampleFmt::I32 => ((p as *const i32).read_unaligned() as f32) / 2_147_483_648.0,
                            };
                            v += s;
                        }
                        v *= inv_ch;
                    }
                    ring[pos] = v;
                    pos = (pos + 1) % N;
                }
                cap.ReleaseBuffer(frames)?;
                if !silent && frames > 0 {
                    last_data = Instant::now();
                }
                n = cap.GetNextPacketSize()?;
            }
            if last_emit.elapsed() >= Duration::from_millis(40) {
                last_emit = Instant::now();
                crate::perf::tick("music bars");
                // nothing played for a moment: loopback delivers no packets at all
                if last_data.elapsed() > Duration::from_millis(250) {
                    ring.fill(0.0);
                }
                let mut sum_sq = 0.0f32;
                for i in 0..N {
                    let wv = ring[(pos + i) % N] * hann[i];
                    x[i] = wv;
                    sum_sq += wv * wv;
                }
                let rms = (sum_sq / N as f32).sqrt();
                if rms < 2e-4 {
                    if !zero_sent {
                        let mut any_visible = false;
                        for b in 0..BANDS {
                            peak[b] = (peak[b] * 0.996).max(0.5);
                            out[b] *= 0.78;
                            if out[b] >= 1e-3 {
                                any_visible = true;
                            } else {
                                out[b] = 0.0;
                            }
                        }
                        if any_visible {
                            emit(out);
                            crate::perf::count("bar frames", 1);
                        } else {
                            out = [0.0; BANDS];
                            emit(out);
                            crate::perf::count("bar frames", 1);
                            zero_sent = true;
                        }
                    }
                } else {
                    zero_sent = false;
                    for b in 0..BANDS {
                        let [c0, c1, c2] = coeffs[b];
                        let p = goertzel(&x, c0) + goertzel(&x, c1) + goertzel(&x, c2);
                        let level = (1.0 + p).ln();
                        // per-band automatic gain: quiet songs still move, loud ones don't clip
                        peak[b] = (peak[b] * 0.996).max(level).max(0.5);
                        let target = (level / peak[b]).clamp(0.0, 1.0).powf(1.6);
                        // fast up, slower down
                        out[b] = if target > out[b] { target } else { out[b] * 0.78 + target * 0.22 };
                    }
                    emit(out);
                    crate::perf::count("bar frames", 1);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = client.Stop();
        emit([0.0; BANDS]);
    }
    Ok(())
}
