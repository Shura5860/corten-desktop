use corten_theme::Palette;
use smithay::backend::renderer::Color32F;

/// Smoothly interpolates `current` zoom magnification toward `target` (spring-damped ease-out).
pub fn step_smooth_zoom(current: f64, target: f64) -> f64 {
    let diff = target - current;
    if diff.abs() < 0.002 {
        target
    } else {
        current + diff * 0.24
    }
}

/// Smoothly interpolates scrollable tiling horizontal offset toward target offset.
pub fn step_smooth_scroll(current: f64, target: f64) -> f64 {
    let diff = target - current;
    if diff.abs() < 0.5 {
        target
    } else {
        current + diff * 0.26
    }
}

/// Computes cohesive SSD titlebar background, accent strip, and text RGB colors,
/// gently dimming inactive windows and highlighting sticky/PiP windows.
pub fn compute_titlebar_colors(
    palette: &Palette,
    is_active: bool,
    is_sticky: bool,
) -> (Color32F, [u8; 3], Option<Color32F>) {
    let bg = if is_active {
        Color32F::new(
            palette.surface.r,
            palette.surface.g,
            palette.surface.b,
            0.96,
        )
    } else {
        Color32F::new(
            palette.background.r * 0.92,
            palette.background.g * 0.92,
            palette.background.b * 0.92,
            0.88,
        )
    };
    let text_rgb = if is_active {
        [
            (palette.text_primary.r * 255.0) as u8,
            (palette.text_primary.g * 255.0) as u8,
            (palette.text_primary.b * 255.0) as u8,
        ]
    } else {
        [
            (palette.text_muted.r * 255.0) as u8,
            (palette.text_muted.g * 255.0) as u8,
            (palette.text_muted.b * 255.0) as u8,
        ]
    };
    let sticky_badge = if is_sticky {
        Some(Color32F::new(
            palette.primary.r,
            palette.primary.g,
            palette.primary.b,
            0.95,
        ))
    } else {
        None
    };
    (bg, text_rgb, sticky_badge)
}

/// Sends a lightweight, non-blocking OSD toast pill to `corten-panel` via D-Bus.
pub fn send_panel_toast(message: &str) {
    let payload = format!("toast:{}", message);
    let _ = std::process::Command::new("busctl")
        .args([
            "--user",
            "call",
            "org.corten.Panel",
            "/org/corten/Panel",
            "org.corten.Panel",
            "TriggerModule",
            "s",
            &payload,
        ])
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use corten_theme::ThemeMode;

    #[test]
    fn test_smooth_zoom_convergence() {
        let mut z = 1.0;
        for _ in 0..40 {
            z = step_smooth_zoom(z, 2.0);
        }
        assert!((z - 2.0).abs() < 1e-6);
    }

    #[test]
    fn test_smooth_scroll_convergence() {
        let mut x = 0.0;
        for _ in 0..40 {
            x = step_smooth_scroll(x, 640.0);
        }
        assert!((x - 640.0).abs() < 1e-6);
    }

    #[test]
    fn test_titlebar_colors_dim_inactive_and_highlight_sticky() {
        let palette = Palette::for_mode(ThemeMode::Dark);
        let (active_bg, active_txt, no_sticky) = compute_titlebar_colors(&palette, true, false);
        let (inactive_bg, inactive_txt, sticky_badge) =
            compute_titlebar_colors(&palette, false, true);
        assert!(active_bg.a() > inactive_bg.a());
        assert!(active_txt[0] > inactive_txt[0]);
        assert!(no_sticky.is_none());
        assert!(sticky_badge.is_some());
    }
}
