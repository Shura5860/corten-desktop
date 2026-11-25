pub mod cursor;
mod glow_shader;
mod handlers;
pub mod hotkeys;
mod layout;
pub mod render;
mod screencopy;
mod state;
pub mod tablet;
mod tearing;
pub mod udev;
mod xwayland;

use std::borrow::BorrowMut;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

use anyhow::Context;

use corten_config::{
    AppearanceConfig, ConfigManager, DisplaySettingsConfig, InputSettingsConfig, KeybindingsConfig,
    LayoutMode, PowerSettingsConfig, WindowRuleConfig,
};
use corten_theme::{Palette, PatinaVariant, ThemeMode};
use glow_shader::{DualKawaseBlurPipeline, GlassRegion, GlowShaderManager};
use handlers::ClientState;
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisRelativeDirection, ButtonState, Event, InputEvent, KeyState,
    KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::element::{Id, Kind, render_elements};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::utils::{CommitCounter, draw_render_elements};
use smithay::backend::renderer::{Color32F, Frame, Renderer};
use smithay::backend::winit::{self, WinitEvent};
use smithay::desktop::{PopupManager, WindowSurfaceType, layer_map_for_output};
use smithay::input::keyboard::FilterResult;
use smithay::input::keyboard::keysyms;
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent, RelativeMotionEvent};
use smithay::output::Mode;
use smithay::reexports::winit::platform::pump_events::PumpStatus;
use smithay::utils::{Point, Rectangle, SERIAL_COUNTER, Scale, Size, Transform};
use smithay::wayland::compositor::{
    SurfaceAttributes, TraversalAction, with_surface_tree_downward,
};
use smithay::wayland::shell::wlr_layer::Layer;
use state::CortenCompositor;
use tracing::info;
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{Display, ListeningSocket};

pub(crate) static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone)]
pub enum CompositorIpcCommand {
    ReloadConfig,
    SwitchWorkspace(usize),
    SwitchWorkspaceOnOutput(String, usize),
    FocusWindow(u64),
    ToggleShowDesktop,
    ToggleOverview,
    TakeScreenshot(Option<Rectangle<i32, smithay::utils::Logical>>),
    CustomAction(String),
}

extern "C" fn request_shutdown(_: nix::libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};

    let action = SigAction::new(
        SigHandler::Handler(request_shutdown),
        SaFlags::empty(),
        SigSet::empty(),
    );
    for signal in [Signal::SIGINT, Signal::SIGTERM] {
        if let Err(error) = unsafe { sigaction(signal, &action) } {
            tracing::warn!(?signal, ?error, "Failed to install shutdown signal handler");
        }
    }
}

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::gles::element::{PixelShaderElement, TextureShaderElement};
use smithay::input::pointer::CursorIcon;
use std::collections::HashMap;

render_elements! {
    pub CustomRenderElement<=GlowRenderer>;
    Surface=WaylandSurfaceRenderElement<GlowRenderer>,
    CroppedSurface=smithay::backend::renderer::element::utils::CropRenderElement<WaylandSurfaceRenderElement<GlowRenderer>>,
    Solid=SolidColorRenderElement,
    PixelShader=PixelShaderElement,
    TextureShader=TextureShaderElement,
    Memory=MemoryRenderBufferRenderElement<GlowRenderer>,
}

/// Clips client surface elements to the window body rectangle (GPU-side crop), so a client
/// buffer that lags behind a resize can never paint outside the window frame.
pub(crate) fn clip_to_body(
    elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>>,
    scale: f64,
    body: smithay::utils::Rectangle<i32, smithay::utils::Physical>,
) -> impl Iterator<Item = CustomRenderElement> {
    use smithay::backend::renderer::element::utils::CropRenderElement;
    elements
        .into_iter()
        .filter_map(move |e| CropRenderElement::from_element(e, scale, body))
        .map(CustomRenderElement::from)
}

// Compositor configuration struct
#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
pub(crate) struct CompositorConfig {
    pub gap: i32,
    pub master_ratio: f32,
    #[serde(default)]
    pub layout_mode: LayoutMode,
    #[serde(default)]
    pub window_rules: Vec<WindowRuleConfig>,
    #[serde(default)]
    pub keybindings: KeybindingsConfig,
}

pub(crate) fn apply_power_settings(state: &mut CortenCompositor) {
    if let Ok(config) = ConfigManager::<PowerSettingsConfig>::load_or_default("power") {
        state.set_power_timeouts(config.get().screen_sleep_mins, config.get().suspend_mins);
    }
}

pub(crate) fn request_system_suspend() {
    std::thread::spawn(|| {
        let result = (|| -> anyhow::Result<()> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                let connection = zbus::Connection::system().await?;
                let proxy = zbus::Proxy::new(
                    &connection,
                    "org.freedesktop.login1",
                    "/org/freedesktop/login1",
                    "org.freedesktop.login1.Manager",
                )
                .await?;
                proxy.call_method("Suspend", &(true,)).await?;
                Ok::<(), zbus::Error>(())
            })?;
            Ok(())
        })();
        if let Err(error) = result {
            tracing::warn!(?error, "systemd-logind suspend request failed");
        }
    });
}

pub(crate) fn send_osd_notification(sync_tag: &str, summary: &str, body: &str, pct: u8) {
    let tag = sync_tag.to_string();
    let sum = summary.to_string();
    let bdy = body.to_string();

    let mod_name = match sync_tag {
        "volume" => format!("osd:volume:{}", pct),
        "brightness" => format!("osd:brightness:{}", pct),
        _ => "osd".to_string(),
    };
    let _ = Command::new("busctl")
        .args([
            "--user",
            "call",
            "org.corten.Panel",
            "/org/corten/Panel",
            "org.corten.Panel",
            "TriggerModule",
            "s",
            &mod_name,
        ])
        .spawn();

    if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        let _ = rt.block_on(async move {
            let conn = zbus::Connection::session().await?;
            let proxy = zbus::Proxy::new(
                &conn,
                "org.freedesktop.Notifications",
                "/org/freedesktop/Notifications",
                "org.freedesktop.Notifications",
            )
            .await?;
            let mut hints = std::collections::HashMap::<&str, zbus::zvariant::Value<'_>>::new();
            hints.insert(
                "x-canonical-private-synchronous",
                zbus::zvariant::Value::from(tag.as_str()),
            );
            hints.insert("value", zbus::zvariant::Value::from(pct.min(100)));
            let actions: Vec<&str> = Vec::new();
            let _: u32 = proxy
                .call(
                    "Notify",
                    &(
                        "Corten OSD",
                        0u32,
                        "corten",
                        sum.as_str(),
                        bdy.as_str(),
                        actions,
                        hints,
                        1800i32,
                    ),
                )
                .await?;
            Ok::<(), zbus::Error>(())
        });
    }
}

pub(crate) fn parse_wpctl_volume(output: &str) -> Option<(u8, bool)> {
    // Example: "Volume: 0.65" or "Volume: 0.65 [MUTED]"
    let muted = output.contains("[MUTED]");
    let after = output.split("Volume:").nth(1)?.trim();
    let num_str = after.split_whitespace().next()?;
    let val = num_str.parse::<f32>().ok()?;
    let pct = (val * 100.0).round().clamp(0.0, 100.0) as u8;
    Some((pct, muted))
}

pub(crate) fn parse_brightnessctl_pct(output: &str) -> Option<u8> {
    // Example: "Current brightness: 120 (50%)"
    let open = output.find('(')?;
    let close = output[open..].find("%)")?;
    let digits = &output[open + 1..open + close];
    digits.trim().parse::<u8>().ok()
}

pub(crate) fn handle_media_keys(sym: u32) -> bool {
    match sym {
        keysyms::KEY_XF86AudioRaiseVolume => {
            std::thread::spawn(|| {
                let _ = Command::new("wpctl")
                    .args(["set-volume", "@DEFAULT_AUDIO_SINK@", "5%+"])
                    .status();
                if let Ok(out) = Command::new("wpctl")
                    .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
                    .output()
                    && let Some((pct, muted)) =
                        parse_wpctl_volume(&String::from_utf8_lossy(&out.stdout))
                {
                    let title = if muted {
                        format!("Ses: %{pct} (Sessiz)")
                    } else {
                        format!("Ses Seviyesi: %{pct}")
                    };
                    send_osd_notification("volume", &title, "PipeWire Ses Çıkışı", pct);
                }
            });
            true
        }
        keysyms::KEY_XF86AudioLowerVolume => {
            std::thread::spawn(|| {
                let _ = Command::new("wpctl")
                    .args(["set-volume", "@DEFAULT_AUDIO_SINK@", "5%-"])
                    .status();
                if let Ok(out) = Command::new("wpctl")
                    .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
                    .output()
                    && let Some((pct, muted)) =
                        parse_wpctl_volume(&String::from_utf8_lossy(&out.stdout))
                {
                    let title = if muted {
                        format!("Ses: %{pct} (Sessiz)")
                    } else {
                        format!("Ses Seviyesi: %{pct}")
                    };
                    send_osd_notification("volume", &title, "PipeWire Ses Çıkışı", pct);
                }
            });
            true
        }
        keysyms::KEY_XF86AudioMute => {
            std::thread::spawn(|| {
                let _ = Command::new("wpctl")
                    .args(["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])
                    .status();
                if let Ok(out) = Command::new("wpctl")
                    .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
                    .output()
                    && let Some((pct, muted)) =
                        parse_wpctl_volume(&String::from_utf8_lossy(&out.stdout))
                {
                    let (title, bar_pct) = if muted {
                        ("Ses Kapatıldı (Sessiz)".to_string(), 0)
                    } else {
                        (format!("Ses Açıldı: %{pct}"), pct)
                    };
                    send_osd_notification("volume", &title, "PipeWire Ses Çıkışı", bar_pct);
                }
            });
            true
        }
        keysyms::KEY_XF86MonBrightnessUp => {
            std::thread::spawn(|| {
                let _ = Command::new("brightnessctl").args(["set", "+5%"]).status();
                if let Ok(out) = Command::new("brightnessctl").output()
                    && let Some(pct) =
                        parse_brightnessctl_pct(&String::from_utf8_lossy(&out.stdout))
                {
                    send_osd_notification(
                        "brightness",
                        &format!("Ekran Parlaklığı: %{pct}"),
                        "Arka Işık Seviyesi",
                        pct,
                    );
                }
            });
            true
        }
        keysyms::KEY_XF86MonBrightnessDown => {
            std::thread::spawn(|| {
                let _ = Command::new("brightnessctl").args(["set", "5%-"]).status();
                if let Ok(out) = Command::new("brightnessctl").output()
                    && let Some(pct) =
                        parse_brightnessctl_pct(&String::from_utf8_lossy(&out.stdout))
                {
                    send_osd_notification(
                        "brightness",
                        &format!("Ekran Parlaklığı: %{pct}"),
                        "Arka Işık Seviyesi",
                        pct,
                    );
                }
            });
            true
        }
        _ => false,
    }
}

pub(crate) fn compositor_key_name(sym: u32) -> Option<&'static str> {
    match sym {
        keysyms::KEY_Return => Some("return"),
        keysyms::KEY_space => Some("space"),
        keysyms::KEY_a | keysyms::KEY_A => Some("a"),
        keysyms::KEY_b | keysyms::KEY_B => Some("b"),
        keysyms::KEY_c | keysyms::KEY_C => Some("c"),
        keysyms::KEY_d | keysyms::KEY_D => Some("d"),
        keysyms::KEY_e | keysyms::KEY_E => Some("e"),
        keysyms::KEY_f | keysyms::KEY_F => Some("f"),
        keysyms::KEY_g | keysyms::KEY_G => Some("g"),
        keysyms::KEY_h | keysyms::KEY_H => Some("h"),
        keysyms::KEY_i | keysyms::KEY_I | keysyms::KEY_comma => Some("i"),
        keysyms::KEY_j | keysyms::KEY_J => Some("j"),
        keysyms::KEY_k | keysyms::KEY_K => Some("k"),
        keysyms::KEY_l | keysyms::KEY_L => Some("l"),
        keysyms::KEY_m | keysyms::KEY_M => Some("m"),
        keysyms::KEY_n | keysyms::KEY_N => Some("n"),
        keysyms::KEY_o | keysyms::KEY_O => Some("o"),
        keysyms::KEY_p | keysyms::KEY_P => Some("p"),
        keysyms::KEY_q | keysyms::KEY_Q => Some("q"),
        keysyms::KEY_r | keysyms::KEY_R => Some("r"),
        keysyms::KEY_s | keysyms::KEY_S => Some("s"),
        keysyms::KEY_t | keysyms::KEY_T => Some("t"),
        keysyms::KEY_u | keysyms::KEY_U => Some("u"),
        keysyms::KEY_v | keysyms::KEY_V => Some("v"),
        keysyms::KEY_w | keysyms::KEY_W => Some("w"),
        keysyms::KEY_x | keysyms::KEY_X => Some("x"),
        keysyms::KEY_y | keysyms::KEY_Y => Some("y"),
        keysyms::KEY_z | keysyms::KEY_Z => Some("z"),
        keysyms::KEY_1 => Some("1"),
        keysyms::KEY_2 => Some("2"),
        keysyms::KEY_3 => Some("3"),
        keysyms::KEY_4 => Some("4"),
        keysyms::KEY_5 => Some("5"),
        keysyms::KEY_6 => Some("6"),
        keysyms::KEY_7 => Some("7"),
        keysyms::KEY_8 => Some("8"),
        keysyms::KEY_9 => Some("9"),
        keysyms::KEY_Tab | keysyms::KEY_ISO_Left_Tab => Some("tab"),
        keysyms::KEY_Right => Some("right"),
        keysyms::KEY_Left => Some("left"),
        keysyms::KEY_Up => Some("up"),
        keysyms::KEY_Down => Some("down"),
        keysyms::KEY_slash | keysyms::KEY_question => Some("slash"),
        keysyms::KEY_F1 => Some("f1"),
        _ => None,
    }
}

pub(crate) fn binding_matches(
    bindings: &KeybindingsConfig,
    action: &str,
    sym: u32,
    logo: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
) -> bool {
    compositor_key_name(sym)
        .is_some_and(|key| bindings.matches(action, key, logo, ctrl, alt, shift))
}

pub(crate) fn secondary_output_location(
    arrangement: &str,
    primary_width: i32,
    next_left_x: &mut i32,
    next_right_x: &mut i32,
    output_width: i32,
    offset_y: i32,
) -> (i32, i32) {
    match arrangement {
        "extend-left" => {
            *next_left_x -= output_width;
            (*next_left_x, offset_y)
        }
        "mirror" => (0, 0),
        _ => {
            let location = (*next_right_x, offset_y);
            *next_right_x += output_width;
            if *next_right_x < primary_width {
                *next_right_x = primary_width;
            }
            location
        }
    }
}

pub(crate) fn apply_display_settings(state: &mut CortenCompositor) {
    let display_config = ConfigManager::<DisplaySettingsConfig>::load_or_default("displays")
        .map(|config| config.get().clone())
        .unwrap_or_default();
    let scale_pct = display_config.scale_pct.clamp(50, 400);
    if display_config.auto_night_light {
        use chrono::Timelike;
        let hour = chrono::Local::now().hour();
        state.set_night_light_enabled(!(7..19).contains(&hour));
    } else {
        state.set_night_light_enabled(display_config.night_light);
    }
    state.vrr_enabled = display_config.vrr;
    state.configured_vrr = display_config.vrr;
    state.allow_tearing = display_config.allow_tearing;
    state.configured_allow_tearing = display_config.allow_tearing;
    state.auto_fullscreen_game_mode = display_config.auto_fullscreen_game_mode;
    state.hdr_enabled = display_config.hdr_enabled;
    state.color_space = display_config.color_space.clone();
    state.xwayland_force_zero_scaling = display_config.xwayland_force_zero_scaling;
    if state.hdr_enabled {
        info!(
            color_space = %state.color_space,
            max_nits = display_config.max_luminance_nits.unwrap_or(400),
            "Compositor: HDR / Wide Gamut display pipeline active"
        );
    }
    if let Some(ref icc_path) = display_config.icc_profile
        && let Ok(info) = corten_services::color_management::load_icc_profile_file(icc_path)
    {
        info!(
            profile = %info.description,
            color_space = %info.color_space,
            "Compositor: Loaded monitor ICC color profile"
        );
    }
    state.output.change_current_state(
        None,
        None,
        Some(smithay::output::Scale::Fractional(scale_pct as f64 / 100.0)),
        None,
    );
    crate::xwayland::sync_x11_scale(scale_pct as f64 / 100.0);
    let primary_width = state
        .output
        .current_mode()
        .map(|mode| mode.size.w)
        .unwrap_or_default();
    let arrangement = display_config.arrangement.as_str();
    let selected_output = display_config.secondary_output.as_str();
    let mut next_left_x = 0;
    let mut next_right_x = primary_width;
    for output in &state.additional_outputs {
        let output_width = output
            .current_mode()
            .map(|mode| mode.size.w)
            .unwrap_or_default();
        let location = secondary_output_location(
            arrangement,
            primary_width,
            &mut next_left_x,
            &mut next_right_x,
            output_width,
            display_config.offset_y,
        )
        .into();
        let secondary_scale = if selected_output == "None"
            || selected_output.is_empty()
            || output.name().contains(selected_output)
        {
            display_config.secondary_scale_pct.clamp(50, 400)
        } else {
            100
        };
        output.change_current_state(
            None,
            None,
            Some(smithay::output::Scale::Fractional(
                secondary_scale as f64 / 100.0,
            )),
            Some(location),
        );
    }
    info!(
        "Compositor: display settings applied (primary {}%, secondary {}%, arrangement {}, offset_y {})",
        scale_pct, display_config.secondary_scale_pct, arrangement, display_config.offset_y
    );
    state.refresh_all_surface_scales();
}

pub(crate) fn secondary_display_settings() -> (u32, String, String, i32) {
    ConfigManager::<DisplaySettingsConfig>::load_or_default("displays")
        .map(|config| {
            (
                config.get().secondary_scale_pct.clamp(50, 400),
                config.get().arrangement.clone(),
                config.get().secondary_output.clone(),
                config.get().offset_y,
            )
        })
        .unwrap_or((100, "extend-right".to_string(), "None".to_string(), 0))
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::secondary_output_location;

    #[test]
    fn secondary_outputs_extend_right_without_overlap() {
        let mut left = 0;
        let mut right = 1920;
        assert_eq!(
            secondary_output_location("extend-right", 1920, &mut left, &mut right, 1280, 0),
            (1920, 0)
        );
        assert_eq!(
            secondary_output_location("extend-right", 1920, &mut left, &mut right, 1080, 0),
            (3200, 0)
        );
    }

    #[test]
    fn secondary_outputs_extend_left_from_primary_origin() {
        let mut left = 0;
        let mut right = 1920;
        assert_eq!(
            secondary_output_location("extend-left", 1920, &mut left, &mut right, 1280, 0),
            (-1280, 0)
        );
        assert_eq!(
            secondary_output_location("extend-left", 1920, &mut left, &mut right, 1080, 0),
            (-2360, 0)
        );
    }

    #[test]
    fn secondary_outputs_support_vertical_offset() {
        let mut left = 0;
        let mut right = 1920;
        assert_eq!(
            secondary_output_location("extend-right", 1920, &mut left, &mut right, 1280, 200),
            (1920, 200)
        );
        assert_eq!(
            secondary_output_location("extend-left", 1920, &mut left, &mut right, 1280, -150),
            (-1280, -150)
        );
    }

    #[test]
    fn mirror_keeps_all_outputs_at_origin() {
        let mut left = 0;
        let mut right = 1920;
        assert_eq!(
            secondary_output_location("mirror", 1920, &mut left, &mut right, 1920, 0),
            (0, 0)
        );
        assert_eq!(left, 0);
        assert_eq!(right, 1920);
    }

    #[test]
    fn semantic_cursors_emit_cursor_kind_elements() {
        use super::{CommitCounter, CursorIcon, Id, Palette, ThemeMode, draw_semantic_cursor};
        use smithay::backend::renderer::element::{Element, Kind};

        let palette = Palette::for_mode(ThemeMode::Dark);
        let body_ids: Vec<Id> = (0..14).map(|_| Id::new()).collect();
        let border_ids: Vec<Id> = (0..14).map(|_| Id::new()).collect();
        let tail_id = Id::new();
        let outline_id = Id::new();

        for icon in [
            CursorIcon::Default,
            CursorIcon::Text,
            CursorIcon::Crosshair,
            CursorIcon::EwResize,
            CursorIcon::NsResize,
            CursorIcon::NwseResize,
            CursorIcon::NeswResize,
            CursorIcon::Move,
            CursorIcon::Pointer,
        ] {
            let mut elements = Vec::new();
            draw_semantic_cursor(
                &mut elements,
                icon,
                100,
                100,
                &body_ids,
                &border_ids,
                &tail_id,
                &outline_id,
                CommitCounter::default(),
                &palette,
            );
            assert!(!elements.is_empty());
            assert!(elements.iter().all(|e| e.kind() == Kind::Cursor));
        }
    }

    #[test]
    fn test_resolve_corten_bin_resolution() {
        let sh = crate::state::resolve_corten_bin("sh");
        assert!(sh.is_file());
    }

    #[test]
    fn test_layershell_vertical_horizontal_expansion_logic() {
        let output_size: smithay::utils::Size<i32, smithay::utils::Logical> = (1920, 1080).into();
        let mut configure_size: smithay::utils::Size<i32, smithay::utils::Logical> = (0, 80).into();
        let anchored_horizontally = true;
        let anchored_vertically = true;

        if configure_size.w == 0 || anchored_horizontally {
            configure_size.w = output_size.w;
        }
        if configure_size.h == 0 || anchored_vertically {
            configure_size.h = output_size.h;
        }

        assert_eq!(configure_size.w, 1920);
        assert_eq!(configure_size.h, 1080);
    }

    #[test]
    fn parses_wpctl_and_brightnessctl_outputs() {
        use super::{parse_brightnessctl_pct, parse_wpctl_volume};
        assert_eq!(parse_wpctl_volume("Volume: 0.65\n"), Some((65, false)));
        assert_eq!(
            parse_wpctl_volume("Volume: 0.40 [MUTED]\n"),
            Some((40, true))
        );
        assert_eq!(
            parse_brightnessctl_pct("Current brightness: 120 (50%)\n"),
            Some(50)
        );
    }

    #[test]
    fn test_render_title_text_to_buffer_produces_crisp_opaque_glyphs() {
        use super::render_title_text_to_buffer;
        let res =
            render_title_text_to_buffer("Corten Terminal — Pencere Başlığı", 1.0, [255, 255, 255]);
        assert!(res.is_some());
        let (_buf, w, h) = res.unwrap();
        assert!(w > 120);
        assert!(h >= 16);
    }

    #[test]
    fn test_render_circular_button_to_buffer_all_states() {
        use super::render_circular_button_to_buffer;
        for kind in ['c', 'm', 'M', 'R'] {
            for is_active in [true, false] {
                for is_hovered in [true, false] {
                    let res = render_circular_button_to_buffer(kind, is_active, is_hovered, 1.0);
                    assert!(res.is_some());
                    let (_buf, w, h) = res.unwrap();
                    assert_eq!(w, 24);
                    assert_eq!(h, 24);
                }
            }
        }
    }

    #[test]
    fn test_render_titlebar_app_icon_all_states() {
        use super::render_titlebar_app_icon;
        let test_apps = [
            ("corten-terminal", "Corten Terminal"),
            ("corten-files", "Corten Files — ~/Documents"),
            ("corten-settings", "Sistem Ayarları"),
            ("corten-forge", "Corten Forge"),
            ("corten-ai", "Corten AI Asistanı"),
            ("corten-monitor", "Corten Monitor"),
            ("unknown-app-xyz", "Bilinmeyen Uygulama"),
        ];

        for (app_id, title) in test_apps {
            let res = render_titlebar_app_icon(app_id, title, 20, 1.0);
            assert!(
                res.is_some(),
                "Failed to render titlebar icon for {}",
                app_id
            );
            let (_buf, w, h) = res.unwrap();
            assert_eq!(w, 20);
            assert_eq!(h, 20);
        }
    }
}

fn spawn_first_terminal() -> std::io::Result<()> {
    let mut last_error = None;
    for program in [
        "corten-terminal",
        "/usr/local/bin/corten-terminal",
        "target/release/corten-terminal",
        "target/debug/corten-terminal",
    ] {
        if Command::new(program).spawn().is_ok() {
            return Ok(());
        }
    }
    if Command::new("konsole")
        .args(["--profile", "Corten"])
        .spawn()
        .is_ok()
    {
        return Ok(());
    }
    if Command::new("/var/lib/flatpak/exports/bin/org.kde.konsole")
        .args(["--profile", "Corten"])
        .spawn()
        .is_ok()
    {
        return Ok(());
    }
    for program in ["foot", "alacritty", "kitty", "konsole", "xterm", "pterm"] {
        match Command::new(program).spawn() {
            Ok(_) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no terminal emulator found")
    }))
}

pub(crate) fn parse_slurp_geometry(input: &str) -> Option<Rectangle<i32, smithay::utils::Logical>> {
    // Expected `slurp` output format: "X,Y WxH"
    let trimmed = input.trim();
    let (coords, dims) = trimmed.split_once(' ')?;
    let (x_str, y_str) = coords.split_once(',')?;
    let (w_str, h_str) = dims.split_once('x')?;
    let x = x_str.trim().parse::<i32>().ok()?;
    let y = y_str.trim().parse::<i32>().ok()?;
    let w = w_str.trim().parse::<i32>().ok()?.max(1);
    let h = h_str.trim().parse::<i32>().ok()?.max(1);
    Some(Rectangle::new(Point::from((x, y)), Size::from((w, h))))
}

pub(crate) fn trigger_screenshot(
    state: &mut CortenCompositor,
    ipc_tx: &std::sync::mpsc::Sender<CompositorIpcCommand>,
    region: bool,
) {
    if !region {
        state.request_screenshot(None);
        return;
    }
    let tx = ipc_tx.clone();
    std::thread::spawn(move || {
        let snipper_path = crate::state::resolve_corten_bin("corten-snipper");
        let output = if snipper_path.exists() {
            Command::new(snipper_path).output()
        } else {
            Command::new("slurp").output()
        };

        match output {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout);
                if let Some(rect) = parse_slurp_geometry(&text) {
                    let _ = tx.send(CompositorIpcCommand::TakeScreenshot(Some(rect)));
                }
            }
            Ok(_) => {
                // User cancelled selection (Esc)
            }
            Err(_) => {
                // Fall back to full-screen capture
                let _ = tx.send(CompositorIpcCommand::TakeScreenshot(None));
            }
        }
    });
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "corten_compositor=info,smithay::backend::drm=error,smithay::backend::renderer=error,error".into()),
        )
        .init();
    install_signal_handlers();
    info!("Starting corten-compositor...");

    // Check if launched directly from TTY / Standalone DRM mode
    let args: Vec<String> = std::env::args().collect();
    let is_tty_mode = args.iter().any(|arg| arg == "--drm" || arg == "--tty")
        || (std::env::var("WAYLAND_DISPLAY").is_err() && std::env::var("DISPLAY").is_err());

    let (ipc_tx, ipc_rx) = std::sync::mpsc::channel::<CompositorIpcCommand>();
    let hotkey_ipc_tx = ipc_tx.clone();
    let (event_broadcaster_tx, mut event_broadcaster_rx) =
        tokio::sync::mpsc::unbounded_channel::<String>();
    let main_event_tx = event_broadcaster_tx.clone();
    let drm_event_tx = event_broadcaster_tx.clone();

    let shared_ipc_snapshot = Arc::new(std::sync::RwLock::new(state::CompositorIpcState {
        active_workspace: 1,
        occupied_workspaces: Vec::new(),
        windows: Vec::new(),
        output_active_workspaces: std::collections::HashMap::new(),
        output_occupied_workspaces: std::collections::HashMap::new(),
    }));
    let server_snapshot = Arc::clone(&shared_ipc_snapshot);

    let shared_a11y_tree = Arc::new(std::sync::RwLock::new(
        corten_services::a11y::AccessibleTree::new("Corten Desktop"),
    ));
    let server_a11y_tree = Arc::clone(&shared_a11y_tree);
    let udev_a11y_tree = Arc::clone(&shared_a11y_tree);
    let winit_a11y_tree = Arc::clone(&shared_a11y_tree);

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async move {
            let (bcast_tx, _) = tokio::sync::broadcast::channel::<String>(64);
            let bcast_tx_clone = bcast_tx.clone();

            // Bridge mpsc events from Compositor state to broadcast subscribers
            tokio::spawn(async move {
                while let Some(msg) = event_broadcaster_rx.recv().await {
                    let _ = bcast_tx_clone.send(msg);
                }
            });

            // Start standard UNIX domain event socket (Hyprland / Sway model)
            let sock_path = corten_ipc::event_socket_path();
            if let Some(parent) = sock_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::remove_file(&sock_path).await;

            if let Ok(listener) = tokio::net::UnixListener::bind(&sock_path) {
                info!("Compositor: Event Socket listening at {:?}", sock_path);
                let bcast_tx_for_listener = bcast_tx.clone();
                tokio::spawn(async move {
                    loop {
                        if let Ok((mut stream, _)) = listener.accept().await {
                            let mut rx = bcast_tx_for_listener.subscribe();
                            tokio::spawn(async move {
                                use tokio::io::AsyncWriteExt;
                                while let Ok(event) = rx.recv().await {
                                    if stream
                                        .write_all(format!("{}\n", event).as_bytes())
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            });
                        }
                    }
                });
            }
            struct CompositorServer {
                tx: std::sync::mpsc::Sender<CompositorIpcCommand>,
                snapshot: Arc<std::sync::RwLock<state::CompositorIpcState>>,
            }

            #[zbus::interface(name = "org.corten.Compositor")]
            impl CompositorServer {
                async fn reload_config(&self) {
                    info!("D-Bus: Compositor reload_config received");
                    let _ = self.tx.send(CompositorIpcCommand::ReloadConfig);
                }

                async fn switch_workspace(&self, workspace: u32) {
                    info!(workspace, "D-Bus: Compositor switch_workspace received");
                    let _ = self
                        .tx
                        .send(CompositorIpcCommand::SwitchWorkspace(workspace as usize));
                }

                async fn switch_workspace_on_output(&self, output_name: String, workspace: u32) {
                    info!(
                        output_name,
                        workspace, "D-Bus: Compositor switch_workspace_on_output received"
                    );
                    let _ = self.tx.send(CompositorIpcCommand::SwitchWorkspaceOnOutput(
                        output_name,
                        workspace as usize,
                    ));
                }

                async fn focus_window(&self, window_id: u64) {
                    info!(window_id, "D-Bus: Compositor focus_window received");
                    let _ = self.tx.send(CompositorIpcCommand::FocusWindow(window_id));
                }

                async fn toggle_show_desktop(&self) {
                    info!("D-Bus: Compositor toggle_show_desktop received");
                    let _ = self.tx.send(CompositorIpcCommand::ToggleShowDesktop);
                }

                async fn toggle_overview(&self) {
                    info!("D-Bus: Compositor toggle_overview received");
                    let _ = self.tx.send(CompositorIpcCommand::ToggleOverview);
                }

                async fn dispatch_action(&self, action: String) -> String {
                    info!(action = %action, "D-Bus: Compositor dispatch_action received");
                    let _ = self
                        .tx
                        .send(CompositorIpcCommand::CustomAction(action.clone()));
                    format!("Dispatched action: {}", action)
                }

                async fn take_screenshot(&self, region: bool) {
                    info!(region, "D-Bus: Compositor take_screenshot received");
                    if !region {
                        let _ = self.tx.send(CompositorIpcCommand::TakeScreenshot(None));
                    } else {
                        let tx = self.tx.clone();
                        std::thread::spawn(move || {
                            let snipper_path = crate::state::resolve_corten_bin("corten-snipper");
                            let output = if snipper_path.exists() {
                                Command::new(snipper_path).output()
                            } else {
                                Command::new("slurp").output()
                            };
                            match output {
                                Ok(out) if out.status.success() => {
                                    let text = String::from_utf8_lossy(&out.stdout);
                                    if let Some(rect) = parse_slurp_geometry(&text) {
                                        let _ = tx
                                            .send(CompositorIpcCommand::TakeScreenshot(Some(rect)));
                                    }
                                }
                                Ok(_) => {}
                                Err(_) => {
                                    let _ = tx.send(CompositorIpcCommand::TakeScreenshot(None));
                                }
                            }
                        });
                    }
                }

                async fn get_active_workspace(&self) -> u32 {
                    self.snapshot
                        .read()
                        .map(|guard| guard.active_workspace.max(1))
                        .unwrap_or(1)
                }

                async fn get_active_workspace_for_output(&self, output_name: String) -> u32 {
                    self.snapshot
                        .read()
                        .map(|guard| {
                            guard
                                .output_active_workspaces
                                .get(&output_name)
                                .copied()
                                .unwrap_or(guard.active_workspace.max(1))
                        })
                        .unwrap_or(1)
                }

                async fn get_occupied_workspaces(&self) -> Vec<u32> {
                    self.snapshot
                        .read()
                        .map(|guard| guard.occupied_workspaces.clone())
                        .unwrap_or_default()
                }

                async fn get_occupied_workspaces_for_output(
                    &self,
                    output_name: String,
                ) -> Vec<u32> {
                    self.snapshot
                        .read()
                        .map(|guard| {
                            guard
                                .output_occupied_workspaces
                                .get(&output_name)
                                .cloned()
                                .unwrap_or_else(|| guard.occupied_workspaces.clone())
                        })
                        .unwrap_or_default()
                }

                async fn get_output_workspaces(&self) -> std::collections::HashMap<String, u32> {
                    self.snapshot
                        .read()
                        .map(|guard| guard.output_active_workspaces.clone())
                        .unwrap_or_default()
                }

                async fn get_windows(&self) -> Vec<(u64, String, String, u32, bool, bool)> {
                    self.snapshot
                        .read()
                        .map(|guard| {
                            guard
                                .windows
                                .iter()
                                .map(|w| {
                                    (
                                        w.id,
                                        w.app_id.clone(),
                                        w.title.clone(),
                                        w.workspace as u32,
                                        w.is_active,
                                        w.is_floating,
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                }

                async fn get_full_state(
                    &self,
                    output_name: String,
                ) -> (u32, Vec<u32>, Vec<(u64, String, String, u32, bool, bool)>) {
                    self.snapshot
                        .read()
                        .map(|guard| {
                            let active_ws = if !output_name.is_empty() {
                                guard
                                    .output_active_workspaces
                                    .get(&output_name)
                                    .copied()
                                    .unwrap_or(guard.active_workspace.max(1))
                            } else {
                                guard.active_workspace.max(1)
                            };
                            let occupied_ws = if !output_name.is_empty() {
                                guard
                                    .output_occupied_workspaces
                                    .get(&output_name)
                                    .cloned()
                                    .unwrap_or_else(|| guard.occupied_workspaces.clone())
                            } else {
                                guard.occupied_workspaces.clone()
                            };
                            let windows = guard
                                .windows
                                .iter()
                                .map(|w| {
                                    (
                                        w.id,
                                        w.app_id.clone(),
                                        w.title.clone(),
                                        w.workspace as u32,
                                        w.is_active,
                                        w.is_floating,
                                    )
                                })
                                .collect();
                            (active_ws, occupied_ws, windows)
                        })
                        .unwrap_or_else(|_| (1, Vec::new(), Vec::new()))
                }
            }

            let sleep_tx = ipc_tx.clone();
            tokio::spawn(async move {
                if let Ok(sys_conn) = zbus::Connection::system().await
                    && let Ok(proxy) = zbus::Proxy::new(
                        &sys_conn,
                        "org.freedesktop.login1",
                        "/org/freedesktop/login1",
                        "org.freedesktop.login1.Manager",
                    )
                    .await
                    && let Ok(mut stream) = proxy.receive_signal("PrepareForSleep").await
                {
                    use zbus::export::ordered_stream::OrderedStreamExt;
                    while let Some(msg) = stream.next().await {
                        let going_to_sleep = msg.body().deserialize::<bool>().unwrap_or(false);
                        let action = if going_to_sleep {
                            "prepare-sleep"
                        } else {
                            "resume-wake"
                        };
                        let _ =
                            sleep_tx.send(CompositorIpcCommand::CustomAction(action.to_string()));
                    }
                }
            });

            let server = CompositorServer {
                tx: ipc_tx,
                snapshot: server_snapshot,
            };
            let a11y_server = corten_services::a11y::AtspiAccessibleServer {
                tree: server_a11y_tree,
            };
            if let Ok(b) = zbus::connection::Builder::session()
                && let Ok(b) = b.name("org.corten.Compositor")
                && let Ok(b) = b.serve_at("/org/corten/Compositor", server)
                && let Ok(b) = b.serve_at("/org/a11y/atspi/accessible/root", a11y_server)
            {
                match b.build().await {
                    Ok(_conn) => {
                        info!("Compositor D-Bus service registered on org.corten.Compositor");
                        std::future::pending::<()>().await;
                    }
                    _ => {
                        tracing::warn!("Failed to build org.corten.Compositor D-Bus connection");
                    }
                }
            }
        });
    });

    if is_tty_mode {
        info!("Corten Compositor: Initializing Standalone TTY (DRM/KMS) hardware session...");
        let drm_state = udev::DrmBackendState::init()
            .context("Failed to initialize DRM/KMS hardware backend in TTY mode")?;
        info!(
            "DRM Hardware Backend successfully initialized on {:?} ({:?}) - Discovered {} secondary GPU(s) for PRIME offload",
            drm_state.primary_gpu,
            drm_state.dev_path,
            drm_state.secondary_gpus.len()
        );
        return udev::run_drm(
            drm_state,
            ipc_rx,
            hotkey_ipc_tx,
            shared_ipc_snapshot,
            udev_a11y_tree,
            drm_event_tx,
        );
    }

    info!("Corten Compositor: Running in Nested Window preview mode (Winit Backend).");

    // Initialize Winit Backend (Nested Window for live preview & testing)
    let (mut backend, mut winit_event_loop) = winit::init::<GlowRenderer>()
        .map_err(|e| anyhow::anyhow!("Failed to initialize Winit backend: {:?}", e))?;

    backend.window().set_title("Corten Desktop Environment");
    let _ = backend
        .window()
        .request_inner_size(smithay::reexports::winit::dpi::LogicalSize::new(
            1920.0_f64, 1080.0_f64,
        ));

    let initial_size = backend.window_size();

    let mut display: Display<CortenCompositor> = Display::new()?;
    let mut state = CortenCompositor::new(&mut display, initial_size);
    state.ipc_snapshot = shared_ipc_snapshot;
    state.a11y_bridge.tree = winit_a11y_tree;
    state.event_broadcaster = Some(main_event_tx);
    state.sync_ipc_snapshot();
    apply_display_settings(&mut state);
    apply_power_settings(&mut state);

    let mut calloop_event_loop: smithay::reexports::calloop::EventLoop<'static, CortenCompositor> =
        smithay::reexports::calloop::EventLoop::try_new()
            .context("Failed to create Calloop event loop for XWayland")?;
    let calloop_handle = calloop_event_loop.handle();
    state.loop_handle = Some(calloop_handle.clone());
    let _xwayland_guard = xwayland::start_xwayland(&display.handle(), &calloop_handle);

    // Register explicit sync (linux-drm-syncobj-v1) if supported by the host GPU in nested mode
    if let Ok(file) = std::fs::File::options()
        .read(true)
        .write(true)
        .open("/dev/dri/renderD128")
        .or_else(|_| {
            std::fs::File::options()
                .read(true)
                .write(true)
                .open("/dev/dri/card0")
        })
    {
        use std::os::fd::OwnedFd;
        let owned_fd: OwnedFd = file.into();
        let device_fd = smithay::utils::DeviceFd::from(owned_fd);
        let drm_device_fd = smithay::backend::drm::DrmDeviceFd::new(device_fd);
        if smithay::wayland::drm_syncobj::supports_syncobj_eventfd(&drm_device_fd) {
            state.syncobj_state = Some(smithay::wayland::drm_syncobj::DrmSyncobjState::new::<
                CortenCompositor,
            >(&display.handle(), drm_device_fd));
            info!("Registered wp_linux_drm_syncobj_manager_v1 (explicit sync) in nested mode");
        }
    }

    let mut input_settings = ConfigManager::<InputSettingsConfig>::load_or_default("input")
        .map(|config| config.get().clone())
        .unwrap_or_default();
    let mut keybindings = ConfigManager::<CompositorConfig>::load_or_default("compositor")
        .map(|config| config.get().keybindings.clone())
        .unwrap_or_default();
    let keyboard_layout = input_settings.keyboard_layout.clone();
    info!("Using keyboard layout: {}", keyboard_layout);

    let xkb_config = smithay::input::keyboard::XkbConfig {
        layout: &keyboard_layout,
        model: "pc105",
        options: Some(if input_settings.remap_caps_to_ctrl {
            "terminate:ctrl_alt_bksp,caps:ctrl_modifier".to_string()
        } else {
            "terminate:ctrl_alt_bksp".to_string()
        }),
        ..Default::default()
    };

    let keyboard = state
        .seat
        .add_keyboard(xkb_config, 400, 25)
        .map_err(|e| anyhow::anyhow!("Failed to add keyboard: {:?}", e))?;
    let pointer = state.seat.add_pointer();

    // Create Wayland listening socket (auto-select corten-0..31 to avoid collisions)
    let listener = ListeningSocket::bind_auto("corten", 0..32)?;
    let socket_name = listener
        .socket_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "corten-0".to_string());
    info!(
        "Corten Compositor listening on Wayland socket: {}",
        socket_name
    );
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("WAYLAND_DISPLAY", &socket_name) };

    let mut clients = Vec::new();
    let start_time = Instant::now();

    let (mut theme_mode, mut active_patina) = (ThemeMode::Dark, PatinaVariant::Rust);
    let mut window_corner_radius: f32 = 10.0;
    let mut window_border_style: String = "subtle".to_string();
    if let Ok(app_cfg) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
        theme_mode = match app_cfg.get().theme_mode {
            corten_config::ThemeModeChoice::Light => ThemeMode::Light,
            corten_config::ThemeModeChoice::Dark => ThemeMode::Dark,
        };
        active_patina = PatinaVariant::from_code(&app_cfg.get().patina);
        window_corner_radius = app_cfg.get().window_corner_radius as f32;
        window_border_style = app_cfg.get().window_border_style.clone();
        state.set_gap_for_all_outputs(app_cfg.get().window_gap);
    }
    let mut cursor_theme_manager = cursor::CursorThemeManager::new();

    if let Ok(a11y_cfg) =
        ConfigManager::<corten_config::AccessibilityConfig>::load_or_default("accessibility")
    {
        state.accessibility = a11y_cfg.get().clone();
        cursor_theme_manager.set_target_size(if state.accessibility.large_cursor {
            48
        } else {
            24
        });
    }

    let mut palette = if state.accessibility.high_contrast {
        Palette::high_contrast_dark()
    } else {
        Palette::for_mode(theme_mode).with_patina(active_patina)
    }
    .with_color_filter(match state.accessibility.color_filter {
        corten_config::ColorFilterChoice::None => corten_theme::ColorFilterMode::None,
        corten_config::ColorFilterChoice::Protanopia => corten_theme::ColorFilterMode::Protanopia,
        corten_config::ColorFilterChoice::Deuteranopia => {
            corten_theme::ColorFilterMode::Deuteranopia
        }
        corten_config::ColorFilterChoice::Tritanopia => corten_theme::ColorFilterMode::Tritanopia,
        corten_config::ColorFilterChoice::Monochrome => corten_theme::ColorFilterMode::Monochrome,
    });
    let mut bg_color = Color32F::new(
        palette.background.r,
        palette.background.g,
        palette.background.b,
        1.0,
    );

    let mut glow_shader: Option<GlowShaderManager> = None;
    let mut blur_pipeline: Option<DualKawaseBlurPipeline> = None;
    let cursor_body_ids: Vec<Id> = (0..48).map(|_| Id::new()).collect();
    let cursor_border_ids: Vec<Id> = (0..48).map(|_| Id::new()).collect();
    let cursor_tail_id = Id::new();
    let cursor_outline_id = Id::new();
    let mut cursor_commit = CommitCounter::default();

    if let Ok(cfg_mgr) = ConfigManager::<CompositorConfig>::load_or_default("compositor") {
        state.set_gap_for_all_outputs(cfg_mgr.get().gap);
        state.set_layout_mode_for_all_outputs(cfg_mgr.get().layout_mode);
        state.window_rules = cfg_mgr.get().window_rules.clone();
        info!("Compositor loaded initial gap: {}px", cfg_mgr.get().gap);
    }

    loop {
        if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
            info!("Shutdown signal received; leaving nested compositor loop for orderly cleanup.");
            break;
        }
        let _ = calloop_event_loop.dispatch(Some(std::time::Duration::from_millis(0)), &mut state);
        if state.update_idle_state() {
            request_system_suspend();
        }
        // Check for IPC configuration reload or window/workspace commands
        if let Ok(command) = ipc_rx.try_recv() {
            match command {
                CompositorIpcCommand::SwitchWorkspace(workspace) => {
                    state.switch_workspace(workspace);
                    continue;
                }
                CompositorIpcCommand::SwitchWorkspaceOnOutput(output, workspace) => {
                    state.switch_workspace_on_output(&output, workspace);
                    continue;
                }
                CompositorIpcCommand::FocusWindow(window_id) => {
                    state.focus_window_by_id(window_id);
                    continue;
                }
                CompositorIpcCommand::ToggleShowDesktop => {
                    state.toggle_show_desktop();
                    continue;
                }
                CompositorIpcCommand::ToggleOverview => {
                    state.overview_mode = !state.overview_mode;
                    continue;
                }
                CompositorIpcCommand::TakeScreenshot(region) => {
                    state.request_screenshot(region);
                    continue;
                }
                CompositorIpcCommand::CustomAction(action) => {
                    state.handle_custom_action(&action);
                    continue;
                }
                CompositorIpcCommand::ReloadConfig => {}
            }
            info!("Compositor: Reloading configuration from ~/.config/corten/compositor.toml");
            if let Ok(cfg_mgr) = ConfigManager::<CompositorConfig>::load_or_default("compositor") {
                state.set_gap_for_all_outputs(cfg_mgr.get().gap);
                state.set_layout_mode_for_all_outputs(cfg_mgr.get().layout_mode);
                state.window_rules = cfg_mgr.get().window_rules.clone();
                keybindings = cfg_mgr.get().keybindings.clone();
                info!(
                    "Compositor: Gap updated dynamically to {}px",
                    cfg_mgr.get().gap
                );
            }
            let mut palette_dirty = false;
            if let Ok(app_cfg) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
                theme_mode = match app_cfg.get().theme_mode {
                    corten_config::ThemeModeChoice::Light => ThemeMode::Light,
                    corten_config::ThemeModeChoice::Dark => ThemeMode::Dark,
                };
                active_patina = PatinaVariant::from_code(&app_cfg.get().patina);
                window_corner_radius = app_cfg.get().window_corner_radius as f32;
                window_border_style = app_cfg.get().window_border_style.clone();
                state.set_gap_for_all_outputs(app_cfg.get().window_gap);
                palette_dirty = true;
                info!("Compositor: Theme and Patina updated: {:?}", active_patina);
            }
            if let Ok(a11y_cfg) =
                ConfigManager::<corten_config::AccessibilityConfig>::load_or_default(
                    "accessibility",
                )
            {
                state.accessibility = a11y_cfg.get().clone();
                cursor_theme_manager.set_target_size(if state.accessibility.large_cursor {
                    48
                } else {
                    24
                });
                palette_dirty = true;
            }
            if palette_dirty {
                palette = if state.accessibility.high_contrast {
                    Palette::high_contrast_dark()
                } else {
                    Palette::for_mode(theme_mode).with_patina(active_patina)
                }
                .with_color_filter(match state.accessibility.color_filter {
                    corten_config::ColorFilterChoice::None => corten_theme::ColorFilterMode::None,
                    corten_config::ColorFilterChoice::Protanopia => {
                        corten_theme::ColorFilterMode::Protanopia
                    }
                    corten_config::ColorFilterChoice::Deuteranopia => {
                        corten_theme::ColorFilterMode::Deuteranopia
                    }
                    corten_config::ColorFilterChoice::Tritanopia => {
                        corten_theme::ColorFilterMode::Tritanopia
                    }
                    corten_config::ColorFilterChoice::Monochrome => {
                        corten_theme::ColorFilterMode::Monochrome
                    }
                });
                bg_color = Color32F::new(
                    palette.background.r,
                    palette.background.g,
                    palette.background.b,
                    1.0,
                );
            }
            if let Ok(config) = ConfigManager::<InputSettingsConfig>::load_or_default("input") {
                input_settings = config.get().clone();
                state.tablet_pressure_curve = input_settings.tablet_pressure_curve.clone();
                state.tablet_left_handed = input_settings.tablet_left_handed;
                let options = if input_settings.remap_caps_to_ctrl {
                    "terminate:ctrl_alt_bksp,caps:ctrl_modifier".to_string()
                } else {
                    "terminate:ctrl_alt_bksp".to_string()
                };
                let xkb_config = smithay::input::keyboard::XkbConfig {
                    layout: &input_settings.keyboard_layout,
                    model: "pc105",
                    options: Some(options),
                    ..Default::default()
                };
                if let Err(error) = keyboard.set_xkb_config(&mut state, xkb_config) {
                    tracing::warn!("Failed to apply input settings: {error:?}");
                } else {
                    info!("Compositor: keyboard/input settings applied");
                }
            }
            apply_display_settings(&mut state);
            apply_power_settings(&mut state);
        }

        // Accept new Wayland clients
        if let Some(stream) = listener.accept()? {
            info!("Accepted new Wayland client connection");
            let client = display
                .handle()
                .insert_client(stream, Arc::new(ClientState::default()))?;
            clients.push(client);
        }

        display.dispatch_clients(&mut state)?;
        display.flush_clients()?;

        let status = winit_event_loop.dispatch_new_events(|event| match event {
            WinitEvent::Resized { size, .. } => {
                state.output_size = Size::from((size.w, size.h));
                let mode = Mode {
                    size,
                    refresh: 60_000,
                };
                state.output.change_current_state(
                    Some(mode),
                    None,
                    None,
                    None,
                );
                let mut layer_map = layer_map_for_output(&state.output);
                layer_map.arrange();
            }
            WinitEvent::Input(event) => {
                state.note_input();
                match event {
                InputEvent::Keyboard { event } => {
                    let key_code = event.key_code();
                    let key_state = event.state();
                    let serial = SERIAL_COUNTER.next_serial();
                    let time = event.time_msec();

                    keyboard.input::<(), _>(
                        &mut state,
                        key_code,
                        key_state,
                        serial,
                        time,
                        |state, modifiers, keysym_handle| {
                            if state.lock_surface_active() {
                                return FilterResult::Forward;
                            }
                            if state.focused_surface_inhibits_shortcuts() {
                                return FilterResult::Forward;
                            }

                            if key_state == KeyState::Pressed {
                                let sym = keysym_handle.modified_sym().raw();
                                match sym {
                                    keysyms::KEY_Right
                                        if binding_matches(
                                            &keybindings,
                                            "move_next_output",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        state.move_active_window_between_outputs(1);
                                        return FilterResult::Intercept(());
                                    }
                                    keysyms::KEY_Left
                                        if binding_matches(
                                            &keybindings,
                                            "move_previous_output",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        state.move_active_window_between_outputs(-1);
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Return: Launch terminal or Swap Master
                                    keysyms::KEY_Return
                                        if binding_matches(
                                            &keybindings,
                                            "swap_master",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+Shift+Return (Swap Master)");
                                        state.layout.swap_master();
                                        return FilterResult::Intercept(());
                                    }
                                    keysyms::KEY_Return
                                        if binding_matches(
                                            &keybindings,
                                            "terminal",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+Return triggered (Launch Terminal)");
                                        let _ = spawn_first_terminal();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Tab: Cycle Tiling Layout Mode (Master-Stack / Dwindle / Monocle)
                                    keysyms::KEY_Tab | keysyms::KEY_ISO_Left_Tab
                                        if binding_matches(
                                            &keybindings,
                                            "cycle_layout",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        let next_mode = state.cycle_layout_mode_for_all_outputs();
                                        info!("Compositor Hotkey: Cycled tiling layout mode to {:?}", next_mode);
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + B: BSP Horizontal Split
                                    keysyms::KEY_b | keysyms::KEY_B
                                        if binding_matches(
                                            &keybindings,
                                            "split_horizontal",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+B (BSP Horizontal Split)");
                                        state.set_split_direction_for_all_outputs(crate::layout::SplitDirection::Horizontal);
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + V: BSP Vertical Split
                                    keysyms::KEY_v | keysyms::KEY_V
                                        if binding_matches(
                                            &keybindings,
                                            "split_vertical",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+V (BSP Vertical Split)");
                                        state.set_split_direction_for_all_outputs(crate::layout::SplitDirection::Vertical);
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Shift + Space: Toggle Floating vs Tiling mode
                                    keysyms::KEY_space
                                        if binding_matches(
                                            &keybindings,
                                            "toggle_floating",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+Shift+Space (Toggle Floating Window)");
                                        if let Some(active) = state.layout.active_window().cloned() {
                                            let is_now_floating = state.layout.toggle_floating(active);
                                            info!("Window floating state toggled: {}", is_now_floating);
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Alt + Space: Launch Corten AI Assistant HUD
                                    keysyms::KEY_space
                                        if binding_matches(
                                            &keybindings,
                                            "ai_assistant",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+Alt+Space triggered (Launch Corten AI HUD)");
                                        let ai_path = crate::state::resolve_corten_bin("corten-ai");
                                        let _ = std::process::Command::new(ai_path).spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Space or Super + D: Launch Spotlight launcher
                                    keysyms::KEY_space | keysyms::KEY_d | keysyms::KEY_D
                                        if binding_matches(
                                            &keybindings,
                                            "launcher",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+Space/D triggered (Launch Spotlight Launcher Toggle)");
                                        crate::state::toggle_corten_launcher_async(&[]);
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + A: Launch KDE-style Application Dashboard Grid
                                    keysyms::KEY_a | keysyms::KEY_A
                                        if binding_matches(
                                            &keybindings,
                                            "dashboard",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+A triggered (Launch Application Dashboard Grid)");
                                        let launcher_path = crate::state::resolve_corten_bin("corten-launcher");
                                        let _ = std::process::Command::new(launcher_path)
                                            .arg("--dashboard")
                                            .spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + F1 or Super + slash / question: Launch Shortcuts Cheat Sheet
                                    keysyms::KEY_F1 | keysyms::KEY_slash | keysyms::KEY_question
                                        if modifiers.logo && !modifiers.ctrl && !modifiers.alt => {
                                        info!("Compositor Hotkey: Super+F1 / Super+? triggered (Launch Shortcuts Cheat Sheet)");
                                        crate::state::toggle_corten_launcher_async(&["--cheat-sheet"]);
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + I or Super + comma: Open Settings
                                    keysyms::KEY_i | keysyms::KEY_I | keysyms::KEY_comma
                                        if binding_matches(
                                            &keybindings,
                                            "settings",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+I triggered (Launch Settings)");
                                        let settings_path = crate::state::resolve_corten_bin("corten-settings");
                                        let _ = std::process::Command::new(settings_path).spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + E: Open Corten File Manager
                                    keysyms::KEY_e | keysyms::KEY_E
                                        if binding_matches(
                                            &keybindings,
                                            "file_manager",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+E triggered (Launch Corten Files)");
                                        let files_path = crate::state::resolve_corten_bin("corten-files");
                                        let _ = std::process::Command::new(files_path).spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + W: Toggle Window Overview (Exposé)
                                    keysyms::KEY_w | keysyms::KEY_W
                                        if binding_matches(
                                            &keybindings,
                                            "window_overview",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+W triggered (Toggle Window Overview)");
                                        state.overview_mode = !state.overview_mode;
                                        if state.overview_mode {
                                            send_osd_notification("overview", "Pencere Genel Bakışı", "Exposé Modu Aktif", 100);
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + X: Open Corten Power & Session Hub
                                    keysyms::KEY_x | keysyms::KEY_X
                                        if binding_matches(
                                            &keybindings,
                                            "power_menu",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+X triggered (Open Corten Power Hub)");
                                        std::thread::spawn(|| {
                                            let _ = std::process::Command::new("busctl")
                                                .args(["--user", "call", "org.corten.Panel", "/org/corten/Panel", "org.corten.Panel", "TriggerModule", "s", "power"])
                                                .spawn();
                                        });
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Escape: Open Corten System Monitor
                                    keysyms::KEY_Escape
                                        if binding_matches(
                                            &keybindings,
                                            "task_manager",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+Escape triggered (Launch Corten Monitor)");
                                        let monitor_path = crate::state::resolve_corten_bin("corten-monitor");
                                        let _ = std::process::Command::new(monitor_path).spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + N: Open Corten Scratchpad / Quick Notes
                                    keysyms::KEY_n | keysyms::KEY_N
                                        if modifiers.logo && !modifiers.ctrl && !modifiers.alt => {
                                        info!("Compositor Hotkey: Super+N triggered (Launch Corten Scratchpad)");
                                        let launcher_path = crate::state::resolve_corten_bin("corten-launcher");
                                        let _ = std::process::Command::new(launcher_path).arg("--notes").spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + . (Period): Open Built-in Emoji & Unicode Symbol Picker
                                    keysyms::KEY_period
                                        if modifiers.logo && !modifiers.ctrl && !modifiers.alt => {
                                        info!("Compositor Hotkey: Super+. triggered (Launch Corten Emoji & Unicode Picker)");
                                        let launcher_path = crate::state::resolve_corten_bin("corten-launcher");
                                        let _ = std::process::Command::new(launcher_path).arg("--emoji").spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Shift + C: System-Wide Color Picker / Eyedropper
                                    keysyms::KEY_c | keysyms::KEY_C
                                        if modifiers.logo && modifiers.shift && !modifiers.ctrl && !modifiers.alt => {
                                        info!("Compositor Hotkey: Super+Shift+C triggered (Color Picker)");
                                        state.handle_custom_action("color-picker");
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Q: Close active window
                                    keysyms::KEY_q | keysyms::KEY_Q
                                        if binding_matches(
                                            &keybindings,
                                            "close_window",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+Q triggered (Close Active Window)");
                                        if let Some(active) = state.layout.active_window() {
                                            layout::close_window(active);
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Shift + J: Swap with next window
                                    keysyms::KEY_j | keysyms::KEY_J
                                        if binding_matches(
                                            &keybindings,
                                            "swap_next",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+Shift+J (Swap Next)");
                                        state.layout.swap_next();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + J: Focus next window
                                    keysyms::KEY_j | keysyms::KEY_J
                                        if binding_matches(
                                            &keybindings,
                                            "focus_next",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+J (Focus Next)");
                                        if let Some(next) = state.layout.next_window().cloned() {
                                            if let Some(x11) = next.x11_surface() {
                                                let _ = x11.set_activated(true);
                                            }
                                            if let Some(surface) = layout::window_wl_surface(&next)
                                                && let Some(kb) = state.seat.get_keyboard() {
                                                    kb.set_focus(state, Some(surface), serial);
                                                }
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Shift + K: Swap with previous window
                                    keysyms::KEY_k | keysyms::KEY_K
                                        if binding_matches(
                                            &keybindings,
                                            "swap_previous",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+Shift+K (Swap Previous)");
                                        state.layout.swap_prev();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + K: Focus previous window
                                    keysyms::KEY_k | keysyms::KEY_K
                                        if binding_matches(
                                            &keybindings,
                                            "focus_previous",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+K (Focus Previous)");
                                        if let Some(prev) = state.layout.prev_window().cloned() {
                                            if let Some(x11) = prev.x11_surface() {
                                                let _ = x11.set_activated(true);
                                            }
                                            if let Some(surface) = layout::window_wl_surface(&prev)
                                                && let Some(kb) = state.seat.get_keyboard() {
                                                    kb.set_focus(state, Some(surface), serial);
                                                }
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + H: Decrease master ratio
                                    keysyms::KEY_h | keysyms::KEY_H
                                        if binding_matches(
                                            &keybindings,
                                            "decrease_master",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        state.adjust_master_ratio_for_all_outputs(-0.05);
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + L: Increase master ratio (or Super+Ctrl+L to lock screen)
                                    keysyms::KEY_l | keysyms::KEY_L
                                        if binding_matches(
                                            &keybindings,
                                            "lock_screen",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Lock Screen triggered");
                                        let lock_path = crate::state::resolve_corten_bin("corten-lock");
                                        let _ = std::process::Command::new(lock_path).spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    keysyms::KEY_l | keysyms::KEY_L
                                        if binding_matches(
                                            &keybindings,
                                            "increase_master",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        state.adjust_master_ratio_for_all_outputs(0.05);
                                        return FilterResult::Intercept(());
                                    }
                                    keysyms::KEY_f | keysyms::KEY_F
                                        if binding_matches(&keybindings, "toggle_fullscreen", sym, modifiers.logo, modifiers.ctrl, modifiers.alt, modifiers.shift) => {
                                        if let Some(active) = state.layout.active_window().cloned() {
                                            let is_fs = state.layout.toggle_fullscreen(&active);
                                            if let Some(toplevel) = active.toplevel() {
                                                toplevel.with_pending_state(|tstate| {
                                                    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
                                                    if is_fs { tstate.states.set(State::Fullscreen); } else { tstate.states.unset(State::Fullscreen); }
                                                });
                                                toplevel.send_configure();
                                            } else if let Some(x11) = active.x11_surface() {
                                                let _ = x11.set_fullscreen(is_fs);
                                            }
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    keysyms::KEY_m | keysyms::KEY_M
                                        if binding_matches(&keybindings, "minimize", sym, modifiers.logo, modifiers.ctrl, modifiers.alt, modifiers.shift) => {
                                        if let Some(active) = state.layout.active_window().cloned() { state.layout.remove_window(&active); }
                                        return FilterResult::Intercept(());
                                    }
                                    sym @ keysyms::KEY_1..=keysyms::KEY_9
                                        if binding_matches(&keybindings, &format!("move_to_workspace_{}", sym - keysyms::KEY_1 + 1), sym, modifiers.logo, modifiers.ctrl, modifiers.alt, modifiers.shift) => {
                                        let ws = (sym - keysyms::KEY_1 + 1) as usize;
                                        state.move_active_window_to_workspace(ws);
                                        return FilterResult::Intercept(());
                                    }
                                    sym @ keysyms::KEY_1..=keysyms::KEY_9
                                        if binding_matches(&keybindings, &format!("workspace_{}", sym - keysyms::KEY_1 + 1), sym, modifiers.logo, modifiers.ctrl, modifiers.alt, modifiers.shift) => {
                                        let ws = sym - keysyms::KEY_1 + 1;
                                        state.switch_workspace(ws as usize);
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Shift + S: Region screenshot (Snip)
                                    keysyms::KEY_s | keysyms::KEY_S
                                        if binding_matches(
                                            &keybindings,
                                            "screenshot",
                                            sym,
                                            modifiers.logo,
                                            modifiers.ctrl,
                                            modifiers.alt,
                                            modifiers.shift,
                                        ) => {
                                        info!("Compositor Hotkey: Super+Shift+S (Region Screenshot)");
                                        trigger_screenshot(state, &hotkey_ipc_tx, true);
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + V: Clipboard history modal
                                    keysyms::KEY_v | keysyms::KEY_V if modifiers.logo && !modifiers.ctrl && !modifiers.alt => {
                                        info!("Compositor Hotkey: Super+V triggered (Open Clipboard History)");
                                        let _ = std::process::Command::new("busctl")
                                            .args(["--user", "call", "org.corten.Panel", "/org/corten/Panel", "org.corten.Panel", "TriggerModule", "s", "clipboard"])
                                            .spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + D: Desktop Context Hub
                                    keysyms::KEY_d | keysyms::KEY_D if modifiers.logo && !modifiers.ctrl && !modifiers.alt => {
                                        info!("Compositor Hotkey: Super+D triggered (Toggle Desktop Context Hub)");
                                        let _ = std::process::Command::new("busctl")
                                            .args(["--user", "call", "org.corten.Panel", "/org/corten/Panel", "org.corten.Panel", "TriggerModule", "s", "desktop_hub"])
                                            .spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    _ if crate::hotkeys::dispatch_shared_compositor_hotkey(
                                        state,
                                        sym,
                                        modifiers.logo,
                                        modifiers.ctrl,
                                        modifiers.alt,
                                        modifiers.shift,
                                    ) => {
                                        return FilterResult::Intercept(());
                                    }
                                    _ => {}
                                }
                            }
                            if key_state == KeyState::Pressed {
                                let sym = keysym_handle.modified_sym().raw();
                                // Alt + Tab: Modern window switcher cycling
                                if modifiers.alt && (sym == keysyms::KEY_Tab || sym == keysyms::KEY_ISO_Left_Tab) {
                                    info!("Compositor Hotkey: Alt+Tab (Cycle Active Window Focus)");
                                    let target_window = if modifiers.shift {
                                        state.layout.prev_window().cloned()
                                    } else {
                                        state.layout.next_window().cloned()
                                    };
                                    if let Some(target) = target_window {
                                        state.layout.set_active_window(&target);
                                        if let Some(x11) = target.x11_surface() {
                                            let _ = x11.set_activated(true);
                                        }
                                        if let Some(surface) = layout::window_wl_surface(&target)
                                            && let Some(kb) = state.seat.get_keyboard() {
                                                kb.set_focus(state, Some(surface), serial);
                                            }
                                    }
                                    let _ = Command::new("busctl")
                                        .args(["--user", "call", "org.corten.Panel", "/org/corten/Panel", "org.corten.Panel", "TriggerModule", "s", "osd:switcher"])
                                        .spawn();
                                    return FilterResult::Intercept(());
                                }
                                if sym == keysyms::KEY_Print || sym == keysyms::KEY_Sys_Req {
                                    info!("Compositor Hotkey: PrintScreen (Full Screenshot)");
                                    trigger_screenshot(state, &hotkey_ipc_tx, false);
                                    return FilterResult::Intercept(());
                                }
                                if handle_media_keys(sym) {
                                    return FilterResult::Intercept(());
                                }
                            }
                            FilterResult::Forward
                        },
                    );
                }
                InputEvent::PointerMotionAbsolute { event } => {
                    let window_size = backend.window_size();
                    let x = event.x_transformed(window_size.w);
                    let y = event.y_transformed(window_size.h);
                    let raw_loc = Point::from((x, y));
                    let dx = raw_loc.x - state.cursor_location.x;
                    let dy = raw_loc.y - state.cursor_location.y;
                    let loc = state.apply_pointer_constraint(raw_loc);
                    state.cursor_location = loc;

                    let serial = SERIAL_COUNTER.next_serial();
                    let motion_event = MotionEvent {
                        location: loc,
                        serial,
                        time: event.time_msec(),
                    };

                    // Detect surface under pointer
                    let mut focused_surface = None;

                    if state.session_locked
                        && let Some((lock_surface, _)) = state.lock_surfaces.first() {
                            focused_surface =
                                Some((lock_surface.wl_surface().clone(), Point::from((0.0, 0.0))));
                        }

                    // 1. Check Top-level Layer Surfaces first (Overlay/Top)
                    if focused_surface.is_none() {
                        let layer_map = layer_map_for_output(&state.output);
                        'top_layers: for layer in [Layer::Overlay, Layer::Top] {
                            for layer_surface in layer_map.layers_on(layer).rev() {
                                if let Some(geo) = layer_map.layer_geometry(layer_surface)
                                    && geo.to_f64().contains(loc) {
                                        let rel_point = loc - geo.loc.to_f64();
                                        if let Some((surface, sub_loc)) = layer_surface.surface_under(rel_point, WindowSurfaceType::all()) {
                                            let surface_origin = (geo.loc + sub_loc).to_f64();
                                            focused_surface = Some((surface, surface_origin));
                                            break 'top_layers;
                                        }
                                    }
                            }
                        }
                    }

                    // 1.5. Check XWayland override-redirect surfaces (menus, tooltips)
                    if focused_surface.is_none() {
                        for (or_surface, or_geo) in state.override_redirect_surfaces().into_iter().rev() {
                            if or_geo.to_f64().contains(loc) {
                                focused_surface = Some((or_surface, or_geo.loc.to_f64()));
                                break;
                            }
                        }
                    }

                    // 2. Check Tiled & Floating Windows (reverse order so top/floating windows win)
                    if focused_surface.is_none() {
                        let usable_area = {
                            let mut map = layer_map_for_output(&state.output);
                            map.arrange();
                            map.non_exclusive_zone()
                        };
                        let layouts = state.layout.arrange_animated(usable_area);
                        for (window, rect) in layouts.into_iter().rev() {
                            if !layout::is_window_mapped(&window) {
                                continue;
                            }
                            if rect.to_f64().contains(loc) {
                                let is_fs = state.layout.is_fullscreen(&window);
                                let titlebar_h = layout::window_titlebar_height(&window, is_fs) as f64;
                                if titlebar_h > 0.0 && (loc.y - rect.loc.y as f64) < titlebar_h {
                                    // Pointer is over the server-side titlebar header!
                                    // DO NOT route motion to client surface.
                                    focused_surface = None;
                                    break;
                                }
                                let csd_offset = layout::window_csd_offset(&window);
                                let origin = Point::from((
                                    (rect.loc.x - csd_offset.x) as f64,
                                    (rect.loc.y - csd_offset.y) as f64 + titlebar_h,
                                ));
                                let rel_point = loc - origin;
                                match window.surface_under(rel_point, WindowSurfaceType::all()) { Some((surface, sub_loc)) => {
                                    let surface_origin = Point::from((
                                        origin.x + sub_loc.x as f64,
                                        origin.y + sub_loc.y as f64,
                                    ));
                                    focused_surface = Some((surface, surface_origin));
                                    break;
                                } _ => { if let Some(surface) = layout::window_wl_surface(&window) {
                                    focused_surface = Some((surface, origin));
                                    break;
                                }}}
                            }
                        }
                    }

                    // 3. Check Bottom-level Layer Surfaces (Bottom/Background, wallpaper)
                    if focused_surface.is_none() {
                        let layer_map = layer_map_for_output(&state.output);
                        'bottom_layers: for layer in [Layer::Bottom, Layer::Background] {
                            for layer_surface in layer_map.layers_on(layer).rev() {
                                if let Some(geo) = layer_map.layer_geometry(layer_surface)
                                    && geo.to_f64().contains(loc) {
                                        let rel_point = loc - geo.loc.to_f64();
                                        if let Some((surface, sub_loc)) = layer_surface.surface_under(rel_point, WindowSurfaceType::all()) {
                                            let surface_origin = (geo.loc + sub_loc).to_f64();
                                            focused_surface = Some((surface, surface_origin));
                                            break 'bottom_layers;
                                        }
                                    }
                            }
                        }
                    }

                    // If currently adjusting tiling split ratio
                    if let Some((drag_start, initial_ratio)) = state.split_drag {
                        let usable_area = {
                            let mut map = layer_map_for_output(&state.output);
                            map.arrange();
                            map.non_exclusive_zone()
                        };
                        let delta_x = (loc.x - drag_start.x) as f32;
                        let new_ratio = initial_ratio + delta_x / (usable_area.size.w.max(1) as f32);
                        state.layout.set_master_ratio(new_ratio);
                    } else { match state.dragged_window.clone() { Some((ref window, drag_start_cursor, initial_rect)) => {
                        // If currently dragging or resizing a window, update its floating geometry smoothly
                        let delta_x = (loc.x - drag_start_cursor.x).round() as i32;
                        let delta_y = (loc.y - drag_start_cursor.y).round() as i32;
                        let mut new_rect = initial_rect;
                        new_rect.loc.x += delta_x;
                        new_rect.loc.y += delta_y;
                        let usable_area = {
                            let mut map = layer_map_for_output(&state.output);
                            map.arrange();
                            map.non_exclusive_zone()
                        };
                        let screen_bounds = {
                            let scale = state.output.current_scale().fractional_scale();
                            let size = state.output
                                .current_mode()
                                .map(|m| smithay::utils::Size::from(((m.size.w as f64 / scale).round() as i32, (m.size.h as f64 / scale).round() as i32)))
                                .unwrap_or(usable_area.size);
                            Rectangle::new(smithay::utils::Point::from((0, 0)), size)
                        };
                        let other_windows: Vec<Rectangle<i32, smithay::utils::Logical>> = state
                            .layout
                            .arrange(usable_area)
                            .into_iter()
                            .filter(|(w, _)| w != window)
                            .map(|(_, r)| r)
                            .collect();

                        // Apply 14px magnetic edge snapping to screen borders, dock, and neighboring windows
                        let snapped_rect = layout::snap_window_to_edges(
                            new_rect,
                            usable_area,
                            screen_bounds,
                            &other_windows,
                            14,
                        );
                        state.layout.set_floating_geometry(window, snapped_rect);
                        let is_near_left = loc.x <= (usable_area.loc.x + 35) as f64;
                        let is_near_right = loc.x >= (usable_area.loc.x + usable_area.size.w - 35) as f64;
                        let is_near_top = loc.y <= (usable_area.loc.y + 35) as f64;
                        let is_near_bottom = loc.y >= (usable_area.loc.y + usable_area.size.h - 35) as f64;

                        let half_w = usable_area.size.w / 2;
                        let half_h = usable_area.size.h / 2;

                        if is_near_left && is_near_top {
                            // Top-left quarter tile
                            state.snap_preview = Some(Rectangle::new(
                                usable_area.loc,
                                Size::from((half_w, half_h)),
                            ));
                            state.drag_split_target = None;
                        } else if is_near_right && is_near_top {
                            // Top-right quarter tile
                            state.snap_preview = Some(Rectangle::new(
                                Point::from((usable_area.loc.x + half_w, usable_area.loc.y)),
                                Size::from((half_w, half_h)),
                            ));
                            state.drag_split_target = None;
                        } else if is_near_left && is_near_bottom {
                            // Bottom-left quarter tile
                            state.snap_preview = Some(Rectangle::new(
                                Point::from((usable_area.loc.x, usable_area.loc.y + half_h)),
                                Size::from((half_w, half_h)),
                            ));
                            state.drag_split_target = None;
                        } else if is_near_right && is_near_bottom {
                            // Bottom-right quarter tile
                            state.snap_preview = Some(Rectangle::new(
                                Point::from((usable_area.loc.x + half_w, usable_area.loc.y + half_h)),
                                Size::from((half_w, half_h)),
                            ));
                            state.drag_split_target = None;
                        } else if is_near_left {
                            // Left half
                            state.snap_preview = Some(Rectangle::new(
                                usable_area.loc,
                                Size::from((half_w, usable_area.size.h)),
                            ));
                            state.drag_split_target = None;
                        } else if is_near_right {
                            // Right half
                            state.snap_preview = Some(Rectangle::new(
                                Point::from((usable_area.loc.x + half_w, usable_area.loc.y)),
                                Size::from((half_w, usable_area.size.h)),
                            ));
                            state.drag_split_target = None;
                        } else if is_near_top {
                            // Full maximize
                            state.snap_preview = Some(usable_area);
                            state.drag_split_target = None;
                        } else {
                            let window_rects = state.layout.arrange_animated(usable_area);
                            match crate::layout::detect_drag_tab_target(loc, &window_rects, window)
                            { Some((_tab_win, tab_rect)) => {
                                state.snap_preview = Some(tab_rect);
                                state.drag_split_target = None;
                            } _ => { match crate::layout::detect_drag_split_target(loc, &window_rects, window)
                            { Some((target_win, dir, new_on_first, preview_rect)) => {
                                state.snap_preview = Some(preview_rect);
                                state.drag_split_target = Some((target_win, dir, new_on_first, preview_rect));
                            } _ => {
                                state.snap_preview = None;
                                state.drag_split_target = None;
                            }}}}
                        }
                    } _ => { match state.resized_window.clone() { Some((ref window, resize_start_cursor, initial_rect, resize_edge)) => {
                        let delta = Point::from((
                            loc.x - resize_start_cursor.x,
                            loc.y - resize_start_cursor.y,
                        ));
                        let min_size = layout::window_min_size(window);
                        let max_size = layout::window_max_size(window);
                        let titlebar_h = layout::window_titlebar_height(window, state.layout.is_fullscreen(window));
                        let new_rect = layout::compute_resized_rect(
                            initial_rect,
                            resize_edge,
                            delta,
                            min_size,
                            max_size,
                            titlebar_h,
                        );
                        let usable_area = state.usable_area_for_window(window);
                        let screen_bounds = {
                            let target_out = state.output_for_window(window);
                            let scale = target_out.current_scale().fractional_scale();
                            let size = target_out
                                .current_mode()
                                .map(|m| smithay::utils::Size::from(((m.size.w as f64 / scale).round() as i32, (m.size.h as f64 / scale).round() as i32)))
                                .unwrap_or(usable_area.size);
                            Rectangle::new(smithay::utils::Point::from((0, 0)), size)
                        };
                        let other_windows: Vec<Rectangle<i32, smithay::utils::Logical>> = state
                            .layout
                            .arrange(usable_area)
                            .into_iter()
                            .filter(|(w, _)| w != window)
                            .map(|(_, r)| r)
                            .collect();

                        let snapped_rect = layout::snap_window_resize(
                            new_rect,
                            usable_area,
                            screen_bounds,
                            &other_windows,
                            14,
                        );
                        state.layout.set_floating_geometry(window, snapped_rect);
                        layout::configure_window_geometry(window, snapped_rect);
                    } _ => {
                        if let Some((_, ref mut end_pt)) = state.selection_box {
                            *end_pt = loc;
                        }
                        // Check hovering over window borders / corners for resize cursor or Snap Layouts Flyout
                        let usable_area = {
                            let mut map = layer_map_for_output(&state.output);
                            map.arrange();
                            map.non_exclusive_zone()
                        };
                        let window_rects = state.layout.arrange_animated(usable_area);
                        match window_rects.into_iter().rev().find(|(_, r)| r.to_f64().contains(loc)) { Some((_, rect)) => {
                            if let Some(zone) = crate::layout::hit_test_snap_flyout(rect, loc) {
                                state.snap_preview = Some(crate::layout::compute_snap_zone_rect(
                                    usable_area,
                                    zone,
                                    state.layout.gap(),
                                ));
                            } else {
                                state.snap_preview = None;
                            }
                            let rw_x = loc.x - rect.loc.x as f64;
                            let rw_y = loc.y - rect.loc.y as f64;
                            let w_f = rect.size.w as f64;
                            let h_f = rect.size.h as f64;
                            if rw_x >= (w_f - 16.0) && rw_y >= (h_f - 16.0) {
                                state.cursor_status = smithay::input::pointer::CursorImageStatus::Named(
                                    smithay::input::pointer::CursorIcon::NwseResize,
                                );
                            } else if rw_x <= 16.0 && rw_y >= (h_f - 16.0) {
                                state.cursor_status = smithay::input::pointer::CursorImageStatus::Named(
                                    smithay::input::pointer::CursorIcon::NeswResize,
                                );
                            } else if rw_x >= (w_f - 8.0) || rw_x <= 8.0 {
                                state.cursor_status = smithay::input::pointer::CursorImageStatus::Named(
                                    smithay::input::pointer::CursorIcon::EwResize,
                                );
                            } else if rw_y >= (h_f - 8.0) || rw_y <= 8.0 {
                                state.cursor_status = smithay::input::pointer::CursorImageStatus::Named(
                                    smithay::input::pointer::CursorIcon::NsResize,
                                );
                            }
                        } _ => {
                            state.snap_preview = None;
                            if let Some(sx) = state.layout.split_x(usable_area)
                                && (loc.x as i32 - sx).abs() <= 8
                                    && loc.y >= usable_area.loc.y as f64
                                    && loc.y <= (usable_area.loc.y + usable_area.size.h) as f64
                                {
                                    state.cursor_status = smithay::input::pointer::CursorImageStatus::Named(
                                        smithay::input::pointer::CursorIcon::EwResize,
                                    );
                                }
                        }}
                    }}}}}

                    let rel_event = RelativeMotionEvent {
                        delta: (dx, dy).into(),
                        delta_unaccel: (dx, dy).into(),
                        utime: u64::from(event.time_msec()) * 1000,
                    };
                    pointer.relative_motion(&mut state, focused_surface.clone(), &rel_event);
                    pointer.motion(&mut state, focused_surface, &motion_event);
                    pointer.frame(&mut state);
                }
                InputEvent::PointerButton { event } => {
                    let serial = SERIAL_COUNTER.next_serial();
                    let button_code = event.button_code();
                    let button_state = event.state();
                    let button_event = ButtonEvent {
                        serial,
                        time: event.time_msec(),
                        button: button_code,
                        state: button_state,
                    };

                    // On mouse click, activate and focus the clicked window or layer surface
                    if button_state == ButtonState::Pressed {
                        if state.session_locked {
                            if let Some((lock_surface, _)) = state.lock_surfaces.first() {
                                let surface = lock_surface.wl_surface().clone();
                                if let Some(kb) = state.seat.get_keyboard() {
                                    kb.set_focus(&mut state, Some(surface), serial);
                                }
                            }
                            pointer.button(&mut state, &button_event);
                            pointer.frame(&mut state);
                            return;
                        }

                        let mut handled_focus = false;
                        let logo_held = state
                            .seat
                            .get_keyboard()
                            .is_some_and(|kb| kb.modifier_state().logo);

                        // Check if clicking on tiling split divider to drag-resize
                        if (button_code == 1 || button_code == 272) && !logo_held {
                            let usable_area = {
                                let mut map = layer_map_for_output(&state.output);
                                map.arrange();
                                map.non_exclusive_zone()
                            };
                            if let Some(sx) = state.layout.split_x(usable_area)
                                && (state.cursor_location.x as i32 - sx).abs() <= 6
                                    && state.cursor_location.y >= (usable_area.loc.y + 36) as f64
                                    && state.cursor_location.y <= (usable_area.loc.y + usable_area.size.h) as f64
                                {
                                    state.split_drag = Some((state.cursor_location, state.layout.master_ratio()));
                                }
                        }

                        // 1. Check if clicking on an interactive layer shell surface (Lock screen, Widgets, Dock)
                        let layer_surface_to_focus = {
                            let layer_map = layer_map_for_output(&state.output);
                            let mut found = None;
                            for layer_surface in layer_map.layers().rev() {
                                if let Some(geo) = layer_map.layer_geometry(layer_surface)
                                    && geo.to_f64().contains(state.cursor_location) {
                                        let rel_point = state.cursor_location - geo.loc.to_f64();
                                        if let Some((surface, _)) = layer_surface.surface_under(rel_point, WindowSurfaceType::all()) {
                                            found = Some(surface);
                                            break;
                                        }
                                    }
                            }
                            found
                        };

                        if let Some(surface) = layer_surface_to_focus {
                            if let Some(kb) = state.seat.get_keyboard() {
                                kb.set_focus(&mut state, Some(surface), serial);
                            }
                            handled_focus = true;
                        }

                        // 2. If not a layer surface, check tiling/floating windows
                        if !handled_focus {
                            let usable_area = {
                                let map = layer_map_for_output(&state.output);
                                map.non_exclusive_zone()
                            };
                            let window_rects = if state.overview_mode {
                                state.layout.arrange_overview(usable_area)
                            } else {
                                state.layout.arrange_animated(usable_area)
                            };
                            let clicked = window_rects
                                .into_iter()
                                .rev()
                                .find(|(_, rect)| rect.to_f64().contains(state.cursor_location));

                            if let Some((window, rect)) = clicked {
                                if state.overview_mode {
                                    state.overview_mode = false;
                                }
                                state.layout.set_active_window(&window);
                                if let Some(x11) = window.x11_surface() {
                                    let _ = x11.set_activated(true);
                                }
                                if let Some(surface) = layout::window_wl_surface(&window)
                                    && let Some(kb) = state.seat.get_keyboard() {
                                        kb.set_focus(&mut state, Some(surface), serial);
                                    }
                                state.sync_ipc_snapshot();

                                let rel_win_x = state.cursor_location.x - rect.loc.x as f64;
                                let rel_win_y = state.cursor_location.y - rect.loc.y as f64;
                                let w_f64 = rect.size.w as f64;

                                if button_code == 1 || button_code == 272 {
                                    if let Some(zone) = crate::layout::hit_test_snap_flyout(rect, state.cursor_location) {
                                        let usable_area = {
                                            let mut map = layer_map_for_output(&state.output);
                                            map.arrange();
                                            map.non_exclusive_zone()
                                        };
                                        let snap_target = crate::layout::compute_snap_zone_rect(
                                            usable_area,
                                            zone,
                                            state.layout.gap(),
                                        );
                                        state.layout.set_fullscreen(&window, false);
                                        state.layout.set_maximized(&window, usable_area, false);
                                        state.layout.set_floating_geometry(&window, snap_target);
                                        layout::configure_window_geometry(&window, snap_target);
                                        state.snap_preview = None;
                                        state.sync_ipc_snapshot();
                                        return;
                                    }
                                    let titlebar_h = layout::window_titlebar_height(
                                        &window,
                                        state.layout.is_fullscreen(&window),
                                    ) as f64;
                                    if titlebar_h > 0.0 && (0.0..=titlebar_h).contains(&rel_win_y) {
                                        // Clicked inside the Server-Side Window Titlebar!
                                        let win_id = crate::layout::window_stable_id(&window);
                                        let tabs = state.layout.window_tabs_metadata(win_id);
                                        if let Some(tab_idx) = crate::layout::hit_test_titlebar_tab(
                                            rect,
                                            tabs.len(),
                                            state.cursor_location,
                                        ) {
                                            let target_tab_id = tabs[tab_idx].0;
                                            if let Some((_, focused_win)) =
                                                state.layout.focus_window_by_id(target_tab_id)
                                                && let Some(surface) = layout::window_wl_surface(&focused_win)
                                                    && let Some(kb) = state.seat.get_keyboard() {
                                                        kb.set_focus(&mut state, Some(surface), serial);
                                                    }
                                            state.sync_ipc_snapshot();
                                            return;
                                        }
                                        if rel_win_x >= (w_f64 - 32.0) && rel_win_x <= w_f64 {
                                            // Close button clicked
                                            info!("Window Titlebar: Close clicked on {:?}", window);
                                            crate::layout::close_window(&window);
                                            return;
                                        } else if rel_win_x >= (w_f64 - 58.0) && rel_win_x < (w_f64 - 32.0) {
                                            // Maximize / Restore toggle clicked
                                            let usable_area = state.usable_area_for_window(&window);
                                            info!("Window Titlebar: Maximize toggle clicked on {:?}", window);
                                            state.layout.toggle_maximize(&window, usable_area);
                                            state.sync_ipc_snapshot();
                                            return;
                                        } else if rel_win_x >= (w_f64 - 88.0) && rel_win_x < (w_f64 - 58.0) {
                                            // Minimize clicked
                                            info!("Window Titlebar: Minimize clicked on {:?}", window);
                                            state.layout.minimize_window(&window);
                                            state.sync_ipc_snapshot();
                                            return;
                                        } else {
                                            // Check double click to maximize / restore
                                            if let Some((prev_win, prev_instant)) = state.last_titlebar_click.take()
                                                && prev_win == window && prev_instant.elapsed().as_millis() < 320 {
                                                    let usable_area = state.usable_area_for_window(&window);
                                                    info!("Window Titlebar: Double-click maximize toggle on {:?}", window);
                                                    state.layout.toggle_maximize(&window, usable_area);
                                                    state.sync_ipc_snapshot();
                                                    return;
                                                }
                                            state.last_titlebar_click = Some((window.clone(), std::time::Instant::now()));

                                            // Dragging anywhere else on the titlebar moves the window
                                            info!("Window Titlebar: Initiating drag on {:?}", window);
                                            let mut drag_rect = rect;
                                            if state.layout.is_maximized(&window) {
                                                let usable_area = state.usable_area_for_window(&window);
                                                let restored = state.layout.set_maximized(&window, usable_area, false);
                                                let new_x = (state.cursor_location.x - (restored.size.w as f64 * 0.5)).round() as i32;
                                                let new_y = state.cursor_location.y.round() as i32 - 12;
                                                drag_rect = Rectangle::new(Point::from((new_x, new_y)), restored.size);
                                                state.layout.set_floating_geometry(&window, drag_rect);
                                                layout::configure_window_geometry(&window, drag_rect);
                                            }
                                            state.dragged_window = Some((window.clone(), state.cursor_location, drag_rect));
                                            return;
                                        }
                                    } else if titlebar_h > 0.0
                                        && let Some(edge) = layout::hit_test_resize_edge(rect, state.cursor_location, 8.0) {
                                            info!("Main: Edge/Corner click ({:?}) initiating resize on {:?}", edge, window);
                                            state.resized_window = Some((window.clone(), state.cursor_location, rect, edge));
                                        }
                                } else if logo_held && (button_code == 1 || button_code == 272) {
                                    state.dragged_window =
                                        Some((window.clone(), state.cursor_location, rect));
                                } else if logo_held && (button_code == 3 || button_code == 273) {
                                    state.resized_window =
                                        Some((window.clone(), state.cursor_location, rect, layout::ResizeEdge::BottomRight));
                                }
                            } else if (button_code == 1 || button_code == 272) && !logo_held {
                                state.selection_box = Some((state.cursor_location, state.cursor_location));
                            } else if (button_code == 2 || button_code == 3 || button_code == 273) && !logo_held {
                                info!("Right-clicked on empty desktop: Spawning Corten Desktop Context Menu at cursor");
                                let cur_x = state.cursor_location.x.round() as i32;
                                let cur_y = state.cursor_location.y.round() as i32;
                                crate::state::spawn_desktop_context_menu_async(cur_x, cur_y);
                            }
                        }
                    } else if button_state == ButtonState::Released {
                        if let Some((window, _, _)) = state.dragged_window.take() {
                            let usable_area = {
                                let mut map = layer_map_for_output(&state.output);
                                map.arrange();
                                map.non_exclusive_zone()
                            };
                            let window_rects = state.layout.arrange_animated(usable_area);
                            match crate::layout::detect_drag_tab_target(
                                state.cursor_location,
                                &window_rects,
                                &window,
                            ) { Some((target_win, target_rect)) => {
                                let host_id = crate::layout::window_stable_id(&target_win);
                                let guest_id = crate::layout::window_stable_id(&window);
                                state.layout.merge_windows_into_tab_group(host_id, guest_id);
                                state.layout.set_fullscreen(&window, false);
                                state.layout.set_floating_geometry(&window, target_rect);
                                layout::configure_window_geometry(&window, target_rect);
                                state.snap_preview = None;
                            } _ => { match state.drag_split_target.take() { Some((target_win, dir, new_on_first, _)) => {
                                state.layout.set_fullscreen(&window, false);
                                state.layout.set_floating(&window, false);
                                state.layout.set_mode(corten_config::LayoutMode::Bsp);
                                state.layout.drag_split_window(window.clone(), target_win, dir, new_on_first);
                                state.snap_preview = None;
                            } _ => if let Some(snap_rect) = state.snap_preview.take() {
                                state.layout.set_fullscreen(&window, false);
                                state.layout.set_floating_geometry(&window, snap_rect);
                                layout::configure_window_geometry(&window, snap_rect);
                            } else if let Some(geo) = state.layout.floating_geometry(&window) {
                                layout::configure_window_geometry(&window, geo);
                            }}}}
                        }
                        state.dragged_window = None;
                        state.resized_window = None;
                        state.split_drag = None;
                        state.snap_preview = None;
                        state.drag_split_target = None;
                        state.selection_box = None;
                    }

                    pointer.button(&mut state, &button_event);
                    pointer.frame(&mut state);
                }
                InputEvent::PointerAxis { event } => {
                    let mod_state = state.seat.get_keyboard().map(|kb| kb.modifier_state());
                    let zoom_scroll = mod_state.as_ref().is_some_and(|m| m.logo && m.ctrl);
                    if zoom_scroll {
                        let v_amount = event
                            .amount(Axis::Vertical)
                            .or_else(|| event.amount_v120(Axis::Vertical).map(|s| s * 15.0 / 120.0))
                            .unwrap_or(0.0);
                        if v_amount.abs() > 0.01 {
                            state.adjust_zoom(if v_amount < 0.0 { 0.15 } else { -0.15 });
                            return;
                        }
                    }
                    let mut frame = AxisFrame::new(event.time_msec()).source(event.source());
                    for axis in [Axis::Horizontal, Axis::Vertical] {
                        let amount = event
                            .amount(axis)
                            .or_else(|| event.amount_v120(axis).map(|steps| steps * 15.0 / 120.0));
                        if let Some(amount) = amount {
                            frame = frame.value(axis, amount);
                            let direction = event.relative_direction(axis);
                            let direction = if input_settings.natural_scrolling {
                                match direction {
                                    AxisRelativeDirection::Identical => AxisRelativeDirection::Inverted,
                                    AxisRelativeDirection::Inverted => AxisRelativeDirection::Identical,
                                }
                            } else {
                                direction
                            };
                            frame = frame.relative_direction(axis, direction);
                        }
                    }
                    pointer.axis(&mut state, frame);
                    pointer.frame(&mut state);
                }
                _ => {}
                }
            }
            WinitEvent::CloseRequested => {}
            _ => {}
        });

        match status {
            PumpStatus::Continue => {}
            PumpStatus::Exit(_) => break,
        }

        let size = backend.window_size();
        let damage = Rectangle::from_size(size);

        static PRINTED_GEO: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if !PRINTED_GEO.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let lm = layer_map_for_output(&state.output);
            info!(
                "DEBUG GEOMETRY: backend_size: {:?}, output_size: {:?}, layers: {:?}",
                size,
                state.output_size,
                lm.layers()
                    .map(|l| (l.layer(), lm.layer_geometry(l)))
                    .collect::<Vec<_>>()
            );
        }

        // Compute usable area after panel/layer-shell reservations
        let usable_area = {
            let mut layer_map = layer_map_for_output(&state.output);
            layer_map.arrange();
            layer_map.non_exclusive_zone()
        };

        // Arrange tiled windows with smooth spring physics transitions or Overview Mode
        let layouts = if state.overview_mode {
            state.layout.arrange_overview(usable_area)
        } else {
            state.layout.arrange_animated(usable_area)
        };

        {
            let (renderer, mut framebuffer) =
                backend.bind().map_err(|e| anyhow::anyhow!("{:?}", e))?;

            let mut elements: Vec<CustomRenderElement> = Vec::new();
            state.step_visual_animations();
            let output_scale = state.output.current_scale();
            let base_scale = output_scale.fractional_scale();
            let fractional_scale = base_scale * state.zoom_level.clamp(1.0, 4.0);
            let zoom_pan = state.zoom_pan_offset(size, base_scale);
            let render_scale = Scale::from(fractional_scale);
            let fullscreen_win = state.layout.active_fullscreen_window();

            // 1. Render Background & Bottom Layer Shell surfaces (Wallpapers / desktop widgets)
            // Performance bypass: Skip background rendering entirely when a window is fullscreen
            if fullscreen_win.is_none() {
                let layer_map = layer_map_for_output(&state.output);
                for layer in [Layer::Background, Layer::Bottom] {
                    for layer_surface in layer_map.layers_on(layer) {
                        if let Some(geo) = layer_map.layer_geometry(layer_surface) {
                            let loc =
                                geo.loc.to_physical_precise_round(fractional_scale) + zoom_pan;
                            let surface_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                                render_elements_from_surface_tree(
                                    renderer,
                                    layer_surface.wl_surface(),
                                    loc,
                                    render_scale,
                                    1.0,
                                    Kind::Unspecified,
                                );
                            elements.extend(
                                surface_elements.into_iter().map(CustomRenderElement::from),
                            );
                        }
                    }
                }
            }

            // Initialize GLSL hardware-accelerated neon glow shader and Dual-Kawase blur pipeline on first frame
            if glow_shader.is_none() {
                let gles: &mut smithay::backend::renderer::gles::GlesRenderer =
                    renderer.borrow_mut();
                match GlowShaderManager::new(gles) {
                    Ok(manager) => glow_shader = Some(manager),
                    Err(e) => tracing::warn!("Failed to compile GLSL neon glow shader: {:?}", e),
                }
                if let Ok(pipeline) = DualKawaseBlurPipeline::new(gles) {
                    blur_pipeline = Some(pipeline);
                }
            }

            // Per-window Dual-Kawase background blur for translucent/terminal windows & snap preview
            if fullscreen_win.is_none() && !state.game_mode {
                let mut win_glass_regions = Vec::new();
                for (window, geo) in layouts.iter() {
                    let win_alpha = state.layout.window_alpha(window);
                    let is_blur =
                        crate::layout::is_window_blur_eligible(window, &state.window_rules)
                            || win_alpha < 0.99;
                    if is_blur {
                        let titlebar_h = layout::window_titlebar_height(
                            window,
                            state.layout.is_fullscreen(window),
                        );
                        let body_rect = Rectangle::new(
                            geo.loc + Point::from((0, titlebar_h)),
                            Size::from((geo.size.w.max(10), (geo.size.h - titlebar_h).max(10))),
                        );
                        win_glass_regions.push(GlassRegion::new(body_rect, 8.0, 0.22));
                    }
                }
                if let Some(snap_rect) = state.snap_preview {
                    let snap_margin = 8;
                    let inset_rect = Rectangle::new(
                        Point::from((snap_rect.loc.x + snap_margin, snap_rect.loc.y + snap_margin)),
                        Size::from((
                            (snap_rect.size.w - snap_margin * 2).max(50),
                            (snap_rect.size.h - snap_margin * 2).max(50),
                        )),
                    );
                    win_glass_regions.push(GlassRegion::snap_preview(inset_rect));
                }
                if !win_glass_regions.is_empty()
                    && let Some(pipeline) = blur_pipeline.as_mut()
                {
                    let backdrops = pipeline.generate_glass_backdrops(
                        renderer,
                        &elements,
                        false,
                        size,
                        fractional_scale,
                        bg_color,
                        &win_glass_regions,
                    );
                    elements.extend(backdrops.into_iter().map(CustomRenderElement::from));
                }
            }

            // 2. Render Windows (Direct scanout / bypass mode when fullscreen, or tiled with neon glow)
            if let Some(ref fs_win) = fullscreen_win {
                // High-performance Direct Scanout / Bypass: Render only fullscreen window without glow, borders, or titlebar
                if let Some(surface) = layout::window_wl_surface(fs_win) {
                    let win_alpha = state.layout.window_alpha(fs_win);
                    let window_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                        render_elements_from_surface_tree(
                            renderer,
                            &surface,
                            (0, 0),
                            render_scale,
                            win_alpha,
                            Kind::Unspecified,
                        );
                    elements.extend(window_elements.into_iter().map(CustomRenderElement::from));

                    // Render any popups (context menus, dropdowns)
                    for (popup, popup_offset) in PopupManager::popups_for_surface(&surface) {
                        let popup_loc = popup_offset.to_physical_precise_round(fractional_scale);
                        let popup_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                            render_elements_from_surface_tree(
                                renderer,
                                popup.wl_surface(),
                                popup_loc,
                                render_scale,
                                1.0,
                                Kind::Unspecified,
                            );
                        elements.extend(popup_elements.into_iter().map(CustomRenderElement::from));
                    }
                }
            } else {
                // 3. Windows with 1:1 Touchpad Gesture tracking & Kinetic Momentum (Rendered in front-to-back Z-order)
                state.step_gesture_kinetic();
                let gesture_offset_x = (state.gesture_swipe_dx * 1.4).round() as i32;
                let active_window = state.layout.active_window().cloned();
                for (window, rect) in layouts.iter().rev() {
                    if !layout::is_window_mapped(window) {
                        continue;
                    }
                    let mut shifted_rect = *rect;
                    shifted_rect.loc.x += gesture_offset_x;
                    let is_active = active_window.as_ref() == Some(window);

                    let phys_rect = Rectangle::new(
                        shifted_rect.loc.to_physical_precise_round(fractional_scale) + zoom_pan,
                        shifted_rect
                            .size
                            .to_physical_precise_round(fractional_scale),
                    );
                    let glow_rect = crate::state::compute_zoomed_glow_rect(
                        shifted_rect,
                        state.zoom_level,
                        zoom_pan,
                        base_scale,
                    );

                    let is_fs = state.layout.is_fullscreen(window);
                    let titlebar_h = layout::window_titlebar_height(window, is_fs);
                    let is_tiled = state.layout.is_window_tiled(window);
                    let is_max = state.layout.is_maximized(window);
                    let zero_radius = is_fs || is_max || (is_tiled && state.layout.gap() == 0);
                    let corner_radii: (f32, f32) = if zero_radius || window_corner_radius <= 0.5 {
                        (0.0, 0.0)
                    } else if titlebar_h > 0 {
                        (window_corner_radius, 2.0)
                    } else {
                        (window_corner_radius, window_corner_radius)
                    };

                    if is_active {
                        let is_high_vis = state.accessibility.high_vis_focus_ring
                            || state.accessibility.high_contrast;
                        let (border_alpha, border_w, halo_radiance) = if is_high_vis {
                            (0.95_f32, 2.0_f32, 0.16_f32)
                        } else {
                            match window_border_style.as_str() {
                                "none" => (0.0, 0.0, 0.0),
                                "copper" => (0.82, 1.5, 0.0),
                                _ => (0.48, 1.25, 0.0),
                            }
                        };
                        let (ring_width, ring_color) = if is_high_vis {
                            (
                                3,
                                Color32F::new(
                                    palette.primary.r,
                                    palette.primary.g,
                                    palette.primary.b,
                                    0.95,
                                ),
                            )
                        } else {
                            (
                                1,
                                Color32F::new(
                                    palette.primary.r,
                                    palette.primary.g,
                                    palette.primary.b,
                                    border_alpha,
                                ),
                            )
                        };

                        // Render clean GLSL elevation shadow + subtle hairline glass rim
                        if let Some(manager) = glow_shader.as_ref() {
                            let halo_color = [
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                halo_radiance,
                            ];
                            let border_color = [
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                border_alpha,
                            ];
                            let glow_elem = manager.create_styled_element(
                                glow_rect,
                                38,
                                corner_radii,
                                halo_color,
                                border_color,
                                border_w,
                            );
                            elements.push(CustomRenderElement::from(glow_elem));
                        } else if border_alpha > 0.01 {
                            draw_glow_ring(&mut elements, phys_rect, 0, ring_width, ring_color, 2);
                        }
                    } else {
                        // Grounded elevation shadow + subtle 1px glass rim for inactive windows
                        if let Some(manager) = glow_shader.as_ref() {
                            let shadow_elem = manager.create_inactive_shadow_element(
                                glow_rect,
                                corner_radii,
                                [1.0, 1.0, 1.0, 0.09],
                            );
                            elements.push(CustomRenderElement::from(shadow_elem));
                        } else {
                            let inactive_color = Color32F::new(1.0, 1.0, 1.0, 0.12);
                            draw_glow_ring(&mut elements, phys_rect, 0, 1, inactive_color, 2);
                        }
                    }

                    let csd_offset = layout::window_csd_offset(window);

                    // Render window surface content (both Wayland toplevels and XWayland surfaces)
                    if let Some(surface) = layout::window_wl_surface(window) {
                        let client_logical_loc =
                            shifted_rect.loc - csd_offset + Point::from((0, titlebar_h));
                        let client_phys_loc = client_logical_loc
                            .to_physical_precise_round(fractional_scale)
                            + zoom_pan;
                        let win_alpha = state.layout.window_alpha(window);
                        let window_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                            render_elements_from_surface_tree(
                                renderer,
                                &surface,
                                client_phys_loc,
                                render_scale,
                                win_alpha,
                                Kind::Unspecified,
                            );
                        if is_fs || titlebar_h == 0 {
                            elements
                                .extend(window_elements.into_iter().map(CustomRenderElement::from));
                        } else {
                            let body = Rectangle::new(
                                client_phys_loc,
                                (shifted_rect.size - smithay::utils::Size::from((0, titlebar_h)))
                                    .to_physical_precise_round(fractional_scale),
                            );
                            elements.extend(clip_to_body(window_elements, fractional_scale, body));
                        }

                        // Render any popups (context menus, dropdowns)
                        for (popup, popup_offset) in PopupManager::popups_for_surface(&surface) {
                            let popup_loc = (client_logical_loc + popup_offset)
                                .to_physical_precise_round(fractional_scale)
                                + zoom_pan;
                            let popup_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                                render_elements_from_surface_tree(
                                    renderer,
                                    popup.wl_surface(),
                                    popup_loc,
                                    render_scale,
                                    1.0,
                                    Kind::Unspecified,
                                );
                            elements
                                .extend(popup_elements.into_iter().map(CustomRenderElement::from));
                        }
                    }

                    // Render clean Corten titlebar & controls ONLY for Server-Side Decorated (SSD) windows
                    if titlebar_h > 0 {
                        let win_id = layout::window_stable_id(window);
                        let tabs = state.layout.window_tabs_metadata(win_id);
                        let (app_id, title) = layout::window_metadata(window);
                        let rounded_header = glow_shader.as_ref().map(|manager| {
                            let (header_bg, _, _) =
                                crate::render::compute_titlebar_colors(&palette, is_active, false);
                            let bg_top = if is_active {
                                [
                                    (header_bg.r() + 0.035).min(1.0),
                                    (header_bg.g() + 0.038).min(1.0),
                                    (header_bg.b() + 0.048).min(1.0),
                                    0.98,
                                ]
                            } else {
                                [
                                    (header_bg.r() + 0.018).min(1.0),
                                    (header_bg.g() + 0.020).min(1.0),
                                    (header_bg.b() + 0.025).min(1.0),
                                    0.95,
                                ]
                            };
                            let bg_bottom = [header_bg.r(), header_bg.g(), header_bg.b(), 0.98];
                            let rim = if is_active {
                                [1.0, 1.0, 1.0, 0.16]
                            } else {
                                [1.0, 1.0, 1.0, 0.07]
                            };
                            let header_log = Rectangle::new(
                                glow_rect.loc,
                                Size::from((glow_rect.size.w, titlebar_h)),
                            );
                            manager.create_rounded_header_element(
                                header_log,
                                corner_radii.0,
                                bg_top,
                                bg_bottom,
                                rim,
                            )
                        });
                        draw_window_titlebar(
                            &mut elements,
                            renderer,
                            phys_rect,
                            is_active,
                            &palette,
                            is_max,
                            &app_id,
                            &title,
                            fractional_scale,
                            &mut state.title_cache,
                            false,
                            state.cursor_location,
                            is_tiled,
                            &tabs,
                            rounded_header,
                            state.snap_flyout_window == Some(win_id),
                        );
                    }
                }
            }

            // 2.5. Render XWayland override-redirect surfaces (menus, tooltips, splash screens)
            for (or_surface, or_geo) in state.override_redirect_surfaces() {
                let loc = or_geo.loc.to_physical_precise_round(fractional_scale);
                let or_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                    render_elements_from_surface_tree(
                        renderer,
                        &or_surface,
                        loc,
                        render_scale,
                        1.0,
                        Kind::Unspecified,
                    );
                elements.extend(or_elements.into_iter().map(CustomRenderElement::from));
            }

            // Render silky translucent luminous mist preview when snapping/docking a window
            if fullscreen_win.is_none() {
                if let Some(snap_rect) = state.snap_preview {
                    let snap_margin = 10;
                    let inset_loc =
                        Point::from((snap_rect.loc.x + snap_margin, snap_rect.loc.y + snap_margin));
                    let inset_size = Size::from((
                        (snap_rect.size.w - snap_margin * 2).max(50),
                        (snap_rect.size.h - snap_margin * 2).max(50),
                    ));
                    let inset_log = Rectangle::new(inset_loc, inset_size);
                    if let Some(manager) = glow_shader.as_ref() {
                        let mist_elem = manager.create_snap_preview_element(
                            inset_log,
                            12.0,
                            [palette.primary.r, palette.primary.g, palette.primary.b, 1.0],
                        );
                        elements.push(CustomRenderElement::from(mist_elem));
                    } else {
                        let phys_snap = Rectangle::new(
                            inset_loc.to_physical_precise_round(fractional_scale),
                            inset_size.to_physical_precise_round(fractional_scale),
                        );
                        let snap_color = Color32F::new(
                            palette.primary.r,
                            palette.primary.g,
                            palette.primary.b,
                            0.10,
                        );
                        elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                            Id::new(),
                            phys_snap,
                            CommitCounter::default(),
                            snap_color,
                            Kind::Unspecified,
                        )));
                    }
                }

                // Render crisp Windows-style rubberband selection box on empty desktop
                if let Some((start, end)) = state.selection_box {
                    let min_x = (start.x.min(end.x) * fractional_scale).round() as i32;
                    let max_x = (start.x.max(end.x) * fractional_scale).round() as i32;
                    let min_y = (start.y.min(end.y) * fractional_scale).round() as i32;
                    let max_y = (start.y.max(end.y) * fractional_scale).round() as i32;
                    let w = (max_x - min_x).max(2);
                    let h = (max_y - min_y).max(2);

                    let sel_fill = Color32F::new(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.20,
                    );
                    let sel_border = Color32F::new(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.90,
                    );

                    // 1. Uniform semi-transparent interior
                    elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                        Id::new(),
                        Rectangle::new(
                            Point::from((min_x + 1, min_y + 1)),
                            Size::from((w - 2, h - 2)),
                        ),
                        cursor_commit,
                        sel_fill,
                        Kind::Unspecified,
                    )));

                    // 2. Crisp 1px solid border lines (Top, Bottom, Left, Right)
                    elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                        Id::new(),
                        Rectangle::new(Point::from((min_x, min_y)), Size::from((w, 1))),
                        cursor_commit,
                        sel_border,
                        Kind::Unspecified,
                    )));
                    elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                        Id::new(),
                        Rectangle::new(Point::from((min_x, min_y + h - 1)), Size::from((w, 1))),
                        cursor_commit,
                        sel_border,
                        Kind::Unspecified,
                    )));
                    elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                        Id::new(),
                        Rectangle::new(
                            Point::from((min_x, min_y + 1)),
                            Size::from((1, (h - 2).max(1))),
                        ),
                        cursor_commit,
                        sel_border,
                        Kind::Unspecified,
                    )));
                    elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                        Id::new(),
                        Rectangle::new(
                            Point::from((min_x + w - 1, min_y + 1)),
                            Size::from((1, (h - 2).max(1))),
                        ),
                        cursor_commit,
                        sel_border,
                        Kind::Unspecified,
                    )));
                }
            }

            // 3. Render Top & Overlay Layer Shell surfaces (Panels, Launchers) with Dual-Kawase frosted glass blur
            {
                if fullscreen_win.is_none() {
                    let mut glass_regions: Vec<GlassRegion> = Vec::new();
                    {
                        let layer_map = layer_map_for_output(&state.output);
                        for layer in [Layer::Top, Layer::Overlay] {
                            if state.session_locked && layer == Layer::Overlay {
                                continue;
                            }
                            for layer_surface in layer_map.layers_on(layer) {
                                if layer_surface.namespace() == "corten-panel"
                                    && let Some(geo) = layer_map.layer_geometry(layer_surface)
                                {
                                    let capsule_w = (geo.size.w - 116).max(0);
                                    if capsule_w > 60 && geo.size.h >= 68 {
                                        let capsule_x = geo.loc.x + 96;
                                        let capsule_y = if geo.loc.y <= 30 {
                                            geo.loc.y + 12
                                        } else {
                                            geo.loc.y + geo.size.h - 62
                                        };
                                        let inset_geo = Rectangle::new(
                                            Point::from((capsule_x, capsule_y)),
                                            Size::from((capsule_w, 44)),
                                        );
                                        glass_regions.push(GlassRegion::panel(inset_geo));
                                    }
                                }
                            }
                        }
                    }
                    if !glass_regions.is_empty()
                        && let Some(pipeline) = blur_pipeline.as_mut()
                    {
                        let backdrops = pipeline.generate_glass_backdrops(
                            renderer,
                            &elements,
                            false,
                            size,
                            fractional_scale,
                            bg_color,
                            &glass_regions,
                        );
                        elements.extend(backdrops.into_iter().map(CustomRenderElement::from));
                    }
                }

                let layer_map = layer_map_for_output(&state.output);
                for layer in [Layer::Top, Layer::Overlay] {
                    if state.session_locked && layer == Layer::Overlay {
                        continue;
                    }
                    for layer_surface in layer_map.layers_on(layer) {
                        if let Some(geo) = layer_map.layer_geometry(layer_surface) {
                            let loc = geo.loc.to_physical_precise_round(fractional_scale);
                            let surface_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                                render_elements_from_surface_tree(
                                    renderer,
                                    layer_surface.wl_surface(),
                                    loc,
                                    render_scale,
                                    1.0,
                                    Kind::Unspecified,
                                );
                            elements.extend(
                                surface_elements.into_iter().map(CustomRenderElement::from),
                            );
                        }
                    }
                }
            }

            // ext-session-lock surfaces are always the final visual layer.
            state.recover_crashed_lock();
            for (lock_surface, lock_output) in &state.lock_surfaces {
                if !state.lock_surface_for_output(lock_output, &state.output) {
                    continue;
                }
                let surface_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                    render_elements_from_surface_tree(
                        renderer,
                        lock_surface.wl_surface(),
                        (0, 0),
                        render_scale,
                        1.0,
                        Kind::Unspecified,
                    );
                elements.extend(surface_elements.into_iter().map(CustomRenderElement::from));
            }

            // Render active Drag & Drop icon surface or client custom cursor surface
            if let Some(ref dnd_surface) = state.dnd_icon {
                let loc = state
                    .cursor_location
                    .to_i32_round::<i32>()
                    .to_physical_precise_round(fractional_scale);
                let dnd_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                    render_elements_from_surface_tree(
                        renderer,
                        dnd_surface,
                        loc,
                        render_scale,
                        1.0,
                        Kind::Unspecified,
                    );
                elements.extend(dnd_elements.into_iter().map(CustomRenderElement::from));
            }
            if !state.is_pointer_locked() {
                match state.cursor_status {
                    smithay::input::pointer::CursorImageStatus::Hidden => {}
                    smithay::input::pointer::CursorImageStatus::Surface(ref cursor_surface) => {
                        let loc = state
                            .cursor_location
                            .to_i32_round::<i32>()
                            .to_physical_precise_round(fractional_scale);
                        let cursor_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                            render_elements_from_surface_tree(
                                renderer,
                                cursor_surface,
                                loc,
                                render_scale,
                                1.0,
                                Kind::Cursor,
                            );
                        elements.extend(cursor_elements.into_iter().map(CustomRenderElement::from));
                    }
                    smithay::input::pointer::CursorImageStatus::Named(icon) => {
                        cursor_commit.increment();
                        let cx = state.cursor_location.x.round() as i32;
                        let cy = state.cursor_location.y.round() as i32;

                        let mut rendered_xcursor = false;
                        if let Some(loaded) = cursor_theme_manager.get(icon) {
                            let phys_loc: smithay::utils::Point<f64, smithay::utils::Physical> =
                                smithay::utils::Point::from((
                                    ((cx - loaded.xhot) as f64) * fractional_scale,
                                    ((cy - loaded.yhot) as f64) * fractional_scale,
                                ));
                            if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
                                renderer,
                                phys_loc,
                                &loaded.buffer,
                                None,
                                None,
                                None,
                                Kind::Cursor,
                            ) {
                                elements.push(CustomRenderElement::from(elem));
                                rendered_xcursor = true;
                            }
                        }

                        if !rendered_xcursor {
                            draw_semantic_cursor(
                                &mut elements,
                                icon,
                                cx,
                                cy,
                                &cursor_body_ids,
                                &cursor_border_ids,
                                &cursor_tail_id,
                                &cursor_outline_id,
                                cursor_commit,
                                &palette,
                            );
                        }
                    }
                }
            }

            if state.screen_asleep {
                elements.push(screen_sleep_overlay(size));
            } else if state.night_light_enabled {
                if let Some(pipeline) = blur_pipeline.as_mut() {
                    match pipeline.generate_night_light_element(
                        renderer,
                        &elements,
                        false,
                        size,
                        fractional_scale,
                        bg_color,
                    ) {
                        Some(nl_elem) => {
                            elements.push(CustomRenderElement::from(nl_elem));
                        }
                        _ => {
                            elements.push(night_light_overlay(size));
                        }
                    }
                } else {
                    elements.push(night_light_overlay(size));
                }
            }

            let mut frame = renderer
                .render(&mut framebuffer, size, Transform::Flipped180)
                .map_err(|e| anyhow::anyhow!("{:?}", e))?;

            // Clear with Corten Dark background
            frame
                .clear(bg_color, &[damage])
                .map_err(|e| anyhow::anyhow!("{:?}", e))?;

            // Draw all composed surfaces in correct Z-order (Smithay's draw_render_elements expects front-to-back)
            elements.reverse();
            draw_render_elements(&mut frame, 1.0, &elements, &[damage])
                .map_err(|e| anyhow::anyhow!("{:?}", e))?;
            elements.reverse();

            let _ = frame.finish().map_err(|e| anyhow::anyhow!("{:?}", e))?;

            // Fulfill any pending `wlr-screencopy-unstable-v1` frames and native screenshots
            screencopy::fulfill_pending_captures(
                &mut state,
                renderer,
                &elements,
                false,
                size,
                fractional_scale,
                bg_color,
            );

            // Notify surfaces about frame timing
            let elapsed_ms = start_time.elapsed().as_millis() as u32;
            for (window, _) in &layouts {
                if let Some(surface) = layout::window_wl_surface(window) {
                    send_frames_surface_tree(&surface, elapsed_ms);
                }
            }
            for (or_surface, _) in state.override_redirect_surfaces() {
                send_frames_surface_tree(&or_surface, elapsed_ms);
            }
            {
                let layer_map = layer_map_for_output(&state.output);
                for layer_surface in layer_map.layers() {
                    send_frames_surface_tree(layer_surface.wl_surface(), elapsed_ms);
                }
            }
            for (lock_surface, _) in &state.lock_surfaces {
                send_frames_surface_tree(lock_surface.wl_surface(), elapsed_ms);
            }
            if let Some(ref dnd_surface) = state.dnd_icon {
                send_frames_surface_tree(dnd_surface, elapsed_ms);
            }
            if let smithay::input::pointer::CursorImageStatus::Surface(ref cursor_surface) =
                state.cursor_status
            {
                send_frames_surface_tree(cursor_surface, elapsed_ms);
            }

            // Accept new Wayland clients
            if let Some(stream) = listener.accept()? {
                info!("Accepted new Wayland client connection");
                let client = display
                    .handle()
                    .insert_client(stream, Arc::new(ClientState::default()))?;
                clients.push(client);
            }

            display.dispatch_clients(&mut state)?;
            display.flush_clients()?;
        }

        backend
            .submit(Some(&[damage]))
            .map_err(|e| anyhow::anyhow!("{:?}", e))?;

        // Adaptive frame rate limiter: 120Hz (8ms) while animating/dragging, 60Hz (16ms) steady, 50ms asleep
        // Tearing / VRR: If an active fullscreen window requests tearing (presentation hint Async), bypass sleep
        let is_animating = state.layout.is_animating(usable_area) || state.dragged_window.is_some();
        let allows_tearing = state.allow_tearing
            || state
                .layout
                .active_fullscreen_window()
                .and_then(|w| layout::window_wl_surface(&w))
                .is_some_and(|surf| state.tearing_state.is_tearing_requested(&surf));

        let pace_ms = if allows_tearing {
            0
        } else if state.screen_asleep {
            50
        } else if is_animating {
            8
        } else {
            16
        };
        if pace_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(pace_ms));
        }
    }

    info!("Corten Compositor shut down safely.");
    Ok(())
}

pub(crate) fn send_frames_surface_tree(surface: &WlSurface, time: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_surf, states, &()| {
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time);
            }
            let mut presentation_feedbacks = std::mem::take(
                &mut states
                    .cached_state
                    .get::<smithay::wayland::presentation::PresentationFeedbackCachedState>()
                    .current()
                    .callbacks,
            );
            for feedback in presentation_feedbacks.drain(..) {
                feedback.discarded();
            }
        },
        |_, _, &()| true,
    );
}

pub(crate) fn screen_sleep_overlay(
    size: Size<i32, smithay::utils::Physical>,
) -> CustomRenderElement {
    CustomRenderElement::from(SolidColorRenderElement::new(
        Id::new(),
        Rectangle::from_size(size),
        CommitCounter::default(),
        Color32F::new(0.0, 0.0, 0.0, 1.0),
        Kind::Unspecified,
    ))
}

pub(crate) fn night_light_overlay(
    size: Size<i32, smithay::utils::Physical>,
) -> CustomRenderElement {
    CustomRenderElement::from(SolidColorRenderElement::new(
        Id::new(),
        Rectangle::from_size(size),
        CommitCounter::default(),
        Color32F::new(1.0, 0.34, 0.08, 0.12),
        Kind::Unspecified,
    ))
}

/// Draws an aesthetic box border/ring around `rect` at distance `expand` with thickness `thickness`.
/// Generates clean, seamless continuous borders without gaps or integer-division clipping.
pub(crate) fn draw_glow_ring(
    elements: &mut Vec<CustomRenderElement>,
    rect: Rectangle<i32, smithay::utils::Physical>,
    expand: i32,
    thickness: i32,
    color: Color32F,
    _corner_radius: i32,
) {
    let t = thickness.max(1);

    // Bounding coordinates of this ring
    let left_x = rect.loc.x - expand - t;
    let right_x = rect.loc.x + rect.size.w + expand;
    let top_y = rect.loc.y - expand - t;
    let bottom_y = rect.loc.y + rect.size.h + expand;
    let total_w = right_x - left_x + t;
    let total_h = bottom_y - top_y + t;

    // 1. Top bar
    elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
        Id::new(),
        Rectangle::new(Point::from((left_x, top_y)), (total_w, t).into()),
        CommitCounter::default(),
        color,
        Kind::Unspecified,
    )));

    // 2. Bottom bar
    elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
        Id::new(),
        Rectangle::new(Point::from((left_x, bottom_y)), (total_w, t).into()),
        CommitCounter::default(),
        color,
        Kind::Unspecified,
    )));

    // 3. Left bar (between top and bottom bars)
    if total_h > t * 2 {
        elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
            Id::new(),
            Rectangle::new(
                Point::from((left_x, top_y + t)),
                (t, total_h - t * 2).into(),
            ),
            CommitCounter::default(),
            color,
            Kind::Unspecified,
        )));
    }

    // 4. Right bar (between top and bottom bars)
    if total_h > t * 2 {
        elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
            Id::new(),
            Rectangle::new(
                Point::from((right_x, top_y + t)),
                (t, total_h - t * 2).into(),
            ),
            CommitCounter::default(),
            color,
            Kind::Unspecified,
        )));
    }
}

/// Rasterizes window title text using embedded Noto Sans font into a memory render buffer.
pub(crate) fn render_title_text_to_buffer(
    title: &str,
    scale: f64,
    color: [u8; 3],
) -> Option<(MemoryRenderBuffer, i32, i32)> {
    let clean = title.trim();
    if clean.is_empty() {
        return None;
    }
    let font = FontRef::try_from_slice(corten_theme::NOTO_SANS_BYTES).ok()?;
    let buf_scale = scale.round().max(1.0) as i32;
    let font_size = 15.5 * (buf_scale as f32);
    let scaled_font = font.as_scaled(PxScale::from(font_size));

    let mut glyphs = Vec::new();
    let mut caret = 2.0f32;
    let max_title_w = 460.0 * (buf_scale as f32);

    for c in clean.chars() {
        if caret > max_title_w {
            break;
        }
        let glyph_id = scaled_font.glyph_id(c);
        let advance = scaled_font.h_advance(glyph_id);
        let mut glyph = scaled_font.scaled_glyph(c);
        glyph.position = ab_glyph::point(caret, scaled_font.ascent() + 1.0);
        caret += advance;
        glyphs.push(glyph);
    }

    let total_w = ((caret.ceil() as i32 + 8).max(buf_scale) / buf_scale) * buf_scale;
    let raw_h = (scaled_font.ascent() - scaled_font.descent()).ceil() as i32 + 6;
    let total_h = ((raw_h.max(buf_scale) + buf_scale - 1) / buf_scale) * buf_scale;

    let mut alpha_map = vec![0.0f32; (total_w * total_h) as usize];

    for glyph in glyphs {
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|x, y, coverage| {
                if coverage <= 0.02 {
                    return;
                }
                // Gamma-boosted coverage + faux-semibold horizontal reinforcement for crisp title legibility
                let boosted = coverage.powf(0.55).clamp(0.0, 1.0);
                let base_x = bounds.min.x as i32 + x as i32;
                let py = bounds.min.y as i32 + y as i32;
                if py >= 0 && py < total_h {
                    if base_x >= 0 && base_x < total_w {
                        let idx = (py * total_w + base_x) as usize;
                        alpha_map[idx] = (alpha_map[idx] + boosted).min(1.0);
                    }
                    if base_x + 1 >= 0 && base_x + 1 < total_w {
                        let idx2 = (py * total_w + base_x + 1) as usize;
                        alpha_map[idx2] = (alpha_map[idx2] + boosted * 0.55).min(1.0);
                    }
                }
            });
        }
    }

    let mut pixels = vec![0u8; (total_w * total_h * 4) as usize];
    for (i, &cov) in alpha_map.iter().enumerate() {
        let out_a = (cov * 255.0).round().clamp(0.0, 255.0) as u8;
        if out_a > 0 {
            let idx = i * 4;
            // Premultiplied ARGB8888 (little-endian BGRA bytes) for Smithay GlowRenderer
            pixels[idx] = (color[2] as u32 * out_a as u32 / 255) as u8; // B
            pixels[idx + 1] = (color[1] as u32 * out_a as u32 / 255) as u8; // G
            pixels[idx + 2] = (color[0] as u32 * out_a as u32 / 255) as u8; // R
            pixels[idx + 3] = out_a; // A
        }
    }

    let mut buffer = MemoryRenderBuffer::new(
        Fourcc::Argb8888,
        (total_w, total_h),
        buf_scale,
        Transform::Normal,
        None,
    );
    let draw_res = buffer.render().draw(|buf| {
        buf.copy_from_slice(&pixels);
        Result::<_, ()>::Ok(vec![smithay::utils::Rectangle::from_size(
            (total_w, total_h).into(),
        )])
    });

    if draw_res.is_ok() {
        Some((buffer, total_w, total_h))
    } else {
        None
    }
}

/// Rasterizes an anti-aliased circular window control button (`'c'` close, `'M'` maximize,
/// `'R'` restore, `'m'` minimize) into a `MemoryRenderBuffer`.
/// Close uses signature Corten orange; Minimize and Maximize use modern dark graphite gray.
pub(crate) fn render_circular_button_to_buffer(
    kind: char,
    is_active: bool,
    is_hovered: bool,
    scale: f64,
) -> Option<(MemoryRenderBuffer, i32, i32)> {
    let buf_scale = scale.round().max(1.0) as i32;
    let s = scale as f32;
    let raw_sz = (24.0 * scale).round() as i32;
    let total_sz = ((raw_sz.max(buf_scale) + buf_scale - 1) / buf_scale) * buf_scale;

    let cx = (total_sz as f32 - 1.0) * 0.5;
    let cy = (total_sz as f32 - 1.0) * 0.5;
    let radius = if is_hovered { 10.2 * s } else { 9.5 * s };
    let stroke_r = (0.88 * s).max(0.82);

    let (top_rgb, bot_rgb, rim_rgb, rim_alpha, glow_rgb, sym_rgb, sym_alpha) = if kind == 'c' {
        // Corten Rust Orange for Close
        if is_hovered {
            (
                [244.0f32, 118.0, 58.0],
                [214.0f32, 86.0, 30.0],
                [255.0f32, 180.0, 138.0],
                0.75f32,
                [238.0f32, 104.0, 44.0],
                [255.0f32, 255.0, 255.0],
                1.0f32,
            )
        } else if is_active {
            (
                [224.0, 98.0, 42.0],
                [190.0, 72.0, 24.0],
                [250.0, 145.0, 95.0],
                0.48,
                [224.0, 98.0, 42.0],
                [252.0, 248.0, 245.0],
                0.92,
            )
        } else {
            (
                [132.0, 66.0, 38.0],
                [104.0, 50.0, 28.0],
                [165.0, 90.0, 58.0],
                0.28,
                [132.0, 66.0, 38.0],
                [225.0, 215.0, 210.0],
                0.65,
            )
        }
    } else {
        // Modern Dark Graphite Gray for Minimize & Maximize
        if is_hovered {
            (
                [72.0, 80.0, 96.0],
                [52.0, 58.0, 72.0],
                [138.0, 150.0, 175.0],
                0.65,
                [95.0, 108.0, 132.0],
                [255.0, 255.0, 255.0],
                1.0,
            )
        } else if is_active {
            (
                [48.0, 53.0, 64.0],
                [35.0, 39.0, 48.0],
                [95.0, 106.0, 126.0],
                0.45,
                [48.0, 53.0, 64.0],
                [230.0, 235.0, 245.0],
                0.86,
            )
        } else {
            (
                [34.0, 38.0, 46.0],
                [26.0, 29.0, 36.0],
                [65.0, 72.0, 86.0],
                0.25,
                [34.0, 38.0, 46.0],
                [175.0, 182.0, 196.0],
                0.52,
            )
        }
    };

    let mut pixels = vec![0u8; (total_sz * total_sz * 4) as usize];

    for py in 0..total_sz {
        for px in 0..total_sz {
            let dx = px as f32 - cx;
            let dy = py as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let d_circle = dist - radius;

            // 1. Soft outer ambient halo when hovered
            let glow_cov = if is_hovered && d_circle > -0.5 && d_circle < 2.2 * s {
                let norm = (1.0 - d_circle.max(0.0) / (2.2 * s)).clamp(0.0, 1.0);
                norm * norm * 0.36
            } else {
                0.0
            };

            // 2. Anti-aliased circular disc coverage
            let circle_cov = (0.5 - d_circle).clamp(0.0, 1.0);
            if circle_cov <= 0.0 && glow_cov <= 0.0 {
                continue;
            }

            let mut out_r = glow_rgb[0];
            let mut out_g = glow_rgb[1];
            let mut out_b = glow_rgb[2];
            let mut out_a = glow_cov;

            if circle_cov > 0.0 {
                let t = ((dy / radius) * 0.5 + 0.5).clamp(0.0, 1.0);
                let mut col_r = top_rgb[0] + (bot_rgb[0] - top_rgb[0]) * t;
                let mut col_g = top_rgb[1] + (bot_rgb[1] - top_rgb[1]) * t;
                let mut col_b = top_rgb[2] + (bot_rgb[2] - top_rgb[2]) * t;

                // Subtle 1px inner glass rim highlight
                let d_rim = (d_circle + 0.75 * s).abs();
                let rim_cov = ((0.85 * s - d_rim) / (0.85 * s)).clamp(0.0, 1.0);
                let rim_weight = rim_cov * rim_alpha * (1.0 - t * 0.55);
                col_r = col_r * (1.0 - rim_weight) + rim_rgb[0] * rim_weight;
                col_g = col_g * (1.0 - rim_weight) + rim_rgb[1] * rim_weight;
                col_b = col_b * (1.0 - rim_weight) + rim_rgb[2] * rim_weight;

                // 3. Foreground symbol SDF
                let d_sym = match kind {
                    'c' => {
                        let u = (dx + dy) * std::f32::consts::FRAC_1_SQRT_2;
                        let v = (dx - dy) * std::f32::consts::FRAC_1_SQRT_2;
                        let l_diag = 3.9 * s;
                        let d1 = ((u.abs() - l_diag).max(0.0).powi(2) + v * v).sqrt() - stroke_r;
                        let d2 = ((v.abs() - l_diag).max(0.0).powi(2) + u * u).sqrt() - stroke_r;
                        d1.min(d2)
                    }
                    'm' => {
                        let l_min = 3.8 * s;
                        ((dx.abs() - l_min).max(0.0).powi(2) + dy * dy).sqrt() - stroke_r
                    }
                    'R' => {
                        let bs = 2.8 * s;
                        let cr = 0.85 * s;
                        let fx = dx + 1.0 * s;
                        let fy = dy - 1.0 * s;
                        let qfx = fx.abs() - (bs - cr);
                        let qfy = fy.abs() - (bs - cr);
                        let d_front_box = (qfx.max(0.0).powi(2) + qfy.max(0.0).powi(2)).sqrt()
                            + qfx.max(qfy).min(0.0)
                            - cr;
                        let d_front = d_front_box.abs() - stroke_r;

                        let bx = dx - 1.2 * s;
                        let by = dy + 1.2 * s;
                        let qbx = bx.abs() - (bs - cr);
                        let qby = by.abs() - (bs - cr);
                        let d_back_box = (qbx.max(0.0).powi(2) + qby.max(0.0).powi(2)).sqrt()
                            + qbx.max(qby).min(0.0)
                            - cr;
                        let d_back =
                            (d_back_box.abs() - stroke_r).max(-(d_front_box - stroke_r * 0.6));
                        d_front.min(d_back)
                    }
                    _ => {
                        // 'M': rounded square outline
                        let box_s = 3.5 * s;
                        let cr = 1.15 * s;
                        let qx = dx.abs() - (box_s - cr);
                        let qy = dy.abs() - (box_s - cr);
                        let d_box = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt()
                            + qx.max(qy).min(0.0)
                            - cr;
                        d_box.abs() - stroke_r
                    }
                };

                let sym_cov = (0.5 - d_sym).clamp(0.0, 1.0) * sym_alpha;
                col_r = col_r * (1.0 - sym_cov) + sym_rgb[0] * sym_cov;
                col_g = col_g * (1.0 - sym_cov) + sym_rgb[1] * sym_cov;
                col_b = col_b * (1.0 - sym_cov) + sym_rgb[2] * sym_cov;

                // Composite circle over outer glow
                out_r = col_r * circle_cov + out_r * (1.0 - circle_cov);
                out_g = col_g * circle_cov + out_g * (1.0 - circle_cov);
                out_b = col_b * circle_cov + out_b * (1.0 - circle_cov);
                out_a = circle_cov + glow_cov * (1.0 - circle_cov);
            }

            let a_u8 = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
            if a_u8 > 0 {
                let idx = ((py * total_sz + px) * 4) as usize;
                let r_u8 = out_r.round().clamp(0.0, 255.0) as u32;
                let g_u8 = out_g.round().clamp(0.0, 255.0) as u32;
                let b_u8 = out_b.round().clamp(0.0, 255.0) as u32;
                let a_u32 = a_u8 as u32;
                // Premultiplied ARGB8888 (little-endian BGRA bytes)
                pixels[idx] = ((b_u8 * a_u32) / 255) as u8;
                pixels[idx + 1] = ((g_u8 * a_u32) / 255) as u8;
                pixels[idx + 2] = ((r_u8 * a_u32) / 255) as u8;
                pixels[idx + 3] = a_u8;
            }
        }
    }

    let mut buffer = MemoryRenderBuffer::new(
        Fourcc::Argb8888,
        (total_sz, total_sz),
        buf_scale,
        Transform::Normal,
        None,
    );
    let draw_res = buffer.render().draw(|buf| {
        buf.copy_from_slice(&pixels);
        Result::<_, ()>::Ok(vec![smithay::utils::Rectangle::from_size(
            (total_sz, total_sz).into(),
        )])
    });

    if draw_res.is_ok() {
        Some((buffer, total_sz, total_sz))
    } else {
        None
    }
}

#[allow(clippy::chunks_exact_to_as_chunks)]
fn decode_png_rgba(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    let decoder = png::Decoder::new(reader);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);

    match info.color_type {
        png::ColorType::Rgba => {
            buf.truncate((w * h * 4) as usize);
            Some((w, h, buf))
        }
        png::ColorType::Rgb => {
            let mut rgba = Vec::with_capacity((w * h * 4) as usize);
            for chunk in buf.chunks_exact(3) {
                rgba.push(chunk[0]);
                rgba.push(chunk[1]);
                rgba.push(chunk[2]);
                rgba.push(255);
            }
            Some((w, h, rgba))
        }
        png::ColorType::GrayscaleAlpha => {
            let mut rgba = Vec::with_capacity((w * h * 4) as usize);
            for chunk in buf.chunks_exact(2) {
                let g = chunk[0];
                let a = chunk[1];
                rgba.push(g);
                rgba.push(g);
                rgba.push(g);
                rgba.push(a);
            }
            Some((w, h, rgba))
        }
        png::ColorType::Grayscale => {
            let mut rgba = Vec::with_capacity((w * h * 4) as usize);
            for &g in buf.iter().take((w * h) as usize) {
                rgba.push(g);
                rgba.push(g);
                rgba.push(g);
                rgba.push(255);
            }
            Some((w, h, rgba))
        }
        _ => None,
    }
}

fn resample_rgba(src_rgba: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Vec<u8> {
    let mut dst = vec![0u8; (dst_w * dst_h * 4) as usize];
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return dst;
    }

    let x_ratio = (src_w as f32) / (dst_w as f32);
    let y_ratio = (src_h as f32) / (dst_h as f32);

    for dy in 0..dst_h {
        let sy = (dy as f32 + 0.5) * y_ratio - 0.5;
        let y0 = (sy.floor() as i32).clamp(0, src_h as i32 - 1) as u32;
        let y1 = (y0 + 1).min(src_h - 1);
        let fy = (sy - y0 as f32).clamp(0.0, 1.0);

        for dx in 0..dst_w {
            let sx = (dx as f32 + 0.5) * x_ratio - 0.5;
            let x0 = (sx.floor() as i32).clamp(0, src_w as i32 - 1) as u32;
            let x1 = (x0 + 1).min(src_w - 1);
            let fx = (sx - x0 as f32).clamp(0.0, 1.0);

            let idx00 = ((y0 * src_w + x0) * 4) as usize;
            let idx10 = ((y0 * src_w + x1) * 4) as usize;
            let idx01 = ((y1 * src_w + x0) * 4) as usize;
            let idx11 = ((y1 * src_w + x1) * 4) as usize;

            let w00 = (1.0 - fx) * (1.0 - fy);
            let w10 = fx * (1.0 - fy);
            let w01 = (1.0 - fx) * fy;
            let w11 = fx * fy;

            let dst_idx = ((dy * dst_w + dx) * 4) as usize;
            for c in 0..4 {
                let v = src_rgba[idx00 + c] as f32 * w00
                    + src_rgba[idx10 + c] as f32 * w10
                    + src_rgba[idx01 + c] as f32 * w01
                    + src_rgba[idx11 + c] as f32 * w11;
                dst[dst_idx + c] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    dst
}

fn resolve_app_icon_png(app_id: &str, title: &str) -> Option<PathBuf> {
    let lower_id = app_id.to_ascii_lowercase();
    let lower_title = title.to_ascii_lowercase();
    let clean_id = lower_id.strip_suffix(".desktop").unwrap_or(&lower_id);
    let combined = format!("{} {}", clean_id, lower_title);

    let corten_candidates: &[(&str, &[&str])] = &[
        (
            "corten-terminal",
            &[
                "corten-terminal",
                "alacritty",
                "kitty",
                "foot",
                "konsole",
                "terminal",
                "bash",
                "zsh",
            ],
        ),
        (
            "corten-files",
            &[
                "corten-files",
                "nautilus",
                "dolphin",
                "thunar",
                "file",
                "dosya",
            ],
        ),
        (
            "corten-settings",
            &[
                "corten-settings",
                "setting",
                "config",
                "control",
                "ayar",
                "preferences",
            ],
        ),
        (
            "corten-forge",
            &[
                "corten-forge",
                "forge",
                "store",
                "software",
                "package",
                "flathub",
                "discover",
            ],
        ),
        (
            "corten-ai",
            &["corten-ai", "ai", "assistant", "intelligence", "gemini"],
        ),
        (
            "corten-monitor",
            &[
                "corten-monitor",
                "system-monitor",
                "htop",
                "btop",
                "monitor",
            ],
        ),
        ("corten-welcome", &["corten-welcome", "welcome", "tour"]),
        (
            "corten-snipper",
            &["corten-snipper", "snipper", "screenshot", "capture"],
        ),
        ("corten-lock", &["corten-lock", "lock", "screenlock"]),
        (
            "corten-browser",
            &[
                "corten-browser",
                "firefox",
                "chromium",
                "chrome",
                "brave",
                "zen",
                "browser",
                "web",
            ],
        ),
        (
            "corten-editor",
            &[
                "corten-editor",
                "code",
                "nvim",
                "rust",
                "text",
                "kate",
                "gedit",
                "editor",
            ],
        ),
    ];

    let mut icon_roots: Vec<PathBuf> = vec![
        PathBuf::from("assets/icons/hicolor"),
        PathBuf::from("/usr/local/share/icons/hicolor"),
        PathBuf::from("/usr/share/icons/hicolor"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        icon_roots.push(PathBuf::from(&home).join(".local/share/icons/hicolor"));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        icon_roots.push(dir.join("assets/icons/hicolor"));
        icon_roots.push(dir.join("../../assets/icons/hicolor"));
    }

    for (target_app, keywords) in corten_candidates {
        if keywords.iter().any(|&kw| combined.contains(kw)) {
            for root in &icon_roots {
                let p48 = root.join(format!("48x48/apps/{}.png", target_app));
                if p48.exists() {
                    return Some(p48);
                }
                let p64 = root.join(format!("64x64/apps/{}.png", target_app));
                if p64.exists() {
                    return Some(p64);
                }
            }
        }
    }

    for root in &icon_roots {
        let p48 = root.join(format!("48x48/apps/{}.png", clean_id));
        if p48.exists() {
            return Some(p48);
        }
        let p64 = root.join(format!("64x64/apps/{}.png", clean_id));
        if p64.exists() {
            return Some(p64);
        }
    }

    let sys_paths = [
        format!("/usr/share/icons/hicolor/48x48/apps/{}.png", clean_id),
        format!("/usr/share/icons/hicolor/64x64/apps/{}.png", clean_id),
        format!("/usr/share/pixmaps/{}.png", clean_id),
        format!("/usr/share/icons/oxygen/base/48x48/apps/{}.png", clean_id),
        format!(
            "/usr/share/icons/AdwaitaLegacy/48x48/legacy/{}.png",
            clean_id
        ),
    ];
    for sp in &sys_paths {
        let p = PathBuf::from(sp);
        if p.exists() {
            return Some(p);
        }
    }

    None
}

fn render_fallback_squircle_icon(_app_id: &str, total_sz: i32, scale: f64) -> Option<Vec<u8>> {
    let s = scale as f32;
    let radius = (total_sz as f32) * 0.28;
    let cx = (total_sz as f32 - 1.0) * 0.5;
    let cy = (total_sz as f32 - 1.0) * 0.5;
    let box_s = (total_sz as f32) * 0.44;

    let mut pixels = vec![0u8; (total_sz * total_sz * 4) as usize];
    let top_rgb = [42.0f32, 47.0, 58.0];
    let bot_rgb = [22.0f32, 25.0, 32.0];
    let rim_rgb = [217.0f32, 119.0, 54.0];

    for py in 0..total_sz {
        for px in 0..total_sz {
            let dx = (px as f32 - cx).abs();
            let dy = (py as f32 - cy).abs();
            let qx = dx - (box_s - radius);
            let qy = dy - (box_s - radius);
            let d_box =
                (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - radius;

            let cov = (0.5 - d_box).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }

            let t = (py as f32 / total_sz as f32).clamp(0.0, 1.0);
            let mut col_r = top_rgb[0] + (bot_rgb[0] - top_rgb[0]) * t;
            let mut col_g = top_rgb[1] + (bot_rgb[1] - top_rgb[1]) * t;
            let mut col_b = top_rgb[2] + (bot_rgb[2] - top_rgb[2]) * t;

            let d_border = (d_box + 0.5 * s).abs();
            let border_cov = ((0.85 * s - d_border) / (0.85 * s)).clamp(0.0, 1.0);
            col_r = col_r * (1.0 - border_cov * 0.8) + rim_rgb[0] * border_cov * 0.8;
            col_g = col_g * (1.0 - border_cov * 0.8) + rim_rgb[1] * border_cov * 0.8;
            col_b = col_b * (1.0 - border_cov * 0.8) + rim_rgb[2] * border_cov * 0.8;

            let idx = ((py * total_sz + px) * 4) as usize;
            let a_u8 = (cov * 255.0).round().clamp(0.0, 255.0) as u8;
            let a_u32 = a_u8 as u32;
            let r_u8 = col_r.round().clamp(0.0, 255.0) as u32;
            let g_u8 = col_g.round().clamp(0.0, 255.0) as u32;
            let b_u8 = col_b.round().clamp(0.0, 255.0) as u32;

            pixels[idx] = ((b_u8 * a_u32) / 255) as u8;
            pixels[idx + 1] = ((g_u8 * a_u32) / 255) as u8;
            pixels[idx + 2] = ((r_u8 * a_u32) / 255) as u8;
            pixels[idx + 3] = a_u8;
        }
    }
    Some(pixels)
}

pub(crate) fn render_titlebar_app_icon(
    app_id: &str,
    title: &str,
    target_size: i32,
    scale: f64,
) -> Option<(MemoryRenderBuffer, i32, i32)> {
    let buf_scale = scale.round().max(1.0) as i32;
    let total_sz = ((target_size.max(buf_scale) + buf_scale - 1) / buf_scale) * buf_scale;

    let pixels = if let Some(png_path) = resolve_app_icon_png(app_id, title) {
        if let Some((src_w, src_h, src_rgba)) = decode_png_rgba(&png_path) {
            let resampled =
                resample_rgba(&src_rgba, src_w, src_h, total_sz as u32, total_sz as u32);
            let mut px = vec![0u8; (total_sz * total_sz * 4) as usize];
            for i in 0..(total_sz * total_sz) as usize {
                let s_idx = i * 4;
                let r = resampled[s_idx] as u32;
                let g = resampled[s_idx + 1] as u32;
                let b = resampled[s_idx + 2] as u32;
                let a = resampled[s_idx + 3] as u32;

                px[s_idx] = ((b * a) / 255) as u8;
                px[s_idx + 1] = ((g * a) / 255) as u8;
                px[s_idx + 2] = ((r * a) / 255) as u8;
                px[s_idx + 3] = a as u8;
            }
            px
        } else {
            render_fallback_squircle_icon(app_id, total_sz, scale)?
        }
    } else {
        render_fallback_squircle_icon(app_id, total_sz, scale)?
    };

    let mut buffer = MemoryRenderBuffer::new(
        Fourcc::Argb8888,
        (total_sz, total_sz),
        buf_scale,
        Transform::Normal,
        None,
    );
    let draw_res = buffer.render().draw(|buf| {
        buf.copy_from_slice(&pixels);
        Result::<_, ()>::Ok(vec![smithay::utils::Rectangle::from_size(
            (total_sz, total_sz).into(),
        )])
    });

    if draw_res.is_ok() {
        Some((buffer, total_sz, total_sz))
    } else {
        None
    }
}

/// Draws an industrial Corten titlebar with app icon, window title, and bold min-max-close control buttons.
/// When `front_to_back` is `true` (DRM/udev backend), foreground glyphs and text are pushed BEFORE
/// capsule and header plate backgrounds so Smithay renders them ON TOP with 100% visibility.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_window_titlebar(
    elements: &mut Vec<CustomRenderElement>,
    renderer: &mut GlowRenderer,
    rect: Rectangle<i32, smithay::utils::Physical>,
    is_active: bool,
    palette: &Palette,
    is_maximized: bool,
    app_id: &str,
    title: &str,
    scale: f64,
    title_cache: &mut HashMap<String, (MemoryRenderBuffer, i32, i32)>,
    front_to_back: bool,
    cursor_loc: Point<f64, smithay::utils::Logical>,
    is_tiled: bool,
    tabs: &[(u64, String, bool)],
    rounded_header: Option<PixelShaderElement>,
    show_snap_flyout: bool,
) {
    let mut tb_elements: Vec<CustomRenderElement> = Vec::with_capacity(96);

    let accent = if is_active {
        Color32F::new(palette.primary.r, palette.primary.g, palette.primary.b, 1.0)
    } else {
        Color32F::new(palette.border.r, palette.border.g, palette.border.b, 0.25)
    };

    let header_h = (36.0 * scale).round() as i32;
    if rect.size.w > 40 && rect.size.h > 20 {
        if let Some(header_elem) = rounded_header {
            // Single-pass GPU rounded top-corners glass header plate
            tb_elements.push(CustomRenderElement::from(header_elem));
        } else {
            // Software fallback solid header bar
            let header_bar = Rectangle::new(rect.loc, (rect.size.w, header_h).into());
            let (header_bg, _, _) =
                crate::render::compute_titlebar_colors(palette, is_active, false);
            tb_elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                Id::new(),
                header_bar,
                CommitCounter::default(),
                header_bg,
                Kind::Unspecified,
            )));
        }

        // Subtle 1px divider between titlebar and window content
        let divider_bar = Rectangle::new(
            Point::from((rect.loc.x, rect.loc.y + header_h - 1)),
            (rect.size.w, 1).into(),
        );
        tb_elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
            Id::new(),
            divider_bar,
            CommitCounter::default(),
            Color32F::new(1.0, 1.0, 1.0, if is_active { 0.06 } else { 0.03 }),
            Kind::Unspecified,
        )));

        // 3. Adaptive bottom separator (softer fused hairline when tiled)
        let bottom_line = Rectangle::new(
            Point::from((rect.loc.x, rect.loc.y + header_h - 1)),
            (rect.size.w, 1).into(),
        );
        let sep_alpha = if is_tiled { 0.22 } else { 0.48 };
        tb_elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
            Id::new(),
            bottom_line,
            CommitCounter::default(),
            Color32F::new(
                palette.border.r,
                palette.border.g,
                palette.border.b,
                sep_alpha,
            ),
            Kind::Unspecified,
        )));

        // 4. Window Icon on the left side (Paket A Bespoke Metallic Squircle Icon)
        let icon_size = (20.0 * scale).round() as i32;
        let icon_x = rect.loc.x + (10.0 * scale).round() as i32;

        let scale_key = (scale * 100.0).round() as i32;
        let icon_key = format!(
            "__corten_titlebar_icon:{}:{}:{}",
            app_id, icon_size, scale_key
        );
        let cached_icon = if let Some(found) = title_cache.get(&icon_key) {
            Some(found.clone())
        } else {
            match render_titlebar_app_icon(app_id, title, icon_size, scale) {
                Some((buf, iw, ih)) => {
                    if title_cache.len() > 256 {
                        title_cache.clear();
                    }
                    title_cache.insert(icon_key, (buf.clone(), iw, ih));
                    Some((buf, iw, ih))
                }
                _ => None,
            }
        };

        if let Some((buf, _iw, ih)) = cached_icon {
            let actual_y = rect.loc.y + (header_h - ih) / 2;
            let icon_pos: smithay::utils::Point<f64, smithay::utils::Physical> =
                smithay::utils::Point::from((icon_x as f64, actual_y as f64));
            if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                icon_pos,
                &buf,
                Some(1.0),
                None,
                None,
                Kind::Unspecified,
            ) {
                tb_elements.push(CustomRenderElement::from(elem));
            }
        }

        let stroke = (2.0 * scale).max(2.0) as i32;

        // 4.5. Dynamic Context Badge & Unsaved Dirty Dot ('●')
        let mut badge_offset_x = 0;
        if let Some(badge) = crate::layout::extract_titlebar_badge(app_id, title) {
            let pill_x = rect.loc.x + (35.0 * scale).round() as i32;
            let pill_h = (16.0 * scale).round() as i32;
            let pill_y = rect.loc.y + (header_h - pill_h) / 2;
            let pill_w = (22.0 * scale).round() as i32;
            let pill_rect = Rectangle::new(Point::from((pill_x, pill_y)), (pill_w, pill_h).into());
            let b_rgb = badge.color_rgb;
            tb_elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                Id::new(),
                pill_rect,
                CommitCounter::default(),
                Color32F::new(b_rgb[0] * 0.25, b_rgb[1] * 0.25, b_rgb[2] * 0.25, 0.92),
                Kind::Unspecified,
            )));
            let bar_w = (3.0 * scale).round().max(2.0) as i32;
            tb_elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                Id::new(),
                Rectangle::new(Point::from((pill_x, pill_y)), (bar_w, pill_h).into()),
                CommitCounter::default(),
                Color32F::new(b_rgb[0], b_rgb[1], b_rgb[2], 1.0),
                Kind::Unspecified,
            )));
            if badge.is_dirty {
                let dot_sz = (6.0 * scale).round().max(4.0) as i32;
                let dot_x = pill_x + (pill_w - dot_sz) / 2 + (2.0 * scale) as i32;
                let dot_y = pill_y + (pill_h - dot_sz) / 2;
                tb_elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                    Id::new(),
                    Rectangle::new(Point::from((dot_x, dot_y)), (dot_sz, dot_sz).into()),
                    CommitCounter::default(),
                    Color32F::new(0.98, 0.62, 0.18, 1.0),
                    Kind::Unspecified,
                )));
            } else {
                let tick_w = (8.0 * scale).round().max(4.0) as i32;
                let tick_x = pill_x + bar_w + (4.0 * scale) as i32;
                let tick_y = pill_y + (pill_h - stroke) / 2;
                tb_elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                    Id::new(),
                    Rectangle::new(Point::from((tick_x, tick_y)), (tick_w, stroke).into()),
                    CommitCounter::default(),
                    Color32F::new(b_rgb[0], b_rgb[1], b_rgb[2], 0.95),
                    Kind::Unspecified,
                )));
            }
            badge_offset_x = (28.0 * scale).round() as i32;
        }

        // 5. Window Title text next to Icon & Context Badge
        let display_title = if !title.trim().is_empty() {
            title.trim()
        } else if !app_id.trim().is_empty() {
            app_id.trim()
        } else {
            "Corten Window"
        };

        let text_color = if is_active {
            [255, 255, 255]
        } else {
            [225, 230, 240]
        };
        let scale_key = (scale * 100.0).round() as i32;
        let cache_key = format!("{}:{}:{}", display_title, scale_key, is_active);

        let cached = if let Some(found) = title_cache.get(&cache_key) {
            Some(found.clone())
        } else {
            match render_title_text_to_buffer(display_title, scale, text_color) {
                Some((buf, tw, th)) => {
                    if title_cache.len() > 256 {
                        title_cache.clear();
                    }
                    title_cache.insert(cache_key, (buf.clone(), tw, th));
                    Some((buf, tw, th))
                }
                _ => None,
            }
        };

        if let Some((buf, _tw, th)) = cached {
            let text_x = rect.loc.x + (36.0 * scale).round() as i32 + badge_offset_x;
            let text_y = rect.loc.y + (header_h - th) / 2;
            let text_pos: smithay::utils::Point<f64, smithay::utils::Physical> =
                smithay::utils::Point::from((text_x as f64, text_y as f64));
            if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                text_pos,
                &buf,
                Some(1.0),
                None,
                None,
                Kind::Unspecified,
            ) {
                tb_elements.push(CustomRenderElement::from(elem));
            }
        }

        // 5.5. Tabbed Window Stacking Strip (when window is part of a Tab Group)
        if tabs.len() >= 2 && rect.size.w >= (260.0 * scale).round() as i32 {
            let start_x = rect.loc.x + (148.0 * scale).round() as i32;
            let max_right = rect.loc.x + rect.size.w - (96.0 * scale).round() as i32;
            let avail_w = (max_right - start_x).max(60);
            let tab_count = tabs.len() as i32;
            let slot_w = (avail_w / tab_count).clamp((36.0 * scale) as i32, (96.0 * scale) as i32);
            let tab_h = (22.0 * scale).round() as i32;
            let tab_y = rect.loc.y + (header_h - tab_h) / 2;
            let gap_px = (4.0 * scale).round().max(2.0) as i32;

            for (idx, (_tid, _tlabel, is_current)) in tabs.iter().enumerate() {
                let tx = start_x + (idx as i32) * slot_w;
                let tw = (slot_w - gap_px).max(16);
                let tab_rect = Rectangle::new(Point::from((tx, tab_y)), (tw, tab_h).into());
                let bg = if *is_current {
                    Color32F::new(0.20, 0.24, 0.32, 0.98)
                } else {
                    Color32F::new(0.11, 0.13, 0.18, 0.90)
                };
                tb_elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                    Id::new(),
                    tab_rect,
                    CommitCounter::default(),
                    bg,
                    Kind::Unspecified,
                )));
                if *is_current {
                    let top_ind = Rectangle::new(
                        Point::from((tx + 2, tab_y)),
                        ((tw - 4).max(4), (2.0 * scale).round().max(2.0) as i32).into(),
                    );
                    tb_elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                        Id::new(),
                        top_ind,
                        CommitCounter::default(),
                        accent,
                        Kind::Unspecified,
                    )));
                }
                // Inner tab index bar glyph
                let bar_w = (tw - (14.0 * scale) as i32).clamp(6, (36.0 * scale) as i32);
                let bar_x = tx + (tw - bar_w) / 2;
                let bar_y = tab_y + (tab_h - stroke) / 2;
                let bar_col = if *is_current {
                    Color32F::new(1.0, 1.0, 1.0, 0.95)
                } else {
                    Color32F::new(0.65, 0.70, 0.80, 0.65)
                };
                tb_elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                    Id::new(),
                    Rectangle::new(Point::from((bar_x, bar_y)), (bar_w, stroke).into()),
                    CommitCounter::default(),
                    bar_col,
                    Kind::Unspecified,
                )));
            }
        }

        // 6. Modern Circular Window Control Buttons (Min/Max in Dark Graphite Gray, Close in Corten Orange)
        if rect.size.w > 140 {
            let log_rect = Rectangle::new(
                Point::from((
                    (rect.loc.x as f64 / scale).round() as i32,
                    (rect.loc.y as f64 / scale).round() as i32,
                )),
                Size::from((
                    (rect.size.w as f64 / scale).round() as i32,
                    (rect.size.h as f64 / scale).round() as i32,
                )),
            );
            let rel_cx = cursor_loc.x - log_rect.loc.x as f64;
            let rel_cy = cursor_loc.y - log_rect.loc.y as f64;
            let w_log = log_rect.size.w as f64;

            let flyout_hovered = show_snap_flyout
                && crate::layout::is_maximize_or_flyout_hovered(log_rect, cursor_loc);

            let hover_close =
                (0.0..=36.0).contains(&rel_cy) && rel_cx >= (w_log - 32.0) && rel_cx <= w_log;
            let hover_max = ((0.0..=36.0).contains(&rel_cy)
                && rel_cx >= (w_log - 60.0)
                && rel_cx < (w_log - 32.0))
                || flyout_hovered;
            let hover_min = (0.0..=36.0).contains(&rel_cy)
                && rel_cx >= (w_log - 88.0)
                && rel_cx < (w_log - 60.0);

            let close_x = rect.loc.x + rect.size.w - (31.0 * scale).round() as i32;
            let max_x = rect.loc.x + rect.size.w - (59.0 * scale).round() as i32;
            let min_x = rect.loc.x + rect.size.w - (87.0 * scale).round() as i32;

            let max_kind = if is_maximized { 'R' } else { 'M' };
            let scale_key = (scale * 100.0).round() as i32;

            for (kind, bx, is_btn_hovered) in [
                ('m', min_x, hover_min),
                (max_kind, max_x, hover_max),
                ('c', close_x, hover_close),
            ] {
                let btn_key = format!(
                    "__corten_circle_btn:{}:{}:{}:{}",
                    kind, is_active, is_btn_hovered, scale_key
                );
                let cached_btn = if let Some(found) = title_cache.get(&btn_key) {
                    Some(found.clone())
                } else {
                    match render_circular_button_to_buffer(kind, is_active, is_btn_hovered, scale) {
                        Some((buf, bw, bh)) => {
                            if title_cache.len() > 256 {
                                title_cache.clear();
                            }
                            title_cache.insert(btn_key, (buf.clone(), bw, bh));
                            Some((buf, bw, bh))
                        }
                        _ => None,
                    }
                };

                if let Some((buf, _bw, bh)) = cached_btn {
                    let by = rect.loc.y + (header_h - bh) / 2;
                    let btn_pos: smithay::utils::Point<f64, smithay::utils::Physical> =
                        smithay::utils::Point::from((bx as f64, by as f64));
                    if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
                        renderer,
                        btn_pos,
                        &buf,
                        Some(1.0),
                        None,
                        None,
                        Kind::Unspecified,
                    ) {
                        tb_elements.push(CustomRenderElement::from(elem));
                    }
                }
            }

            // 7. Interactive Snap Layouts Flyout Card when hovering Maximize ('□') button
            if flyout_hovered {
                let flyout_log = crate::layout::snap_flyout_bounds(log_rect);
                let hovered_zone = crate::layout::hit_test_snap_flyout(log_rect, cursor_loc);
                let flyout_phys = Rectangle::new(
                    flyout_log.loc.to_physical_precise_round(scale),
                    flyout_log.size.to_physical_precise_round(scale),
                );

                // Smoked acrylic flyout plate
                tb_elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                    Id::new(),
                    flyout_phys,
                    CommitCounter::default(),
                    Color32F::new(0.08, 0.09, 0.13, 0.97),
                    Kind::Unspecified,
                )));
                draw_glow_ring(&mut tb_elements, flyout_phys, 0, 2, accent, 4);

                let pad = (6.0 * scale).round() as i32;
                let gap = (4.0 * scale).round() as i32;
                let group_w = (flyout_phys.size.w - pad * 2 - gap * 2) / 3;
                let group_h = (flyout_phys.size.h - pad * 2).max(16);
                let gy = flyout_phys.loc.y + pad;

                let tile_color = |zone: crate::layout::SnapZone| {
                    if hovered_zone == Some(zone) {
                        Color32F::new(
                            palette.primary.r,
                            palette.primary.g,
                            palette.primary.b,
                            0.95,
                        )
                    } else {
                        Color32F::new(0.22, 0.26, 0.34, 0.92)
                    }
                };

                // Group 1: [1/2 Left | 1/2 Right]
                let g1_x = flyout_phys.loc.x + pad;
                let half_gw = (group_w - gap) / 2;
                tb_elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                    Id::new(),
                    Rectangle::new(Point::from((g1_x, gy)), (half_gw, group_h).into()),
                    CommitCounter::default(),
                    tile_color(crate::layout::SnapZone::LeftHalf),
                    Kind::Unspecified,
                )));
                tb_elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                    Id::new(),
                    Rectangle::new(
                        Point::from((g1_x + half_gw + gap, gy)),
                        (group_w - half_gw - gap, group_h).into(),
                    ),
                    CommitCounter::default(),
                    tile_color(crate::layout::SnapZone::RightHalf),
                    Kind::Unspecified,
                )));

                // Group 2: [2/3 Left | 1/3 Right]
                let g2_x = g1_x + group_w + gap;
                let two_thirds_gw = ((group_w - gap) * 2) / 3;
                tb_elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                    Id::new(),
                    Rectangle::new(Point::from((g2_x, gy)), (two_thirds_gw, group_h).into()),
                    CommitCounter::default(),
                    tile_color(crate::layout::SnapZone::LeftTwoThirds),
                    Kind::Unspecified,
                )));
                tb_elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                    Id::new(),
                    Rectangle::new(
                        Point::from((g2_x + two_thirds_gw + gap, gy)),
                        (group_w - two_thirds_gw - gap, group_h).into(),
                    ),
                    CommitCounter::default(),
                    tile_color(crate::layout::SnapZone::RightOneThird),
                    Kind::Unspecified,
                )));

                // Group 3: [Top-Left 1/4 | Bottom-Right 1/4]
                let g3_x = g2_x + group_w + gap;
                let half_gh = (group_h - gap) / 2;
                tb_elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                    Id::new(),
                    Rectangle::new(Point::from((g3_x, gy)), (group_w, half_gh).into()),
                    CommitCounter::default(),
                    tile_color(crate::layout::SnapZone::TopLeftQuarter),
                    Kind::Unspecified,
                )));
                tb_elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                    Id::new(),
                    Rectangle::new(
                        Point::from((g3_x, gy + half_gh + gap)),
                        (group_w, group_h - half_gh - gap).into(),
                    ),
                    CommitCounter::default(),
                    tile_color(crate::layout::SnapZone::BottomRightQuarter),
                    Kind::Unspecified,
                )));
            }
        }
    }

    if front_to_back {
        elements.extend(tb_elements.into_iter().rev());
    } else {
        elements.extend(tb_elements);
    }
}

/// Renders semantic cursor shapes (`CursorIcon`) using persistent `Id` slots for hardware/software cursor composition.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_semantic_cursor(
    elements: &mut Vec<CustomRenderElement>,
    icon: CursorIcon,
    cx: i32,
    cy: i32,
    cursor_body_ids: &[Id],
    cursor_border_ids: &[Id],
    cursor_tail_id: &Id,
    cursor_outline_id: &Id,
    cursor_commit: CommitCounter,
    palette: &Palette,
) {
    let cursor_color = Color32F::new(0.98, 0.98, 1.0, 1.0);
    let border_color = Color32F::new(0.08, 0.09, 0.11, 0.95);
    let accent_color = Color32F::new(palette.primary.r, palette.primary.g, palette.primary.b, 1.0);
    let accent_subtle = Color32F::new(
        palette.primary.r,
        palette.primary.g,
        palette.primary.b,
        0.45,
    );

    let get_body_id = |idx: usize| {
        cursor_body_ids
            .get(idx)
            .unwrap_or(&cursor_body_ids[0])
            .clone()
    };
    let get_border_id = |idx: usize| {
        cursor_border_ids
            .get(idx)
            .unwrap_or(&cursor_border_ids[0])
            .clone()
    };

    match icon {
        CursorIcon::Text | CursorIcon::VerticalText => {
            // Elegant I-beam text cursor with serif caps and dark contrasting outline
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(0),
                Rectangle::new(Point::from((cx - 4, cy - 9)), (9, 2).into()),
                cursor_commit,
                accent_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_border_id(0),
                Rectangle::new(Point::from((cx - 5, cy - 10)), (11, 1).into()),
                cursor_commit,
                border_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(1),
                Rectangle::new(Point::from((cx, cy - 7)), (2, 14).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                cursor_outline_id.clone(),
                Rectangle::new(Point::from((cx - 1, cy - 7)), (1, 14).into()),
                cursor_commit,
                border_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                cursor_tail_id.clone(),
                Rectangle::new(Point::from((cx + 2, cy - 7)), (1, 14).into()),
                cursor_commit,
                border_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(2),
                Rectangle::new(Point::from((cx - 4, cy + 7)), (9, 2).into()),
                cursor_commit,
                accent_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_border_id(1),
                Rectangle::new(Point::from((cx - 5, cy + 9)), (11, 1).into()),
                cursor_commit,
                border_color,
                Kind::Cursor,
            )));
        }
        CursorIcon::Crosshair | CursorIcon::Cell => {
            // Precision crosshair with center reticle gap
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(0),
                Rectangle::new(Point::from((cx - 9, cy)), (7, 2).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(1),
                Rectangle::new(Point::from((cx + 3, cy)), (7, 2).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(2),
                Rectangle::new(Point::from((cx, cy - 9)), (2, 7).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(3),
                Rectangle::new(Point::from((cx, cy + 3)), (2, 7).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                cursor_tail_id.clone(),
                Rectangle::new(Point::from((cx, cy)), (2, 2).into()),
                cursor_commit,
                accent_color,
                Kind::Cursor,
            )));
        }
        CursorIcon::EwResize
        | CursorIcon::ColResize
        | CursorIcon::WResize
        | CursorIcon::EResize => {
            // Clean horizontal resize double-arrow (←→)
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(0),
                Rectangle::new(Point::from((cx - 8, cy - 1)), (17, 2).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            // Left arrowhead
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(1),
                Rectangle::new(Point::from((cx - 10, cy - 1)), (2, 2).into()),
                cursor_commit,
                accent_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(2),
                Rectangle::new(Point::from((cx - 8, cy - 3)), (2, 6).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(3),
                Rectangle::new(Point::from((cx - 6, cy - 5)), (2, 10).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            // Right arrowhead
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(4),
                Rectangle::new(Point::from((cx + 5, cy - 5)), (2, 10).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(5),
                Rectangle::new(Point::from((cx + 7, cy - 3)), (2, 6).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(6),
                Rectangle::new(Point::from((cx + 9, cy - 1)), (2, 2).into()),
                cursor_commit,
                accent_color,
                Kind::Cursor,
            )));
            // Dark outline
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                cursor_outline_id.clone(),
                Rectangle::new(Point::from((cx - 6, cy - 2)), (13, 1).into()),
                cursor_commit,
                border_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                cursor_tail_id.clone(),
                Rectangle::new(Point::from((cx - 6, cy + 1)), (13, 1).into()),
                cursor_commit,
                border_color,
                Kind::Cursor,
            )));
        }
        CursorIcon::NsResize
        | CursorIcon::RowResize
        | CursorIcon::NResize
        | CursorIcon::SResize => {
            // Clean vertical resize double-arrow (↕)
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(0),
                Rectangle::new(Point::from((cx - 1, cy - 8)), (2, 17).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            // Top arrowhead
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(1),
                Rectangle::new(Point::from((cx - 1, cy - 10)), (2, 2).into()),
                cursor_commit,
                accent_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(2),
                Rectangle::new(Point::from((cx - 3, cy - 8)), (6, 2).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(3),
                Rectangle::new(Point::from((cx - 5, cy - 6)), (10, 2).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            // Bottom arrowhead
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(4),
                Rectangle::new(Point::from((cx - 5, cy + 5)), (10, 2).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(5),
                Rectangle::new(Point::from((cx - 3, cy + 7)), (6, 2).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(6),
                Rectangle::new(Point::from((cx - 1, cy + 9)), (2, 2).into()),
                cursor_commit,
                accent_color,
                Kind::Cursor,
            )));
            // Dark outline
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                cursor_outline_id.clone(),
                Rectangle::new(Point::from((cx - 2, cy - 6)), (1, 13).into()),
                cursor_commit,
                border_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                cursor_tail_id.clone(),
                Rectangle::new(Point::from((cx + 1, cy - 6)), (1, 13).into()),
                cursor_commit,
                border_color,
                Kind::Cursor,
            )));
        }
        CursorIcon::NwseResize => {
            // Diagonal resize double-arrow ↖ ↘ (bottom-right / top-left corner)
            // Top-left arrowhead
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(0),
                Rectangle::new(Point::from((cx - 8, cy - 8)), (6, 2).into()),
                cursor_commit,
                accent_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(1),
                Rectangle::new(Point::from((cx - 8, cy - 8)), (2, 6).into()),
                cursor_commit,
                accent_color,
                Kind::Cursor,
            )));
            // Diagonal shaft
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(2),
                Rectangle::new(Point::from((cx - 6, cy - 6)), (3, 3).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(3),
                Rectangle::new(Point::from((cx - 3, cy - 3)), (3, 3).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(4),
                Rectangle::new(Point::from((cx, cy)), (3, 3).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(5),
                Rectangle::new(Point::from((cx + 3, cy + 3)), (3, 3).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            // Bottom-right arrowhead
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(6),
                Rectangle::new(Point::from((cx + 3, cy + 7)), (6, 2).into()),
                cursor_commit,
                accent_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(7),
                Rectangle::new(Point::from((cx + 7, cy + 3)), (2, 6).into()),
                cursor_commit,
                accent_color,
                Kind::Cursor,
            )));
        }
        CursorIcon::NeswResize => {
            // Diagonal resize double-arrow ↗ ↙ (bottom-left / top-right corner)
            // Top-right arrowhead
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(0),
                Rectangle::new(Point::from((cx + 3, cy - 8)), (6, 2).into()),
                cursor_commit,
                accent_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(1),
                Rectangle::new(Point::from((cx + 7, cy - 8)), (2, 6).into()),
                cursor_commit,
                accent_color,
                Kind::Cursor,
            )));
            // Diagonal shaft
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(2),
                Rectangle::new(Point::from((cx + 4, cy - 6)), (3, 3).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(3),
                Rectangle::new(Point::from((cx + 1, cy - 3)), (3, 3).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(4),
                Rectangle::new(Point::from((cx - 2, cy)), (3, 3).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(5),
                Rectangle::new(Point::from((cx - 5, cy + 3)), (3, 3).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            // Bottom-left arrowhead
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(6),
                Rectangle::new(Point::from((cx - 8, cy + 7)), (6, 2).into()),
                cursor_commit,
                accent_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(7),
                Rectangle::new(Point::from((cx - 8, cy + 3)), (2, 6).into()),
                cursor_commit,
                accent_color,
                Kind::Cursor,
            )));
        }
        CursorIcon::Pointer | CursorIcon::Grab | CursorIcon::Grabbing => {
            // Sleek hand pointer with extended index finger
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(0),
                Rectangle::new(Point::from((cx + 2, cy)), (3, 10).into()),
                cursor_commit,
                accent_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(1),
                Rectangle::new(Point::from((cx - 2, cy + 6)), (11, 8).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(2),
                Rectangle::new(Point::from((cx - 5, cy + 8)), (4, 4).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                cursor_outline_id.clone(),
                Rectangle::new(Point::from((cx + 1, cy - 1)), (5, 1).into()),
                cursor_commit,
                border_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                cursor_tail_id.clone(),
                Rectangle::new(Point::from((cx - 3, cy + 14)), (13, 1).into()),
                cursor_commit,
                border_color,
                Kind::Cursor,
            )));
        }
        CursorIcon::Move | CursorIcon::AllScroll => {
            // 4-way arrow cross
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(0),
                Rectangle::new(Point::from((cx - 7, cy)), (15, 2).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(1),
                Rectangle::new(Point::from((cx, cy - 7)), (2, 15).into()),
                cursor_commit,
                cursor_color,
                Kind::Cursor,
            )));
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                get_body_id(2),
                Rectangle::new(Point::from((cx - 1, cy - 1)), (4, 4).into()),
                cursor_commit,
                accent_color,
                Kind::Cursor,
            )));
        }
        _ => {
            // Default Corten obsidian & pearl precision arrow
            for i in 0..16 {
                let row = i as i32;
                let w = ((row * 7) / 10 + 1).min(12);
                let id = get_body_id(i);
                elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                    id,
                    Rectangle::new(Point::from((cx + 1, cy + row)), (w, 1).into()),
                    cursor_commit,
                    if i < 3 { accent_color } else { cursor_color },
                    Kind::Cursor,
                )));
                let b_id = get_border_id(i);
                elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                    b_id,
                    Rectangle::new(Point::from((cx + w + 1, cy + row)), (1, 1).into()),
                    cursor_commit,
                    border_color,
                    Kind::Cursor,
                )));
            }
            // Left boundary outline
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                cursor_outline_id.clone(),
                Rectangle::new(Point::from((cx, cy)), (1, 17).into()),
                cursor_commit,
                border_color,
                Kind::Cursor,
            )));
            // Sleek tail stem
            elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                cursor_tail_id.clone(),
                Rectangle::new(Point::from((cx + 5, cy + 11)), (3, 6).into()),
                cursor_commit,
                accent_subtle,
                Kind::Cursor,
            )));
        }
    }
}
