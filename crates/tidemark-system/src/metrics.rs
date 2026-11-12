use crate::{Error, Result};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuSample {
    total: u64,
    idle: u64,
}
impl CpuSample {
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() > 1 << 20 {
            return Err(Error::Limit);
        }
        let mut fields = text
            .lines()
            .next()
            .ok_or(Error::InvalidResponse)?
            .split_whitespace();
        if fields.next() != Some("cpu") {
            return Err(Error::InvalidResponse);
        }
        let values: Vec<u64> = fields
            .take(10)
            .map(|s| s.parse().map_err(|_| Error::InvalidResponse))
            .collect::<Result<_>>()?;
        if values.len() < 4 {
            return Err(Error::InvalidResponse);
        }
        let total = values.iter().take(8).try_fold(0u64, |sum, &n| {
            sum.checked_add(n).ok_or(Error::InvalidResponse)
        })?;
        let idle = values[3]
            .checked_add(values.get(4).copied().unwrap_or(0))
            .ok_or(Error::InvalidResponse)?;
        Ok(Self { total, idle })
    }
    pub fn utilization(self, next: Self) -> Result<f64> {
        let total = next.total.checked_sub(self.total).ok_or(Error::Changed)?;
        let idle = next.idle.checked_sub(self.idle).ok_or(Error::Changed)?;
        if total == 0 {
            return Err(Error::NotReady);
        }
        let active = total.checked_sub(idle).ok_or(Error::InvalidResponse)?;
        Ok(active as f64 * 100.0 / total as f64)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Memory {
    pub total_bytes: u64,
    pub available_bytes: u64,
}
impl Memory {
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() > 1 << 20 {
            return Err(Error::Limit);
        }
        let (mut total, mut available) = (None, None);
        for line in text.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let slot = match key {
                "MemTotal" => &mut total,
                "MemAvailable" => &mut available,
                _ => continue,
            };
            if slot.is_some() {
                return Err(Error::InvalidResponse);
            }
            let fields: Vec<_> = value.split_whitespace().collect();
            if fields.len() != 2 || fields[1] != "kB" {
                return Err(Error::InvalidResponse);
            }
            *slot = Some(
                fields[0]
                    .parse::<u64>()
                    .ok()
                    .and_then(|n| n.checked_mul(1024))
                    .ok_or(Error::InvalidResponse)?,
            );
        }
        let (total_bytes, available_bytes) = (
            total.ok_or(Error::Unsupported)?,
            available.ok_or(Error::Unsupported)?,
        );
        if total_bytes == 0 || available_bytes > total_bytes {
            return Err(Error::InvalidResponse);
        }
        Ok(Self {
            total_bytes,
            available_bytes,
        })
    }
    pub fn used_bytes(self) -> u64 {
        self.total_bytes - self.available_bytes
    }
}
#[cfg(target_os = "linux")]
pub fn cpu() -> Result<CpuSample> {
    CpuSample::parse(&crate::process::read_text(
        std::path::Path::new("/proc/stat"),
        1 << 20,
    )?)
}
#[cfg(target_os = "linux")]
pub fn memory() -> Result<Memory> {
    Memory::parse(&crate::process::read_text(
        std::path::Path::new("/proc/meminfo"),
        1 << 20,
    )?)
}
// Portable parsers; no host reads in tests.
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_excludes_double_counted_guest_and_detects_reset() {
        let first = CpuSample::parse("cpu 100 0 50 800 20 10 10 10 80 0\ncpu0 0").unwrap();
        let next = CpuSample::parse("cpu 120 0 60 860 30 10 10 10 90 0").unwrap();
        assert!((first.utilization(next).unwrap() - 30.0).abs() < 0.01);
        assert!(next.utilization(first).is_err());
        assert!(first.utilization(first).is_err());
    }
    #[test]
    fn memory_never_synthesizes_missing_available_or_accepts_overflow() {
        let memory =
            Memory::parse("MemTotal: 1000 kB\nMemFree: 100 kB\nMemAvailable: 250 kB\n").unwrap();
        assert_eq!(memory.used_bytes(), 750 * 1024);
        assert!(Memory::parse("MemTotal: 1000 kB").is_err());
        assert!(Memory::parse("MemTotal: 18446744073709551615 kB\nMemAvailable: 2 kB").is_err());
    }
}
