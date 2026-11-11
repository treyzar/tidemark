//! Beat detection for the "dancing edge". Captures what the PC plays
//! (WASAPI loopback, no microphone), keeps only the bass with a two-pole
//! low-pass filter and reports energy spikes against a moving average.
//! Runs only while music plays; dropping the handle stops the thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use windows::core::GUID;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_BEAT: u32 = WM_APP + 5;
const LOWPASS_HZ: f32 = 150.0;
const BLOCK_SECONDS: f32 = 0.02;
const MIN_GAP_SECONDS: f32 = 0.25;
const THRESHOLD: f32 = 1.6;
/// Blocks to learn the average level before reporting beats (~0.4 s).
const WARMUP_BLOCKS: usize = 20;
const POLL: Duration = Duration::from_millis(30);
const WAVE_FORMAT_IEEE_FLOAT: u32 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u32 = 0xFFFE;
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

/// Bass-energy onset detector over a mono sample stream.
pub struct Detector {
    alpha: f32,
    low1: f32,
    low2: f32,
    blocks_seen: usize,
    block_len: usize,
    block_fill: usize,
    block_energy: f32,
    average: f32,
    blocks_since_beat: usize,
    min_gap_blocks: usize,
}

impl Detector {
    pub fn new(sample_rate: u32) -> Self {
        let fs = sample_rate.max(8000) as f32;
        let block_len = (fs * BLOCK_SECONDS) as usize;
        Self {
            alpha: 1.0 - (-std::f32::consts::TAU * LOWPASS_HZ / fs).exp(),
            low1: 0.0,
            low2: 0.0,
            blocks_seen: 0,
            block_len,
            block_fill: 0,
            block_energy: 0.0,
            average: 0.0,
            blocks_since_beat: usize::MAX / 2,
            min_gap_blocks: (MIN_GAP_SECONDS / BLOCK_SECONDS) as usize,
        }
    }

    /// Feeds one sample; returns a beat strength in 0..1 on an onset.
    pub fn push(&mut self, sample: f32) -> Option<f32> {
        // Two cascaded one-pole filters: -12 dB/octave above the bass.
        self.low1 += self.alpha * (sample - self.low1);
        self.low2 += self.alpha * (self.low1 - self.low2);
        self.block_energy += self.low2 * self.low2;
        self.block_fill += 1;
        if self.block_fill < self.block_len {
            return None;
        }
        let energy = self.block_energy / self.block_len as f32;
        self.block_energy = 0.0;
        self.block_fill = 0;
        self.blocks_since_beat = self.blocks_since_beat.saturating_add(1);
        self.blocks_seen += 1;
        if self.blocks_seen <= WARMUP_BLOCKS {
            // Learn the level first: start the average at the mean so far.
            self.average += (energy - self.average) / self.blocks_seen as f32;
            return None;
        }

        let beat = self.average > 1e-7
            && energy > self.average * THRESHOLD
            && energy > 1e-6
            && self.blocks_since_beat >= self.min_gap_blocks;
        // ~0.7 s memory for the moving average.
        self.average += (energy - self.average) * 0.03;
        if beat {
            self.blocks_since_beat = 0;
            Some(((energy / self.average.max(1e-9) - 1.0) / 3.0).clamp(0.0, 1.0))
        } else {
            None
        }
    }
}

pub struct Capture {
    stop: Arc<AtomicBool>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Capture {
    pub fn start(hwnd: HWND) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let hwnd_raw = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("beat".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let _ = run(HWND(hwnd_raw as *mut _), &flag);
            })
            .ok()?;
        Some(Self { stop })
    }
}

fn run(hwnd: HWND, stop: &AtomicBool) -> windows::core::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let fmt = client.GetMixFormat()?;
        let header = std::ptr::read_unaligned(fmt);
        let channels = header.nChannels.max(1) as usize;
        let rate = header.nSamplesPerSec;
        let bits = header.wBitsPerSample;
        let tag = header.wFormatTag as u32;
        let is_float = tag == WAVE_FORMAT_IEEE_FLOAT
            || (tag == WAVE_FORMAT_EXTENSIBLE && { std::ptr::read_unaligned(std::ptr::addr_of!((*(fmt as *const WAVEFORMATEXTENSIBLE)).SubFormat)) } == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT);
        let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 10_000_000, 0, fmt, None);
        CoTaskMemFree(Some(fmt as *const _));
        init?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;

        let mut detector = Detector::new(rate);
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(POLL);
            while capture.GetNextPacketSize()? > 0 {
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                let n = frames as usize * channels;
                let mut strongest: Option<f32> = None;
                if !silent && !data.is_null() {
                    let mut feed = |mono: f32| {
                        if let Some(s) = detector.push(mono) {
                            strongest = Some(strongest.map_or(s, |p: f32| p.max(s)));
                        }
                    };
                    if is_float && bits == 32 {
                        let samples = std::slice::from_raw_parts(data as *const f32, n);
                        for frame in samples.chunks_exact(channels) {
                            feed(frame.iter().sum::<f32>() / channels as f32);
                        }
                    } else if bits == 16 {
                        let samples = std::slice::from_raw_parts(data as *const i16, n);
                        for frame in samples.chunks_exact(channels) {
                            feed(frame.iter().map(|&v| v as f32 / 32768.0).sum::<f32>() / channels as f32);
                        }
                    }
                }
                capture.ReleaseBuffer(frames)?;
                if let Some(s) = strongest {
                    let _ = PostMessageW(Some(hwnd), WM_BEAT, WPARAM((s * 1000.0) as usize), LPARAM(0));
                }
            }
        }
        let _ = client.Stop();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Quiet bass hum with a loud 60 Hz kick every 0.5 s.
    fn kicks(rate: u32, seconds: f32) -> Vec<f32> {
        let n = (rate as f32 * seconds) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / rate as f32;
                let since_kick = t % 0.5;
                let env = if since_kick < 0.08 { 1.0 - since_kick / 0.08 } else { 0.0 };
                let hum = 0.05 * (std::f32::consts::TAU * 80.0 * t).sin();
                hum + 0.8 * env * (std::f32::consts::TAU * 60.0 * t).sin()
            })
            .collect()
    }

    #[test]
    fn finds_regular_kicks() {
        let mut d = Detector::new(48_000);
        let beats = kicks(48_000, 4.0).into_iter().filter_map(|s| d.push(s)).count();
        // 8 kicks in 4 s; the first may be missed while the average warms up.
        assert!((6..=8).contains(&beats), "found {beats} beats");
    }

    #[test]
    fn ignores_steady_tone_and_silence() {
        let mut d = Detector::new(48_000);
        let tone = (0..48_000 * 3).map(|i| 0.5 * (std::f32::consts::TAU * 60.0 * i as f32 / 48_000.0).sin());
        assert_eq!(tone.filter_map(|s| d.push(s)).count(), 0);
        let mut d = Detector::new(48_000);
        assert_eq!((0..48_000).filter_map(|_| d.push(0.0)).count(), 0);
    }

    #[test]
    fn treble_does_not_trigger() {
        // Hi-hat-like 5 kHz bursts: filtered out by the bass low-pass.
        let mut d = Detector::new(48_000);
        let hats = (0..48_000 * 3).map(|i| {
            let t = i as f32 / 48_000.0;
            let env = if t % 0.25 < 0.02 { 1.0 } else { 0.0 };
            0.02 * (std::f32::consts::TAU * 80.0 * t).sin() + 0.8 * env * (std::f32::consts::TAU * 5000.0 * t).sin()
        });
        assert!(hats.filter_map(|s| d.push(s)).count() <= 1);
    }
}
