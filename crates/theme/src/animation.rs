//! Motion tokens and easing curves shared across Corten UI applications.

/// Material Design 3 inspired motion duration tokens (in milliseconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Motion;

impl Motion {
    pub const DURATION_SHORT: u32 = 200;
    pub const DURATION_MEDIUM: u32 = 300;
    /// Entering / opening overlays, menus, and sheets.
    pub const DURATION_EMPHASIZED_DECELERATE: u32 = 400;
    /// Exiting / closing overlays and sheets.
    pub const DURATION_EMPHASIZED_ACCELERATE: u32 = 200;
}

/// Evaluates a 1D cubic-bezier easing curve `(0,0) -> (x1,y1) -> (x2,y2) -> (1,1)` at progress `x`.
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }

    let sample_x = |t: f32| -> f32 {
        let u = 1.0 - t;
        3.0 * u * u * t * x1 + 3.0 * u * t * t * x2 + t * t * t
    };
    let sample_dx = |t: f32| -> f32 {
        let u = 1.0 - t;
        3.0 * u * u * x1 + 6.0 * u * t * (x2 - x1) + 3.0 * t * t * (1.0 - x2)
    };
    let sample_y = |t: f32| -> f32 {
        let u = 1.0 - t;
        3.0 * u * u * t * y1 + 3.0 * u * t * t * y2 + t * t * t
    };

    // Newton-Raphson refinement, falling back to bisection when slope is flat.
    let mut t = x;
    for _ in 0..6 {
        let err = sample_x(t) - x;
        if err.abs() < 1e-5 {
            return sample_y(t);
        }
        let dx = sample_dx(t);
        if dx.abs() < 1e-6 {
            break;
        }
        t = (t - err / dx).clamp(0.0, 1.0);
    }

    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    for _ in 0..12 {
        let mid = 0.5 * (lo + hi);
        if sample_x(mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    sample_y(0.5 * (lo + hi))
}

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

    /// Material 3 Emphasized Decelerate: `cubic-bezier(0.05, 0.7, 0.1, 1.0)`.
    /// Starts briskly and settles smoothly; ideal for opening popovers, sheets, and drawers.
    pub fn emphasized_decelerate(self) -> f32 {
        cubic_bezier(0.05, 0.7, 0.1, 1.0, self.progress)
    }

    /// Material 3 Emphasized Accelerate: `cubic-bezier(0.3, 0.0, 0.8, 0.15)`.
    pub fn emphasized_accelerate(self) -> f32 {
        cubic_bezier(0.3, 0.0, 0.8, 0.15, self.progress)
    }

    /// Material 3 Standard curve: `cubic-bezier(0.2, 0.0, 0.0, 1.0)`.
    pub fn standard(self) -> f32 {
        cubic_bezier(0.2, 0.0, 0.0, 1.0, self.progress)
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
    use super::{Motion, Transition, cubic_bezier};

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
    fn emphasized_decelerate_front_loads_velocity_and_is_monotonic() {
        let mut prev = 0.0_f32;
        for step in 0..=20 {
            let x = step as f32 / 20.0;
            let y = cubic_bezier(0.05, 0.7, 0.1, 1.0, x);
            assert!(y >= prev - 1e-5, "non-monotonic at {x}: {y} < {prev}");
            prev = y;
        }
        assert_eq!(cubic_bezier(0.05, 0.7, 0.1, 1.0, 0.0), 0.0);
        assert_eq!(cubic_bezier(0.05, 0.7, 0.1, 1.0, 1.0), 1.0);
        // At 25% of duration, emphasized decelerate is already well past 70% displacement.
        let mut t = Transition::new(Motion::DURATION_EMPHASIZED_DECELERATE);
        t.tick(100);
        assert!(
            t.emphasized_decelerate() > 0.70,
            "expected > 0.70 at 25%, got {}",
            t.emphasized_decelerate()
        );
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
