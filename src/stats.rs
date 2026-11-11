//! Optional animation stats for tuning. Enabled with WIN_ISLAND_STATS=1;
//! each finished animation appends one line to %TEMP%\win-island-stats.log.

use std::io::Write;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

fn process_cpu() -> Duration {
    let (mut a, mut b, mut kernel, mut user) = Default::default();
    unsafe {
        let _ = GetProcessTimes(GetCurrentProcess(), &mut a, &mut b, &mut kernel, &mut user);
    }
    let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
    Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
}

pub struct Stats {
    enabled: bool,
    run: Option<Run>,
}

struct Run {
    started: Instant,
    cpu_at_start: Duration,
    frames: u32,
    max_frame: Duration,
    cache_rebuilds: u32,
}

impl Stats {
    pub fn new() -> Self {
        Self { enabled: std::env::var_os("WIN_ISLAND_STATS").is_some_and(|v| v == "1"), run: None }
    }

    /// Records one frame; `took` is the wall time of draw + present.
    pub fn frame(&mut self, took: Duration, rebuilt_cache: bool) {
        if !self.enabled {
            return;
        }
        let run = self.run.get_or_insert_with(|| Run {
            started: Instant::now(),
            cpu_at_start: process_cpu(),
            frames: 0,
            max_frame: Duration::ZERO,
            cache_rebuilds: 0,
        });
        run.frames += 1;
        run.max_frame = run.max_frame.max(took);
        run.cache_rebuilds += rebuilt_cache as u32;
    }

    /// Called when the island settles; writes the finished run.
    pub fn settled(&mut self, label: &str) {
        let Some(run) = self.run.take() else { return };
        let cpu = process_cpu().saturating_sub(run.cpu_at_start);
        let line = format!(
            "{label}: frames={} wall={}ms cpu={}ms max_frame={}ms cache_rebuilds={}\n",
            run.frames,
            run.started.elapsed().as_millis(),
            cpu.as_millis(),
            run.max_frame.as_millis(),
            run.cache_rebuilds,
        );
        let path = std::env::temp_dir().join("win-island-stats.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// Appends a free-form line when stats are enabled (WIN_ISLAND_STATS=1).
pub fn log_line(line: &str) {
    if !std::env::var_os("WIN_ISLAND_STATS").is_some_and(|v| v == "1") {
        return;
    }
    let path = std::env::temp_dir().join("win-island-stats.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{:?} {line}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() % 100_000);
    }
}
