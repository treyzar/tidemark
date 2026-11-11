//! Screen recording with system sound, on demand: Windows.Graphics.Capture
//! for the monitor (cursor included; the island excludes itself from
//! capture), WASAPI loopback for what the PC plays, Media Foundation's
//! sink writer for an H.264 + AAC .mp4 in the Videos folder.
//!
//! Everything (a hardware D3D device, the encoder, buffers) lives on the
//! recording thread and is gone when it ends.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::{factory, Interface, GUID, HSTRING};
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{MonitorFromPoint, HMONITOR, MONITOR_DEFAULTTONEAREST};
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, PostMessageW, WM_APP};

/// Posted when a recording ends; `take_result` has the file (or None).
pub const WM_RECORDED: u32 = WM_APP + 19;
const FPS: u64 = 30;
const TICKS_PER_SECOND: u64 = 10_000_000;
const FRAME_TICKS: u64 = TICKS_PER_SECOND / FPS;
/// A frame is written at least this often even when nothing moves.
const KEYFRAME_EVERY: Duration = Duration::from_millis(500);
const MF_VERSION: u32 = 0x0002_0070;
const WAVE_FORMAT_IEEE_FLOAT: u32 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u32 = 0xFFFE;
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

pub struct Recording {
    stop: Arc<AtomicBool>,
    pub started: Instant,
}

impl Recording {
    /// Starts recording the monitor under the cursor into `path`.
    pub fn start(hwnd: HWND, path: String) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let hwnd_raw = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("record".into())
            .stack_size(512 * 1024)
            .spawn(move || {
                let ok = run(&path, &flag).is_ok();
                if let Ok(mut slot) = RESULT.lock() {
                    *slot = Some(ok.then_some(path));
                }
                unsafe {
                    let _ = PostMessageW(Some(HWND(hwnd_raw as *mut _)), WM_RECORDED, WPARAM(0), LPARAM(0));
                }
            })
            .ok()?;
        Some(Self { stop, started: Instant::now() })
    }

    /// Asks the thread to finish the file; WM_RECORDED follows.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

static RESULT: std::sync::Mutex<Option<Option<String>>> = std::sync::Mutex::new(None);

/// The finished file, or None if recording failed.
pub fn take_result() -> Option<String> {
    RESULT.lock().ok().and_then(|mut s| s.take()).flatten()
}

/// "Запись экрана 2026-11-11 10-42-05.mp4" in the Videos folder.
pub fn new_path() -> String {
    let dir = unsafe {
        windows::Win32::UI::Shell::SHGetKnownFolderPath(&windows::Win32::UI::Shell::FOLDERID_Videos, windows::Win32::UI::Shell::KNOWN_FOLDER_FLAG(0), None)
            .ok()
            .map(|p| {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as *const _));
                s
            })
    }
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| std::env::var("USERPROFILE").map(|h| format!("{h}\\Videos")).unwrap_or_default());
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{dir}\\Запись экрана {:04}-{:02}-{:02} {:02}-{:02}-{:02}.mp4",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

/// "1:05" for a running recording.
pub fn elapsed_label(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn cursor_monitor() -> HMONITOR {
    unsafe {
        let mut pt = Default::default();
        let _ = GetCursorPos(&mut pt);
        MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
    }
}

/// Size the encoder gets: even, and at most 4096 wide (H.264 levels).
pub fn video_size(w: i32, h: i32) -> (u32, u32) {
    let (mut w, mut h) = (w.max(2) as u32, h.max(2) as u32);
    if w > 4096 {
        h = h * 4096 / w;
        w = 4096;
    }
    (w & !1, h & !1)
}

fn bitrate(w: u32, h: u32) -> u32 {
    // ~0.12 bits per pixel per frame at 30 fps: 1080p ≈ 7.5 Mbit/s.
    ((w as u64 * h as u64 * FPS * 12 / 100) as u32).clamp(2_000_000, 25_000_000)
}

struct Audio {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    channels: usize,
    rate: u32,
    is_float: bool,
    bits: u16,
    /// Stereo frames handed to the writer so far.
    written: u64,
}

fn open_audio() -> windows::core::Result<Audio> {
    unsafe {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let fmt = client.GetMixFormat()?;
        let header = std::ptr::read_unaligned(fmt);
        let tag = header.wFormatTag as u32;
        let is_float = tag == WAVE_FORMAT_IEEE_FLOAT
            || (tag == WAVE_FORMAT_EXTENSIBLE && { std::ptr::read_unaligned(std::ptr::addr_of!((*(fmt as *const WAVEFORMATEXTENSIBLE)).SubFormat)) } == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT);
        let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 2_000_000, 0, fmt, None);
        CoTaskMemFree(Some(fmt as *const _));
        init?;
        let capture: IAudioCaptureClient = client.GetService()?;
        Ok(Audio { client, capture, channels: header.nChannels.max(1) as usize, rate: header.nSamplesPerSec, is_float, bits: header.wBitsPerSample, written: 0 })
    }
}

/// Interleaved stereo i16 from a loopback packet.
pub fn to_stereo_i16(data: &[u8], channels: usize, is_float: bool, bits: u16) -> Vec<i16> {
    let channels = channels.max(1);
    let sample = |frame: &[u8], ch: usize| -> i16 {
        let ch = ch.min(channels - 1);
        if is_float && bits == 32 {
            let o = ch * 4;
            let v = f32::from_le_bytes([frame[o], frame[o + 1], frame[o + 2], frame[o + 3]]);
            (v.clamp(-1.0, 1.0) * 32767.0) as i16
        } else if bits == 16 {
            let o = ch * 2;
            i16::from_le_bytes([frame[o], frame[o + 1]])
        } else {
            0
        }
    };
    let frame_bytes = channels * (bits as usize / 8).max(1);
    let mut out = Vec::with_capacity(data.len() / frame_bytes * 2);
    for frame in data.chunks_exact(frame_bytes) {
        out.push(sample(frame, 0));
        out.push(sample(frame, 1));
    }
    out
}

unsafe fn media_type(major: &GUID, sub: &GUID) -> windows::core::Result<IMFMediaType> {
    let t = MFCreateMediaType()?;
    t.SetGUID(&MF_MT_MAJOR_TYPE, major)?;
    t.SetGUID(&MF_MT_SUBTYPE, sub)?;
    Ok(t)
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

unsafe fn write_bytes(writer: &IMFSinkWriter, stream: u32, bytes: &[u8], time: u64, duration: u64) -> windows::core::Result<()> {
    let buffer = MFCreateMemoryBuffer(bytes.len() as u32)?;
    let mut ptr = std::ptr::null_mut();
    buffer.Lock(&mut ptr, None, None)?;
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
    buffer.Unlock()?;
    buffer.SetCurrentLength(bytes.len() as u32)?;
    let sample = MFCreateSample()?;
    sample.AddBuffer(&buffer)?;
    sample.SetSampleTime(time as i64)?;
    sample.SetSampleDuration(duration as i64)?;
    writer.WriteSample(stream, &sample)
}

fn run(path: &str, stop: &AtomicBool) -> windows::core::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_LITE)?;
        let result = record(path, stop);
        let _ = MFShutdown();
        if result.is_err() {
            let _ = std::fs::remove_file(path);
        }
        result
    }
}

unsafe fn record(path: &str, stop: &AtomicBool) -> windows::core::Result<()> {
    // Hardware device: the capture hands out GPU textures.
    let mut device = None;
    let mut context = None;
    D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?;
    let (device, context): (ID3D11Device, ID3D11DeviceContext) = (device.unwrap(), context.unwrap());
    let dxgi: IDXGIDevice = device.cast()?;
    let winrt_device: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;

    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    let item: GraphicsCaptureItem = interop.CreateForMonitor(cursor_monitor())?;
    let size = item.Size()?;
    let (w, h) = video_size(size.Width, size.Height);
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)?;
    let session = pool.CreateCaptureSession(&item)?;
    let _ = session.SetIsCursorCaptureEnabled(true);
    let _ = session.SetIsBorderRequired(false);

    let staging = {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut t = None;
        device.CreateTexture2D(&desc, None, Some(&mut t))?;
        t.unwrap()
    };

    // Encoder: H.264 video, AAC audio when the mix format allows it.
    let attrs = {
        let mut a = None;
        MFCreateAttributes(&mut a, 2)?;
        let a = a.unwrap();
        a.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        a.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
        a
    };
    let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path), None, &attrs)?;
    let out = media_type(&MFMediaType_Video, &MFVideoFormat_H264)?;
    out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate(w, h))?;
    out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    out.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
    out.SetUINT64(&MF_MT_FRAME_RATE, pack(FPS as u32, 1))?;
    out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
    let video = writer.AddStream(&out)?;
    let input = media_type(&MFMediaType_Video, &MFVideoFormat_RGB32)?;
    input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    input.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
    input.SetUINT64(&MF_MT_FRAME_RATE, pack(FPS as u32, 1))?;
    input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
    // Top-down rows, as the GPU gives them.
    input.SetUINT32(&MF_MT_DEFAULT_STRIDE, w * 4)?;
    writer.SetInputMediaType(video, &input, None)?;

    let mut audio = open_audio().ok().filter(|a| a.rate == 44_100 || a.rate == 48_000);
    let mut audio_stream = None;
    if let Some(a) = &audio {
        let out = media_type(&MFMediaType_Audio, &MFAudioFormat_AAC)?;
        out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, a.rate)?;
        out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)?;
        let input = media_type(&MFMediaType_Audio, &MFAudioFormat_PCM)?;
        input.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, a.rate)?;
        input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
        input.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, a.rate * 4)?;
        let ok = writer.AddStream(&out).and_then(|s| writer.SetInputMediaType(s, &input, None).map(|_| s));
        match ok {
            Ok(s) => audio_stream = Some(s),
            Err(_) => audio = None,
        }
    }
    writer.BeginWriting()?;
    if let Some(a) = &audio {
        a.client.Start()?;
    }
    session.StartCapture()?;

    let start = Instant::now();
    let mut frame_buf = vec![0u8; (w * h * 4) as usize];
    let mut have_frame = false;
    let mut last_written: Option<Instant> = None;
    let mut next_tick = start;
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        if next_tick > now {
            std::thread::sleep(next_tick - now);
        }
        next_tick += Duration::from_nanos(1_000_000_000 / FPS);
        let elapsed = start.elapsed();
        let time = elapsed.as_nanos() as u64 / 100;

        // Newest frame only; older ones are dropped.
        let mut newest = None;
        while let Ok(f) = pool.TryGetNextFrame() {
            newest = Some(f);
        }
        let fresh = newest.is_some();
        if let Some(frame) = newest {
            let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
            let tex: ID3D11Texture2D = access.GetInterface()?;
            let region = D3D11_BOX { left: 0, top: 0, front: 0, right: w, bottom: h, back: 1 };
            context.CopySubresourceRegion(&staging, 0, 0, 0, 0, &tex, 0, Some(&region));
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let row = (w * 4) as usize;
            for y in 0..h as usize {
                let src = std::slice::from_raw_parts((mapped.pData as *const u8).add(y * mapped.RowPitch as usize), row);
                frame_buf[y * row..(y + 1) * row].copy_from_slice(src);
            }
            context.Unmap(&staging, 0);
            have_frame = true;
        }
        if have_frame && (fresh || last_written.is_none_or(|t| t.elapsed() >= KEYFRAME_EVERY)) {
            write_bytes(&writer, video, &frame_buf, time, FRAME_TICKS)?;
            last_written = Some(Instant::now());
        }

        if let (Some(a), Some(stream)) = (audio.as_mut(), audio_stream) {
            let mut pcm: Vec<i16> = Vec::new();
            while a.capture.GetNextPacketSize()? > 0 {
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                a.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let bytes = frames as usize * a.channels * (a.bits as usize / 8);
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    pcm.extend(std::iter::repeat_n(0i16, frames as usize * 2));
                } else {
                    pcm.extend(to_stereo_i16(std::slice::from_raw_parts(data, bytes), a.channels, a.is_float, a.bits));
                }
                a.capture.ReleaseBuffer(frames)?;
            }
            // Loopback sends nothing while the PC is silent: fill the gap so
            // sound stays in sync with the picture.
            let due = elapsed.as_micros() as u64 * a.rate as u64 / 1_000_000;
            let have = a.written + pcm.len() as u64 / 2;
            let lag = a.rate as u64 / 10;
            if pcm.is_empty() && due > have + lag {
                pcm.extend(std::iter::repeat_n(0i16, ((due - have) * 2) as usize));
            }
            if !pcm.is_empty() {
                let frames = pcm.len() as u64 / 2;
                let t = a.written * TICKS_PER_SECOND / a.rate as u64;
                let d = frames * TICKS_PER_SECOND / a.rate as u64;
                let bytes = std::slice::from_raw_parts(pcm.as_ptr() as *const u8, pcm.len() * 2);
                write_bytes(&writer, stream, bytes, t, d)?;
                a.written += frames;
            }
        }
    }
    let _ = session.Close();
    let _ = pool.Close();
    if let Some(a) = &audio {
        let _ = a.client.Stop();
    }
    writer.Finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_even_and_bounded() {
        assert_eq!(video_size(1920, 1080), (1920, 1080));
        assert_eq!(video_size(1367, 769), (1366, 768));
        assert_eq!(video_size(5120, 1440), (4096, 1152));
    }

    #[test]
    fn labels() {
        assert_eq!(elapsed_label(Duration::from_secs(65)), "1:05");
        assert_eq!(elapsed_label(Duration::from_secs(3725)), "1:02:05");
    }

    #[test]
    fn stereo_from_float_and_mono() {
        let mut data = Vec::new();
        for v in [0.5f32, -0.5, 1.0, 2.0] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(to_stereo_i16(&data, 2, true, 32), vec![16383, -16383, 32767, 32767]);
        let mono: Vec<u8> = [100i16, -7].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(to_stereo_i16(&mono, 1, false, 16), vec![100, 100, -7, -7]);
    }

    /// Live: records 4 s of the screen; `cargo test record::tests::live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live() {
        let path = std::env::temp_dir().join("win-island-rec-test.mp4").to_string_lossy().into_owned();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let p = path.clone();
        let t = std::thread::spawn(move || run(&p, &flag));
        std::thread::sleep(Duration::from_secs(4));
        stop.store(true, Ordering::Relaxed);
        println!("{:?} -> {path}", t.join().unwrap());
    }
}
