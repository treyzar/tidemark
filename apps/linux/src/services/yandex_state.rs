//! Pure request identity for explicit Yandex favorite operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackKey {
    pub player_bus: String,
    pub player_owner: String,
    pub track_id: String,
    pub title: String,
    pub artists: Vec<String>,
}
impl TrackKey {
    pub fn valid(&self) -> bool {
        self.player_bus.starts_with("org.mpris.MediaPlayer2.")
            && self.player_owner.starts_with(':')
            && self.track_id.starts_with('/')
            && self.track_id.len() > 1
            && self.track_id != "/org/mpris/MediaPlayer2/TrackList/NoTrack"
            && !self.title.trim().is_empty()
            && !self.artists.is_empty()
            && self.artists.len() <= 32
            && self.artists.iter().all(|s| !s.trim().is_empty())
            && self.title.len() <= 8192
            && self.artists.iter().map(String::len).sum::<usize>() <= 8192
            && self.track_id.len() <= 1024
            && self.player_bus.len() <= 255
            && self.player_owner.len() <= 255
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub epoch: u64,
    pub key: TrackKey,
    pub want: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartError {
    Busy,
    NoTrack,
    InvalidMetadata,
    Inactive,
    WorkerUnavailable,
}
impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Busy => "Предыдущая операция с лайком ещё выполняется.",
            Self::NoTrack => "Нет текущего трека.",
            Self::InvalidMetadata => {
                "Не удалось подтвердить идентификатор, название и исполнителя трека."
            }
            Self::Inactive => "Управление лайками не запущено.",
            Self::WorkerUnavailable => "Не удалось запустить управление лайками.",
        })
    }
}
#[derive(Default)]
pub struct State {
    current: Option<TrackKey>,
    epoch: u64,
    next: u64,
    pending: Option<Request>,
}
impl State {
    pub fn observe(&mut self, key: Option<TrackKey>) {
        if self.current != key {
            self.epoch = self.epoch.wrapping_add(1);
            self.current = key;
        }
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn begin(&mut self, want: bool) -> Result<Request, StartError> {
        if self.busy() {
            return Err(StartError::Busy);
        }
        let key = self.current.clone().ok_or(StartError::NoTrack)?;
        if !key.valid() {
            return Err(StartError::InvalidMetadata);
        }
        self.next = self.next.wrapping_add(1);
        let request = Request {
            id: self.next,
            epoch: self.epoch,
            key,
            want,
        };
        self.pending = Some(request.clone());
        Ok(request)
    }
    pub fn complete(&mut self, request: &Request) -> bool {
        if self.pending.as_ref() != Some(request) {
            return false;
        }
        self.pending = None;
        self.current.as_ref() == Some(&request.key) && self.epoch == request.epoch
    }
    pub fn pending(&self) -> Option<&Request> {
        self.pending.as_ref()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn key() -> TrackKey {
        TrackKey {
            player_bus: "org.mpris.MediaPlayer2.test".into(),
            player_owner: ":1.5".into(),
            track_id: "/track/1".into(),
            title: "Song".into(),
            artists: vec!["Artist".into()],
        }
    }
    #[test]
    fn changed_track_and_aba_cannot_accept_an_old_result() {
        let a = key();
        let mut b = a.clone();
        b.track_id = "/track/2".into();
        let mut state = State::default();
        state.observe(Some(a.clone()));
        let old = state.begin(true).unwrap();
        state.observe(Some(b));
        state.observe(Some(a));
        assert!(!state.complete(&old));
        assert!(!state.busy());
    }
    #[test]
    fn explicit_desired_state_and_single_inflight() {
        let mut state = State::default();
        state.observe(Some(key()));
        let request = state.begin(false).unwrap();
        assert!(!request.want);
        assert_eq!(state.begin(true), Err(StartError::Busy));
        assert!(state.complete(&request));
        assert!(!state.busy());
        let next = state.begin(true).unwrap();
        assert!(next.want);
        assert_ne!(next.id, request.id);
        assert!(!state.complete(&request));
        assert!(state.busy());
        assert!(state.complete(&next));
    }
    #[test]
    fn owner_and_metadata_are_part_of_identity() {
        let mut state = State::default();
        let a = key();
        state.observe(Some(a.clone()));
        let old = state.begin(true).unwrap();
        let mut restarted = a;
        restarted.player_owner = ":1.6".into();
        state.observe(Some(restarted));
        assert!(!state.complete(&old));
    }
    #[test]
    fn missing_or_unverifiable_metadata_never_starts() {
        let mut state = State::default();
        assert_eq!(state.begin(true), Err(StartError::NoTrack));
        let mut invalid = key();
        invalid.track_id.clear();
        state.observe(Some(invalid));
        assert_eq!(state.begin(true), Err(StartError::InvalidMetadata));
        assert!(!state.busy());
    }
}

#[cfg(test)]
mod identity_regressions {
    use super::*;
    fn key() -> TrackKey {
        TrackKey {
            player_bus: "org.mpris.MediaPlayer2.test".into(),
            player_owner: ":1.5".into(),
            track_id: "/track/1".into(),
            title: "Song".into(),
            artists: vec!["Artist".into()],
        }
    }
    #[test]
    fn repeated_identical_music_updates_do_not_invalidate_request() {
        let mut s = State::default();
        let k = key();
        s.observe(Some(k.clone()));
        let request = s.begin(true).unwrap();
        s.observe(Some(k));
        assert!(s.complete(&request));
    }
    #[test]
    fn stopped_playback_and_returning_same_track_is_an_aba() {
        let mut s = State::default();
        let k = key();
        s.observe(Some(k.clone()));
        let request = s.begin(true).unwrap();
        s.observe(None);
        s.observe(Some(k));
        assert!(!s.complete(&request));
        assert!(!s.busy());
    }
    #[test]
    fn same_id_with_changed_metadata_is_stale() {
        for artist in [false, true] {
            let mut s = State::default();
            let mut k = key();
            s.observe(Some(k.clone()));
            let request = s.begin(true).unwrap();
            if artist {
                k.artists = vec!["Other".into()];
            } else {
                k.title = "Other".into();
            }
            s.observe(Some(k));
            assert!(!s.complete(&request));
        }
    }
}
