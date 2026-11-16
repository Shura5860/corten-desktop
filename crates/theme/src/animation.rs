//! Small, backend-independent animation primitives shared by Corten UI apps.

#[derive(Debug, Clone, Copy)]
pub struct Transition {
    progress: f32,
    target: f32,
    duration_ms: f32,
}

impl Transition {
    pub const fn new(duration_ms: u32) -> Self {
        Self {
            progress: 0.0,
            target: 1.0,
            duration_ms: duration_ms as f32,
        }
    }

    pub const fn completed(duration_ms: u32) -> Self {
        Self {
            progress: 1.0,
            target: 1.0,
            duration_ms: duration_ms as f32,
        }
    }

    pub fn restart(&mut self) {
        self.progress = 0.0;
        self.target = 1.0;
    }

    pub fn tick(&mut self, elapsed_ms: u32) -> bool {
        let distance = self.target - self.progress;
        if distance.abs() < f32::EPSILON {
            self.progress = self.target;
            return false;
        }

        let step = elapsed_ms as f32 / self.duration_ms.max(1.0);
        self.progress = (self.progress + distance.signum() * step.clamp(0.0, 1.0)).clamp(0.0, 1.0);
        true
    }

    pub fn eased(self) -> f32 {
        1.0 - (1.0 - self.progress).powi(3)
    }

    /// Spring overshoot easing for tactile bounce (elastic settle at target 1.0)
    pub fn spring(self) -> f32 {
        let p = self.progress;
        if p >= 1.0 {
            1.0
        } else {
            let s = 1.70158_f32;
            let p_adj = p - 1.0;
            p_adj * p_adj * ((s + 1.0) * p_adj + s) + 1.0
        }
    }

    pub fn is_complete(self) -> bool {
        (self.progress - self.target).abs() < f32::EPSILON
    }

    /// Computes staggered cascade progress (`0.0..=1.0`) for `item_index` in a list.
    pub fn staggered(self, item_index: usize, stagger_step: f32) -> f32 {
        let delay = (item_index.min(10) as f32 * stagger_step).clamp(0.0, 0.65);
        let span = (1.0 - delay).max(0.15);
        let local_p = ((self.progress - delay) / span).clamp(0.0, 1.0);
        1.0 - (1.0 - local_p).powi(3)
    }

    /// Computes a 4-cycle exponentially damped sinusoidal shake offset (`-amplitude..=amplitude`),
    /// settling cleanly to `0.0` when `is_complete()`.
    pub fn damped_shake(self, amplitude: f32) -> f32 {
        if self.is_complete() || self.progress <= 0.0 {
            return 0.0;
        }
        let decay = (1.0 - self.progress).powi(2);
        let wave = (self.progress * std::f32::consts::TAU * 3.5).sin();
        wave * decay * amplitude
    }

    /// Smoothly interpolates `current` toward `target` by `factor` (`0.0..=1.0`), snapping when close.
    pub fn smooth_lerp(current: f32, target: f32, factor: f32) -> f32 {
        let diff = target - current;
        if diff.abs() < 0.05 {
            target
        } else {
            current + diff * factor.clamp(0.05, 1.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Transition;

    #[test]
    fn transition_reaches_target() {
        let mut transition = Transition::new(100);
        for _ in 0..20 {
            transition.tick(16);
        }
        assert!(transition.is_complete());
        assert!((transition.eased() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn restart_returns_to_start() {
        let mut transition = Transition::new(100);
        transition.tick(100);
        transition.restart();
        assert!(!transition.is_complete());
        assert!(transition.eased() < 1.0);
    }

    #[test]
    fn spring_reaches_one_at_completion() {
        let mut transition = Transition::new(100);
        for _ in 0..20 {
            transition.tick(16);
        }
        assert!((transition.spring() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn staggered_and_damped_shake_settle_cleanly() {
        let mut t = Transition::new(200);
        assert_eq!(t.staggered(3, 0.08), 0.0);
        t.tick(60);
        assert!(t.damped_shake(12.0).abs() > 0.0);
        for _ in 0..20 {
            t.tick(16);
        }
        assert!(t.is_complete());
        assert!((t.staggered(5, 0.08) - 1.0).abs() < f32::EPSILON);
        assert_eq!(t.damped_shake(12.0), 0.0);
    }
}
