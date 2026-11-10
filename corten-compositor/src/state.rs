use corten_config::{LayoutMode, WindowRuleConfig};
use smithay::desktop::{layer_map_for_output, PopupManager, Window};
use smithay::input::{Seat, SeatState};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::utils::{Point, Rectangle, Size, Transform, SERIAL_COUNTER};
use smithay::wayland::compositor::CompositorState;
use smithay::wayland::alpha_modifier::AlphaModifierState;
use smithay::wayland::content_type::ContentTypeState;
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufState};
use smithay::wayland::fractional_scale::FractionalScaleManagerState;
use smithay::wayland::idle_inhibit::IdleInhibitManagerState;
use smithay::wayland::keyboard_shortcuts_inhibit::{
    KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitorSeat,
};
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::pointer_constraints::{
    with_pointer_constraint, PointerConstraint, PointerConstraintsState,
};
use smithay::wayland::pointer_gestures::PointerGesturesState;
use smithay::wayland::presentation::PresentationState;
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::selection::wlr_data_control::DataControlState;
use smithay::wayland::session_lock::{LockSurface, SessionLockManagerState, SessionLocker};
use smithay::wayland::shell::wlr_layer::WlrLayerShellState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shm::ShmState;
use smithay::wayland::single_pixel_buffer::SinglePixelBufferState;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::xdg_activation::XdgActivationState;
use smithay::wayland::xwayland_shell::XWaylandShellState;
use smithay::xwayland::xwm::X11Wm;
use smithay::xwayland::X11Surface;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use wayland_server::protocol::wl_output::WlOutput;
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{Client, Display, Resource};

use crate::layout::{window_wl_surface, LayoutManager, WindowSnapshot};
use crate::screencopy::{PendingScreencopy, PendingScreenshot, ScreencopyManagerState};

/// Thread-safe snapshot of the compositor state exposed to D-Bus/IPC consumers (such as `corten-panel`).
#[derive(Debug, Clone, Default)]
pub struct CompositorIpcState {
    pub active_workspace: u32,
    pub occupied_workspaces: Vec<u32>,
    pub windows: Vec<WindowSnapshot>,
    pub output_active_workspaces: HashMap<String, u32>,
    pub output_occupied_workspaces: HashMap<String, Vec<u32>>,
}

pub struct CortenCompositor {
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    #[allow(dead_code)]
    pub xdg_decoration_state: XdgDecorationState,
    pub layer_shell_state: WlrLayerShellState,
    pub layer_surfaces: Vec<smithay::desktop::LayerSurface>,
    pub session_lock_state: SessionLockManagerState,
    #[allow(dead_code)]
    pub output_manager_state: OutputManagerState,
    pub shm_state: ShmState,
    pub dmabuf_state: DmabufState,
    pub dmabuf_global: Option<DmabufGlobal>,
    /// Explicit-sync (`linux-drm-syncobj-v1`) state; `None` when the GPU lacks `syncobj_eventfd`.
    pub syncobj_state: Option<smithay::wayland::drm_syncobj::DrmSyncobjState>,
    /// Event loop handle used to register fence/dmabuf readiness sources for commit blockers.
    pub loop_handle: Option<smithay::reexports::calloop::LoopHandle<'static, CortenCompositor>>,
    pub display_handle: wayland_server::DisplayHandle,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub data_control_state: DataControlState,
    #[allow(dead_code)]
    pub cursor_shape_manager_state: CursorShapeManagerState,
    pub tablet_state: smithay::wayland::tablet_manager::TabletManagerState,
    #[allow(dead_code)]
    pub viewporter_state: ViewporterState,
    #[allow(dead_code)]
    pub fractional_scale_manager_state: FractionalScaleManagerState,
    #[allow(dead_code)]
    pub relative_pointer_manager_state: RelativePointerManagerState,
    #[allow(dead_code)]
    pub pointer_constraints_state: PointerConstraintsState,
    #[allow(dead_code)]
    pub pointer_gestures_state: PointerGesturesState,
    #[allow(dead_code)]
    pub idle_inhibit_manager_state: IdleInhibitManagerState,
    pub idle_inhibiting_surfaces: Vec<WlSurface>,
    pub keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
    #[allow(dead_code)]
    pub single_pixel_buffer_state: SinglePixelBufferState,
    #[allow(dead_code)]
    pub presentation_state: PresentationState,
    #[allow(dead_code)]
    pub alpha_modifier_state: AlphaModifierState,
    #[allow(dead_code)]
    pub content_type_state: ContentTypeState,
    pub xdg_activation_state: XdgActivationState,
    #[allow(dead_code)]
    pub text_input_state: smithay::wayland::text_input::TextInputManagerState,
    pub tearing_state: crate::tearing::TearingControlState,
    #[allow(dead_code)]
    pub screencopy_state: ScreencopyManagerState,
    pub pending_screencopies: Vec<PendingScreencopy>,
    pub pending_screenshot: Option<PendingScreenshot>,
    pub xwayland_shell_state: XWaylandShellState,
    pub xwm: Option<X11Wm>,
    pub xwayland_client: Option<Client>,
    pub override_redirect_windows: Vec<X11Surface>,
    pub output_size: Size<i32, smithay::utils::Logical>,
    pub panel_height: i32,

    pub seat: Seat<Self>,
    pub output: Output,
    /// Outputs beyond the primary output. The primary remains in `output` for
    /// the existing layout/input path; DRM mode registers additional outputs
    /// here so clients can discover the real topology.
    pub additional_outputs: Vec<Output>,
    pub output_layouts: HashMap<String, LayoutManager>,
    pub output_workspaces: HashMap<String, usize>,
    pub layout: LayoutManager,
    pub active_workspace: usize,
    pub pre_show_desktop_workspace: Option<usize>,
    pub ipc_snapshot: Arc<RwLock<CompositorIpcState>>,
    pub popup_manager: PopupManager,
    pub cursor_location: Point<f64, smithay::utils::Logical>,
    pub cursor_status: smithay::input::pointer::CursorImageStatus,
    pub dnd_icon: Option<WlSurface>,
    pub window_rules: Vec<WindowRuleConfig>,
    pub gesture_swipe_dx: f64,
    pub gesture_swipe_dy: f64,
    pub gesture_samples: Vec<(u64, f64, f64)>,
    pub gesture_kinetic_active: bool,
    pub gesture_kinetic_velocity: f64,
    pub overview_mode: bool,
    #[allow(clippy::type_complexity)]
    pub dragged_window: Option<(
        smithay::desktop::Window,
        Point<f64, smithay::utils::Logical>,
        Rectangle<i32, smithay::utils::Logical>,
    )>,
    #[allow(clippy::type_complexity)]
    pub resized_window: Option<(
        smithay::desktop::Window,
        Point<f64, smithay::utils::Logical>,
        Rectangle<i32, smithay::utils::Logical>,
    )>,
    pub split_drag: Option<(Point<f64, smithay::utils::Logical>, f32)>,
    pub snap_preview: Option<Rectangle<i32, smithay::utils::Logical>>,
    pub drag_split_target: Option<(
        smithay::desktop::Window,
        crate::layout::SplitDirection,
        bool,
        Rectangle<i32, smithay::utils::Logical>,
    )>,
    pub selection_box: Option<(Point<f64, smithay::utils::Logical>, Point<f64, smithay::utils::Logical>)>,
    pub alt_tab_active: bool,
    pub alt_tab_instant: Option<std::time::Instant>,
    pub lock_surfaces: Vec<(LockSurface, WlOutput)>,
    pub pending_lock: Option<SessionLocker>,
    pub session_locked: bool,
    pub last_input: Instant,
    pub screen_sleep_after: Option<Duration>,
    pub suspend_after: Option<Duration>,
    pub suspend_requested: bool,
    pub screen_asleep: bool,
    pub night_light_enabled: bool,
    pub vrr_enabled: bool,
    pub allow_tearing: bool,
    pub hdr_enabled: bool,
    pub color_space: String,
    pub event_broadcaster: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    pub last_broadcast_event: std::sync::Mutex<Option<String>>,
    pub title_cache: HashMap<String, (smithay::backend::renderer::element::memory::MemoryRenderBuffer, i32, i32)>,
    pub last_titlebar_click: Option<(smithay::desktop::Window, Instant)>,
    pub accessibility: corten_config::AccessibilityConfig,
    pub a11y_bridge: corten_services::a11y::AtspiBridge,
}

impl CortenCompositor {
    pub fn lock_surface_active(&self) -> bool {
        if self.session_locked {
            return true;
        }
        let layer_map = layer_map_for_output(&self.output);
        let active = layer_map
            .layers()
            .any(|layer_surface| layer_surface.namespace() == "corten-lock");
        active
    }

    pub fn lock_surface_for_output(&self, lock_output: &WlOutput, output: &Output) -> bool {
        Output::from_resource(lock_output).is_some_and(|owner| owner == *output)
    }

    pub fn set_power_timeouts(&mut self, screen_sleep_minutes: u32, suspend_minutes: u32) {
        self.screen_sleep_after = (screen_sleep_minutes > 0)
            .then(|| Duration::from_secs(u64::from(screen_sleep_minutes) * 60));
        self.suspend_after =
            (suspend_minutes > 0).then(|| Duration::from_secs(u64::from(suspend_minutes) * 60));
        self.last_input = Instant::now();
        self.screen_asleep = false;
        self.suspend_requested = false;
    }

    pub fn note_input(&mut self) {
        self.last_input = Instant::now();
        self.screen_asleep = false;
        self.suspend_requested = false;
    }

    pub fn is_idle_inhibited(&mut self) -> bool {
        self.idle_inhibiting_surfaces.retain(|s| s.is_alive());
        !self.idle_inhibiting_surfaces.is_empty()
    }

    pub fn focused_surface_inhibits_shortcuts(&self) -> bool {
        let Some(kb) = self.seat.get_keyboard() else {
            return false;
        };
        let Some(focused) = kb.current_focus() else {
            return false;
        };
        self.seat
            .keyboard_shortcuts_inhibitor_for_surface(&focused)
            .is_some_and(|inhibitor| inhibitor.is_active())
    }

    /// Enforces `zwp_pointer_constraints_v1` (`Locked` or `Confined`) for the currently focused surface.
    /// - When `Locked` is active, keeps `cursor_location` stationary while relative motion events flow to 3D games.
    /// - When `Confined` is active, clamps `target_loc` to the active window's logical bounds (or constraint region).
    pub fn apply_pointer_constraint(
        &self,
        target_loc: Point<f64, smithay::utils::Logical>,
    ) -> Point<f64, smithay::utils::Logical> {
        let Some(pointer) = self.seat.get_pointer() else {
            return target_loc;
        };
        let focused_surface = pointer.current_focus().or_else(|| {
            self.layout
                .active_window()
                .and_then(crate::layout::window_wl_surface)
        });
        let Some(surface) = focused_surface else {
            return target_loc;
        };

        let mut constrained = None;
        with_pointer_constraint(&surface, &pointer, |constraint| {
            let Some(constraint) = constraint else {
                return;
            };
            if !constraint.is_active() {
                constraint.activate();
            }
            match &*constraint {
                PointerConstraint::Locked(_locked) => {
                    constrained = Some(self.cursor_location);
                }
                PointerConstraint::Confined(confined) => {
                    // Find the window geometry containing `surface` (checking floating first, then tiling)
                    let usable = self.usable_area();
                    let win_rect = self
                        .layout
                        .floating_windows()
                        .iter()
                        .find(|w| {
                            crate::layout::window_wl_surface(w).as_ref() == Some(&surface)
                        })
                        .and_then(|w| self.layout.floating_geometry(w))
                        .unwrap_or_else(|| {
                            let arranged = self.layout.arrange(usable);
                            arranged
                                .iter()
                                .find(|(w, _)| {
                                    crate::layout::window_wl_surface(w).as_ref() == Some(&surface)
                                })
                                .map(|(_, r)| *r)
                                .unwrap_or(usable)
                        });

                    let mut min_x = win_rect.loc.x as f64;
                    let mut min_y = win_rect.loc.y as f64;
                    let mut max_x = (win_rect.loc.x + win_rect.size.w.max(1) - 1) as f64;
                    let mut max_y = (win_rect.loc.y + win_rect.size.h.max(1) - 1) as f64;

                    if let Some(region) = confined.region() {
                        if let Some((_, ext)) = region.rects.first() {
                            if ext.size.w > 0 && ext.size.h > 0 {
                                min_x = (win_rect.loc.x + ext.loc.x) as f64;
                                min_y = (win_rect.loc.y + ext.loc.y) as f64;
                                max_x = (win_rect.loc.x + ext.loc.x + ext.size.w - 1) as f64;
                                max_y = (win_rect.loc.y + ext.loc.y + ext.size.h - 1) as f64;
                            }
                        }
                    }

                    let clamped_x = target_loc.x.clamp(min_x, max_x.max(min_x));
                    let clamped_y = target_loc.y.clamp(min_y, max_y.max(min_y));
                    constrained = Some(Point::from((clamped_x, clamped_y)));
                }
            }
        });

        constrained.unwrap_or(target_loc)
    }

    /// Returns `true` if a client surface currently has an active pointer lock constraint.
    pub fn is_pointer_locked(&self) -> bool {
        let Some(pointer) = self.seat.get_pointer() else {
            return false;
        };
        let focused_surface = pointer.current_focus().or_else(|| {
            self.layout
                .active_window()
                .and_then(crate::layout::window_wl_surface)
        });
        let Some(surface) = focused_surface else {
            return false;
        };
        let mut locked = false;
        with_pointer_constraint(&surface, &pointer, |constraint| {
            if let Some(constraint) = constraint {
                if constraint.is_active() {
                    if let PointerConstraint::Locked(_) = &*constraint {
                        locked = true;
                    }
                }
            }
        });
        locked
    }

    /// Record a swipe gesture sample for kinetic momentum and fling velocity calculations.
    pub fn record_gesture_sample(&mut self, time_usec: u64, dx: f64, dy: f64) {
        self.gesture_samples.push((time_usec, dx, dy));
        if self.gesture_samples.len() > 12 {
            self.gesture_samples.remove(0);
        }
    }

    /// Compute instantaneous gesture velocity (in pixels per second) over recent updates.
    pub fn compute_gesture_velocity(&self) -> (f64, f64) {
        calculate_gesture_velocity(&self.gesture_samples)
    }

    /// Activate smooth kinetic spring decay towards resting state.
    pub fn start_gesture_kinetic(&mut self, initial_velocity: f64) {
        self.gesture_kinetic_active = true;
        self.gesture_kinetic_velocity = initial_velocity;
    }

    /// Step kinetic decay and spring restitution for 1:1 gesture transitions.
    /// Returns true if still animating.
    pub fn step_gesture_kinetic(&mut self) -> bool {
        if !self.gesture_kinetic_active {
            return false;
        }
        let (offset, velocity, active) =
            step_kinetic_physics(self.gesture_swipe_dx, self.gesture_kinetic_velocity);
        self.gesture_swipe_dx = offset;
        self.gesture_kinetic_velocity = velocity;
        self.gesture_kinetic_active = active;
        active
    }

    pub fn update_idle_state(&mut self) -> bool {
        if self.session_locked {
            return false;
        }
        if self.is_idle_inhibited() {
            self.last_input = Instant::now();
            self.screen_asleep = false;
            self.suspend_requested = false;
            return false;
        }
        if let Some(timeout) = self.screen_sleep_after {
            let was_asleep = self.screen_asleep;
            self.screen_asleep = self.last_input.elapsed() >= timeout;
            if self.screen_asleep && !was_asleep {
                let lock_path = resolve_corten_bin("corten-lock");
                let _ = std::process::Command::new(lock_path).spawn();
            }
        }
        if !self.suspend_requested
            && self
                .suspend_after
                .is_some_and(|timeout| self.last_input.elapsed() >= timeout)
        {
            self.suspend_requested = true;
            return true;
        }
        false
    }

    pub fn set_night_light_enabled(&mut self, enabled: bool) {
        self.night_light_enabled = enabled;
    }

    pub fn create_output(
        &self,
        name: String,
        mode: Mode,
        location: Point<i32, smithay::utils::Logical>,
        scale: smithay::output::Scale,
    ) -> Output {
        let output = Output::new(
            name,
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Corten Desktop".to_string(),
                model: "DRM Display".to_string(),
            },
        );
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(scale),
            Some(location),
        );
        output.set_preferred(mode);
        output
    }

    pub fn register_output(&mut self, display: &mut Display<Self>, output: Output) {
        output.create_global::<Self>(&display.handle());
        self.additional_outputs.push(output.clone());
        self.output_layouts
            .insert(output.name(), LayoutManager::new());
    }

    /// New windows are assigned to the primary output until an explicit move
    /// command is requested. Keeping one owner prevents the same surface from
    /// being rendered independently on every monitor.
    pub fn add_window_to_primary_output(&mut self, window: Window) {
        self.layout.add_window(window.clone());
        self.sync_ipc_snapshot();
    }

    pub fn add_floating_window_to_primary_output(&mut self, window: Window) {
        self.layout.add_floating_window(window.clone());
        self.sync_ipc_snapshot();
    }

    pub fn switch_workspace(&mut self, workspace: usize) {
        let workspace = workspace.clamp(1, 9);
        if self.accessibility.sound_feedback {
            corten_services::a11y::play_sound_cue(corten_services::a11y::SoundCue::WorkspaceSwitch);
        }
        if self.accessibility.screen_reader || corten_services::a11y::is_screen_reader_enabled() {
            corten_services::a11y::announce_screen_reader(&format!("Çalışma Alanı {}", workspace));
        }
        let focused_output = self.output_at(self.cursor_location);
        if focused_output.name() == self.output.name() {
            self.active_workspace = workspace;
            self.layout.switch_workspace(workspace);
            if let Some(active) = self.layout.active_window().cloned() {
                if let Some(surface) = crate::layout::window_wl_surface(&active) {
                    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                    if let Some(kb) = self.seat.get_keyboard() {
                        kb.set_focus(self, Some(surface), serial);
                    }
                }
            }
        } else {
            self.output_workspaces.insert(focused_output.name(), workspace);
            if let Some(layout) = self.output_layouts.get_mut(&focused_output.name()) {
                layout.switch_workspace(workspace);
                if let Some(active) = layout.active_window().cloned() {
                    if let Some(surface) = crate::layout::window_wl_surface(&active) {
                        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                        if let Some(kb) = self.seat.get_keyboard() {
                            kb.set_focus(self, Some(surface), serial);
                        }
                    }
                }
            }
        }
        self.sync_ipc_snapshot();
    }

    pub fn switch_workspace_on_output(&mut self, output_name: &str, workspace: usize) {
        let workspace = workspace.clamp(1, 9);
        if output_name == self.output.name() {
            self.active_workspace = workspace;
            self.layout.switch_workspace(workspace);
            if let Some(active) = self.layout.active_window().cloned() {
                if let Some(surface) = crate::layout::window_wl_surface(&active) {
                    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                    if let Some(kb) = self.seat.get_keyboard() {
                        kb.set_focus(self, Some(surface), serial);
                    }
                }
            }
        } else {
            self.output_workspaces.insert(output_name.to_string(), workspace);
            if let Some(layout) = self.output_layouts.get_mut(output_name) {
                layout.switch_workspace(workspace);
                if let Some(active) = layout.active_window().cloned() {
                    if let Some(surface) = crate::layout::window_wl_surface(&active) {
                        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                        if let Some(kb) = self.seat.get_keyboard() {
                            kb.set_focus(self, Some(surface), serial);
                        }
                    }
                }
            }
        }
        self.sync_ipc_snapshot();
    }

    pub fn move_active_window_to_workspace(&mut self, workspace: usize) {
        let workspace = workspace.clamp(1, 9);
        let focused_output = self.output_at(self.cursor_location);
        let moved = if focused_output.name() == self.output.name() {
            self.layout.move_active_to_workspace(workspace).is_some()
        } else if let Some(layout) = self.output_layouts.get_mut(&focused_output.name()) {
            layout.move_active_to_workspace(workspace).is_some()
        } else {
            false
        };
        if moved {
            self.sync_ipc_snapshot();
        }
    }

    pub fn toggle_show_desktop(&mut self) {
        if let Some(prev_ws) = self.pre_show_desktop_workspace.take() {
            if self.accessibility.screen_reader || corten_services::a11y::is_screen_reader_enabled() {
                corten_services::a11y::announce_screen_reader("Pencereler Geri Yüklendi");
            }
            self.switch_workspace(prev_ws);
        } else {
            let current = self.active_workspace;
            if self.accessibility.screen_reader || corten_services::a11y::is_screen_reader_enabled() {
                corten_services::a11y::announce_screen_reader("Masaüstü Gösteriliyor");
            }
            // Find an empty workspace (prefer 9)
            let occupied = self.layout.occupied_workspaces();
            let target = (1..=9)
                .rev()
                .find(|ws| !occupied.contains(&(*ws as u32)))
                .unwrap_or(9);
            if target != current {
                self.pre_show_desktop_workspace = Some(current);
                self.switch_workspace(target);
            }
        }
    }

    pub fn focus_window_by_id(&mut self, window_id: u64) -> bool {
        if let Some((target_ws, window)) = self.layout.focus_window_by_id(window_id) {
            self.active_workspace = target_ws;
            for layout in self.output_layouts.values_mut() {
                layout.switch_workspace(target_ws);
            }
            if let Some(x11) = window.x11_surface() {
                let _ = x11.set_activated(true);
            }
            if let Some(surface) = window_wl_surface(&window) {
                let serial = SERIAL_COUNTER.next_serial();
                if let Some(kb) = self.seat.get_keyboard() {
                    kb.set_focus(self, Some(surface), serial);
                }
            }
            self.sync_ipc_snapshot();
            return true;
        }
        false
    }

    pub fn usable_area(&self) -> Rectangle<i32, smithay::utils::Logical> {
        let size = self
            .output
            .current_mode()
            .map(|m| Size::from((m.size.w, m.size.h)))
            .unwrap_or(self.output_size);
        let top_offset = self.panel_height.max(0);
        Rectangle::new(
            Point::from((0, top_offset)),
            Size::from((size.w.max(100), (size.h - top_offset).max(100))),
        )
    }

    /// Resolves the fractional scale of the output a surface lives on (windows and layer
    /// surfaces on secondary outputs); falls back to the primary output.
    pub fn scale_for_surface(&self, surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface) -> f64 {
        for out in &self.additional_outputs {
            let on_window = self.output_layouts.get(&out.name()).is_some_and(|layout| {
                layout
                    .windows()
                    .iter()
                    .any(|w| crate::layout::window_wl_surface(w).as_ref() == Some(surface))
            });
            let on_layer = on_window
                || smithay::desktop::layer_map_for_output(out)
                    .layers()
                    .any(|l| l.wl_surface() == surface);
            if on_layer {
                return out.current_scale().fractional_scale();
            }
        }
        self.output.current_scale().fractional_scale()
    }

    /// Broadcasts updated fractional scale factors to all open client window surfaces
    pub fn refresh_all_surface_scales(&self) {
        let primary_scale = self.output.current_scale().fractional_scale();
        for layer in smithay::desktop::layer_map_for_output(&self.output).layers() {
            smithay::wayland::compositor::with_states(layer.wl_surface(), |states| {
                smithay::wayland::fractional_scale::with_fractional_scale(states, |fscale| {
                    fscale.set_preferred_scale(primary_scale);
                });
            });
        }
        for out in &self.additional_outputs {
            let scale = out.current_scale().fractional_scale();
            for layer in smithay::desktop::layer_map_for_output(out).layers() {
                smithay::wayland::compositor::with_states(layer.wl_surface(), |states| {
                    smithay::wayland::fractional_scale::with_fractional_scale(states, |fscale| {
                        fscale.set_preferred_scale(scale);
                    });
                });
            }
        }
        for window in self.layout.windows() {
            if let Some(surface) = crate::layout::window_wl_surface(window) {
                smithay::wayland::compositor::with_states(&surface, |states| {
                    smithay::wayland::fractional_scale::with_fractional_scale(states, |fscale| {
                        fscale.set_preferred_scale(primary_scale);
                    });
                });
            }
        }
        for (out_name, layout) in &self.output_layouts {
            let scale = self.additional_outputs.iter()
                .find(|o| o.name() == *out_name)
                .map(|o| o.current_scale().fractional_scale())
                .unwrap_or(primary_scale);
            for window in layout.windows() {
                if let Some(surface) = crate::layout::window_wl_surface(window) {
                    smithay::wayland::compositor::with_states(&surface, |states| {
                        smithay::wayland::fractional_scale::with_fractional_scale(states, |fscale| {
                            fscale.set_preferred_scale(scale);
                        });
                    });
                }
            }
        }
    }

    pub fn request_screenshot(&mut self, region: Option<Rectangle<i32, smithay::utils::Logical>>) {
        if self.accessibility.sound_feedback {
            corten_services::a11y::play_sound_cue(corten_services::a11y::SoundCue::Screenshot);
        }
        self.pending_screenshot = Some(PendingScreenshot { region });
    }

    pub fn sync_ipc_snapshot(&self) {
        if let Ok(mut guard) = self.ipc_snapshot.write() {
            let mut occupied = self.layout.occupied_workspaces();
            let mut windows = self.layout.window_snapshots();
            let mut output_active_workspaces = HashMap::new();
            let mut output_occupied_workspaces = HashMap::new();

            output_active_workspaces.insert(self.output.name(), self.active_workspace as u32);
            output_occupied_workspaces.insert(self.output.name(), occupied.clone());

            for (name, layout) in &self.output_layouts {
                let ws = self.output_workspaces.get(name).copied().unwrap_or(1) as u32;
                output_active_workspaces.insert(name.clone(), ws);
                let out_occ = layout.occupied_workspaces();
                output_occupied_workspaces.insert(name.clone(), out_occ.clone());
                for w_ws in out_occ {
                    if !occupied.contains(&w_ws) {
                        occupied.push(w_ws);
                    }
                }
                for snap in layout.window_snapshots() {
                    if !windows.iter().any(|w| w.id == snap.id) {
                        windows.push(snap);
                    }
                }
            }
            occupied.sort_unstable();
            let total_windows = windows.len();
            let a11y_snaps: Vec<corten_services::a11y::WindowA11ySnapshot> = windows
                .iter()
                .map(|w| corten_services::a11y::WindowA11ySnapshot {
                    id: w.id,
                    app_id: w.app_id.clone(),
                    title: w.title.clone(),
                    workspace: w.workspace,
                    is_active: w.is_active,
                    is_floating: w.is_floating,
                })
                .collect();
            self.a11y_bridge.sync_windows(&a11y_snaps, self.active_workspace as u32);
            guard.active_workspace = self.active_workspace as u32;
            guard.occupied_workspaces = occupied;
            guard.windows = windows;
            guard.output_active_workspaces = output_active_workspaces;
            guard.output_occupied_workspaces = output_occupied_workspaces;
            if let Some(ref tx) = self.event_broadcaster {
                let active_id = self.layout.active_window().map(crate::layout::window_stable_id).unwrap_or(0);
                let payload = format!(
                    r#"{{"event":"workspace:changed","data":{{"workspace":{},"active_window":{},"total_windows":{}}}}}"#,
                    self.active_workspace, active_id, total_windows
                );
                if let Ok(mut last) = self.last_broadcast_event.lock() {
                    if last.as_deref() != Some(&payload) {
                        let _ = tx.send(payload.clone());
                        *last = Some(payload);
                    }
                }
            }
        }
    }

    pub fn remove_window_from_all_outputs(&mut self, window: &Window) {
        self.layout.remove_window(window);
        for layout in self.output_layouts.values_mut() {
            layout.remove_window(window);
        }
        self.sync_ipc_snapshot();
    }

    pub fn minimize_window(&mut self, window: &Window) {
        self.layout.minimize_window(window);
        for layout in self.output_layouts.values_mut() {
            layout.minimize_window(window);
        }
        self.sync_ipc_snapshot();
    }

    /// Rescues all windows from a disconnected or removed output and migrates them
    /// to the primary output so windows are never orphaned or lost when unplugging displays.
    pub fn rescue_orphan_windows(&mut self, removed_output_name: &str) {
        tracing::warn!("Rescuing orphaned windows from disconnected display '{}'...", removed_output_name);

        let mut removed_layout = self.output_layouts.remove(removed_output_name);
        self.output_workspaces.remove(removed_output_name);
        self.additional_outputs.retain(|o| o.name() != removed_output_name);

        let primary_usable_area = {
            let mut map = layer_map_for_output(&self.output);
            map.arrange();
            map.non_exclusive_zone()
        };

        if let Some(mut layout) = removed_layout.take() {
            let drained = layout.drain_all_windows();
            for (ws, window, is_floating, geo) in drained {
                let (app_id, title) = crate::layout::window_metadata(&window);
                tracing::info!(
                    "Migrated orphaned window '{}' ({}) from disconnected output '{}' to primary workspace {}",
                    title,
                    app_id,
                    removed_output_name,
                    ws
                );
                self.layout.rescue_window(ws, window.clone(), is_floating, geo);
            }
        }

        // Clamp rescued floating windows to the visible bounds of the primary display
        self.layout.clamp_floating_geometries_to_area(primary_usable_area);

        // Notify client surfaces of updated geometries
        for window in self.layout.floating_windows().iter() {
            if let Some(geo) = self.layout.floating_geometry(window) {
                crate::layout::configure_window_geometry(window, geo);
            }
        }

        // OSD notification & JSON-RPC IPC event broadcast
        crate::send_osd_notification(
            "display",
            "Ekran Ayrıldı",
            &format!("Pencereler ana ekrana güvenle taşındı ({})", removed_output_name),
            100,
        );
        if let Some(ref tx) = self.event_broadcaster {
            let payload = format!(
                r#"{{"event":"output:disconnected","data":{{"output":"{}"}}}}"#,
                removed_output_name
            );
            let _ = tx.send(payload);
        }

        self.sync_ipc_snapshot();
    }

    pub fn move_active_window_between_outputs(&mut self, direction: i32) -> bool {
        let mut outputs = Vec::with_capacity(self.additional_outputs.len() + 1);
        outputs.push(self.output.clone());
        outputs.extend(self.additional_outputs.iter().cloned());
        if outputs.len() < 2 {
            return false;
        }

        let source_index = outputs.iter().position(|output| {
            self.layout_for_output(output)
                .active_window()
                .is_some_and(|window| self.layout_for_output(output).contains_window(window))
        });
        let Some(source_index) = source_index else {
            return false;
        };
        let target_index = if direction > 0 {
            (source_index + 1) % outputs.len()
        } else if source_index == 0 {
            outputs.len() - 1
        } else {
            source_index - 1
        };
        let Some(window) = self
            .layout_for_output(&outputs[source_index])
            .active_window()
            .cloned()
        else {
            return false;
        };
        self.remove_window_from_all_outputs(&window);
        self.layout_for_output_mut(&outputs[target_index])
            .add_window(window);
        true
    }

    pub fn set_gap_for_all_outputs(&mut self, gap: i32) {
        self.layout.set_gap(gap);
        for layout in self.output_layouts.values_mut() {
            layout.set_gap(gap);
        }
    }

    pub fn set_layout_mode_for_all_outputs(&mut self, mode: LayoutMode) {
        self.layout.set_mode(mode);
        for layout in self.output_layouts.values_mut() {
            layout.set_mode(mode);
        }
    }

    pub fn cycle_layout_mode_for_all_outputs(&mut self) -> LayoutMode {
        let next = self.layout.cycle_mode();
        for layout in self.output_layouts.values_mut() {
            layout.set_mode(next);
        }
        if self.accessibility.screen_reader || corten_services::a11y::is_screen_reader_enabled() {
            corten_services::a11y::announce_screen_reader(next.label());
        }
        next
    }

    pub fn adjust_master_ratio_for_all_outputs(&mut self, delta: f32) {
        self.layout.adjust_master_ratio(delta);
        for layout in self.output_layouts.values_mut() {
            layout.adjust_master_ratio(delta);
        }
    }

    pub fn toggle_floating_for_all_outputs(&mut self, window: Window) {
        self.layout.toggle_floating(window.clone());
        for layout in self.output_layouts.values_mut() {
            layout.toggle_floating(window.clone());
        }
    }

    pub fn set_split_direction_for_all_outputs(&mut self, dir: crate::layout::SplitDirection) {
        self.layout.set_split_direction(dir);
        for layout in self.output_layouts.values_mut() {
            layout.set_split_direction(dir);
        }
    }

    pub fn layout_for_output(&self, output: &Output) -> &LayoutManager {
        if output.name() == self.output.name() {
            &self.layout
        } else {
            self.output_layouts
                .get(&output.name())
                .unwrap_or(&self.layout)
        }
    }

    pub fn layout_for_output_mut(&mut self, output: &Output) -> &mut LayoutManager {
        if output.name() == self.output.name() {
            &mut self.layout
        } else {
            self.output_layouts.entry(output.name()).or_default()
        }
    }

    /// Checks if the given output has an active fullscreen window that requested asynchronous presentation (tearing).
    pub fn is_tearing_allowed_for_output(&self, output: &Output) -> bool {
        let layout = self.layout_for_output(output);
        if let Some(fs_window) = layout.active_fullscreen_window() {
            if let Some(surface) = crate::layout::window_wl_surface(&fs_window) {
                return self.tearing_state.is_tearing_requested(&surface);
            }
        }
        false
    }

    pub fn output_at(&self, point: Point<f64, smithay::utils::Logical>) -> Output {
        for output in &self.additional_outputs {
            if let Some(mode) = output.current_mode() {
                let origin = output.current_location();
                let bounds = Rectangle::new(
                    Point::from((origin.x as f64, origin.y as f64)),
                    Size::from((mode.size.w as f64, mode.size.h as f64)),
                );
                if bounds.to_f64().contains(point) {
                    return output.clone();
                }
            }
        }
        self.output.clone()
    }

    pub fn new(
        display: &mut Display<Self>,
        initial_size: Size<i32, smithay::utils::Physical>,
    ) -> Self {
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let session_lock_state = SessionLockManagerState::new::<Self, _>(&dh, |_| true);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let mut seat_state = SeatState::new();
        let seat = seat_state.new_wl_seat(&dh, "corten-seat");
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);
        let data_control_state =
            DataControlState::new::<Self, _>(&dh, Some(&primary_selection_state), |_| true);
        let cursor_shape_manager_state = CursorShapeManagerState::new::<Self>(&dh);
        let viewporter_state = ViewporterState::new::<Self>(&dh);
        let fractional_scale_manager_state = FractionalScaleManagerState::new::<Self>(&dh);
        let relative_pointer_manager_state = RelativePointerManagerState::new::<Self>(&dh);
        let pointer_constraints_state = PointerConstraintsState::new::<Self>(&dh);
        let pointer_gestures_state = PointerGesturesState::new::<Self>(&dh);
        let idle_inhibit_manager_state = IdleInhibitManagerState::new::<Self>(&dh);
        let keyboard_shortcuts_inhibit_state = KeyboardShortcutsInhibitState::new::<Self>(&dh);
        let single_pixel_buffer_state = SinglePixelBufferState::new::<Self>(&dh);
        let presentation_state = PresentationState::new::<Self>(&dh, 1); // CLOCK_MONOTONIC = 1
        let alpha_modifier_state = AlphaModifierState::new::<Self>(&dh);
        let content_type_state = ContentTypeState::new::<Self>(&dh);
        let xdg_activation_state = XdgActivationState::new::<Self>(&dh);
        let screencopy_state = ScreencopyManagerState::new::<Self>(&dh);
        let xwayland_shell_state = XWaylandShellState::new::<Self>(&dh);
        let text_input_state = smithay::wayland::text_input::TextInputManagerState::new::<Self>(&dh);
        let tablet_state = smithay::wayland::tablet_manager::TabletManagerState::new::<Self>(&dh);
        let tearing_state = crate::tearing::TearingControlState::new();
        let _tearing_global = dh.create_global::<Self, wayland_protocols::wp::tearing_control::v1::server::wp_tearing_control_manager_v1::WpTearingControlManagerV1, ()>(1, ());

        // Create virtual output for nested compositor
        let output = Output::new(
            "Corten-Output-1".to_string(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Corten Desktop".to_string(),
                model: "Virtual Display".to_string(),
            },
        );

        let mode = Mode {
            size: initial_size,
            refresh: 60_000,
        };

        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(smithay::output::Scale::Integer(1)),
            Some(Point::from((0, 0))),
        );
        output.set_preferred(mode);

        // Register global wl_output so Wayland and Layer-shell clients discover it!
        let _global = output.create_global::<Self>(&dh);

        let dmabuf_state = DmabufState::new();
        let ipc_snapshot = Arc::new(RwLock::new(CompositorIpcState {
            active_workspace: 1,
            occupied_workspaces: Vec::new(),
            windows: Vec::new(),
            output_active_workspaces: HashMap::new(),
            output_occupied_workspaces: HashMap::new(),
        }));

        Self {
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            layer_shell_state,
            layer_surfaces: Vec::new(),
            session_lock_state,
            output_manager_state,
            shm_state,
            dmabuf_state,
            dmabuf_global: None,
            syncobj_state: None,
            loop_handle: None,
            display_handle: dh.clone(),
            seat_state,
            data_device_state,
            primary_selection_state,
            data_control_state,
            cursor_shape_manager_state,
            tablet_state,
            viewporter_state,
            fractional_scale_manager_state,
            relative_pointer_manager_state,
            pointer_constraints_state,
            pointer_gestures_state,
            idle_inhibit_manager_state,
            idle_inhibiting_surfaces: Vec::new(),
            keyboard_shortcuts_inhibit_state,
            single_pixel_buffer_state,
            presentation_state,
            alpha_modifier_state,
            content_type_state,
            xdg_activation_state,
            text_input_state,
            tearing_state,
            screencopy_state,
            pending_screencopies: Vec::new(),
            pending_screenshot: None,
            xwayland_shell_state,
            xwm: None,
            xwayland_client: None,
            override_redirect_windows: Vec::new(),
            output_size: Size::from((initial_size.w, initial_size.h)),
            panel_height: 44,
            seat,
            output,
            additional_outputs: Vec::new(),
            output_layouts: HashMap::new(),
            output_workspaces: HashMap::new(),
            layout: LayoutManager::new(),
            active_workspace: 1,
            pre_show_desktop_workspace: None,
            ipc_snapshot,
            popup_manager: PopupManager::default(),
            cursor_location: Point::from((0.0, 0.0)),
            cursor_status: smithay::input::pointer::CursorImageStatus::default_named(),
            dnd_icon: None,
            window_rules: Vec::new(),
            gesture_swipe_dx: 0.0,
            gesture_swipe_dy: 0.0,
            gesture_samples: Vec::new(),
            gesture_kinetic_active: false,
            gesture_kinetic_velocity: 0.0,
            overview_mode: false,
            dragged_window: None,
            resized_window: None,
            split_drag: None,
            snap_preview: None,
            drag_split_target: None,
            selection_box: None,
            alt_tab_active: false,
            alt_tab_instant: None,
            lock_surfaces: Vec::new(),
            pending_lock: None,
            session_locked: false,
            last_input: Instant::now(),
            screen_sleep_after: None,
            suspend_after: None,
            suspend_requested: false,
            screen_asleep: false,
            night_light_enabled: false,
            vrr_enabled: false,
            allow_tearing: false,
            hdr_enabled: false,
            color_space: "srgb".to_string(),
            event_broadcaster: None,
            last_broadcast_event: std::sync::Mutex::new(None),
            title_cache: HashMap::new(),
            last_titlebar_click: None,
            accessibility: corten_config::ConfigManager::<corten_config::AccessibilityConfig>::load_or_default("accessibility")
                .map(|m| m.get().clone())
                .unwrap_or_default(),
            a11y_bridge: corten_services::a11y::AtspiBridge::new("Corten Desktop"),
        }
    }
}

pub fn resolve_corten_bin(name: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(name);
    if p.is_file() {
        return p.to_path_buf();
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    if let Ok(cur) = std::env::current_dir() {
        let candidate = cur.join(name);
        if candidate.is_file() {
            return candidate;
        }
        let rel_cand = cur.join("target/release").join(name);
        if rel_cand.is_file() {
            return rel_cand;
        }
        let target_cand = cur.join("target/debug").join(name);
        if target_cand.is_file() {
            return target_cand;
        }
    }
    let ws_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap_or(std::path::Path::new("."));
    let ws_rel = ws_root.join("target/release").join(name);
    if ws_rel.is_file() {
        return ws_rel;
    }
    let ws_deb = ws_root.join("target/debug").join(name);
    if ws_deb.is_file() {
        return ws_deb;
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&paths) {
            let candidate = path.join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    std::path::PathBuf::from(name)
}

/// Calculates kinetic velocity (px/sec) from recent gesture samples.
pub fn calculate_gesture_velocity(samples: &[(u64, f64, f64)]) -> (f64, f64) {
    if samples.len() < 2 {
        return (0.0, 0.0);
    }
    let last_time = samples.last().unwrap().0;
    // Consider samples within the last 150ms (150,000 microseconds)
    let recent: Vec<_> = samples
        .iter()
        .filter(|(t, _, _)| last_time.saturating_sub(*t) <= 150_000)
        .collect();

    if recent.len() < 2 {
        return (0.0, 0.0);
    }
    let first_time = recent.first().unwrap().0;
    let dt_usec = last_time.saturating_sub(first_time);
    if dt_usec < 5_000 {
        return (0.0, 0.0);
    }
    let dt_sec = dt_usec as f64 / 1_000_000.0;
    let sum_dx: f64 = recent.iter().skip(1).map(|(_, dx, _)| *dx).sum();
    let sum_dy: f64 = recent.iter().skip(1).map(|(_, _, dy)| *dy).sum();
    (sum_dx / dt_sec, sum_dy / dt_sec)
}

/// Steps kinetic offset and velocity with spring dampening and friction.
/// Returns (new_offset, new_velocity, is_still_active).
pub fn step_kinetic_physics(mut offset: f64, mut velocity: f64) -> (f64, f64, bool) {
    offset += velocity * 0.016;
    velocity *= 0.82;
    offset *= 0.76;

    if offset.abs() < 0.5 && velocity.abs() < 8.0 {
        (0.0, 0.0, false)
    } else {
        (offset, velocity, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::utils::{Point, Rectangle, Size};

    #[test]
    fn test_gesture_velocity_calculation() {
        // Test empty & single sample
        assert_eq!(calculate_gesture_velocity(&[]), (0.0, 0.0));
        assert_eq!(calculate_gesture_velocity(&[(100_000, 0.0, 0.0)]), (0.0, 0.0));

        // Test sample sequence: 3 updates in 30ms (30,000 usec)
        // t=100,000: start
        // t=110,000: dx=10.0, dy=5.0
        // t=120,000: dx=15.0, dy=5.0
        // t=130,000: dx=20.0, dy=10.0
        // total dx = 45.0, total dy = 20.0, dt = 30,000 usec = 0.030 s
        // velocity_x = 45.0 / 0.030 = 1500.0 px/s
        // velocity_y = 20.0 / 0.030 = 666.67 px/s
        let samples = vec![
            (100_000, 0.0, 0.0),
            (110_000, 10.0, 5.0),
            (120_000, 15.0, 5.0),
            (130_000, 20.0, 10.0),
        ];
        let (vx, vy) = calculate_gesture_velocity(&samples);
        assert!((vx - 1500.0).abs() < 1.0, "vx expected ~1500, got {vx}");
        assert!((vy - 666.67).abs() < 1.0, "vy expected ~666.67, got {vy}");

        // Test stale samples outside 150ms window are filtered out
        let samples_with_stale = vec![
            (10_000, 100.0, 100.0), // 390ms old -> discarded
            (200_000, 0.0, 0.0),
            (250_000, 25.0, 0.0),
            (300_000, 25.0, 0.0), // dt = 100ms = 0.1s, dx = 50.0 -> vx = 500 px/s
            (400_000, 50.0, 0.0), // dt = 200ms from 200k -> filtered to 250k..400k (150ms)
        ];
        let (vx_stale, _) = calculate_gesture_velocity(&samples_with_stale);
        assert!(vx_stale > 0.0);
    }

    #[test]
    fn test_kinetic_spring_convergence() {
        // Initial state: offset at 120.0px (incoming workspace), velocity 400 px/s
        let mut offset = 120.0;
        let mut velocity = 400.0;
        let mut steps = 0;

        while steps < 100 {
            let (new_off, new_vel, active) = step_kinetic_physics(offset, velocity);
            offset = new_off;
            velocity = new_vel;
            steps += 1;
            if !active {
                break;
            }
        }

        // Must converge to exactly 0.0 in fewer than 30 frames (~500ms at 60Hz)
        assert_eq!(offset, 0.0);
        assert_eq!(velocity, 0.0);
        assert!(steps < 30, "Kinetic spring took too many frames: {steps}");
    }

    #[test]
    fn test_pointer_constraint_confinement_bounds() {
        let win_rect: Rectangle<i32, smithay::utils::Logical> =
            Rectangle::new(Point::from((100, 200)), Size::from((800, 600)));
        let min_x = win_rect.loc.x as f64;
        let min_y = win_rect.loc.y as f64;
        let max_x = (win_rect.loc.x + win_rect.size.w.max(1) - 1) as f64;
        let max_y = (win_rect.loc.y + win_rect.size.h.max(1) - 1) as f64;

        let inside: Point<f64, smithay::utils::Logical> = Point::from((300.0, 400.0));
        assert_eq!(inside.x.clamp(min_x, max_x), 300.0);
        assert_eq!(inside.y.clamp(min_y, max_y), 400.0);

        let outside_left: Point<f64, smithay::utils::Logical> = Point::from((50.0, 400.0));
        assert_eq!(outside_left.x.clamp(min_x, max_x), 100.0);

        let outside_bottom: Point<f64, smithay::utils::Logical> = Point::from((300.0, 900.0));
        assert_eq!(outside_bottom.y.clamp(min_y, max_y), 799.0);
    }
}
