use smithay::input::keyboard::keysyms;
use tracing::info;

use crate::state::CortenCompositor;

/// Shared compositor hotkey dispatcher used by both the Nested (Winit) and Standalone (DRM/KMS)
/// backends so shortcut behavior stays 100% consistent without code duplication.
///
/// Returns `true` when the key event was handled and should be intercepted from Wayland clients.
pub fn dispatch_shared_compositor_hotkey(
    state: &mut CortenCompositor,
    sym: u32,
    logo: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
) -> bool {
    if !logo || alt {
        return false;
    }

    match sym {
        // Super + . (Period): Built-in Emoji, Math & Developer Unicode Picker
        keysyms::KEY_period if !ctrl => {
            info!("Compositor Hotkey: Super+. (Emoji & Unicode Picker)");
            let launcher_path = crate::state::resolve_corten_bin("corten-launcher");
            let _ = std::process::Command::new(launcher_path)
                .arg("--emoji")
                .spawn();
            true
        }
        // Super + N: Quick Notes / Scratchpad Launcher
        keysyms::KEY_n | keysyms::KEY_N if !ctrl && !shift => {
            info!("Compositor Hotkey: Super+N (Quick Notes)");
            let launcher_path = crate::state::resolve_corten_bin("corten-launcher");
            let _ = std::process::Command::new(launcher_path)
                .arg("--notes")
                .spawn();
            true
        }
        // Super + Shift + C: System-Wide Color Picker / Eyedropper
        keysyms::KEY_c | keysyms::KEY_C if shift && !ctrl => {
            info!("Compositor Hotkey: Super+Shift+C (Color Picker)");
            state.handle_custom_action("color-picker");
            true
        }
        // Super + Minus: Toggle Scratchpad Window / Super + Shift + Minus: Send to Scratchpad
        keysyms::KEY_minus | keysyms::KEY_underscore if !ctrl => {
            if shift {
                state.handle_custom_action("move-to-scratchpad");
            } else {
                state.handle_custom_action("toggle-scratchpad");
            }
            true
        }
        // Super + S (without Shift): Toggle Sticky / PiP Window Across Workspaces
        keysyms::KEY_s | keysyms::KEY_S if !shift && !ctrl => {
            state.handle_custom_action("toggle-sticky");
            true
        }
        // Super + G: One-Click Game Mode
        keysyms::KEY_g | keysyms::KEY_G if !ctrl => {
            state.handle_custom_action("toggle-game-mode");
            true
        }
        // Super + Equal / Plus / KP_Add: GPU Compositor Zoom In
        keysyms::KEY_equal | keysyms::KEY_plus | keysyms::KEY_KP_Add => {
            state.handle_custom_action("zoom-in");
            true
        }
        // Super + Ctrl + Minus / KP_Subtract: GPU Compositor Zoom Out
        keysyms::KEY_minus | keysyms::KEY_KP_Subtract if ctrl => {
            state.handle_custom_action("zoom-out");
            true
        }
        // Super + 0: Reset GPU Compositor Zoom
        keysyms::KEY_0 if !shift => {
            state.handle_custom_action("zoom-reset");
            true
        }
        // Super + [ / Super + ]: Scrollable Tiling Strip Navigation
        keysyms::KEY_bracketleft if !ctrl => {
            state.handle_custom_action("scroll-left");
            true
        }
        keysyms::KEY_bracketright if !ctrl => {
            state.handle_custom_action("scroll-right");
            true
        }
        // Super + R: Cycle Scrollable Column Width (33% / 50% / 66% / 100%)
        keysyms::KEY_r | keysyms::KEY_R if !ctrl && !shift => {
            state.handle_custom_action("cycle-column-width");
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::utils::Size;
    use wayland_server::Display;

    #[test]
    fn test_shared_hotkey_zoom_and_game_mode() {
        let mut display: Display<CortenCompositor> = Display::new().unwrap();
        let mut state = CortenCompositor::new(&mut display, Size::from((1920, 1080)));

        assert!((state.zoom_level - 1.0).abs() < 1e-6);
        assert!(dispatch_shared_compositor_hotkey(
            &mut state,
            keysyms::KEY_equal,
            true,
            false,
            false,
            false
        ));
        assert!((state.target_zoom_level - 1.25).abs() < 1e-6);

        assert!(dispatch_shared_compositor_hotkey(
            &mut state,
            keysyms::KEY_KP_Subtract,
            true,
            true,
            false,
            false
        ));
        assert!((state.target_zoom_level - 1.0).abs() < 1e-6);

        assert!(dispatch_shared_compositor_hotkey(
            &mut state,
            keysyms::KEY_g,
            true,
            false,
            false,
            false
        ));
        assert!(state.game_mode);

        // Non-Super key should not be intercepted
        assert!(!dispatch_shared_compositor_hotkey(
            &mut state,
            keysyms::KEY_g,
            false,
            false,
            false,
            false
        ));
    }
}
