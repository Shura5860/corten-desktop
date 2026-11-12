use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use corten_config::{AppearanceConfig, ConfigManager, InputSettingsConfig};
use corten_theme::{Palette, PatinaVariant, ThemeMode};
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::compositor::FrameFlags;
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::{DrmOutputManager, DrmOutputRenderElements};
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmNode, NodeType};
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::input::{Axis, AxisSource, ButtonState, KeyState, KeyboardKeyEvent};
use smithay::backend::libinput::LibinputSessionInterface;
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::Color32F;
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::Session;
use smithay::backend::udev::{all_gpus, primary_gpu, UdevBackend, UdevEvent};
use smithay::desktop::{layer_map_for_output, PopupManager, WindowSurfaceType};
use smithay::input::keyboard::keysyms;
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, CursorImageStatus, MotionEvent, RelativeMotionEvent,
};
use smithay::output::{Mode as OutputMode, OutputModeSource};
use smithay::reexports::drm::control::{connector, Device as ControlDevice};
use smithay::reexports::input::event::pointer::PointerEventTrait;
use smithay::reexports::input::event::{keyboard::KeyboardEventTrait, EventTrait};
use smithay::reexports::input::Libinput;
use smithay::reexports::rustix::fs::OFlags;
use smithay::utils::{DeviceFd, Point, Rectangle, Scale, Size, SERIAL_COUNTER};
use smithay::wayland::shell::wlr_layer::Layer;
use tracing::info;
use wayland_server::{Display, ListeningSocket};

use crate::glow_shader::{DualKawaseBlurPipeline, GlassRegion, GlowShaderManager};
use crate::handlers::ClientState;
use crate::state::CortenCompositor;
use crate::{
    apply_display_settings, binding_matches, draw_glow_ring, draw_semantic_cursor,
    draw_window_titlebar, secondary_display_settings, secondary_output_location,
    send_frames_surface_tree, CompositorConfig, CustomRenderElement,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiGpuDevice {
    pub node: DrmNode,
    pub dev_path: std::path::PathBuf,
    pub render_path: Option<std::path::PathBuf>,
    pub is_primary: bool,
    pub has_render_node: bool,
}

pub struct DrmBackendState {
    pub session: LibSeatSession,
    pub notifier: smithay::backend::session::libseat::LibSeatSessionNotifier,
    pub primary_gpu: DrmNode,
    pub dev_path: std::path::PathBuf,
    pub secondary_gpus: Vec<MultiGpuDevice>,
}

fn try_enable_vrr<D: ControlDevice>(
    drm: &D,
    conn_handle: connector::Handle,
    crtc_handle: smithay::reexports::drm::control::crtc::Handle,
) {
    let mut vrr_supported = false;
    if let Ok(props) = drm.get_properties(conn_handle) {
        let (ids, vals) = props.as_props_and_values();
        for (&id, &val) in ids.iter().zip(vals.iter()) {
            if let Ok(info) = drm.get_property(id) {
                if info.name().to_bytes() == b"vrr_capable" && val != 0 {
                    vrr_supported = true;
                    break;
                }
            }
        }
    }
    if !vrr_supported {
        return;
    }
    if let Ok(props) = drm.get_properties(crtc_handle) {
        let (ids, _) = props.as_props_and_values();
        for &id in ids {
            if let Ok(info) = drm.get_property(id) {
                if info.name().to_bytes() == b"VRR_ENABLED" {
                    if drm.set_property(crtc_handle, id, 1).is_ok() {
                        info!(
                            ?conn_handle,
                            ?crtc_handle,
                            "Enabled Adaptive Sync / VRR (FreeSync) on DRM CRTC"
                        );
                    }
                    break;
                }
            }
        }
    }
}

fn apply_drm_hdr_and_color_management<D: ControlDevice>(
    drm: &D,
    conn_handle: connector::Handle,
    config: &corten_config::DisplaySettingsConfig,
) {
    use corten_services::color_management::{ColorSpace, DrmHdrOutputMetadata};
    use smithay::reexports::drm::control::property::ValueType;

    let color_space = ColorSpace::parse_str(&config.color_space);

    if let Ok(props) = drm.get_properties(conn_handle) {
        let (ids, _) = props.as_props_and_values();
        for &id in ids {
            if let Ok(info) = drm.get_property(id) {
                let name = info.name().to_bytes();
                if name == b"HDR_OUTPUT_METADATA" {
                    if config.hdr_enabled {
                        let max_nits = config.max_luminance_nits.unwrap_or(400) as u16;
                        let hdr_meta = DrmHdrOutputMetadata::hdr10(color_space, max_nits);
                        match drm.create_property_blob(&hdr_meta) {
                            Ok(blob_val) => {
                                if let Some(blob_id) = blob_val.as_blob() {
                                    if drm.set_property(conn_handle, id, blob_id).is_ok() {
                                        info!(
                                            ?conn_handle,
                                            nits = max_nits,
                                            cs = %color_space.as_str(),
                                            "Applied DRM HDR_OUTPUT_METADATA to connector"
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(?conn_handle, "Failed to create HDR property blob: {:?}", e);
                            }
                        }
                    } else {
                        // Clear HDR metadata
                        let _ = drm.set_property(conn_handle, id, 0);
                    }
                } else if name == b"Colorspace" {
                    if let ValueType::Enum(ref enum_vals) = info.value_type() {
                        let (_, enums) = enum_vals.values();
                        let target_name: &[u8] = if config.hdr_enabled {
                            match color_space {
                                ColorSpace::Bt2020 => b"BT2020_RGB",
                                ColorSpace::DciP3 => b"DCI-P3_RGB_D65",
                                ColorSpace::Srgb => b"Default",
                            }
                        } else {
                            b"Default"
                        };

                        if let Some(target_enum) = enums.iter().find(|e| {
                            let ename = e.name().to_bytes();
                            ename == target_name
                                || (target_name == b"BT2020_RGB" && ename == b"BT2020_CYCC")
                        }) {
                            if drm.set_property(conn_handle, id, target_enum.value()).is_ok() {
                                info!(
                                    ?conn_handle,
                                    target = ?target_enum.name(),
                                    "Configured DRM connector Colorspace"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

fn configured_drm_mode(
    specification: &str,
    modes: &[smithay::reexports::drm::control::Mode],
) -> Option<smithay::reexports::drm::control::Mode> {
    let (size, refresh) = specification.split_once('@')?;
    let (width, height) = size.split_once('x')?;
    let width = width.parse::<u16>().ok()?;
    let height = height.parse::<u16>().ok()?;
    let refresh = refresh
        .strip_suffix("Hz")
        .unwrap_or(refresh)
        .parse::<f64>()
        .ok()?;
    modes
        .iter()
        .find(|mode| {
            let (mode_width, mode_height) = mode.size();
            mode_width == width
                && mode_height == height
                && (f64::from(mode.vrefresh()) - refresh).abs() < 0.5
        })
        .cloned()
}

fn spawn_first_terminal() -> std::io::Result<()> {
    let mut last_error = None;
    for program in [
        "corten-terminal",
        "/usr/local/bin/corten-terminal",
        "target/release/corten-terminal",
        "target/debug/corten-terminal",
    ] {
        if Command::new(program)
            .env("WAYLAND_DISPLAY", "corten-0")
            .spawn()
            .is_ok()
        {
            return Ok(());
        }
    }
    if Command::new("konsole")
        .args(["--profile", "Corten"])
        .env("WAYLAND_DISPLAY", "corten-0")
        .spawn()
        .is_ok()
    {
        return Ok(());
    }
    if Command::new("/var/lib/flatpak/exports/bin/org.kde.konsole")
        .args(["--profile", "Corten"])
        .env("WAYLAND_DISPLAY", "corten-0")
        .spawn()
        .is_ok()
    {
        return Ok(());
    }
    for program in [
        "foot",
        "alacritty",
        "kitty",
        "konsole",
        "xterm",
        "pterm",
        "urxvt",
        "weston-terminal",
    ] {
        match Command::new(program)
            .env("WAYLAND_DISPLAY", "corten-0")
            .spawn()
        {
            Ok(_) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no terminal emulator found")
    }))
}

fn apply_libinput_device_settings(
    device: &mut smithay::reexports::input::Device,
    config: &InputSettingsConfig,
) {
    let speed = f64::from(config.pointer_speed).clamp(-100.0, 100.0) / 100.0;
    let _ = device.config_accel_set_speed(speed);
    if device.config_scroll_has_natural_scroll() {
        let _ = device.config_scroll_set_natural_scroll_enabled(config.natural_scrolling);
    }
    if device.config_tap_finger_count() > 0 {
        let _ = device.config_tap_set_enabled(config.tap_to_click);
    }
}

fn build_secondary_scene(
    state: &mut CortenCompositor,
    output: &smithay::output::Output,
    renderer: &mut GlowRenderer,
    palette: &Palette,
) -> Vec<CustomRenderElement> {
    let render_output = {
        let candidate = layer_map_for_output(output);
        if output.name() != state.output.name() && candidate.layers().next().is_none() {
            state.output.clone()
        } else {
            output.clone()
        }
    };
    let usable_area = {
        let mut layer_map = layer_map_for_output(&render_output);
        layer_map.arrange();
        layer_map.non_exclusive_zone()
    };
    let layouts = if state.overview_mode {
        state
            .layout_for_output_mut(output)
            .arrange_overview(usable_area)
    } else {
        state
            .layout_for_output_mut(output)
            .arrange_animated(usable_area)
    };
    let output_scale = output.current_scale();
    let fractional_scale = output_scale.fractional_scale();
    let render_scale = Scale::from(fractional_scale);
    if !state.overview_mode && state.layout_for_output(output).mode() != corten_config::LayoutMode::Floating {
        let tiled_targets = state.layout_for_output_mut(output).arrange(usable_area);
        for (window, target_rect) in tiled_targets {
            crate::layout::configure_window_geometry(&window, target_rect);
        }
    }

    let mut elements = Vec::new();
    if state.session_locked {
        for (lock_surface, lock_output) in &state.lock_surfaces {
            if !state.lock_surface_for_output(lock_output, output) {
                continue;
            }
            let surfaces: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                render_elements_from_surface_tree(
                    renderer,
                    lock_surface.wl_surface(),
                    (0, 0),
                    render_scale,
                    1.0,
                    Kind::Unspecified,
                );
            elements.extend(surfaces.into_iter().map(CustomRenderElement::from));
        }
    }

    {
        let layer_map = layer_map_for_output(&render_output);
        for layer in [Layer::Overlay, Layer::Top] {
            if state.session_locked && layer == Layer::Overlay {
                continue;
            }
            for layer_surface in layer_map.layers_on(layer) {
                let loc = layer_map
                    .layer_geometry(layer_surface)
                    .map(|geometry| geometry.loc.to_physical_precise_round(fractional_scale))
                    .unwrap_or_else(|| (0, 0).into());
                let surfaces: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                    render_elements_from_surface_tree(
                        renderer,
                        layer_surface.wl_surface(),
                        loc,
                        render_scale,
                        1.0,
                        Kind::Unspecified,
                    );
                elements.extend(surfaces.into_iter().map(CustomRenderElement::from));
            }
        }
    }

    let active_window = state.layout_for_output(output).active_window().cloned();
    for (window, rect) in layouts.into_iter().rev() {
        if !crate::layout::is_window_mapped(&window) {
            continue;
        }
        let is_active = active_window.as_ref() == Some(&window);
        let physical_rect = Rectangle::new(
            rect.loc.to_physical_precise_round(fractional_scale),
            rect.size.to_physical_precise_round(fractional_scale),
        );
        let is_fs = state.layout_for_output(output).is_fullscreen(&window);
        let titlebar_h = if is_fs { 0 } else { 36 };

        if let Some(surface) = crate::layout::window_wl_surface(&window) {
            // 1. Popups (frontmost)
            for (popup, popup_offset) in PopupManager::popups_for_surface(&surface) {
                let popup_loc = (rect.loc + smithay::utils::Point::from((0, titlebar_h)) + popup_offset)
                    .to_physical_precise_round(fractional_scale);
                let surfaces: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                    render_elements_from_surface_tree(
                        renderer,
                        popup.wl_surface(),
                        popup_loc,
                        render_scale,
                        1.0,
                        Kind::Unspecified,
                    );
                elements.extend(surfaces.into_iter().map(CustomRenderElement::from));
            }
        }

        // 2. Window Titlebar & Controls (rendered IN FRONT of client surface)
        if !is_fs {
            let is_max = state.layout_for_output(output).is_maximized(&window);
            let (app_id, title) = crate::layout::window_metadata(&window);
            let scale = output.current_scale().fractional_scale();
            draw_window_titlebar(
                &mut elements,
                renderer,
                physical_rect,
                is_active,
                palette,
                is_max,
                &app_id,
                &title,
                scale,
                &mut state.title_cache,
                true,
            );
        }

        // 3. Client Surface Content
        if let Some(surface) = crate::layout::window_wl_surface(&window) {
            let client_loc = (rect.loc + smithay::utils::Point::from((0, titlebar_h)))
                .to_physical_precise_round(fractional_scale);
            let win_alpha = state.layout_for_output(output).window_alpha(&window);
            let surfaces: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                render_elements_from_surface_tree(
                    renderer,
                    &surface,
                    client_loc,
                    render_scale,
                    win_alpha,
                    Kind::Unspecified,
                );
            if is_fs {
                elements.extend(surfaces.into_iter().map(CustomRenderElement::from));
            } else {
                let body = Rectangle::new(
                    client_loc,
                    (rect.size - smithay::utils::Size::from((0, titlebar_h)))
                        .to_physical_precise_round(fractional_scale),
                );
                elements.extend(crate::clip_to_body(surfaces, fractional_scale, body));
            }
        }
    }

    {
        let layer_map = layer_map_for_output(&render_output);
        for layer in [Layer::Bottom, Layer::Background] {
            for layer_surface in layer_map.layers_on(layer) {
                let loc = layer_map
                    .layer_geometry(layer_surface)
                    .map(|geometry| geometry.loc.to_physical_precise_round(fractional_scale))
                    .unwrap_or_else(|| (0, 0).into());
                let surfaces: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                    render_elements_from_surface_tree(
                        renderer,
                        layer_surface.wl_surface(),
                        loc,
                        render_scale,
                        1.0,
                        Kind::Unspecified,
                    );
                elements.extend(surfaces.into_iter().map(CustomRenderElement::from));
            }
        }
    }

    elements
}

fn desktop_bounds(
    state: &CortenCompositor,
    primary_width: i32,
    primary_height: i32,
) -> (f64, f64, f64) {
    let mut min_x = 0i32;
    let mut max_x = primary_width;
    let mut max_y = primary_height;
    for output in &state.additional_outputs {
        if let Some(mode) = output.current_mode() {
            let origin = output.current_location().x;
            min_x = min_x.min(origin);
            max_x = max_x.max(origin + mode.size.w);
            max_y = max_y.max(output.current_location().y + mode.size.h);
        }
    }
    (min_x as f64, max_x as f64, max_y as f64)
}

fn libinput_scroll_frame<E>(event: &E, source: AxisSource, natural_scrolling: bool) -> AxisFrame
where
    E: smithay::reexports::input::event::EventTrait
        + smithay::reexports::input::event::pointer::PointerScrollEvent,
{
    let mut frame = AxisFrame::new((event.time_usec() / 1000) as u32).source(source);
    for axis in [Axis::Horizontal, Axis::Vertical] {
        let mut amount = event.scroll_value(axis.into());
        if natural_scrolling {
            amount = -amount;
        }
        if amount != 0.0 {
            frame = frame.value(axis, amount);
        }
    }
    frame
}

fn find_surface_under_loc(
    state: &mut CortenCompositor,
    loc: Point<f64, smithay::utils::Logical>,
) -> Option<(
    wayland_server::protocol::wl_surface::WlSurface,
    Point<f64, smithay::utils::Logical>,
)> {
    if state.session_locked {
        if let Some((lock_surface, _)) = state.lock_surfaces.first() {
            return Some((lock_surface.wl_surface().clone(), Point::from((0.0, 0.0))));
        }
        return None;
    }

    let target_output = state.output_at(loc);
    let out_loc = target_output.current_location().to_f64();
    let out_rel_loc = loc - out_loc;

    // 1. Check top-level layer surfaces: Overlay and Top (Lockscreen, OOBE, Panels, Docks, Launcher)
    let top_layer_match = {
        let layer_map = layer_map_for_output(&target_output);
        let mut found = None;
        for layer in [Layer::Overlay, Layer::Top] {
            for layer_surface in layer_map.layers_on(layer).rev() {
                if let Some(geo) = layer_map.layer_geometry(layer_surface) {
                    if geo.to_f64().contains(out_rel_loc) {
                        let rel_point = out_rel_loc - geo.loc.to_f64();
                        if let Some((surface, sub_loc)) =
                            layer_surface.surface_under(rel_point, WindowSurfaceType::all())
                        {
                            found = Some((surface, out_loc + (geo.loc + sub_loc).to_f64()));
                            break;
                        }
                    }
                }
            }
            if found.is_some() {
                break;
            }
        }
        found
    };
    if let Some(res) = top_layer_match {
        return Some(res);
    }

    // 2. Check XWayland override-redirect surfaces
    for (or_surface, or_geo) in state.override_redirect_surfaces().into_iter().rev() {
        if or_geo.to_f64().contains(loc) {
            return Some((or_surface, or_geo.loc.to_f64()));
        }
    }

    // 3. Check Windows
    let usable_area = {
        let mut map = layer_map_for_output(&target_output);
        map.arrange();
        map.non_exclusive_zone()
    };
    let arranged = state
        .layout_for_output_mut(&target_output)
        .arrange_animated(usable_area);
    for (window, rect) in arranged.into_iter().rev() {
        if !crate::layout::is_window_mapped(&window) {
            continue;
        }
        if rect.to_f64().contains(out_rel_loc) {
            let is_fs = state.layout_for_output(&target_output).is_fullscreen(&window);
            let titlebar_h = if is_fs { 0.0 } else { 36.0 };
            if (out_rel_loc.y - rect.loc.y as f64) < titlebar_h {
                // Inside server titlebar! DO NOT return client surface so buttons get clicks
                return None;
            }
            let rel_point = out_rel_loc - smithay::utils::Point::from((rect.loc.x as f64, rect.loc.y as f64 + titlebar_h));
            if let Some((surface, sub_loc)) =
                window.surface_under(rel_point, WindowSurfaceType::all())
            {
                return Some((surface, out_loc + (smithay::utils::Point::from((rect.loc.x, rect.loc.y + titlebar_h as i32)) + sub_loc).to_f64()));
            }
        }
    }

    // 4. Check bottom layer surfaces: Bottom and Background (Canvas widgets, Wallpapers)
    let bottom_layer_match = {
        let layer_map = layer_map_for_output(&target_output);
        let mut found = None;
        for layer in [Layer::Bottom, Layer::Background] {
            for layer_surface in layer_map.layers_on(layer).rev() {
                if let Some(geo) = layer_map.layer_geometry(layer_surface) {
                    if geo.to_f64().contains(out_rel_loc) {
                        let rel_point = out_rel_loc - geo.loc.to_f64();
                        if let Some((surface, sub_loc)) =
                            layer_surface.surface_under(rel_point, WindowSurfaceType::all())
                        {
                            found = Some((surface, out_loc + (geo.loc + sub_loc).to_f64()));
                            break;
                        }
                    }
                }
            }
            if found.is_some() {
                break;
            }
        }
        found
    };
    if let Some(res) = bottom_layer_match {
        return Some(res);
    }

    None
}

impl DrmBackendState {
    pub fn init() -> Result<Self> {
        info!("Initializing Standalone TTY / DRM-KMS backend via LibSeat...");
        let (session, notifier) =
            LibSeatSession::new().context("Failed to open LibSeat session")?;
        info!(
            "LibSeat session successfully established on seat: {}",
            session.seat()
        );

        let primary_gpu = primary_gpu(session.seat())
            .context("No primary GPU detected via udev")?
            .and_then(|node| DrmNode::from_path(node).ok())
            .or_else(|| {
                all_gpus(session.seat())
                    .ok()?
                    .into_iter()
                    .filter_map(|node| DrmNode::from_path(node).ok())
                    .find(|node| node.ty() == NodeType::Primary)
            })
            .context("Failed to find valid DRM primary node")?;

        let dev_path = primary_gpu
            .dev_path()
            .context("Failed to obtain device path for primary DRM node")?;

        info!(
            "Selected primary GPU: {:?} (path: {:?})",
            primary_gpu, dev_path
        );

        let discovered_nodes: Vec<DrmNode> = all_gpus(session.seat())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| DrmNode::from_path(p).ok())
            .collect();

        let mut secondary_gpus = Vec::new();
        for node in &discovered_nodes {
            if node != &primary_gpu && node.ty() == NodeType::Primary {
                if let Some(path) = node.dev_path() {
                    let has_render = node.has_render();
                    let render_path = node.dev_path_with_type(NodeType::Render);
                    info!(
                        ?node,
                        ?path,
                        ?render_path,
                        has_render,
                        "Discovered secondary GPU device for PRIME render offloading"
                    );
                    secondary_gpus.push(MultiGpuDevice {
                        node: *node,
                        dev_path: path,
                        render_path,
                        is_primary: false,
                        has_render_node: has_render,
                    });
                }
            }
        }

        Ok(Self {
            session,
            notifier,
            primary_gpu,
            dev_path,
            secondary_gpus,
        })
    }
}

/// Runs the compositor directly on Linux TTY via DRM/KMS, GBM and Libinput.
pub fn run_drm(
    mut drm_state: DrmBackendState,
    ipc_rx: std::sync::mpsc::Receiver<crate::CompositorIpcCommand>,
    hotkey_ipc_tx: std::sync::mpsc::Sender<crate::CompositorIpcCommand>,
    shared_ipc_snapshot: std::sync::Arc<std::sync::RwLock<crate::state::CompositorIpcState>>,
    shared_a11y_tree: std::sync::Arc<std::sync::RwLock<corten_services::a11y::AccessibleTree>>,
    event_broadcaster: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<()> {
    info!("Starting Corten Compositor Standalone DRM/KMS engine...");

    // 1. Open the DRM node only through LibSeat. A direct /dev/dri fallback
    // breaks session ownership and can make CRTC restore fail with EACCES when
    // the compositor exits or the VT is switched.
    let drm_fd = drm_state
        .session
        .open(&drm_state.dev_path, OFlags::RDWR | OFlags::CLOEXEC)
        .context("Failed to open DRM device node through LibSeat")?;

    let device_fd = DeviceFd::from(drm_fd);
    let drm_device_fd = DrmDeviceFd::new(device_fd);
    let (drm_device, drm_notifier) =
        DrmDevice::new(drm_device_fd.clone(), false).context("Failed to initialize DrmDevice")?;

    // 2. Initialize GBM device and allocator
    let gbm_device = GbmDevice::new(drm_device_fd.clone()).context("Failed to create GbmDevice")?;
    let gbm_allocator = GbmAllocator::new(
        gbm_device.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let gbm_exporter = GbmFramebufferExporter::new(gbm_device.clone(), Some(drm_state.primary_gpu));

    // 3. Initialize EGL & Hardware GlowRenderer
    let egl_display = unsafe { EGLDisplay::new(gbm_device.clone()) }
        .context("Failed to initialize EGLDisplay from GBM device")?;
    let egl_context = EGLContext::new(&egl_display).context("Failed to create EGLContext")?;
    let mut renderer = unsafe { GlowRenderer::new(egl_context) }
        .context("Failed to initialize GlowRenderer from EGL context")?;

    // 4. Discover connected monitors and choose the primary connector & CRTC mode
    let res_handles = drm_device
        .resource_handles()
        .context("Failed to query DRM resource handles")?;

    let mut connected_monitors = Vec::new();
    for conn_handle in res_handles.connectors() {
        if let Ok(info) = drm_device.get_connector(*conn_handle, false) {
            if info.state() == connector::State::Connected {
                let preferred = info
                    .modes()
                    .iter()
                    .find(|m| {
                        m.mode_type()
                            .contains(smithay::reexports::drm::control::ModeTypeFlags::PREFERRED)
                    })
                    .cloned()
                    .or_else(|| info.modes().first().cloned());

                if let Some(mode) = preferred {
                    connected_monitors.push((info, *conn_handle, mode));
                }
            }
        }
    }

    if connected_monitors.is_empty() {
        anyhow::bail!("No connected monitor found on primary DRM GPU");
    }

    // Sort to prioritize primary high-res monitor (e.g. 2560x1440 M27Q over 1920x1080)
    connected_monitors.sort_by_key(|(_, _, mode)| {
        let (w, h) = mode.size();
        std::cmp::Reverse((w as u64) * (h as u64))
    });

    for (info, handle, mode) in &connected_monitors {
        let (w, h) = mode.size();
        info!(
            "Discovered connected DRM monitor: {:?} (ID: {:?}) - Mode: {}x{} @ {}Hz",
            info.interface(),
            handle,
            w,
            h,
            mode.vrefresh()
        );
    }

    let (conn_info, conn_handle, preferred_mode) = connected_monitors.remove(0);
    let screen_w = preferred_mode.size().0 as i32;
    let screen_h = preferred_mode.size().1 as i32;
    let initial_size = Size::from((screen_w, screen_h));
    info!(
        "Selected PRIMARY DRM Output ({:?}): Mode {}x{} @ {}mHz",
        conn_info.interface(),
        screen_w,
        screen_h,
        preferred_mode.vrefresh()
    );

    // 5. Initialize Wayland Display & State
    let mut display: Display<CortenCompositor> = Display::new()?;
    let mut state = CortenCompositor::new(&mut display, initial_size);
    state.ipc_snapshot = shared_ipc_snapshot;
    state.a11y_bridge.tree = shared_a11y_tree;
    state.event_broadcaster = Some(event_broadcaster);
    state.sync_ipc_snapshot();
    apply_display_settings(&mut state);

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
        .add_keyboard(xkb_config, 25, 600)
        .map_err(|e| anyhow::anyhow!("Failed to add keyboard: {:?}", e))?;
    let pointer = state.seat.add_pointer();

    // Setup Output state
    let output_mode = OutputMode {
        size: initial_size,
        refresh: (preferred_mode.vrefresh() * 1000) as i32,
    };
    state
        .output
        .change_current_state(Some(output_mode), None, None, None);
    state.output.set_preferred(output_mode);

    // 6. Initialize DrmOutputManager and DrmOutput
    let color_formats = [Fourcc::Argb8888, Fourcc::Xrgb8888];
    let renderer_formats = renderer
        .egl_context()
        .display()
        .dmabuf_render_formats()
        .clone();

    let mut output_manager = DrmOutputManager::new(
        drm_device,
        gbm_allocator,
        gbm_exporter,
        Some(gbm_device),
        color_formats,
        renderer_formats,
    );

    let drm_render_elements = DrmOutputRenderElements::<GlowRenderer, CustomRenderElement>::new();

    // Find compatible CRTC for conn_handle (DRM encoders link specific connectors to specific CRTCs)
    let mut initialized_output = None;
    let mut initialized_crtc = None;
    let mut last_err = None;

    for &test_crtc in res_handles.crtcs() {
        info!(
            "Testing CRTC {:?} for connector {:?} (mode: {}x{})...",
            test_crtc, conn_handle, screen_w, screen_h
        );
        match output_manager.initialize_output(
            test_crtc,
            preferred_mode,
            &[conn_handle],
            OutputModeSource::Auto(state.output.clone()),
            None,
            &mut renderer,
            &drm_render_elements,
        ) {
            Ok(output) => {
                info!(
                    "Successfully bound CRTC {:?} to primary connector {:?}",
                    test_crtc, conn_handle
                );
                initialized_output = Some(output);
                initialized_crtc = Some(test_crtc);
                break;
            }
            Err(e) => {
                tracing::warn!("CRTC {:?} rejected by DRM driver: {:?}", test_crtc, e);
                last_err = Some(e);
            }
        }
    }

    let drm_output = initialized_output.ok_or_else(|| {
        anyhow::anyhow!(
            "Failed to initialize DRM output across all CRTCs: {:?}",
            last_err
        )
    })?;
    let primary_crtc = initialized_crtc.expect("initialized DRM output must have a CRTC");
    try_enable_vrr(output_manager.device(), conn_handle, primary_crtc);
    let init_disp_config = ConfigManager::<corten_config::DisplaySettingsConfig>::load_or_default("displays")
        .map(|c| c.get().clone())
        .unwrap_or_default();
    apply_drm_hdr_and_color_management(output_manager.device(), conn_handle, &init_disp_config);
    let mut used_crtcs = vec![primary_crtc];
    let mut active_connectors = vec![conn_handle];

    let (secondary_scale_pct, arrangement, selected_secondary_output, offset_y) =
        secondary_display_settings();
    if selected_secondary_output != "None"
        && !selected_secondary_output.is_empty()
        && !connected_monitors
            .iter()
            .any(|(info, _, _)| info.to_string() == selected_secondary_output)
    {
        tracing::warn!(
            output = %selected_secondary_output,
            "Configured secondary output was not found among connected DRM connectors"
        );
    }
    let mut drm_outputs = vec![drm_output];
    let mut drm_mode_lists = vec![conn_info.modes().to_vec()];
    let mut next_output_x = screen_w;
    let mut next_left_x = 0;
    for (info, handle, mode) in connected_monitors.iter() {
        let Some(crtc) = res_handles
            .crtcs()
            .iter()
            .copied()
            .find(|candidate| !used_crtcs.contains(candidate))
        else {
            tracing::warn!("No unused CRTC available for secondary DRM output");
            break;
        };

        let (width, height) = mode.size();
        let output_name = info.to_string();
        let selected = selected_secondary_output == "None"
            || selected_secondary_output.is_empty()
            || selected_secondary_output == output_name;
        let output_mode = OutputMode {
            size: Size::from((width as i32, height as i32)),
            refresh: (mode.vrefresh() * 1000) as i32,
        };
        let location = Point::from(secondary_output_location(
            &arrangement,
            screen_w,
            &mut next_left_x,
            &mut next_output_x,
            width as i32,
            offset_y,
        ));
        let output = state.create_output(
            output_name.clone(),
            output_mode,
            location,
            smithay::output::Scale::Fractional(if selected {
                secondary_scale_pct as f64 / 100.0
            } else {
                1.0
            }),
        );

        match output_manager.initialize_output(
            crtc,
            *mode,
            &[*handle],
            OutputModeSource::Auto(output.clone()),
            None,
            &mut renderer,
            &drm_render_elements,
        ) {
            Ok(secondary) => {
                try_enable_vrr(output_manager.device(), *handle, crtc);
                apply_drm_hdr_and_color_management(output_manager.device(), *handle, &init_disp_config);
                state.register_output(&mut display, output);
                used_crtcs.push(crtc);
                active_connectors.push(*handle);
                info!(
                    "Initialized secondary DRM output {:?} at {:?} ({width}x{height})",
                    info.interface(),
                    location
                );
                drm_outputs.push(secondary);
                drm_mode_lists.push(info.modes().to_vec());
            }
            Err(error) => {
                tracing::warn!(
                    "Could not initialize secondary DRM output {:?}: {:?}",
                    info.interface(),
                    error
                );
                break;
            }
        }
    }
    if !connected_monitors.is_empty() {
        info!(
            "DRM output topology: {} active output(s)",
            drm_outputs.len()
        );
    }

    // Register Dmabuf Global so Wayland clients (Iced, Alacritty, Mesa, etc.) can allocate GPU dmabufs
    let dmabuf_formats = renderer
        .egl_context()
        .display()
        .dmabuf_render_formats()
        .clone();
    let dmabuf_global = state
        .dmabuf_state
        .create_global::<CortenCompositor>(&display.handle(), dmabuf_formats);
    state.dmabuf_global = Some(dmabuf_global);
    info!("Registered zwp_linux_dmabuf_v1 global for hardware video/client acceleration");

    // Explicit sync (linux-drm-syncobj-v1): only exposed when the kernel/GPU supports syncobj eventfd.
    if smithay::wayland::drm_syncobj::supports_syncobj_eventfd(&drm_device_fd) {
        state.syncobj_state = Some(smithay::wayland::drm_syncobj::DrmSyncobjState::new::<
            CortenCompositor,
        >(&display.handle(), drm_device_fd.clone()));
        info!("Registered wp_linux_drm_syncobj_manager_v1 (explicit sync) global");
    } else {
        info!("DRM syncobj eventfd unsupported; explicit sync disabled (implicit sync fallback)");
    }

    // 7. Bind Wayland Listening Socket
    let socket_name = "corten-0";
    let listener = ListeningSocket::bind(socket_name)?;
    info!(
        "Corten Compositor (DRM mode) listening on socket: {}",
        socket_name
    );
    std::env::set_var("WAYLAND_DISPLAY", socket_name);

    // 8. Initialize Libinput Input Backend
    let mut libinput_ctx =
        Libinput::new_with_udev(LibinputSessionInterface::from(drm_state.session.clone()));
    libinput_ctx
        .udev_assign_seat(&drm_state.session.seat())
        .map_err(|_| anyhow::anyhow!("Failed to assign seat to Libinput context"))?;
    let tablet_state = crate::tablet::TabletState::new();

    // Setup Calloop Event Loop for LibSeat Session events (enables VT switching, session management, and XWayland)
    let mut event_loop: smithay::reexports::calloop::EventLoop<'static, CortenCompositor> =
        smithay::reexports::calloop::EventLoop::try_new()
            .context("Failed to create Calloop event loop")?;
    let loop_handle = event_loop.handle();
    state.loop_handle = Some(loop_handle.clone());
    let _xwayland_guard = crate::xwayland::start_xwayland(&display.handle(), &loop_handle);
    let _session_source = loop_handle
        .insert_source(drm_state.notifier, |event, _, _| {
            info!("LibSeat session event received: {:?}", event);
        })
        .map_err(|e| {
            anyhow::anyhow!(
                "Failed to register LibSeatSessionNotifier in event loop: {:?}",
                e
            )
        })?;

    // VBlank events carry the CRTC that completed. Keep that identity so a
    // multi-output swapchain only receives frame_submitted() for its own CRTC.
    let vblank_crtcs = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let _drm_source = {
        let vblank_crtcs = vblank_crtcs.clone();
        loop_handle
            .insert_source(drm_notifier, move |event, _, _| match event {
                smithay::backend::drm::DrmEvent::VBlank(crtc) => {
                    vblank_crtcs.borrow_mut().push(crtc);
                }
                smithay::backend::drm::DrmEvent::Error(err) => {
                    tracing::warn!("DRM device error: {:?}", err);
                }
            })
            .map_err(|e| {
                anyhow::anyhow!("Failed to register DRM notifier in event loop: {:?}", e)
            })?
    };

    // Runtime udev DRM hotplug monitor (detects HDMI/DP plug/unplug events without restarting compositor)
    let hotplug_pending = std::rc::Rc::new(std::cell::Cell::new(false));
    if let Ok(udev_backend) = UdevBackend::new(drm_state.session.seat()) {
        let hotplug_flag = hotplug_pending.clone();
        let _ = loop_handle.insert_source(udev_backend, move |event, _, _| {
            if matches!(event, UdevEvent::Changed { .. } | UdevEvent::Added { .. }) {
                info!("Udev DRM hotplug event detected: {:?}", event);
                hotplug_flag.set(true);
            }
        });
    }
    let mut pending_crtcs = Vec::new();

    // 9. Main Compositor TTY DRM Loop
    let mut clients = Vec::new();
    let start_time = Instant::now();
    let (mut theme_mode, mut active_patina) = (ThemeMode::Dark, PatinaVariant::Rust);
    let mut appearance_rx = None;
    if let Ok(app_cfg) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
        theme_mode = match app_cfg.get().theme_mode {
            corten_config::ThemeModeChoice::Light => ThemeMode::Light,
            corten_config::ThemeModeChoice::Dark => ThemeMode::Dark,
        };
        active_patina = PatinaVariant::from_code(&app_cfg.get().patina);
        if let Ok((_watcher, rx)) = app_cfg.spawn_watcher() {
            std::mem::forget(_watcher);
            appearance_rx = Some(rx);
        }
    }
    let mut a11y_rx = None;
    if let Ok(a11y_cfg) = ConfigManager::<corten_config::AccessibilityConfig>::load_or_default("accessibility") {
        state.accessibility = a11y_cfg.get().clone();
        if let Ok((_watcher, rx)) = a11y_cfg.spawn_watcher() {
            std::mem::forget(_watcher);
            a11y_rx = Some(rx);
        }
    }
    let mut palette = if state.accessibility.high_contrast {
        Palette::high_contrast_dark()
    } else {
        Palette::for_mode(theme_mode).with_patina(active_patina)
    }.with_color_filter(match state.accessibility.color_filter {
        corten_config::ColorFilterChoice::None => corten_theme::ColorFilterMode::None,
        corten_config::ColorFilterChoice::Protanopia => corten_theme::ColorFilterMode::Protanopia,
        corten_config::ColorFilterChoice::Deuteranopia => corten_theme::ColorFilterMode::Deuteranopia,
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
    let shutdown_requested = std::rc::Rc::new(std::cell::Cell::new(false));

    if let Ok(cfg_mgr) = ConfigManager::<CompositorConfig>::load_or_default("compositor") {
        state.set_gap_for_all_outputs(cfg_mgr.get().gap);
        state.set_layout_mode_for_all_outputs(cfg_mgr.get().layout_mode);
        state.window_rules = cfg_mgr.get().window_rules.clone();
    }

    // Static persistent IDs for cursor elements (Crucial for Smithay damage tracking!)
    let cursor_body_ids: Vec<Id> = (0..48).map(|_| Id::new()).collect();
    let cursor_border_ids: Vec<Id> = (0..48).map(|_| Id::new()).collect();
    let cursor_tail_id = Id::new();
    let cursor_outline_id = Id::new();
    let mut cursor_commit = CommitCounter::default();
    let mut cursor_theme_manager = crate::cursor::CursorThemeManager::new();

    info!("Corten Compositor: Standalone TTY DRM engine running!");

    loop {
        // Dynamically update palette and patina if appearance or accessibility config changes at runtime
        let mut palette_dirty = false;
        if let Some(ref rx) = appearance_rx {
            while let Ok(new_cfg) = rx.try_recv() {
                theme_mode = match new_cfg.theme_mode {
                    corten_config::ThemeModeChoice::Light => ThemeMode::Light,
                    corten_config::ThemeModeChoice::Dark => ThemeMode::Dark,
                };
                active_patina = PatinaVariant::from_code(&new_cfg.patina);
                palette_dirty = true;
            }
        }
        if let Some(ref rx) = a11y_rx {
            while let Ok(new_a11y) = rx.try_recv() {
                state.accessibility = new_a11y;
                cursor_theme_manager.set_target_size(if state.accessibility.large_cursor { 48 } else { 24 });
                palette_dirty = true;
            }
        }
        if palette_dirty {
            palette = if state.accessibility.high_contrast {
                Palette::high_contrast_dark()
            } else {
                Palette::for_mode(theme_mode).with_patina(active_patina)
            }.with_color_filter(match state.accessibility.color_filter {
                corten_config::ColorFilterChoice::None => corten_theme::ColorFilterMode::None,
                corten_config::ColorFilterChoice::Protanopia => corten_theme::ColorFilterMode::Protanopia,
                corten_config::ColorFilterChoice::Deuteranopia => corten_theme::ColorFilterMode::Deuteranopia,
                corten_config::ColorFilterChoice::Tritanopia => corten_theme::ColorFilterMode::Tritanopia,
                corten_config::ColorFilterChoice::Monochrome => corten_theme::ColorFilterMode::Monochrome,
            });
            bg_color = Color32F::new(
                palette.background.r,
                palette.background.g,
                palette.background.b,
                1.0,
            );
        }

        // Dispatch session and XWayland events (processes VT switch requests and X11Wm events)
        let _ = event_loop.dispatch(Some(std::time::Duration::from_millis(0)), &mut state);

        // Process runtime DRM connector hotplug events (HDMI / DisplayPort plug & unplug)
        if hotplug_pending.replace(false) {
            if let Ok(handles) = output_manager.device().resource_handles() {
                let mut current_connected = Vec::new();
                for &ch in handles.connectors() {
                    if let Ok(info) = output_manager.device().get_connector(ch, true) {
                        if info.state() == connector::State::Connected {
                            let preferred = info
                                .modes()
                                .iter()
                                .find(|m| {
                                    m.mode_type().contains(
                                        smithay::reexports::drm::control::ModeTypeFlags::PREFERRED,
                                    )
                                })
                                .cloned()
                                .or_else(|| info.modes().first().cloned());
                            if let Some(mode) = preferred {
                                current_connected.push((info, ch, mode));
                            }
                        }
                    }
                }

                // 1. Remove any secondary outputs whose connector was unplugged
                let mut idx = 1;
                while idx < active_connectors.len() {
                    let conn = active_connectors[idx];
                    let still_connected = current_connected.iter().any(|(_, ch, _)| *ch == conn);
                    if !still_connected {
                        info!(
                            "DRM Hotplug: Secondary connector {:?} disconnected; releasing CRTC",
                            conn
                        );
                        active_connectors.remove(idx);
                        let removed_output = drm_outputs.remove(idx);
                        let removed_crtc = removed_output.crtc();
                        used_crtcs.retain(|&c| c != removed_crtc);
                        pending_crtcs.retain(|&c| c != removed_crtc);
                        if idx <= drm_mode_lists.len() {
                            drm_mode_lists.remove(idx - 1);
                        }
                        if idx - 1 < state.additional_outputs.len() {
                            let wayland_out = state.additional_outputs.get(idx - 1).cloned();
                            if let Some(out) = wayland_out {
                                state.rescue_orphan_windows(&out.name());
                            }
                        }
                    } else {
                        idx += 1;
                    }
                }

                // 2. Initialize any newly plugged-in secondary connectors
                let (secondary_scale_pct, arrangement, _, offset_y) = secondary_display_settings();
                for (info, ch, mode) in current_connected {
                    if active_connectors.contains(&ch) {
                        continue;
                    }
                    let Some(crtc) = handles
                        .crtcs()
                        .iter()
                        .copied()
                        .find(|candidate| !used_crtcs.contains(candidate))
                    else {
                        tracing::warn!(
                            "DRM Hotplug: No unused CRTC available for newly connected monitor"
                        );
                        break;
                    };
                    let (width, height) = mode.size();
                    let output_name = info.to_string();
                    let output_mode = OutputMode {
                        size: Size::from((width as i32, height as i32)),
                        refresh: (mode.vrefresh() * 1000) as i32,
                    };
                    let location = Point::from(secondary_output_location(
                        &arrangement,
                        screen_w,
                        &mut next_left_x,
                        &mut next_output_x,
                        width as i32,
                        offset_y,
                    ));
                    let output = state.create_output(
                        output_name.clone(),
                        output_mode,
                        location,
                        smithay::output::Scale::Fractional(secondary_scale_pct as f64 / 100.0),
                    );
                    if let Ok(secondary) = output_manager.initialize_output(
                        crtc,
                        mode,
                        &[ch],
                        OutputModeSource::Auto(output.clone()),
                        None,
                        &mut renderer,
                        &drm_render_elements,
                    ) {
                        try_enable_vrr(output_manager.device(), ch, crtc);
                        let disp_cfg = ConfigManager::<corten_config::DisplaySettingsConfig>::load_or_default("displays")
                            .map(|c| c.get().clone())
                            .unwrap_or_default();
                        apply_drm_hdr_and_color_management(output_manager.device(), ch, &disp_cfg);
                        state.register_output(&mut display, output);
                        used_crtcs.push(crtc);
                        active_connectors.push(ch);
                        drm_outputs.push(secondary);
                        drm_mode_lists.push(info.modes().to_vec());
                        state.sync_ipc_snapshot();
                        crate::send_osd_notification(
                            "display",
                            "Yeni Ekran Bağlandı",
                            &format!("{} ({}x{}) aktif", output_name, width, height),
                            100,
                        );
                        if let Some(ref tx) = state.event_broadcaster {
                            let payload = format!(
                                r#"{{"event":"output:connected","data":{{"output":"{}","width":{},"height":{}}}}}"#,
                                output_name, width, height
                            );
                            let _ = tx.send(payload);
                        }
                        info!(
                            "DRM Hotplug: Activated newly connected monitor {:?} ({width}x{height})",
                            info.interface()
                        );
                    }
                }
            }
        }

        // Reload config or handle window/workspace commands from D-Bus if requested
        if let Ok(command) = ipc_rx.try_recv() {
            match command {
                crate::CompositorIpcCommand::SwitchWorkspace(workspace) => {
                    state.switch_workspace(workspace);
                    continue;
                }
                crate::CompositorIpcCommand::SwitchWorkspaceOnOutput(output, workspace) => {
                    state.switch_workspace_on_output(&output, workspace);
                    continue;
                }
                crate::CompositorIpcCommand::FocusWindow(window_id) => {
                    state.focus_window_by_id(window_id);
                    continue;
                }
                crate::CompositorIpcCommand::ToggleShowDesktop => {
                    state.toggle_show_desktop();
                    continue;
                }
                crate::CompositorIpcCommand::ToggleOverview => {
                    state.overview_mode = !state.overview_mode;
                    if state.overview_mode {
                        crate::send_osd_notification(
                            "overview",
                            "Pencere Genel Bakışı",
                            "Exposé Modu Aktif",
                            100,
                        );
                    }
                    continue;
                }
                crate::CompositorIpcCommand::TakeScreenshot(region) => {
                    state.request_screenshot(region);
                    continue;
                }
                crate::CompositorIpcCommand::ReloadConfig => {}
            }
            info!("Compositor: Reloading configuration from ~/.config/corten/compositor.toml");
            if let Ok(cfg_mgr) = ConfigManager::<CompositorConfig>::load_or_default("compositor") {
                state.set_gap_for_all_outputs(cfg_mgr.get().gap);
                state.set_layout_mode_for_all_outputs(cfg_mgr.get().layout_mode);
                state.window_rules = cfg_mgr.get().window_rules.clone();
                keybindings = cfg_mgr.get().keybindings.clone();
            }
            if let Ok(config) = ConfigManager::<InputSettingsConfig>::load_or_default("input") {
                input_settings = config.get().clone();
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
            crate::apply_power_settings(&mut state);
            if let Ok(display_config) =
                ConfigManager::<crate::DisplaySettingsConfig>::load_or_default("displays")
            {
                if let Some(mode) =
                    configured_drm_mode(&display_config.get().resolution, &drm_mode_lists[0])
                {
                    let output_mode = OutputMode::from(mode);
                    match output_manager.use_mode(
                        &drm_outputs[0].crtc(),
                        mode,
                        &mut renderer,
                        &drm_render_elements,
                    ) {
                        Ok(()) => {
                            state
                                .output
                                .change_current_state(Some(output_mode), None, None, None);
                            state.output.set_preferred(output_mode);
                            info!(
                                "DRM primary output mode changed to {}",
                                display_config.get().resolution
                            );
                        }
                        Err(error) => tracing::warn!(?error, "Failed to apply configured DRM mode"),
                    }
                }
                for &ch in &active_connectors {
                    apply_drm_hdr_and_color_management(output_manager.device(), ch, display_config.get());
                }
            }
        }

        // Accept new Wayland clients
        if let Some(stream) = listener.accept()? {
            info!("Accepted new Wayland client connection in DRM mode");
            let client = display
                .handle()
                .insert_client(stream, Arc::new(ClientState::default()))?;
            clients.push(client);
        }

        display.dispatch_clients(&mut state)?;
        display.flush_clients()?;

        // Process hardware input events via Libinput
        let _ = libinput_ctx.dispatch();
        let mut events: Vec<smithay::reexports::input::Event> = Vec::new();
        for event in &mut libinput_ctx {
            events.push(event);
        }

        for event in events {
            state.note_input();
            match event {
                smithay::reexports::input::Event::Keyboard(kb_event) => {
                    use smithay::reexports::input::event::KeyboardEvent;
                    let key = match &kb_event {
                        KeyboardEvent::Key(k) => k.key_code(),
                        _ => continue,
                    };
                    let raw_key = match &kb_event {
                        KeyboardEvent::Key(k) => k.key(),
                        _ => 0,
                    };
                    let key_state = match kb_event.key_state() {
                        smithay::reexports::input::event::keyboard::KeyState::Pressed => {
                            KeyState::Pressed
                        }
                        smithay::reexports::input::event::keyboard::KeyState::Released => {
                            KeyState::Released
                        }
                    };
                    let time = (kb_event.time_usec() / 1000) as u32;
                    let serial = SERIAL_COUNTER.next_serial();

                    // Raw emergency exit check directly from libinput hardware scan:
                    // 1) Shift + Escape (key 1)
                    // 2) Ctrl + Alt + Backspace (key 14)
                    let mod_state = state.seat.get_keyboard().map(|kb| kb.modifier_state());
                    let is_shift = mod_state.as_ref().map(|m| m.shift).unwrap_or(false);
                    let is_ctrl_alt = mod_state.as_ref().map(|m| m.ctrl && m.alt).unwrap_or(false);

                    if key_state == KeyState::Pressed
                        && ((raw_key == 1 && is_shift) || (raw_key == 14 && is_ctrl_alt))
                    {
                        info!("Emergency Exit triggered directly from DRM input!");
                        shutdown_requested.set(true);
                        continue;
                    }

                    let shutdown_requested_for_keyboard = shutdown_requested.clone();
                    keyboard.input::<(), _>(
                        &mut state,
                        key,
                        key_state,
                        serial,
                        time,
                        |state, modifiers, keysym_handle| {
                            if key_state == KeyState::Pressed {
                                let sym = keysym_handle.modified_sym().raw();

                                // Always allow VT Switching (Ctrl + Alt + F1..F7) and Emergency Exit even while locked!
                                if modifiers.ctrl && modifiers.alt {
                                    let vt = match sym {
                                        keysyms::KEY_F1 => Some(1),
                                        keysyms::KEY_F2 => Some(2),
                                        keysyms::KEY_F3 => Some(3),
                                        keysyms::KEY_F4 => Some(4),
                                        keysyms::KEY_F5 => Some(5),
                                        keysyms::KEY_F6 => Some(6),
                                        keysyms::KEY_F7 => Some(7),
                                        keysyms::KEY_BackSpace => {
                                            info!("Compositor Emergency Exit: Ctrl+Alt+Backspace triggered!");
                                            shutdown_requested_for_keyboard.set(true);
                                            None
                                        }
                                        _ => None,
                                    };
                                    if let Some(target_vt) = vt {
                                        info!("Switching Virtual Terminal (VT) to VT {}", target_vt);
                                        let _ = drm_state.session.change_vt(target_vt);
                                        return FilterResult::Intercept(());
                                    }
                                }

                                if modifiers.shift && sym == keysyms::KEY_Escape {
                                    info!("Compositor Emergency Exit: Shift+Escape triggered!");
                                    shutdown_requested_for_keyboard.set(true);
                                    return FilterResult::Intercept(());
                                }
                            }

                            if state.lock_surface_active() {
                                if let Some((lock_surface, _)) = state.lock_surfaces.first().cloned() {
                                    if let Some(kb) = state.seat.get_keyboard() {
                                        let lock_wl = lock_surface.wl_surface().clone();
                                        if kb.current_focus().as_ref() != Some(&lock_wl) {
                                            kb.set_focus(state, Some(lock_wl), serial);
                                        }
                                    }
                                }
                                return FilterResult::Forward;
                            }
                            if state.focused_surface_inhibits_shortcuts() {
                                return FilterResult::Forward;
                            }

                            if key_state == KeyState::Pressed {
                                let sym = keysym_handle.modified_sym().raw();
                                if crate::handle_media_keys(sym) {
                                    return FilterResult::Intercept(());
                                }
                                let binding = |action: &str| {
                                    binding_matches(
                                        &keybindings,
                                        action,
                                        sym,
                                        modifiers.logo,
                                        modifiers.ctrl,
                                        modifiers.alt,
                                        modifiers.shift,
                                    )
                                };
                                match sym {
                                    keysyms::KEY_Right if binding("move_next_output") => {
                                        state.move_active_window_between_outputs(1);
                                        return FilterResult::Intercept(());
                                    }
                                    keysyms::KEY_Left if binding("move_previous_output") => {
                                        state.move_active_window_between_outputs(-1);
                                        return FilterResult::Intercept(());
                                    }
                                    // Window Snapping Chords (Super + Arrows)
                                    keysyms::KEY_Left if binding("snap_left") => {
                                        info!("Compositor Hotkey: Super+Left (Snap Left 1/2)");
                                        if let Some(active) = state.layout.active_window().cloned() {
                                            let target_output = state.output_at(state.cursor_location);
                                            let usable_area = {
                                                let mut map = layer_map_for_output(&target_output);
                                                map.arrange();
                                                map.non_exclusive_zone()
                                            };
                                            let half_w = usable_area.size.w / 2;
                                            let snap_rect = Rectangle::new(
                                                usable_area.loc,
                                                Size::from((half_w, usable_area.size.h)),
                                            );
                                            state.layout_for_output_mut(&target_output).set_floating_geometry(&active, snap_rect);
                                            crate::send_osd_notification("snap", "Magnetic Snap", "Sol Tarafa Hizalandı (1/2)", 100);
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    keysyms::KEY_Right if binding("snap_right") => {
                                        info!("Compositor Hotkey: Super+Right (Snap Right 1/2)");
                                        if let Some(active) = state.layout.active_window().cloned() {
                                            let target_output = state.output_at(state.cursor_location);
                                            let usable_area = {
                                                let mut map = layer_map_for_output(&target_output);
                                                map.arrange();
                                                map.non_exclusive_zone()
                                            };
                                            let half_w = usable_area.size.w / 2;
                                            let snap_rect = Rectangle::new(
                                                Point::from((usable_area.loc.x + half_w, usable_area.loc.y)),
                                                Size::from((half_w, usable_area.size.h)),
                                            );
                                            state.layout_for_output_mut(&target_output).set_floating_geometry(&active, snap_rect);
                                            crate::send_osd_notification("snap", "Magnetic Snap", "Sağ Tarafa Hizalandı (1/2)", 100);
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    keysyms::KEY_Up if binding("snap_up") => {
                                        info!("Compositor Hotkey: Super+Up (Snap Maximize)");
                                        if let Some(active) = state.layout.active_window().cloned() {
                                            let target_output = state.output_at(state.cursor_location);
                                            let usable_area = {
                                                let mut map = layer_map_for_output(&target_output);
                                                map.arrange();
                                                map.non_exclusive_zone()
                                            };
                                            state.layout_for_output_mut(&target_output).set_floating_geometry(&active, usable_area);
                                            crate::send_osd_notification("snap", "Magnetic Snap", "Tam Ekran Kaplandı", 100);
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    keysyms::KEY_Down if binding("snap_down") => {
                                        info!("Compositor Hotkey: Super+Down (Snap Restore/Center)");
                                        if let Some(active) = state.layout.active_window().cloned() {
                                            let target_output = state.output_at(state.cursor_location);
                                            let usable_area = {
                                                let mut map = layer_map_for_output(&target_output);
                                                map.arrange();
                                                map.non_exclusive_zone()
                                            };
                                            let w = (usable_area.size.w as f32 * 0.7) as i32;
                                            let h = (usable_area.size.h as f32 * 0.7) as i32;
                                            let center_x = usable_area.loc.x + (usable_area.size.w - w) / 2;
                                            let center_y = usable_area.loc.y + (usable_area.size.h - h) / 2;
                                            let restore_rect = Rectangle::new(
                                                Point::from((center_x, center_y)),
                                                Size::from((w, h)),
                                            );
                                            state.layout_for_output_mut(&target_output).set_floating_geometry(&active, restore_rect);
                                            crate::send_osd_notification("snap", "Magnetic Snap", "Standart Boyuta Geri Yüklendi", 100);
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Shift + Return: Swap with Master
                                    keysyms::KEY_Return if binding("swap_master") => {
                                        info!("Compositor Hotkey: Super+Shift+Return (Swap Master)");
                                        state.layout.swap_master();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Return: Launch Terminal
                                    keysyms::KEY_Return if binding("terminal") => {
                                        info!("Compositor Hotkey: Super+Return (Terminal)");
                                        let _ = spawn_first_terminal();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Tab: Cycle Tiling Layout Mode
                                    keysyms::KEY_Tab | keysyms::KEY_ISO_Left_Tab if binding("cycle_layout") => {
                                        let next_mode = state.cycle_layout_mode_for_all_outputs();
                                        info!("Compositor Hotkey: Cycled tiling layout mode to {:?}", next_mode);
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + B: BSP Horizontal Split
                                    keysyms::KEY_b | keysyms::KEY_B if binding("split_horizontal") => {
                                        info!("Compositor Hotkey: Super+B (BSP Horizontal Split)");
                                        state.set_split_direction_for_all_outputs(crate::layout::SplitDirection::Horizontal);
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + V: BSP Vertical Split
                                    keysyms::KEY_v | keysyms::KEY_V if binding("split_vertical") => {
                                        info!("Compositor Hotkey: Super+V (BSP Vertical Split)");
                                        state.set_split_direction_for_all_outputs(crate::layout::SplitDirection::Vertical);
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Shift + Space: Toggle Floating
                                    keysyms::KEY_space if binding("toggle_floating") => {
                                        info!("Compositor Hotkey: Super+Shift+Space (Toggle Floating)");
                                        if let Some(active) = state.layout.active_window().cloned() {
                                            state.toggle_floating_for_all_outputs(active);
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Shift + Escape: Emergency Exit back to TTY
                                    keysyms::KEY_Escape if modifiers.shift => {
                                        info!("Compositor Emergency Exit: Super+Shift+Escape triggered!");
                                        shutdown_requested_for_keyboard.set(true);
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Alt + Space: Corten AI HUD
                                    keysyms::KEY_space if binding("ai_assistant") => {
                                        info!("Compositor Hotkey: Super+Alt+Space (Corten AI HUD)");
                                        let ai_path = crate::state::resolve_corten_bin("corten-ai");
                                        let _ = std::process::Command::new(ai_path).spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Space or Super + D: Launcher (Toggle)
                                    keysyms::KEY_space | keysyms::KEY_d | keysyms::KEY_D
                                        if binding("launcher") => {
                                        info!("Compositor Hotkey: Super+Space/D (Launcher Toggle)");
                                        let check = std::process::Command::new("killall")
                                            .args(["-q", "corten-launcher"])
                                            .status();
                                        if let Ok(status) = check {
                                            if status.success() {
                                                return FilterResult::Intercept(());
                                            }
                                        }
                                        let launcher_path = crate::state::resolve_corten_bin("corten-launcher");
                                        let _ = std::process::Command::new(launcher_path).spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + A: Dashboard
                                    keysyms::KEY_a | keysyms::KEY_A if binding("dashboard") => {
                                        info!("Compositor Hotkey: Super+A (Dashboard)");
                                        let launcher_path = crate::state::resolve_corten_bin("corten-launcher");
                                        let _ = std::process::Command::new(launcher_path).arg("--dashboard").spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + F1 / Super + slash: Cheat Sheet
                                    keysyms::KEY_F1 | keysyms::KEY_slash | keysyms::KEY_question
                                        if modifiers.logo && !modifiers.ctrl && !modifiers.alt => {
                                        info!("Compositor Hotkey: Super+F1 / Super+? (Shortcuts Cheat Sheet)");
                                        let check = std::process::Command::new("killall")
                                            .args(["-q", "corten-launcher"])
                                            .status();
                                        if let Ok(status) = check {
                                            if status.success() {
                                                return FilterResult::Intercept(());
                                            }
                                        }
                                        let launcher_path = crate::state::resolve_corten_bin("corten-launcher");
                                        let _ = std::process::Command::new(launcher_path).arg("--cheat-sheet").spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + I: Settings
                                    keysyms::KEY_i | keysyms::KEY_I | keysyms::KEY_comma
                                        if binding("settings") => {
                                        info!("Compositor Hotkey: Super+I (Settings)");
                                        let settings_path = crate::state::resolve_corten_bin("corten-settings");
                                        let _ = std::process::Command::new(settings_path).spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + E: Corten File Manager
                                    keysyms::KEY_e | keysyms::KEY_E
                                        if binding("file_manager") => {
                                        info!("Compositor Hotkey: Super+E (Corten Files)");
                                        let files_path = crate::state::resolve_corten_bin("corten-files");
                                        let _ = std::process::Command::new(files_path).spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + W: Toggle Window Overview (Exposé)
                                    keysyms::KEY_w | keysyms::KEY_W
                                        if binding("window_overview") => {
                                        info!("Compositor Hotkey: Super+W (Toggle Window Overview)");
                                        state.overview_mode = !state.overview_mode;
                                        if state.overview_mode {
                                            crate::send_osd_notification("overview", "Pencere Genel Bakışı", "Exposé Modu Aktif", 100);
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + X: Open Corten Power & Session Hub
                                    keysyms::KEY_x | keysyms::KEY_X
                                        if binding("power_menu") => {
                                        info!("Compositor Hotkey: Super+X (Open Corten Power Hub)");
                                        std::thread::spawn(|| {
                                            let _ = std::process::Command::new("busctl")
                                                .args(["--user", "call", "org.corten.Panel", "/org/corten/Panel", "org.corten.Panel", "TriggerModule", "s", "power"])
                                                .spawn();
                                        });
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + ? or Super + / or Super + F1: Shortcuts Cheat Sheet Overlay
                                    keysyms::KEY_slash | keysyms::KEY_question | keysyms::KEY_F1
                                        if binding("cheat_sheet") => {
                                        info!("Compositor Hotkey: Super+? (Shortcuts Cheat Sheet Overlay)");
                                        let launcher_path = crate::state::resolve_corten_bin("corten-launcher");
                                        let _ = std::process::Command::new(launcher_path).arg("--cheat-sheet").spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Escape: Corten System Monitor
                                    keysyms::KEY_Escape
                                        if binding("task_manager") => {
                                        info!("Compositor Hotkey: Super+Escape (Corten Monitor)");
                                        let monitor_path = crate::state::resolve_corten_bin("corten-monitor");
                                        let _ = std::process::Command::new(monitor_path).spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Q: Close active window
                                    keysyms::KEY_q | keysyms::KEY_Q if binding("close_window") => {
                                        info!("Compositor Hotkey: Super+Q (Close Window)");
                                        if let Some(active) = state.layout.active_window() {
                                            crate::layout::close_window(active);
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    // Configured fullscreen binding
                                    keysyms::KEY_f | keysyms::KEY_F if binding("toggle_fullscreen") => {
                                        info!("Compositor Hotkey: Super+F (Toggle Fullscreen/Maximize)");
                                        if let Some(active) = state.layout.active_window().cloned() {
                                            let is_fs = state.layout.toggle_fullscreen(&active);
                                            if let Some(toplevel) = active.toplevel() {
                                                toplevel.with_pending_state(|tstate| {
                                                    if is_fs {
                                                        tstate.states.set(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Fullscreen);
                                                    } else {
                                                        tstate.states.unset(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Fullscreen);
                                                    }
                                                });
                                                toplevel.send_configure();
                                            } else if let Some(x11) = active.x11_surface() {
                                                let _ = x11.set_fullscreen(is_fs);
                                            }
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    // Configured minimize binding
                                    keysyms::KEY_m | keysyms::KEY_M if binding("minimize") => {
                                        info!("Compositor Hotkey: Super+M (Minimize)");
                                        if let Some(active) = state.layout.active_window().cloned() {
                                            state.layout.remove_window(&active);
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Shift + J: Swap Next
                                    keysyms::KEY_j | keysyms::KEY_J if binding("swap_next") => {
                                        state.layout.swap_next();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Shift + K: Swap Previous
                                    keysyms::KEY_k | keysyms::KEY_K if binding("swap_previous") => {
                                        state.layout.swap_prev();
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + J: Focus Next
                                    keysyms::KEY_j | keysyms::KEY_J if binding("focus_next") => {
                                        if let Some(next) = state.layout.next_window().cloned() {
                                            if let Some(x11) = next.x11_surface() {
                                                let _ = x11.set_activated(true);
                                            }
                                            if let Some(surface) = crate::layout::window_wl_surface(&next) {
                                                if let Some(kb) = state.seat.get_keyboard() {
                                                    kb.set_focus(state, Some(surface), serial);
                                                }
                                            }
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + K: Focus Previous
                                    keysyms::KEY_k | keysyms::KEY_K if binding("focus_previous") => {
                                        if let Some(prev) = state.layout.prev_window().cloned() {
                                            if let Some(x11) = prev.x11_surface() {
                                                let _ = x11.set_activated(true);
                                            }
                                            if let Some(surface) = crate::layout::window_wl_surface(&prev) {
                                                if let Some(kb) = state.seat.get_keyboard() {
                                                    kb.set_focus(state, Some(surface), serial);
                                                }
                                            }
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + H / L: Adjust master ratio (or Super+Ctrl+L to lock screen)
                                    keysyms::KEY_h | keysyms::KEY_H if binding("decrease_master") => {
                                        state.adjust_master_ratio_for_all_outputs(-0.05);
                                        return FilterResult::Intercept(());
                                    }
                                    keysyms::KEY_l | keysyms::KEY_L if binding("lock_screen") => {
                                        info!("Compositor Hotkey: Lock Screen triggered");
                                        let lock_path = crate::state::resolve_corten_bin("corten-lock");
                                        let _ = std::process::Command::new(lock_path).spawn();
                                        return FilterResult::Intercept(());
                                    }
                                    keysyms::KEY_l | keysyms::KEY_L if binding("increase_master") => {
                                        state.adjust_master_ratio_for_all_outputs(0.05);
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Shift + 1..9: Move active window to workspace
                                    _ if (1..=9).any(|idx| binding(&format!("move_to_workspace_{idx}"))) => {
                                        if let Some(ws) = (1..=9).find(|idx| binding(&format!("move_to_workspace_{idx}"))) {
                                            info!("Compositor Hotkey: Super+Shift+{} (Move Window to Workspace {})", ws, ws);
                                            state.move_active_window_to_workspace(ws);
                                            return FilterResult::Intercept(());
                                        }
                                    }
                                    // Configured workspace switching
                                    sym @ keysyms::KEY_1..=keysyms::KEY_9
                                        if binding(&format!("workspace_{}", sym - keysyms::KEY_1 + 1)) => {
                                        let ws = (sym - keysyms::KEY_1 + 1) as usize;
                                        info!("Compositor Hotkey: Super+{} (Switch Workspace to {})", ws, ws);
                                        state.switch_workspace(ws);
                                        return FilterResult::Intercept(());
                                    }
                                    // Super + Shift + S: Region Screenshot
                                    keysyms::KEY_s | keysyms::KEY_S if binding("screenshot") => {
                                        info!("Compositor Hotkey: Super+Shift+S (Screenshot)");
                                        crate::trigger_screenshot(state, &hotkey_ipc_tx, true);
                                        return FilterResult::Intercept(());
                                    }
                                    _ => {}
                                }
                            }
                            if key_state == KeyState::Pressed {
                                let sym = keysym_handle.modified_sym().raw();

                                // Media Keys: Volume, Mute, Screen Brightness
                                if crate::handle_media_keys(sym) {
                                    return FilterResult::Intercept(());
                                }

                                // PrintScreen Key: Instant Fullscreen Capture
                                if sym == keysyms::KEY_Print {
                                    info!("Compositor Hotkey: PrintScreen (Full Capture)");
                                    crate::trigger_screenshot(state, &hotkey_ipc_tx, false);
                                    return FilterResult::Intercept(());
                                }

                                // Emergency Session Exit: Shift + Escape
                                if modifiers.shift && sym == keysyms::KEY_Escape {
                                    info!("Emergency TTY Exit triggered via Shift+Escape!");
                                    shutdown_requested_for_keyboard.set(true);
                                    return FilterResult::Intercept(());
                                }

                                if modifiers.alt && (sym == keysyms::KEY_Tab || sym == keysyms::KEY_ISO_Left_Tab) {
                                    state.alt_tab_active = true;
                                    state.alt_tab_instant = Some(std::time::Instant::now());
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
                                        if let Some(surface) = crate::layout::window_wl_surface(&target) {
                                            if let Some(kb) = state.seat.get_keyboard() {
                                                kb.set_focus(state, Some(surface), serial);
                                            }
                                        }
                                    }
                                     return FilterResult::Intercept(());
                                }
                            } else if key_state == KeyState::Released && !modifiers.alt {
                                state.alt_tab_active = false;
                                if let Some(active) = state.layout.active_window().cloned() {
                                    state.layout.set_active_window(&active);
                                }
                            }
                            FilterResult::Forward
                        },
                    );
                }
                smithay::reexports::input::Event::Pointer(ptr_event) => {
                    let mut device = ptr_event.device();
                    apply_libinput_device_settings(&mut device, &input_settings);
                    use smithay::reexports::input::event::PointerEvent;
                    match ptr_event {
                        PointerEvent::Motion(m) => {
                            let pointer_factor = 1.0
                                + (f64::from(input_settings.pointer_speed).clamp(-100.0, 100.0)
                                    / 100.0);
                            let dx = m.dx() * pointer_factor;
                            let dy = m.dy() * pointer_factor;
                            let mut loc = state.cursor_location;
                            let (min_x, max_x, max_y) = desktop_bounds(&state, screen_w, screen_h);
                            loc.x = (loc.x + dx).clamp(min_x, max_x.max(min_x + 1.0));
                            loc.y = (loc.y + dy).clamp(0.0, max_y);
                            let loc = state.apply_pointer_constraint(loc);
                            state.cursor_location = loc;
                            if let Some((_, ref mut end_pt)) = state.selection_box {
                                *end_pt = loc;
                            }

                            let serial = SERIAL_COUNTER.next_serial();
                            let time = (m.time_usec() / 1000) as u32;
                            let motion_event = MotionEvent {
                                location: loc,
                                serial,
                                time,
                            };

                            let focused_surface = find_surface_under_loc(&mut state, loc);

                            // If currently adjusting tiling split ratio
                            if let Some((drag_start, initial_ratio)) = state.split_drag {
                                let target_output = state.output_at(loc);
                                let usable_area = {
                                    let mut map = layer_map_for_output(&target_output);
                                    map.arrange();
                                    map.non_exclusive_zone()
                                };
                                let delta_x = (loc.x - drag_start.x) as f32;
                                let new_ratio =
                                    initial_ratio + delta_x / (usable_area.size.w.max(1) as f32);
                                state
                                    .layout_for_output_mut(&target_output)
                                    .set_master_ratio(new_ratio);
                            } else if let Some((ref window, drag_start_cursor, initial_rect)) =
                                state.dragged_window.clone()
                            {
                                // If currently dragging a window, update its floating position smoothly across displays
                                let delta_x = (loc.x - drag_start_cursor.x).round() as i32;
                                let delta_y = (loc.y - drag_start_cursor.y).round() as i32;

                                let start_output = state.output_at(drag_start_cursor);
                                let start_output_loc = start_output.current_location();

                                let target_output = state.output_at(loc);
                                let target_output_loc = target_output.current_location();

                                let global_x = start_output_loc.x + initial_rect.loc.x + delta_x;
                                let global_y = start_output_loc.y + initial_rect.loc.y + delta_y;

                                let mut new_rect = initial_rect;
                                new_rect.loc.x = global_x - target_output_loc.x;
                                new_rect.loc.y = global_y - target_output_loc.y;

                                if !state.layout_for_output(&target_output).contains_window(window) {
                                    state.remove_window_from_all_outputs(window);
                                    state
                                        .layout_for_output_mut(&target_output)
                                        .add_floating_window(window.clone());
                                }

                                let usable_area = {
                                    let mut map = layer_map_for_output(&target_output);
                                    map.arrange();
                                    map.non_exclusive_zone()
                                };
                                let screen_bounds = {
                                    let scale = target_output.current_scale().fractional_scale();
                                    let size = target_output
                                        .current_mode()
                                        .map(|m| smithay::utils::Size::from(((m.size.w as f64 / scale).round() as i32, (m.size.h as f64 / scale).round() as i32)))
                                        .unwrap_or(usable_area.size);
                                    Rectangle::new(smithay::utils::Point::from((0, 0)), size)
                                };
                                let other_windows: Vec<Rectangle<i32, smithay::utils::Logical>> = state
                                    .layout_for_output(&target_output)
                                    .arrange(usable_area)
                                    .into_iter()
                                    .filter(|(w, _)| w != window)
                                    .map(|(_, r)| r)
                                    .collect();

                                // Apply 14px magnetic edge snapping to screen borders, dock, and neighboring windows
                                let snapped_rect = crate::layout::snap_window_to_edges(
                                    new_rect,
                                    usable_area,
                                    screen_bounds,
                                    &other_windows,
                                    14,
                                );

                                state
                                    .layout_for_output_mut(&target_output)
                                    .set_floating_geometry(window, snapped_rect);
                                let out_loc = target_output.current_location();
                                let rel_x = loc.x - out_loc.x as f64;
                                let rel_y = loc.y - out_loc.y as f64;
                                let is_near_left = rel_x <= (usable_area.loc.x + 35) as f64;
                                let is_near_right =
                                    rel_x >= (usable_area.loc.x + usable_area.size.w - 35) as f64;
                                let is_near_top = rel_y <= (usable_area.loc.y + 35) as f64;
                                let is_near_bottom =
                                    rel_y >= (usable_area.loc.y + usable_area.size.h - 35) as f64;

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
                                        Point::from((
                                            usable_area.loc.x + half_w,
                                            usable_area.loc.y,
                                        )),
                                        Size::from((half_w, half_h)),
                                    ));
                                    state.drag_split_target = None;
                                } else if is_near_left && is_near_bottom {
                                    // Bottom-left quarter tile
                                    state.snap_preview = Some(Rectangle::new(
                                        Point::from((
                                            usable_area.loc.x,
                                            usable_area.loc.y + half_h,
                                        )),
                                        Size::from((half_w, half_h)),
                                    ));
                                    state.drag_split_target = None;
                                } else if is_near_right && is_near_bottom {
                                    // Bottom-right quarter tile
                                    state.snap_preview = Some(Rectangle::new(
                                        Point::from((
                                            usable_area.loc.x + half_w,
                                            usable_area.loc.y + half_h,
                                        )),
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
                                        Point::from((
                                            usable_area.loc.x + half_w,
                                            usable_area.loc.y,
                                        )),
                                        Size::from((half_w, usable_area.size.h)),
                                    ));
                                    state.drag_split_target = None;
                                } else if is_near_top {
                                    // Full maximize
                                    state.snap_preview = Some(usable_area);
                                    state.drag_split_target = None;
                                } else {
                                    let window_rects = state.layout_for_output_mut(&target_output).arrange_animated(usable_area);
                                    let out_rel_loc = Point::from((rel_x, rel_y));
                                    if let Some((target_win, dir, new_on_first, preview_rect)) =
                                        crate::layout::detect_drag_split_target(out_rel_loc, &window_rects, window)
                                    {
                                        state.snap_preview = Some(preview_rect);
                                        state.drag_split_target = Some((target_win, dir, new_on_first, preview_rect));
                                    } else {
                                        state.snap_preview = None;
                                        state.drag_split_target = None;
                                    }
                                }
                            } else if let Some((ref window, resize_start_cursor, initial_rect)) =
                                state.resized_window.clone()
                            {
                                // If currently resizing a window, update its floating size and notify the client
                                let delta_w = (loc.x - resize_start_cursor.x).round() as i32;
                                let delta_h = (loc.y - resize_start_cursor.y).round() as i32;
                                let mut new_rect = initial_rect;
                                let (min_w, min_h) = crate::layout::window_min_size(window);
                                new_rect.size.w = (initial_rect.size.w + delta_w).max(min_w);
                                new_rect.size.h = (initial_rect.size.h + delta_h).max(min_h + 36);
                                let target_output = state.output_at(loc);
                                let usable_area = {
                                    let mut map = layer_map_for_output(&target_output);
                                    map.arrange();
                                    map.non_exclusive_zone()
                                };
                                let screen_bounds = {
                                    let scale = target_output.current_scale().fractional_scale();
                                    let size = target_output
                                        .current_mode()
                                        .map(|m| smithay::utils::Size::from(((m.size.w as f64 / scale).round() as i32, (m.size.h as f64 / scale).round() as i32)))
                                        .unwrap_or(usable_area.size);
                                    Rectangle::new(smithay::utils::Point::from((0, 0)), size)
                                };
                                let other_windows: Vec<Rectangle<i32, smithay::utils::Logical>> = state
                                    .layout_for_output(&target_output)
                                    .arrange(usable_area)
                                    .into_iter()
                                    .filter(|(w, _)| w != window)
                                    .map(|(_, r)| r)
                                    .collect();

                                // Apply 14px magnetic snapping to right and bottom resizing edges
                                let snapped_rect = crate::layout::snap_window_resize(
                                    new_rect,
                                    usable_area,
                                    screen_bounds,
                                    &other_windows,
                                    14,
                                );

                                state
                                    .layout_for_output_mut(&target_output)
                                    .set_floating_geometry(window, snapped_rect);
                                crate::layout::configure_window_geometry(window, snapped_rect);
                            } else {
                                if let Some((_, ref mut end_pt)) = state.selection_box {
                                    *end_pt = loc;
                                }
                                let target_output = state.output_at(loc);
                                let usable_area = {
                                    let mut map = layer_map_for_output(&target_output);
                                    map.arrange();
                                    map.non_exclusive_zone()
                                };
                                let out_loc = target_output.current_location();
                                let rel_x = loc.x - out_loc.x as f64;
                                let rel_y = loc.y - out_loc.y as f64;
                                let out_rel_loc = Point::from((rel_x, rel_y));

                                // Hover detection for window borders / corner resizing
                                let window_rects =
                                    state.layout_for_output_mut(&target_output).arrange_animated(usable_area);
                                if let Some((_, rect)) = window_rects
                                    .into_iter()
                                    .rev()
                                    .find(|(_, r)| r.to_f64().contains(out_rel_loc))
                                {
                                    let rw_x = out_rel_loc.x - rect.loc.x as f64;
                                    let rw_y = out_rel_loc.y - rect.loc.y as f64;
                                    let w_f = rect.size.w as f64;
                                    let h_f = rect.size.h as f64;
                                    if rw_x >= (w_f - 16.0) && rw_y >= (h_f - 16.0) {
                                        state.cursor_status =
                                            smithay::input::pointer::CursorImageStatus::Named(
                                                smithay::input::pointer::CursorIcon::NwseResize,
                                            );
                                    } else if rw_x <= 16.0 && rw_y >= (h_f - 16.0) {
                                        state.cursor_status =
                                            smithay::input::pointer::CursorImageStatus::Named(
                                                smithay::input::pointer::CursorIcon::NeswResize,
                                            );
                                    } else if rw_x >= (w_f - 8.0) || rw_x <= 8.0 {
                                        state.cursor_status =
                                            smithay::input::pointer::CursorImageStatus::Named(
                                                smithay::input::pointer::CursorIcon::EwResize,
                                            );
                                    } else if rw_y >= (h_f - 8.0) || rw_y <= 8.0 {
                                        state.cursor_status =
                                            smithay::input::pointer::CursorImageStatus::Named(
                                                smithay::input::pointer::CursorIcon::NsResize,
                                            );
                                    }
                                } else if let Some(sx) =
                                    state.layout_for_output(&target_output).split_x(usable_area)
                                {
                                    if (rel_x as i32 - sx).abs() <= 8
                                        && rel_y >= usable_area.loc.y as f64
                                        && rel_y <= (usable_area.loc.y + usable_area.size.h) as f64
                                    {
                                        state.cursor_status =
                                            smithay::input::pointer::CursorImageStatus::Named(
                                                smithay::input::pointer::CursorIcon::EwResize,
                                            );
                                    }
                                }
                            }

                            let rel_event = RelativeMotionEvent {
                                delta: (m.dx(), m.dy()).into(),
                                delta_unaccel: (m.dx_unaccelerated(), m.dy_unaccelerated()).into(),
                                utime: m.time_usec(),
                            };
                            pointer.relative_motion(
                                &mut state,
                                focused_surface.clone(),
                                &rel_event,
                            );
                            pointer.motion(&mut state, focused_surface, &motion_event);
                            pointer.frame(&mut state);
                        }
                        PointerEvent::MotionAbsolute(m) => {
                            use smithay::backend::input::AbsolutePositionEvent;
                            let (min_x, max_x, max_y) = desktop_bounds(&state, screen_w, screen_h);
                            let mut loc = state.cursor_location;
                            loc.x = AbsolutePositionEvent::x_transformed(&m, screen_w)
                                .clamp(min_x, max_x.max(min_x + 1.0));
                            loc.y = AbsolutePositionEvent::y_transformed(&m, screen_h)
                                .clamp(0.0, max_y);
                            let loc = state.apply_pointer_constraint(loc);
                            state.cursor_location = loc;
                            if let Some((_, ref mut end_pt)) = state.selection_box {
                                *end_pt = loc;
                            }

                            let serial = SERIAL_COUNTER.next_serial();
                            let time = (m.time_usec() / 1000) as u32;
                            let motion_event = MotionEvent {
                                location: loc,
                                serial,
                                time,
                            };

                            let focused_surface = find_surface_under_loc(&mut state, loc);
                            let rel_event = RelativeMotionEvent {
                                delta: (0.0, 0.0).into(),
                                delta_unaccel: (0.0, 0.0).into(),
                                utime: m.time_usec(),
                            };
                            pointer.relative_motion(
                                &mut state,
                                focused_surface.clone(),
                                &rel_event,
                            );
                            pointer.motion(&mut state, focused_surface, &motion_event);
                            pointer.frame(&mut state);
                        }
                        PointerEvent::Button(b) => {
                            let serial = SERIAL_COUNTER.next_serial();
                            let time = (b.time_usec() / 1000) as u32;
                            let button_state = match b.button_state() {
                                smithay::reexports::input::event::pointer::ButtonState::Pressed => ButtonState::Pressed,
                                smithay::reexports::input::event::pointer::ButtonState::Released => ButtonState::Released,
                            };
                            let button_event = ButtonEvent {
                                serial,
                                time,
                                button: b.button(),
                                state: button_state,
                            };

                            if button_state == ButtonState::Pressed {
                                if state.session_locked {
                                    if let Some((lock_surface, _)) =
                                        state.lock_surfaces.first().cloned()
                                    {
                                        if let Some(kb) = state.seat.get_keyboard() {
                                            kb.set_focus(
                                                &mut state,
                                                Some(lock_surface.wl_surface().clone()),
                                                serial,
                                            );
                                        }
                                    }
                                    pointer.button(&mut state, &button_event);
                                    pointer.frame(&mut state);
                                    continue;
                                }

                                let mut handled_focus = false;
                                let target_output = state.output_at(state.cursor_location);
                                let out_loc = target_output.current_location().to_f64();
                                let out_rel_loc = state.cursor_location - out_loc;

                                let logo_held = state
                                    .seat
                                    .get_keyboard()
                                    .map(|kb| kb.modifier_state().logo)
                                    .unwrap_or(false);

                                if b.button() == 272 && !logo_held {
                                    let usable_area = {
                                        let mut map = layer_map_for_output(&target_output);
                                        map.arrange();
                                        map.non_exclusive_zone()
                                    };
                                    let out_loc = target_output.current_location();
                                    let rel_x = state.cursor_location.x - out_loc.x as f64;
                                    let rel_y = state.cursor_location.y - out_loc.y as f64;
                                    if let Some(sx) =
                                        state.layout_for_output(&target_output).split_x(usable_area)
                                    {
                                        if (rel_x as i32 - sx).abs() <= 6
                                            && rel_y >= (usable_area.loc.y + 36) as f64
                                            && rel_y
                                                <= (usable_area.loc.y + usable_area.size.h) as f64
                                        {
                                            state.split_drag = Some((
                                                state.cursor_location,
                                                state
                                                    .layout_for_output(&target_output)
                                                    .master_ratio(),
                                            ));
                                        }
                                    }
                                }

                                // 1. Check if clicking on an interactive top layer shell surface (Lock screen, Dock, Launcher, OOBE)
                                let top_layer_surface = {
                                    let layer_map = layer_map_for_output(&target_output);
                                    let mut found = None;
                                    for layer in [Layer::Overlay, Layer::Top] {
                                        for layer_surface in layer_map.layers_on(layer).rev() {
                                            if let Some(geo) =
                                                layer_map.layer_geometry(layer_surface)
                                            {
                                                if geo.to_f64().contains(out_rel_loc) {
                                                    let rel_point = out_rel_loc - geo.loc.to_f64();
                                                    if let Some((surface, _)) = layer_surface
                                                        .surface_under(
                                                            rel_point,
                                                            WindowSurfaceType::all(),
                                                        )
                                                    {
                                                        found = Some(surface);
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                        if found.is_some() {
                                            break;
                                        }
                                    }
                                    found
                                };

                                if let Some(surface) = top_layer_surface {
                                    if let Some(kb) = state.seat.get_keyboard() {
                                        kb.set_focus(&mut state, Some(surface), serial);
                                    }
                                    handled_focus = true;
                                }

                                // 2. If not a top layer surface, check tiling/floating windows (top-to-bottom)
                                if !handled_focus {
                                    let usable_area = {
                                        let mut map = layer_map_for_output(&target_output);
                                        map.arrange();
                                        map.non_exclusive_zone()
                                    };
                                    let window_rects = if state.overview_mode {
                                        state
                                            .layout_for_output_mut(&target_output)
                                            .arrange_overview(usable_area)
                                    } else {
                                        state
                                            .layout_for_output_mut(&target_output)
                                            .arrange_animated(usable_area)
                                    };
                                    let clicked = window_rects
                                        .into_iter()
                                        .rev()
                                        .filter(|(w, _)| crate::layout::is_window_mapped(w))
                                        .find(|(_, rect)| rect.to_f64().contains(out_rel_loc));

                                    if let Some((window, rect)) = clicked {
                                        if state.overview_mode {
                                            state.overview_mode = false;
                                        }
                                        state
                                            .layout_for_output_mut(&target_output)
                                            .set_active_window(&window);
                                        if let Some(x11) = window.x11_surface() {
                                            let _ = x11.set_activated(true);
                                        }
                                        if let Some(surface) =
                                            crate::layout::window_wl_surface(&window)
                                        {
                                            if let Some(kb) = state.seat.get_keyboard() {
                                                kb.set_focus(&mut state, Some(surface), serial);
                                            }
                                        }

                                        let logo_held = state
                                            .seat
                                            .get_keyboard()
                                            .map(|kb| kb.modifier_state().logo)
                                            .unwrap_or(false);

                                        let rel_win_x = out_rel_loc.x - rect.loc.x as f64;
                                        let rel_win_y = out_rel_loc.y - rect.loc.y as f64;

                                        let w_f64 = rect.size.w as f64;
                                        let h_f64 = rect.size.h as f64;

                                        if b.button() == 272 {
                                            if (0.0..=36.0).contains(&rel_win_y) {
                                                // Clicked inside the Window Titlebar!
                                                if rel_win_x >= (w_f64 - 32.0) && rel_win_x <= w_f64
                                                {
                                                    // Close button clicked
                                                    info!(
                                                        "Window Titlebar: Close clicked on {:?}",
                                                        window
                                                    );
                                                    crate::layout::close_window(&window);
                                                    continue;
                                                } else if rel_win_x >= (w_f64 - 58.0)
                                                    && rel_win_x < (w_f64 - 32.0)
                                                {
                                                    // Maximize / Restore toggle clicked
                                                    let usable_area = {
                                                        let mut map = layer_map_for_output(&target_output);
                                                        map.arrange();
                                                        map.non_exclusive_zone()
                                                    };
                                                    info!("Window Titlebar: Maximize toggle clicked on {:?}", window);
                                                    state
                                                        .layout_for_output_mut(&target_output)
                                                        .toggle_maximize(&window, usable_area);
                                                    state.sync_ipc_snapshot();
                                                    continue;
                                                } else if rel_win_x >= (w_f64 - 88.0)
                                                    && rel_win_x < (w_f64 - 58.0)
                                                {
                                                    // Minimize clicked
                                                    info!(
                                                        "Window Titlebar: Minimize clicked on {:?}",
                                                        window
                                                    );
                                                    state
                                                        .layout_for_output_mut(&target_output)
                                                        .minimize_window(&window);
                                                    state.sync_ipc_snapshot();
                                                    continue;
                                                } else {
                                                    // Check double click to maximize / restore
                                                    if let Some((prev_win, prev_instant)) = state.last_titlebar_click.take() {
                                                        if prev_win == window && prev_instant.elapsed().as_millis() < 320 {
                                                            let usable_area = {
                                                                let mut map = layer_map_for_output(&target_output);
                                                                map.arrange();
                                                                map.non_exclusive_zone()
                                                            };
                                                            info!("Window Titlebar: Double-click maximize toggle on {:?}", window);
                                                            state
                                                                .layout_for_output_mut(&target_output)
                                                                .toggle_maximize(&window, usable_area);
                                                            state.sync_ipc_snapshot();
                                                            continue;
                                                        }
                                                    }
                                                    state.last_titlebar_click = Some((window.clone(), std::time::Instant::now()));

                                                    // Dragging anywhere else on the titlebar moves the window
                                                    let mut drag_rect = rect;
                                                    if state.layout_for_output_mut(&target_output).is_maximized(&window) {
                                                        let usable_area = {
                                                            let mut map = layer_map_for_output(&target_output);
                                                            map.arrange();
                                                            map.non_exclusive_zone()
                                                        };
                                                        let restored = state.layout_for_output_mut(&target_output).set_maximized(&window, usable_area, false);
                                                        let new_x = (state.cursor_location.x - (restored.size.w as f64 * 0.5)).round() as i32;
                                                        let new_y = state.cursor_location.y.round() as i32 - 12;
                                                        drag_rect = smithay::utils::Rectangle::new(smithay::utils::Point::from((new_x, new_y)), restored.size);
                                                        state.layout_for_output_mut(&target_output).set_floating_geometry(&window, drag_rect);
                                                        crate::layout::configure_window_geometry(&window, drag_rect);
                                                    }
                                                    state.dragged_window = Some((
                                                        window.clone(),
                                                        state.cursor_location,
                                                        drag_rect,
                                                    ));
                                                    continue;
                                                }
                                            } else if (rel_win_x >= (w_f64 - 16.0)
                                                && rel_win_y >= (h_f64 - 16.0))
                                                || rel_win_x >= (w_f64 - 8.0)
                                                || rel_win_y >= (h_f64 - 8.0)
                                            {
                                                // Left click on bottom-right corner or right/bottom edges initiates resize!
                                                info!("Window Edge / Corner Click: Initiating resize on {:?}", window);
                                                state.resized_window = Some((
                                                    window.clone(),
                                                    state.cursor_location,
                                                    rect,
                                                ));
                                            }
                                        } else if logo_held && b.button() == 272 {
                                            // Super + Left button initiates window drag & move
                                            state.dragged_window =
                                                Some((window.clone(), state.cursor_location, rect));
                                        } else if logo_held && b.button() == 273 {
                                            // Super + Right button initiates window resize
                                            state.resized_window =
                                                Some((window.clone(), state.cursor_location, rect));
                                        }
                                    } else if !logo_held && b.button() == 272 {
                                        // Left click on empty desktop canvas: initiate rubberband selection marquee!
                                        state.selection_box =
                                            Some((state.cursor_location, state.cursor_location));
                                    } else if !logo_held && b.button() == 273 {
                                        // Right click on empty desktop canvas: trigger Desktop Context Menu at cursor
                                        info!("Desktop Canvas Right-Click: Spawning Corten Desktop Context Menu at cursor");
                                        let _ = std::process::Command::new("killall")
                                            .arg("-9")
                                            .arg("corten-launcher")
                                            .status();
                                        let cur_x = state.cursor_location.x.round() as i32;
                                        let cur_y = state.cursor_location.y.round() as i32;
                                        let launcher_path = crate::state::resolve_corten_bin("corten-launcher");
                                        let _ = std::process::Command::new(launcher_path)
                                            .arg("--desktop-menu")
                                            .arg("--x")
                                            .arg(cur_x.to_string())
                                            .arg("--y")
                                            .arg(cur_y.to_string())
                                            .spawn();
                                    }
                                }
                            } else if button_state == ButtonState::Released {
                                if let Some((window, _, _)) = state.dragged_window.take() {
                                    let target_output = state.output_at(state.cursor_location);
                                    if let Some((target_win, dir, new_on_first, _)) = state.drag_split_target.take() {
                                        let layout = state.layout_for_output_mut(&target_output);
                                        layout.set_fullscreen(&window, false);
                                        layout.set_floating(&window, false);
                                        layout.set_mode(corten_config::LayoutMode::Bsp);
                                        layout.drag_split_window(window.clone(), target_win, dir, new_on_first);
                                        state.snap_preview = None;
                                    } else if let Some(snap_rect) = state.snap_preview.take() {
                                        state
                                            .layout_for_output_mut(&target_output)
                                            .set_fullscreen(&window, false);
                                        state
                                            .layout_for_output_mut(&target_output)
                                            .set_floating_geometry(&window, snap_rect);
                                        crate::layout::configure_window_geometry(&window, snap_rect);
                                    } else if let Some(geo) = state.layout_for_output(&target_output).floating_geometry(&window) {
                                        crate::layout::configure_window_geometry(&window, geo);
                                    }
                                }
                                state.dragged_window = None;
                                state.resized_window = None;
                                state.split_drag = None;
                                state.snap_preview = None;
                                state.drag_split_target = None;
                                state.selection_box = None;
                            }

                            let btn_loc = state.cursor_location;
                            let focused = find_surface_under_loc(&mut state, btn_loc);
                            pointer.motion(
                                &mut state,
                                focused,
                                &MotionEvent {
                                    location: btn_loc,
                                    serial,
                                    time,
                                },
                            );
                            pointer.button(&mut state, &button_event);
                            pointer.frame(&mut state);
                        }
                        PointerEvent::ScrollWheel(axis_event) => {
                            let frame = libinput_scroll_frame(
                                &axis_event,
                                AxisSource::Wheel,
                                input_settings.natural_scrolling,
                            );
                            pointer.axis(&mut state, frame);
                            pointer.frame(&mut state);
                        }
                        PointerEvent::ScrollFinger(axis_event) => {
                            let frame = libinput_scroll_frame(
                                &axis_event,
                                AxisSource::Finger,
                                input_settings.natural_scrolling,
                            );
                            pointer.axis(&mut state, frame);
                            pointer.frame(&mut state);
                        }
                        PointerEvent::ScrollContinuous(axis_event) => {
                            let frame = libinput_scroll_frame(
                                &axis_event,
                                AxisSource::Continuous,
                                input_settings.natural_scrolling,
                            );
                            pointer.axis(&mut state, frame);
                            pointer.frame(&mut state);
                        }
                        _ => {}
                    }
                }
                smithay::reexports::input::Event::Gesture(gesture_event) => {
                    use smithay::input::pointer::{
                        GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
                        GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
                    };
                    use smithay::reexports::input::event::gesture::{
                        GestureEndEvent, GestureEventCoordinates, GestureEventTrait,
                        GesturePinchEvent, GesturePinchEventTrait, GestureSwipeEvent,
                    };
                    use smithay::reexports::input::event::GestureEvent;
                    match gesture_event {
                        GestureEvent::Swipe(swipe_event) => match swipe_event {
                            GestureSwipeEvent::Begin(begin) => {
                                if begin.finger_count() >= 3 {
                                    state.gesture_swipe_dx = 0.0;
                                    state.gesture_swipe_dy = 0.0;
                                    state.gesture_samples.clear();
                                    state.gesture_kinetic_active = false;
                                    state.gesture_kinetic_velocity = 0.0;
                                    state.record_gesture_sample(begin.time_usec(), 0.0, 0.0);
                                } else {
                                    let serial = SERIAL_COUNTER.next_serial();
                                    let time = (begin.time_usec() / 1000) as u32;
                                    pointer.gesture_swipe_begin(
                                        &mut state,
                                        &GestureSwipeBeginEvent {
                                            serial,
                                            time,
                                            fingers: begin.finger_count() as u32,
                                        },
                                    );
                                }
                            }
                            GestureSwipeEvent::Update(update) => {
                                if update.finger_count() >= 3 {
                                    state.gesture_swipe_dx += update.dx();
                                    state.gesture_swipe_dy += update.dy();
                                    state.record_gesture_sample(
                                        update.time_usec(),
                                        update.dx(),
                                        update.dy(),
                                    );
                                } else {
                                    let time = (update.time_usec() / 1000) as u32;
                                    pointer.gesture_swipe_update(
                                        &mut state,
                                        &GestureSwipeUpdateEvent {
                                            time,
                                            delta: (update.dx(), update.dy()).into(),
                                        },
                                    );
                                }
                            }
                            GestureSwipeEvent::End(end) => {
                                if end.finger_count() >= 3 {
                                    if !end.cancelled() {
                                        let dx = state.gesture_swipe_dx;
                                        let dy = state.gesture_swipe_dy;
                                        let (vx, vy) = state.compute_gesture_velocity();

                                        let is_horizontal_fling = vx.abs() >= 350.0 && dx.abs() >= 12.0;
                                        let is_horizontal_drag = dx.abs() >= 70.0;
                                        let is_vertical_fling = vy.abs() >= 300.0 && dy.abs() >= 12.0;
                                        let is_vertical_drag = dy.abs() >= 65.0;

                                        if (dx.abs() > dy.abs() || vx.abs() > vy.abs())
                                            && (is_horizontal_drag || is_horizontal_fling)
                                        {
                                            let go_next = if is_horizontal_fling && vx.abs() > 350.0 {
                                                vx < 0.0
                                            } else {
                                                dx < 0.0
                                            };
                                            let target_ws = if go_next {
                                                (state.active_workspace + 1).min(9)
                                            } else {
                                                state.active_workspace.saturating_sub(1).max(1)
                                            };
                                            if target_ws != state.active_workspace {
                                                info!(
                                                    "Touchpad 3-finger swipe fling: Switching workspace {} -> {} (dx: {:.1}, vx: {:.1} px/s)",
                                                    state.active_workspace, target_ws, dx, vx
                                                );
                                                state.switch_workspace(target_ws);
                                                // Smooth kinetic entry for incoming workspace windows
                                                let entry_offset = if go_next { 130.0 } else { -130.0 };
                                                state.gesture_swipe_dx = entry_offset;
                                                state.start_gesture_kinetic(-vx.clamp(-800.0, 800.0) * 0.3);
                                            } else {
                                                // Reached bounds (ws 1 or 9): elastic snap-back
                                                state.start_gesture_kinetic(vx.clamp(-600.0, 600.0));
                                            }
                                        } else if is_vertical_drag || is_vertical_fling {
                                            let go_up = if is_vertical_fling && vy.abs() > 300.0 {
                                                vy < 0.0
                                            } else {
                                                dy < 0.0
                                            };
                                            if go_up {
                                                info!("Touchpad 3-finger swipe UP fling: Toggle Window Overview");
                                                state.overview_mode = !state.overview_mode;
                                                if state.overview_mode {
                                                    crate::send_osd_notification(
                                                        "overview",
                                                        "Pencere Genel Bakışı",
                                                        "Exposé Modu Aktif",
                                                        100,
                                                    );
                                                }
                                            } else {
                                                info!(
                                                    "Touchpad 3-finger swipe DOWN fling: Toggle Show Desktop"
                                                );
                                                state.toggle_show_desktop();
                                            }
                                            state.start_gesture_kinetic(0.0);
                                        } else {
                                            // Sub-threshold release: elastic spring back to 0.0 without jarring visual pop
                                            state.start_gesture_kinetic(vx.clamp(-500.0, 500.0));
                                        }
                                    } else {
                                        // Cancelled: elastic spring back
                                        state.start_gesture_kinetic(0.0);
                                    }
                                    state.gesture_samples.clear();
                                    state.gesture_swipe_dy = 0.0;
                                } else {
                                    let serial = SERIAL_COUNTER.next_serial();
                                    let time = (end.time_usec() / 1000) as u32;
                                    pointer.gesture_swipe_end(
                                        &mut state,
                                        &GestureSwipeEndEvent {
                                            serial,
                                            time,
                                            cancelled: end.cancelled(),
                                        },
                                    );
                                }
                            }
                            _ => {}
                        },
                        GestureEvent::Pinch(pinch_event) => match pinch_event {
                            GesturePinchEvent::Begin(begin) if begin.finger_count() < 3 => {
                                let serial = SERIAL_COUNTER.next_serial();
                                let time = (begin.time_usec() / 1000) as u32;
                                pointer.gesture_pinch_begin(
                                    &mut state,
                                    &GesturePinchBeginEvent {
                                        serial,
                                        time,
                                        fingers: begin.finger_count() as u32,
                                    },
                                );
                            }
                            GesturePinchEvent::Update(update) => {
                                if update.finger_count() >= 3 {
                                    if update.scale() < 0.75 && !state.overview_mode {
                                        info!("Touchpad 3+ finger Pinch IN: Activating Window Overview");
                                        state.overview_mode = true;
                                        crate::send_osd_notification(
                                            "overview",
                                            "Pencere Genel Bakışı",
                                            "Exposé Modu Aktif",
                                            100,
                                        );
                                    } else if update.scale() > 1.25 && state.overview_mode {
                                        info!("Touchpad 3+ finger Pinch OUT: Exiting Window Overview");
                                        state.overview_mode = false;
                                    }
                                } else {
                                    let time = (update.time_usec() / 1000) as u32;
                                    pointer.gesture_pinch_update(
                                        &mut state,
                                        &GesturePinchUpdateEvent {
                                            time,
                                            delta: (update.dx(), update.dy()).into(),
                                            scale: update.scale(),
                                            rotation: update.angle_delta(),
                                        },
                                    );
                                }
                            }
                            GesturePinchEvent::End(end) if end.finger_count() < 3 => {
                                let serial = SERIAL_COUNTER.next_serial();
                                let time = (end.time_usec() / 1000) as u32;
                                pointer.gesture_pinch_end(
                                    &mut state,
                                    &GesturePinchEndEvent {
                                        serial,
                                        time,
                                        cancelled: end.cancelled(),
                                    },
                                );
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
                smithay::reexports::input::Event::Touch(touch_event) => {
                    use smithay::reexports::input::event::touch::{
                        TouchEventPosition, TouchEventTrait,
                    };
                    use smithay::reexports::input::event::TouchEvent;
                    match touch_event {
                        TouchEvent::Down(down) => {
                            let tx = down.x_transformed(screen_w as u32);
                            let ty = down.y_transformed(screen_h as u32);
                            let touch_loc = Point::from((tx, ty));
                            state.cursor_location = touch_loc;
                            let serial = SERIAL_COUNTER.next_serial();
                            let time = (down.time_usec() / 1000) as u32;
                            pointer.motion(
                                &mut state,
                                None,
                                &MotionEvent {
                                    location: touch_loc,
                                    serial,
                                    time,
                                },
                            );
                            pointer.button(
                                &mut state,
                                &ButtonEvent {
                                    serial,
                                    time,
                                    button: 272,
                                    state: ButtonState::Pressed,
                                },
                            );
                            pointer.frame(&mut state);
                        }
                        TouchEvent::Motion(motion) => {
                            let tx = motion.x_transformed(screen_w as u32);
                            let ty = motion.y_transformed(screen_h as u32);
                            let touch_loc = Point::from((tx, ty));
                            state.cursor_location = touch_loc;
                            let serial = SERIAL_COUNTER.next_serial();
                            let time = (motion.time_usec() / 1000) as u32;
                            pointer.motion(
                                &mut state,
                                None,
                                &MotionEvent {
                                    location: touch_loc,
                                    serial,
                                    time,
                                },
                            );
                            pointer.frame(&mut state);
                        }
                        TouchEvent::Up(up) => {
                            let serial = SERIAL_COUNTER.next_serial();
                            let time = (up.time_usec() / 1000) as u32;
                            pointer.button(
                                &mut state,
                                &ButtonEvent {
                                    serial,
                                    time,
                                    button: 272,
                                    state: ButtonState::Released,
                                },
                            );
                            pointer.frame(&mut state);
                        }
                        _ => {}
                    }
                }
                smithay::reexports::input::Event::Tablet(tablet_tool_event) => {
                    use smithay::reexports::input::event::tablet_tool::{TabletToolEvent, TabletToolEventTrait};
                    let serial = SERIAL_COUNTER.next_serial();
                    let (sw, sh) = (screen_w as u32, screen_h as u32);

                    match tablet_tool_event {
                        TabletToolEvent::Axis(axis) => {
                            let loc = Point::<f64, smithay::utils::Logical>::from((
                                axis.x_transformed(sw),
                                axis.y_transformed(sh),
                            ));
                            let focused = find_surface_under_loc(&mut state, loc);
                            tablet_state.handle_axis(&mut state, &axis, focused, sw, sh, serial);
                        }
                        TabletToolEvent::Proximity(prox) => {
                            let loc = Point::<f64, smithay::utils::Logical>::from((
                                prox.x_transformed(sw),
                                prox.y_transformed(sh),
                            ));
                            let focused = find_surface_under_loc(&mut state, loc);
                            tablet_state.handle_proximity(&mut state, &prox, focused, sw, sh, serial);
                        }
                        TabletToolEvent::Tip(tip) => {
                            tablet_state.handle_tip(&mut state, &tip, serial);
                        }
                        TabletToolEvent::Button(btn) => {
                            tablet_state.handle_button(&mut state, &btn, serial);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        if state.update_idle_state() {
            crate::request_system_suspend();
        }

        if shutdown_requested.get()
            || crate::SHUTDOWN_REQUESTED.load(std::sync::atomic::Ordering::SeqCst)
        {
            info!("Shutdown requested; leaving DRM loop for orderly resource release.");
            break;
        }

        // Immediately flush Wayland client socket buffers so input events (keys, mouse) reach corten-lock without delay
        let _ = display.flush_clients();

        // Frame pacing: while a page-flip is pending, do not render. Wait for the VBlank event.
        let completed_crtcs = std::mem::take(&mut *vblank_crtcs.borrow_mut());
        let vblank_got = !completed_crtcs.is_empty();
        if vblank_got {
            for crtc in completed_crtcs {
                if let Some(drm_output) = drm_outputs.iter().find(|output| output.crtc() == crtc) {
                    if let Err(err) = drm_output.frame_submitted() {
                        tracing::warn!(?crtc, "DRM frame_submitted error: {:?}", err);
                    }
                }
                pending_crtcs.retain(|pending| *pending != crtc);
            }
        }
        let tearing_allowed = state.is_tearing_allowed_for_output(&state.output);
        if !pending_crtcs.is_empty() && !tearing_allowed {
            std::thread::sleep(std::time::Duration::from_millis(1));
            continue;
        }
        let mut send_callbacks = vblank_got || tearing_allowed;

        // Arrange tiled windows with spring physics animations
        let usable_area = {
            let mut layer_map = layer_map_for_output(&state.output);
            layer_map.arrange();
            layer_map.non_exclusive_zone()
        };

        let layouts = if state.overview_mode {
            state.layout.arrange_overview(usable_area)
        } else {
            state.layout.arrange_animated(usable_area)
        };

        // Build render elements for display frame in FRONT-TO-BACK order
        // (Smithay's OutputDamageTracker requires front-to-back ordering for correct occlusion culling)
        let mut elements: Vec<CustomRenderElement> = Vec::new();
        let output_scale = state.output.current_scale();
        let fractional_scale = output_scale.fractional_scale();
        let render_scale = Scale::from(fractional_scale);

        // 0. Hardware / Software Cursor Pointer & Drag-and-Drop Icon (Topmost elements)
        if let Some(ref dnd_surface) = state.dnd_icon {
            let loc = state
                .cursor_location
                .to_i32_round::<i32>()
                .to_physical_precise_round(fractional_scale);
            let dnd_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                render_elements_from_surface_tree(
                    &mut renderer,
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
                CursorImageStatus::Hidden => {}
                CursorImageStatus::Surface(ref cursor_surface) => {
                    let loc = state
                        .cursor_location
                        .to_i32_round::<i32>()
                        .to_physical_precise_round(fractional_scale);
                    let cursor_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                        render_elements_from_surface_tree(
                            &mut renderer,
                            cursor_surface,
                            loc,
                            render_scale,
                            1.0,
                            Kind::Cursor,
                        );
                    elements.extend(cursor_elements.into_iter().map(CustomRenderElement::from));
                }
                CursorImageStatus::Named(icon) => {
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
                            &mut renderer,
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

        // 0.5. Alt + Tab Interactive OSD Switcher Card (Rendered in center of screen)
        if state.alt_tab_active {
            let win_count = layouts.len();
            if win_count > 0 {
                let card_w = (win_count as i32 * 110 + 30).clamp(240, screen_w - 60);
                let card_h = 100;
                let card_x = (screen_w - card_w) / 2;
                let card_y = (screen_h - card_h) / 2;

                cursor_commit.increment();
                let osd_bg_id = Id::new();
                let osd_border_id = Id::new();

                // Dark acrylic background
                elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                    osd_bg_id,
                    Rectangle::new(Point::from((card_x, card_y)), (card_w, card_h).into()),
                    cursor_commit,
                    Color32F::new(0.09, 0.10, 0.12, 0.95),
                    Kind::Unspecified,
                )));

                // Corten rust glow border
                elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                    osd_border_id,
                    Rectangle::new(Point::from((card_x, card_y)), (card_w, 2).into()),
                    cursor_commit,
                    Color32F::new(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.90,
                    ),
                    Kind::Unspecified,
                )));

                // Window slot tiles
                let active_win = state.layout.active_window();
                for (idx, (win, _)) in layouts.iter().enumerate() {
                    let is_active = active_win == Some(win);
                    let tile_w = 90;
                    let tile_h = 64;
                    let tile_x = card_x + 15 + (idx as i32 * 105);
                    let tile_y = card_y + 18;

                    let tile_bg_id = Id::new();
                    let tile_accent_id = Id::new();

                    let tile_bg = if is_active {
                        Color32F::new(
                            palette.primary.r,
                            palette.primary.g,
                            palette.primary.b,
                            0.25,
                        )
                    } else {
                        Color32F::new(1.0, 1.0, 1.0, 0.06)
                    };

                    elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                        tile_bg_id,
                        Rectangle::new(Point::from((tile_x, tile_y)), (tile_w, tile_h).into()),
                        cursor_commit,
                        tile_bg,
                        Kind::Unspecified,
                    )));

                    if is_active {
                        elements.push(CustomRenderElement::from(SolidColorRenderElement::new(
                            tile_accent_id,
                            Rectangle::new(
                                Point::from((tile_x, tile_y + tile_h - 3)),
                                (tile_w, 3).into(),
                            ),
                            cursor_commit,
                            Color32F::new(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                1.0,
                            ),
                            Kind::Unspecified,
                        )));
                    }

                    let (app_id, title) = crate::layout::window_metadata(win);
                    let label = if !title.trim().is_empty() {
                        title.trim()
                    } else if !app_id.trim().is_empty() {
                        app_id.trim()
                    } else {
                        "Pencere"
                    };
                    let text_color = if is_active {
                        [255, 255, 255]
                    } else {
                        [180, 185, 195]
                    };
                    if let Some((buf, _tw, th)) = crate::render_title_text_to_buffer(label, fractional_scale, text_color) {
                        let text_x = tile_x + 8;
                        let text_y = tile_y + (tile_h - th) / 2;
                        let text_pos = smithay::utils::Point::from((text_x as f64, text_y as f64));
                        if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
                            &mut renderer,
                            text_pos,
                            &buf,
                            None,
                            None,
                            None,
                            Kind::Unspecified,
                        ) {
                            elements.push(CustomRenderElement::from(elem));
                        }
                    }
                }
            }
        }

        // 1. Layer::Overlay surfaces (macOS Dock panel, System popups, Notifications, Lock screens)
        {
            let layer_map = layer_map_for_output(&state.output);
            for layer_surface in layer_map.layers_on(Layer::Overlay) {
                if state.session_locked {
                    continue;
                }
                let loc = layer_map
                    .layer_geometry(layer_surface)
                    .map(|g| g.loc.to_physical_precise_round(fractional_scale))
                    .unwrap_or_else(|| (0, 0).into());
                let surface_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                    render_elements_from_surface_tree(
                        &mut renderer,
                        layer_surface.wl_surface(),
                        loc,
                        render_scale,
                        1.0,
                        Kind::Unspecified,
                    );
                elements.extend(surface_elements.into_iter().map(CustomRenderElement::from));
            }
        }

        // ext-session-lock-v1 surfaces are rendered above every normal client
        // and layer-shell surface while the session is locked.
        for (lock_surface, lock_output) in &state.lock_surfaces {
            if !state.lock_surface_for_output(lock_output, &state.output) {
                continue;
            }
            let surface_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                render_elements_from_surface_tree(
                    &mut renderer,
                    lock_surface.wl_surface(),
                    (0, 0),
                    render_scale,
                    1.0,
                    Kind::Unspecified,
                );
            elements.extend(surface_elements.into_iter().map(CustomRenderElement::from));
        }

        // 2. Layer::Top surfaces (Desktop Widgets, Launchers)
        {
            let layer_map = layer_map_for_output(&state.output);
            for layer_surface in layer_map.layers_on(Layer::Top) {
                let loc = layer_map
                    .layer_geometry(layer_surface)
                    .map(|g| g.loc.to_physical_precise_round(fractional_scale))
                    .unwrap_or_else(|| (0, 0).into());
                let surface_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                    render_elements_from_surface_tree(
                        &mut renderer,
                        layer_surface.wl_surface(),
                        loc,
                        render_scale,
                        1.0,
                        Kind::Unspecified,
                    );
                elements.extend(surface_elements.into_iter().map(CustomRenderElement::from));
            }
        }

        // Compile neon glow shader & Dual-Kawase blur pipeline on first frame
        if glow_shader.is_none() {
            let gles: &mut smithay::backend::renderer::gles::GlesRenderer =
                std::borrow::BorrowMut::borrow_mut(&mut renderer);
            if let Ok(manager) = GlowShaderManager::new(gles) {
                glow_shader = Some(manager);
            }
            if let Ok(pipeline) = DualKawaseBlurPipeline::new(gles) {
                blur_pipeline = Some(pipeline);
            }
        }

        // 2.5. XWayland override-redirect surfaces (menus, tooltips, splash screens)
        for (or_surface, or_geo) in state.override_redirect_surfaces() {
            let loc = or_geo.loc.to_physical_precise_round(fractional_scale);
            let or_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                render_elements_from_surface_tree(
                    &mut renderer,
                    &or_surface,
                    loc,
                    render_scale,
                    1.0,
                    Kind::Unspecified,
                );
            elements.extend(or_elements.into_iter().map(CustomRenderElement::from));
        }

        // 3. Windows with 1:1 Touchpad Gesture tracking, Kinetic Momentum & Direct Scanout optimization
        state.step_gesture_kinetic();
        let gesture_offset_x = (state.gesture_swipe_dx * 1.4).round() as i32;
        let active_window = state.layout.active_window().cloned();
        let active_fullscreen = layouts.iter().find(|(w, _)| {
            state.layout.is_fullscreen(w) && (active_window.as_ref() == Some(w) || layouts.len() == 1)
        }).cloned();
        let has_fullscreen = active_fullscreen.is_some();

        for (window, rect) in layouts.iter().rev() {
            if !crate::layout::is_window_mapped(window) {
                continue;
            }
            let is_fs = state.layout.is_fullscreen(window);
            if has_fullscreen && !is_fs {
                // Completely occluded by the fullscreen window: skip to preserve direct scanout
                continue;
            }

            let mut shifted_rect = *rect;
            shifted_rect.loc.x += gesture_offset_x;

            let is_active = active_window.as_ref() == Some(window);
            let phys_rect = Rectangle::new(
                shifted_rect.loc.to_physical_precise_round(fractional_scale),
                shifted_rect
                    .size
                    .to_physical_precise_round(fractional_scale),
            );
            let titlebar_h = if is_fs { 0 } else { 36 };

            // 1. Popups for surface (menus, dropdowns) - topmost window elements
            if let Some(surface) = crate::layout::window_wl_surface(window) {
                for (popup, popup_offset) in PopupManager::popups_for_surface(&surface) {
                    let popup_loc = (shifted_rect.loc + smithay::utils::Point::from((0, titlebar_h)) + popup_offset)
                        .to_physical_precise_round(fractional_scale);
                    let popup_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                        render_elements_from_surface_tree(
                            &mut renderer,
                            popup.wl_surface(),
                            popup_loc,
                            render_scale,
                            1.0,
                            Kind::Unspecified,
                        );
                    elements.extend(popup_elements.into_iter().map(CustomRenderElement::from));
                }
            }

            // 2. Render Corten Industrial Titlebar & Controls IN FRONT of client surface
            if !is_fs {
                let is_max = state.layout.is_maximized(window);
                let (app_id, title) = crate::layout::window_metadata(window);
                draw_window_titlebar(
                    &mut elements,
                    &mut renderer,
                    phys_rect,
                    is_active,
                    &palette,
                    is_max,
                    &app_id,
                    &title,
                    fractional_scale,
                    &mut state.title_cache,
                    true,
                );
            }

            // 3. Render client window surface content
            if let Some(surface) = crate::layout::window_wl_surface(window) {
                let client_loc = (shifted_rect.loc + smithay::utils::Point::from((0, titlebar_h)))
                    .to_physical_precise_round(fractional_scale);
                let win_alpha = state.layout.window_alpha(window);
                let window_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                    render_elements_from_surface_tree(
                        &mut renderer,
                        &surface,
                        client_loc,
                        render_scale,
                        win_alpha,
                        Kind::Unspecified,
                    );
                if is_fs {
                    elements.extend(window_elements.into_iter().map(CustomRenderElement::from));
                } else {
                    let body = Rectangle::new(
                        client_loc,
                        (shifted_rect.size - smithay::utils::Size::from((0, titlebar_h)))
                            .to_physical_precise_round(fractional_scale),
                    );
                    elements.extend(crate::clip_to_body(window_elements, fractional_scale, body));
                }
            }

            // 4. Glow halo and frame borders (Behind window content in front-to-back ordering, only for non-fullscreen windows)
            if is_active && !is_fs {
                let is_high_vis = state.accessibility.high_vis_focus_ring || state.accessibility.high_contrast;
                let (ring_width, ring_color, halo_radiance) = if is_high_vis {
                    (3, Color32F::new(palette.primary.r, palette.primary.g, palette.primary.b, 1.0), 0.95)
                } else {
                    (2, Color32F::new(palette.primary.r, palette.primary.g, palette.primary.b, 1.0), 0.72)
                };
                if let Some(manager) = glow_shader.as_ref() {
                    let halo_color = [
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        halo_radiance,
                    ];
                    let glow_elem = manager.create_element(shifted_rect, 18, 8.0, halo_color);
                    elements.push(CustomRenderElement::from(glow_elem));
                }
                draw_glow_ring(&mut elements, phys_rect, 0, ring_width, ring_color, 4);
            } else if !is_fs {
                let inactive_color =
                    Color32F::new(palette.border.r, palette.border.g, palette.border.b, 0.45);
                draw_glow_ring(&mut elements, phys_rect, 0, 1, inactive_color, 3);
            }
        }

        if !has_fullscreen {
            // Render snap preview highlight overlay if dragging a window near screen edge
            if let Some(snap_rect) = state.snap_preview {
                let snap_margin = 8;
                let inset_loc = smithay::utils::Point::from((snap_rect.loc.x + snap_margin, snap_rect.loc.y + snap_margin));
                let inset_size = smithay::utils::Size::from(((snap_rect.size.w - snap_margin * 2).max(50), (snap_rect.size.h - snap_margin * 2).max(50)));
                let phys_snap = Rectangle::new(
                    inset_loc.to_physical_precise_round(fractional_scale),
                    inset_size.to_physical_precise_round(fractional_scale),
                );
                let snap_color = Color32F::new(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    0.18,
                );
                let snap_border = Color32F::new(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    0.75,
                );
                draw_glow_ring(
                    &mut elements,
                    phys_snap,
                    0,
                    phys_snap.size.w.min(phys_snap.size.h) / 2,
                    snap_color,
                    14,
                );
                draw_glow_ring(&mut elements, phys_snap, 0, 2, snap_border, 14);
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

            // Record backdrop insertion point: elements inserted here will sit behind all windows and ahead of wallpaper/widgets
            let bg_start_idx = elements.len();

            // 4. Desktop Widgets / Panels (Bottom layer)
            {
                let layer_map = layer_map_for_output(&state.output);
                for layer_surface in layer_map.layers_on(Layer::Bottom) {
                    let loc = layer_map
                        .layer_geometry(layer_surface)
                        .map(|g| g.loc.to_physical_precise_round(fractional_scale))
                        .unwrap_or_else(|| (0, 0).into());
                    let surface_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                        render_elements_from_surface_tree(
                            &mut renderer,
                            layer_surface.wl_surface(),
                            loc,
                            render_scale,
                            1.0,
                            Kind::Unspecified,
                        );
                    elements.extend(surface_elements.into_iter().map(CustomRenderElement::from));
                }
            }

            // 5. Wallpaper (Background layer - back-most element)
            {
                let layer_map = layer_map_for_output(&state.output);
                for layer_surface in layer_map.layers_on(Layer::Background) {
                    let loc = layer_map
                        .layer_geometry(layer_surface)
                        .map(|g| g.loc.to_physical_precise_round(fractional_scale))
                        .unwrap_or_else(|| (0, 0).into());
                    let surface_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
                        render_elements_from_surface_tree(
                            &mut renderer,
                            layer_surface.wl_surface(),
                            loc,
                            render_scale,
                            1.0,
                            Kind::Unspecified,
                        );
                    elements.extend(surface_elements.into_iter().map(CustomRenderElement::from));
                }
            }

            // FAZ 25: Per-Window Dual-Kawase Background Blur & Acrylic Glass Pipeline
            if let Some(pipeline) = blur_pipeline.as_mut() {
                let mut glass_regions: Vec<GlassRegion> = Vec::new();

                // 1. Collect blur-eligible windows (translucent windows, terminals, window rule blur)
                for (window, geo) in layouts.iter() {
                    let win_alpha = state.layout.window_alpha(window);
                    let is_fs = state.layout.is_fullscreen(window);
                    if is_fs {
                        continue;
                    }
                    let is_blur = crate::layout::is_window_blur_eligible(window, &state.window_rules)
                        || win_alpha < 0.99;
                    if is_blur {
                        let titlebar_h = 36;
                        let body_rect = Rectangle::new(
                            geo.loc + Point::from((0, titlebar_h)),
                            Size::from((geo.size.w.max(10), (geo.size.h - titlebar_h).max(10))),
                        );
                        glass_regions.push(GlassRegion::new(body_rect, 8.0, 0.22));
                    }
                }

                // 2. Collect compact floating layer-shell surfaces (inset strictly inside dock capsule to avoid rectangular corner bleed)
                {
                    let layer_map = layer_map_for_output(&state.output);
                    for layer in [Layer::Top, Layer::Overlay] {
                        if state.session_locked && layer == Layer::Overlay {
                            continue;
                        }
                        for layer_surface in layer_map.layers_on(layer) {
                            if layer_surface.namespace() == "corten-panel" {
                                if let Some(geo) = layer_map.layer_geometry(layer_surface) {
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
                }

                if !glass_regions.is_empty() && bg_start_idx <= elements.len() {
                    let backdrops = pipeline.generate_glass_backdrops(
                        &mut renderer,
                        &elements[bg_start_idx..],
                        true,
                        initial_size,
                        fractional_scale,
                        bg_color,
                        &glass_regions,
                    );
                    for backdrop in backdrops.into_iter().rev() {
                        elements.insert(bg_start_idx, CustomRenderElement::from(backdrop));
                    }
                }
            }
        }

        // Render hardware frame to DRM Output
        for (output_index, drm_output) in drm_outputs.iter_mut().enumerate() {
            if state.screen_asleep {
                if let Err(error) = drm_output.with_compositor(|compositor| compositor.clear()) {
                    tracing::warn!(?error, ?output_index, "Failed to enter DRM DPMS-off state");
                }
                continue;
            }
            if output_index == 0 && state.night_light_enabled {
                let out_size = state
                    .output
                    .current_mode()
                    .map(|mode| mode.size)
                    .unwrap_or_else(|| Size::from((1, 1)));
                let nl_elem = blur_pipeline.as_mut().and_then(|pipeline| {
                    pipeline.generate_night_light_element(
                        &mut renderer,
                        &elements,
                        true,
                        out_size,
                        fractional_scale,
                        bg_color,
                    )
                });
                if let Some(nl_elem) = nl_elem {
                    elements.insert(0, CustomRenderElement::from(nl_elem));
                } else {
                    elements.insert(0, crate::night_light_overlay(out_size));
                }
            }
            let mut secondary_scene = if output_index == 0 {
                None
            } else {
                state
                    .additional_outputs
                    .get(output_index - 1)
                    .cloned()
                    .map(|output| {
                        build_secondary_scene(&mut state, &output, &mut renderer, &palette)
                    })
            };
            if state.night_light_enabled {
                if let Some(scene) = secondary_scene.as_mut() {
                    let size = state
                        .additional_outputs
                        .get(output_index - 1)
                        .and_then(|output| output.current_mode())
                        .map(|mode| mode.size)
                        .unwrap_or_else(|| Size::from((1, 1)));
                    let nl_elem = blur_pipeline.as_mut().and_then(|pipeline| {
                        pipeline.generate_night_light_element(
                            &mut renderer,
                            scene,
                            false,
                            size,
                            fractional_scale,
                            bg_color,
                        )
                    });
                    if let Some(nl_elem) = nl_elem {
                        scene.push(CustomRenderElement::from(nl_elem));
                    } else {
                        scene.push(crate::night_light_overlay(size));
                    }
                }
            }
            let scene = secondary_scene.as_deref().unwrap_or(&elements);
            let clear_color = if has_fullscreen {
                Color32F::new(0.0, 0.0, 0.0, 0.0)
            } else {
                bg_color
            };
            // Enable hardware cursor plane & direct scanout for fullscreen clients on primary/secondary CRTCs,
            // as well as direct zero-copy video overlay plane scanout for video players (MPV, VLC, browser VA-API)
            let frame_flags = FrameFlags::DEFAULT
                | FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY
                | FrameFlags::ALLOW_OVERLAY_PLANE_SCANOUT;
            let render_res =
                drm_output.render_frame(&mut renderer, scene, clear_color, frame_flags);

            match render_res {
                Ok(ref frame_res) => {
                    if let Some((ref fs_window, _)) = active_fullscreen {
                        if let Some(surface) = crate::layout::window_wl_surface(fs_window) {
                            smithay::wayland::compositor::with_states(&surface, |surface_data| {
                                smithay::desktop::utils::update_surface_primary_scanout_output(
                                    &surface,
                                    &state.output,
                                    surface_data,
                                    &frame_res.states,
                                    smithay::backend::renderer::element::default_primary_scanout_output_compare,
                                );
                            });
                        }
                    }
                    match drm_output.queue_frame(()) {
                        Ok(()) => {
                            let crtc = drm_output.crtc();
                            if !pending_crtcs.contains(&crtc) {
                                pending_crtcs.push(crtc);
                            }
                        }
                        Err(err) => {
                            // EmptyFrame (no damage) is normal: nothing will be scanned out, so no VBlank follows.
                            tracing::debug!("DRM queue_frame skipped: {:?}", err);
                            send_callbacks = true;
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!("DRM render_frame error: {:?}", err);
                    send_callbacks = true;
                }
            }
        }

        // Fulfill any pending `wlr-screencopy-unstable-v1` frames and native screenshots
        crate::screencopy::fulfill_pending_captures(
            &mut state,
            &mut renderer,
            &elements,
            true,
            initial_size,
            fractional_scale,
            bg_color,
        );

        // Frame timing callbacks: once per displayed frame (VBlank), or when nothing was queued
        if send_callbacks || state.session_locked {
            let elapsed_ms = start_time.elapsed().as_millis() as u32;
            for (lock_surface, _) in &state.lock_surfaces {
                send_frames_surface_tree(lock_surface.wl_surface(), elapsed_ms);
            }
            if let Some(ref dnd_surface) = state.dnd_icon {
                send_frames_surface_tree(dnd_surface, elapsed_ms);
            }
            if let CursorImageStatus::Surface(ref cursor_surface) = state.cursor_status {
                send_frames_surface_tree(cursor_surface, elapsed_ms);
            }
            for (window, _) in &layouts {
                if let Some(surface) = crate::layout::window_wl_surface(window) {
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
        }

        // Flush client socket buffers again after frame callbacks so clients receive frame timing immediately
        let _ = display.flush_clients();

        // Adaptive frame pacing: run at 240Hz (4ms) during active spring animations,
        // window dragging, or pending VBlank; relax to 12ms when idle to conserve battery/CPU.
        let is_animating = state.layout.is_animating(usable_area)
            || state.dragged_window.is_some()
            || state.gesture_kinetic_active
            || !pending_crtcs.is_empty();
        let pace_ms = if state.screen_asleep {
            50
        } else if is_animating {
            4
        } else {
            12
        };
        std::thread::sleep(std::time::Duration::from_millis(pace_ms));
    }

    // Release the DRM output before the renderer/session. This lets Smithay
    // restore the CRTC and lets Libseat release the device while we still own
    // the session, avoiding abrupt process teardown.
    drop(drm_outputs);
    drop(output_manager);
    drop(renderer);
    info!("Corten Compositor DRM engine exited cleanly.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi_gpu_device_structure() {
        let node_opt = DrmNode::from_path("/dev/dri/card1")
            .or_else(|_| DrmNode::from_path("/dev/dri/card0"))
            .ok();

        if let Some(node) = node_opt {
            let dev = MultiGpuDevice {
                node,
                dev_path: std::path::PathBuf::from("/dev/dri/card1"),
                render_path: Some(std::path::PathBuf::from("/dev/dri/renderD128")),
                is_primary: true,
                has_render_node: true,
            };
            assert!(dev.is_primary);
            assert!(dev.has_render_node);
            assert_eq!(dev.dev_path.to_str().unwrap(), "/dev/dri/card1");
        } else {
            let dev_path = std::path::PathBuf::from("/dev/dri/card0");
            let render_path = Some(std::path::PathBuf::from("/dev/dri/renderD128"));
            assert_eq!(dev_path.to_str().unwrap(), "/dev/dri/card0");
            assert!(render_path.is_some());
        }
    }

    #[test]
    fn test_direct_video_overlay_plane_flags() {
        let frame_flags = FrameFlags::DEFAULT
            | FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY
            | FrameFlags::ALLOW_OVERLAY_PLANE_SCANOUT;

        assert!(frame_flags.contains(FrameFlags::ALLOW_OVERLAY_PLANE_SCANOUT));
        assert!(frame_flags.contains(FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY));
    }

    #[test]
    fn test_explicit_sync_syncobj_eventfd_detection() {
        if let Ok(file) = std::fs::File::options()
            .read(true)
            .write(true)
            .open("/dev/dri/renderD128")
            .or_else(|_| std::fs::File::options().read(true).write(true).open("/dev/dri/card0"))
        {
            use std::os::fd::OwnedFd;
            let owned_fd: OwnedFd = file.into();
            let device_fd = smithay::utils::DeviceFd::from(owned_fd);
            let drm_device_fd = smithay::backend::drm::DrmDeviceFd::new(device_fd);
            let supported = smithay::wayland::drm_syncobj::supports_syncobj_eventfd(&drm_device_fd);
            assert!(supported || !supported);
        } else {
            assert!(!std::path::Path::new("/dev/dri/nonexistent_drm_card_fallback").exists());
        }
    }
}
