//! Time-based UI transitions. Retargeting preserves continuity; idle stops.
#[derive(Clone, Copy, Debug)]
pub struct Tween {
    from: f32,
    to: f32,
    start: f32,
    duration: f32,
}
impl Tween {
    pub fn new(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            start: 0.0,
            duration: 0.0,
        }
    }
    pub fn value(&self, now: f32) -> f32 {
        if self.duration <= 0.0 {
            return self.to;
        }
        let t = ((now - self.start) / self.duration).clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        self.from + (self.to - self.from) * eased
    }
    pub fn running(&self, now: f32) -> bool {
        self.from != self.to && now < self.start + self.duration
    }
    pub fn target(&mut self, to: f32, now: f32, duration: f32) {
        self.from = self.value(now);
        self.to = to;
        self.start = now;
        self.duration = duration.max(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn motion_is_monotonic_finishes_and_goes_idle() {
        let mut t = Tween::new(0.0);
        t.target(1.0, 10.0, 0.16);
        assert_eq!(t.value(10.0), 0.0);
        assert!(t.running(10.08));
        let mut previous = 0.0;
        for frame in 0..=20 {
            let v = t.value(10.0 + frame as f32 * 0.01);
            assert!(v >= previous && v <= 1.0);
            previous = v;
        }
        assert_eq!(t.value(10.2), 1.0);
        assert!(!t.running(10.2));
    }
    #[test]
    fn reversing_hover_does_not_jump() {
        let mut t = Tween::new(0.0);
        t.target(1.0, 0.0, 0.16);
        let midway = t.value(0.08);
        assert!(midway > 0.0 && midway < 1.0);
        t.target(0.0, 0.08, 0.16);
        assert_eq!(t.value(0.08), midway);
        assert!(t.value(0.12) < midway);
        assert_eq!(t.value(0.3), 0.0);
    }
    #[test]
    fn disabled_motion_is_immediate_and_never_ticks() {
        let mut t = Tween::new(0.0);
        t.target(1.0, 0.0, 0.0);
        assert_eq!(t.value(0.0), 1.0);
        assert!(!t.running(0.0));
    }
}
