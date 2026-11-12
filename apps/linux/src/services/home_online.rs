//! Pure request identity and refresh policy; no GTK, network, or runtime threads.
use std::time::{Duration, Instant};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request<K> {
    pub key: K,
    generation: u64,
    sequence: u64,
}
pub struct Latest<K> {
    key: Option<K>,
    generation: u64,
    sequence: u64,
    pending: Option<Request<K>>,
}
impl<K> Default for Latest<K> {
    fn default() -> Self {
        Self {
            key: None,
            generation: 0,
            sequence: 0,
            pending: None,
        }
    }
}
impl<K: Clone + Eq> Latest<K> {
    pub fn set(&mut self, key: Option<K>) -> bool {
        if self.key == key {
            return false;
        }
        self.key = key;
        self.generation = self.generation.wrapping_add(1);
        true
    }
    pub fn key(&self) -> Option<&K> {
        self.key.as_ref()
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn begin(&mut self) -> Option<Request<K>> {
        if self.busy() {
            return None;
        }
        self.sequence = self.sequence.wrapping_add(1);
        let request = Request {
            key: self.key.clone()?,
            generation: self.generation,
            sequence: self.sequence,
        };
        self.pending = Some(request.clone());
        Some(request)
    }
    pub fn finish(&mut self, request: &Request<K>) -> bool {
        if self.pending.as_ref() != Some(request) {
            return false;
        }
        self.pending = None;
        request.generation == self.generation && self.key.as_ref() == Some(&request.key)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackKey {
    pub bus_name: String,
    pub owner: String,
    pub track_id: Option<String>,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration: Option<Duration>,
}
#[derive(Default)]
pub struct WeatherSchedule {
    latest: Latest<String>,
    next: Option<Instant>,
    blocked_until: Option<Instant>,
}
impl WeatherSchedule {
    pub fn set_city(&mut self, city: &str) -> bool {
        let city = city.trim();
        let changed = self.latest.set((!city.is_empty()).then(|| city.to_owned()));
        if changed {
            self.next = None;
        }
        changed
    }
    pub fn defer(&mut self, now: Instant, delay: Duration) {
        let until = now + delay.min(Duration::from_secs(365 * 24 * 3600));
        self.blocked_until = Some(self.blocked_until.map_or(until, |old| old.max(until)));
    }
    pub fn city(&self) -> Option<&str> {
        self.latest.key().map(String::as_str)
    }
    pub fn busy(&self) -> bool {
        self.latest.busy()
    }
    pub fn begin(&mut self, now: Instant, manual: bool, smoke: bool) -> Option<Request<String>> {
        if smoke
            || self.blocked_until.is_some_and(|until| now < until)
            || (!manual && self.next.is_some_and(|next| now < next))
        {
            return None;
        }
        self.latest.begin()
    }
    pub fn finish(&mut self, request: &Request<String>, now: Instant, success: bool) -> bool {
        if !self.latest.finish(request) {
            return false;
        }
        self.next = Some(now + Duration::from_secs(if success { 20 * 60 } else { 5 * 60 }));
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changing_city_to_empty_opts_out_and_old_response_cannot_return() {
        let mut s = WeatherSchedule::default();
        let now = Instant::now();
        assert!(s.set_city(" First "));
        assert_eq!(s.city(), Some("First"));
        assert!(!s.busy());
        let old = s.begin(now, false, false).unwrap();
        assert!(s.set_city(""));
        assert!(s.begin(now, true, false).is_none());
        assert!(!s.finish(&old, now, true));
        assert!(s
            .begin(now + Duration::from_secs(3600), true, false)
            .is_none());
    }
    #[test]
    fn weather_has_one_inflight_periodic_interval_and_smoke_opt_out() {
        let mut s = WeatherSchedule::default();
        let now = Instant::now();
        s.set_city("City");
        assert!(s.begin(now, true, true).is_none());
        let request = s.begin(now, false, false).unwrap();
        assert!(s.begin(now, true, false).is_none());
        assert!(s.finish(&request, now, true));
        assert!(s
            .begin(now + Duration::from_secs(1199), false, false)
            .is_none());
        assert!(s
            .begin(now + Duration::from_secs(1200), false, false)
            .is_some());
    }
    #[test]
    fn returning_to_same_track_or_restarted_owner_does_not_revive_old_lyrics() {
        let mut s = Latest::default();
        s.set(Some("player@owner1/trackA".to_owned()));
        let a = s.begin().unwrap();
        s.set(Some("player@owner2/trackA".to_owned()));
        s.set(Some("player@owner1/trackA".to_owned()));
        assert!(s.begin().is_none());
        assert!(!s.finish(&a));
        let fresh = s.begin().unwrap();
        assert!(!s.finish(&a));
        assert!(s.busy());
        assert!(s.finish(&fresh));
    }
}

#[cfg(test)]
mod quota_tests {
    use super::*;
    #[test]
    fn provider_retry_delay_blocks_manual_refresh_and_city_switch() {
        let mut s = WeatherSchedule::default();
        let now = Instant::now();
        s.set_city("City");
        let request = s.begin(now, false, false).unwrap();
        s.finish(&request, now, false);
        s.defer(now, Duration::from_secs(3600));
        s.set_city("Other city");
        assert!(s
            .begin(now + Duration::from_secs(3599), true, false)
            .is_none());
        assert!(s
            .begin(now + Duration::from_secs(3600), false, false)
            .is_some());
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    #[test]
    fn exact_track_metadata_and_bus_owner_pin_every_result() {
        let original = TrackKey {
            bus_name: "org.mpris.MediaPlayer2.test".into(),
            owner: ":1.1".into(),
            track_id: Some("/track/1".into()),
            title: "Synthetic".into(),
            artists: vec!["Artist".into()],
            album: None,
            duration: Some(Duration::from_secs(120)),
        };
        for field in 0..7 {
            let mut state = Latest::default();
            state.set(Some(original.clone()));
            let request = state.begin().unwrap();
            let mut changed = original.clone();
            match field {
                0 => changed.bus_name.push_str(".other"),
                1 => changed.owner = ":1.2".into(),
                2 => changed.track_id = Some("/track/2".into()),
                3 => changed.title.push('2'),
                4 => changed.artists.push("Other".into()),
                5 => changed.album = Some("Album".into()),
                _ => changed.duration = None,
            }
            assert!(state.set(Some(changed)));
            assert!(!state.finish(&request));
        }
    }
}
