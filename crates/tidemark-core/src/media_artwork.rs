//! Short, bounded retries for providers that publish art after track metadata.
use std::time::{Duration, Instant};

const RETRIES: [u64; 4] = [250, 500, 1000, 2000];

#[derive(Default)]
pub struct ArtworkSchedule {
    key: Option<u64>,
    invalidated: bool,
    retry: usize,
    due: Option<Instant>,
}
impl ArtworkSchedule {
    pub fn needs_read(&self,key:u64,now:Instant)->bool {
        self.key!=Some(key)||self.invalidated||self.due.is_some_and(|due|due<=now)
    }
    pub fn read(&mut self,key:u64,now:Instant) {
        if self.key!=Some(key)||self.invalidated {
            self.key=Some(key);self.retry=0;self.due=Some(now+Duration::from_millis(RETRIES[0]));
        } else if self.due.is_some_and(|due|due<=now) {
            self.retry+=1;self.due=RETRIES.get(self.retry).map(|ms|now+Duration::from_millis(*ms));
        }
        self.invalidated=false;
    }
    pub fn invalidate(&mut self) {self.invalidated=true;}
    // A temporarily unavailable provider must not leave a zero-timeout retry loop.
    pub fn finish_attempt(&mut self,now:Instant) {
        if self.due.is_some_and(|due|due<=now) {
            if let Some(key)=self.key {self.read(key,now);}
        }
    }
    pub fn wait(&self,now:Instant)->Option<Duration> {self.due.map(|due|due.saturating_duration_since(now))}
    pub fn reset(&mut self) {*self=Self::default();}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_art_is_rechecked_and_eventually_goes_idle() {
        let mut s=ArtworkSchedule::default();let mut now=Instant::now();
        assert!(s.needs_read(1,now));s.read(1,now);assert!(!s.needs_read(1,now));
        for delay in RETRIES {assert_eq!(s.wait(now),Some(Duration::from_millis(delay)));now+=Duration::from_millis(delay);assert!(s.needs_read(1,now));s.read(1,now);}
        assert!(s.wait(now).is_none());assert!(!s.needs_read(1,now+Duration::from_secs(60)));
    }
    #[test]
    fn same_track_metadata_event_reloads_cached_cover() {
        let mut s=ArtworkSchedule::default();let now=Instant::now();s.read(7,now);
        assert!(!s.needs_read(7,now));s.invalidate();assert!(s.needs_read(7,now));s.read(7,now);assert!(!s.needs_read(7,now));
    }
    #[test]
    fn different_session_or_track_cannot_reuse_old_art() {
        let mut s=ArtworkSchedule::default();let now=Instant::now();s.read(1,now);assert!(s.needs_read(2,now));s.read(2,now);s.reset();assert!(s.needs_read(2,now));
    }
    #[test]
    fn metadata_event_rearms_retries_after_original_window_expired() {
        let mut s=ArtworkSchedule::default();let mut now=Instant::now();s.read(1,now);
        for delay in RETRIES {now+=Duration::from_millis(delay);s.read(1,now);}
        assert!(s.wait(now).is_none());s.invalidate();s.read(1,now);assert_eq!(s.wait(now),Some(Duration::from_millis(250)));
    }
    #[test]
    fn unavailable_provider_consumes_due_retries_without_busy_loop() {
        let mut s=ArtworkSchedule::default();let mut now=Instant::now();s.read(1,now);
        for delay in RETRIES {
            now+=Duration::from_millis(delay);s.finish_attempt(now);
            assert!(s.wait(now).is_none_or(|wait|wait>Duration::ZERO));
        }
        assert!(s.wait(now).is_none());
    }
}
