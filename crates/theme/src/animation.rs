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
}
