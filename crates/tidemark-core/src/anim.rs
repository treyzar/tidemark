//! Critically-damped-ish spring used for every island transition.

#[derive(Debug, Clone, Copy)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    stiffness: f32,
    damping: f32,
}

impl Spring {
    pub fn new(value: f32, stiffness: f32, damping: f32) -> Self {
        Self { value, velocity: 0.0, target: value, stiffness, damping }
    }

    /// Values are in DIPs: within a quarter DIP and slower than 2 DIP/s the
    /// remaining motion is invisible, so the animation (and the render loop)
    /// stops there instead of chasing sub-pixel ripples.
    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() < 0.25 && self.velocity.abs() < 2.0
    }

    /// Advances the spring by `dt` seconds. Snaps to the target once settled
    /// so the render loop can stop and the UI thread can go back to sleep.
    pub fn step(&mut self, dt: f32) {
        if self.is_settled() {
            self.value = self.target;
            self.velocity = 0.0;
            return;
        }
        // Semi-implicit Euler in small sub-steps keeps stiff springs stable.
        let steps = (dt / 0.004).ceil().max(1.0) as u32;
        let h = dt / steps as f32;
        for _ in 0..steps {
            let force = -self.stiffness * (self.value - self.target) - self.damping * self.velocity;
            self.velocity += force * h;
            self.value += self.velocity * h;
        }
        if self.is_settled() {
            self.value = self.target;
            self.velocity = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_settles_on_target() {
        let mut s = Spring::new(0.0, 300.0, 28.0);
        s.target = 100.0;
        for _ in 0..240 {
            s.step(1.0 / 60.0);
        }
        assert!(s.is_settled());
        assert_eq!(s.value, 100.0);
    }

    #[test]
    fn settled_spring_does_not_move() {
        let mut s = Spring::new(5.0, 300.0, 28.0);
        s.step(1.0 / 60.0);
        assert_eq!(s.value, 5.0);
        assert!(s.is_settled());
    }

    #[test]
    fn island_expansion_settles_quickly() {
        // Same constants as the island's size springs: collapsed -> expanded.
        let mut s = Spring::new(36.0, 320.0, 26.0);
        s.target = 314.0;
        let mut t = 0.0;
        while !s.is_settled() {
            s.step(1.0 / 60.0);
            t += 1.0 / 60.0;
            assert!(t < 1.0, "spring still moving after 1 s");
        }
        assert!(t < 0.8, "settled after {t} s");
    }
}
