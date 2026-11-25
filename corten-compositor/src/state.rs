use corten_config::{LayoutMode, WindowRuleConfig};
use smithay::desktop::{PopupManager, Window, layer_map_for_output};
use smithay::input::{Seat, SeatState};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::utils::{Point, Rectangle, SERIAL_COUNTER, Size, Transform};
use smithay::wayland::alpha_modifier::AlphaModifierState;
use smithay::wayland::compositor::CompositorState;
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
    PointerConstraint, PointerConstraintsState, with_pointer_constraint,
};
use smithay::wayland::pointer_gestures::PointerGesturesState;
use smithay::wayland::presentation::PresentationState;
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::selection::wlr_data_control::DataControlState;
use smithay::wayland::session_lock::{LockSurface, SessionLockManagerState, SessionLocker};
use smithay::wayland::shell::wlr_layer::WlrLayerShellState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shm::ShmState;
use smithay::wayland::single_pixel_buffer::SinglePixelBufferState;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::xdg_activation::XdgActivationState;
use smithay::wayland::xwayland_keyboard_grab::XWaylandKeyboardGrabState;
use smithay::wayland::xwayland_shell::XWaylandShellState;
use smithay::xwayland::X11Surface;
use smithay::xwayland::xwm::X11Wm;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use wayland_server::protocol::wl_output::WlOutput;
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{Client, Display, Resource};

use crate::layout::{LayoutManager, WindowSnapshot, window_wl_surface};
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
    pub idle_notifier_state: smithay::wayland::idle_notify::IdleNotifierState<Self>,
    pub idle_event_loop:
        Option<std::sync::Mutex<smithay::reexports::calloop::EventLoop<'static, Self>>>,
    pub xdg_foreign_state: smithay::wayland::xdg_foreign::XdgForeignState,
    pub foreign_toplevel_list_state:
        smithay::wayland::foreign_toplevel_list::ForeignToplevelListState,
    pub foreign_toplevel_handles: HashMap<
        u64,
        (
            smithay::wayland::foreign_toplevel_list::ForeignToplevelHandle,
            String,
            String,
        ),
    >,
    #[allow(dead_code)]
    pub virtual_keyboard_manager_state:
        smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState,
    #[allow(dead_code)]
    pub input_method_manager_state: smithay::wayland::input_method::InputMethodManagerState,
    pub tearing_state: crate::tearing::TearingControlState,
    #[allow(dead_code)]
    pub screencopy_state: ScreencopyManagerState,
    pub pending_screencopies: Vec<PendingScreencopy>,
    pub pending_screenshot: Option<PendingScreenshot>,
    pub xwayland_shell_state: XWaylandShellState,
    pub xwayland_keyboard_grab_state: XWaylandKeyboardGrabState,
    pub xwayland_keyboard_grab_surface: Option<WlSurface>,
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
        crate::layout::ResizeEdge,
    )>,
    pub split_drag: Option<(Point<f64, smithay::utils::Logical>, f32)>,
    pub snap_preview: Option<Rectangle<i32, smithay::utils::Logical>>,
    pub snap_flyout_window: Option<u64>,
    pub drag_split_target: Option<(
        smithay::desktop::Window,
        crate::layout::SplitDirection,
        bool,
        Rectangle<i32, smithay::utils::Logical>,
    )>,
    pub selection_box: Option<(
        Point<f64, smithay::utils::Logical>,
        Point<f64, smithay::utils::Logical>,
    )>,
    pub alt_tab_active: bool,
    pub alt_tab_instant: Option<std::time::Instant>,
    pub lock_surfaces: Vec<(LockSurface, WlOutput)>,
    pub pending_lock: Option<SessionLocker>,
    pub session_locked: bool,
    pub last_lock_respawn: Option<Instant>,
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
    pub title_cache: HashMap<
        String,
        (
            smithay::backend::renderer::element::memory::MemoryRenderBuffer,
            i32,
            i32,
        ),
    >,
    pub last_titlebar_click: Option<(smithay::desktop::Window, Instant)>,
    pub accessibility: corten_config::AccessibilityConfig,
    pub a11y_bridge: corten_services::a11y::AtspiBridge,
    /// One-click Game Mode: disables blur/effects, enables tearing/VRR, inhibits idle & DND
    pub game_mode: bool,
    /// Automatically activate Game Mode / VRR while a fullscreen window is focused
    pub auto_fullscreen_game_mode: bool,
    /// True if current `game_mode` was entered automatically by fullscreen detection
    pub auto_game_mode_active: bool,
    /// Configured baseline VRR / tearing flags to restore when leaving auto fullscreen Game Mode
    pub configured_vrr: bool,
    pub configured_allow_tearing: bool,
    /// Signals udev backend to re-apply DRM `VRR_ENABLED` property on CRTCs
    pub vrr_dirty: bool,
    /// Whether any connected DRM output reports `vrr_capable == 1`
    pub vrr_hardware_capable: bool,
    /// Whether the most recent fullscreen DRM frame achieved zero-copy Primary Plane Direct Scanout
    pub last_frame_direct_scanout: bool,
    /// Last pointer movement timestamp for automatic inactivity cursor hiding during Game Mode
    pub last_pointer_motion: Instant,
    /// Power profile saved prior to entering Game Mode so it can be restored cleanly on exit
    pub saved_power_profile: Option<String>,
    /// Compositor GPU screen magnifier current animated zoom factor (`1.0` = normal, up to `4.0`)
    pub zoom_level: f64,
    /// Target GPU screen magnifier zoom factor for smooth spring interpolation
    pub target_zoom_level: f64,
    /// Native X11 DPI zero-scaling mode for razor-sharp XWayland windows on fractional HiDPI
    pub xwayland_force_zero_scaling: bool,
    /// Graphics tablet stylus pressure response curve (`"soft"`, `"linear"`, `"firm"`)
    pub tablet_pressure_curve: String,
    /// Graphics tablet 180-degree left-handed orientation mode
    pub tablet_left_handed: bool,
}

impl CortenCompositor {
    pub fn lock_surface_active(&self) -> bool {
        if self.session_locked {
            return true;
        }
        let layer_map = layer_map_for_output(&self.output);

        layer_map
            .layers()
            .any(|layer_surface| layer_surface.namespace() == "corten-lock")
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
        self.idle_notifier_state.notify_activity(&self.seat);
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
        let wayland_inhibit = self
            .seat
            .keyboard_shortcuts_inhibitor_for_surface(&focused)
            .is_some_and(|inhibitor| inhibitor.is_active());
        let xwayland_grab = self
            .xwayland_keyboard_grab_surface
            .as_ref()
            .is_some_and(|grabbed| grabbed.is_alive() && *grabbed == focused);
        wayland_inhibit || xwayland_grab
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
                        .find(|w| crate::layout::window_wl_surface(w).as_ref() == Some(&surface))
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

                    if let Some(region) = confined.region()
                        && let Some((_, ext)) = region.rects.first()
                        && ext.size.w > 0
                        && ext.size.h > 0
                    {
                        min_x = (win_rect.loc.x + ext.loc.x) as f64;
                        min_y = (win_rect.loc.y + ext.loc.y) as f64;
                        max_x = (win_rect.loc.x + ext.loc.x + ext.size.w - 1) as f64;
                        max_y = (win_rect.loc.y + ext.loc.y + ext.size.h - 1) as f64;
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
            if let Some(constraint) = constraint
                && constraint.is_active()
                && let PointerConstraint::Locked(_) = &*constraint
            {
                locked = true;
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
        if let Some(loop_mutex) = self.idle_event_loop.take()
            && let Ok(mut el) = loop_mutex.into_inner()
        {
            let _ = el.dispatch(Some(Duration::ZERO), self);
            self.idle_event_loop = Some(std::sync::Mutex::new(el));
        }
        if self.session_locked {
            // While locked, still power off displays (DPMS-off) after inactivity
            // to protect battery & OLED panels; any input wakes immediately via note_input().
            let lock_dpms_timeout = self
                .screen_sleep_after
                .map(|t| t.min(Duration::from_secs(60)))
                .unwrap_or(Duration::from_secs(60));
            self.screen_asleep = self.last_input.elapsed() >= lock_dpms_timeout;
            return false;
        }
        if self.game_mode {
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
        self.create_output_with_physical_size(name, mode, location, scale, (0, 0))
    }

    pub fn create_output_with_physical_size(
        &self,
        name: String,
        mode: Mode,
        location: Point<i32, smithay::utils::Logical>,
        scale: smithay::output::Scale,
        physical_mm: (u32, u32),
    ) -> Output {
        let output = Output::new(
            name,
            PhysicalProperties {
                size: (physical_mm.0 as i32, physical_mm.1 as i32).into(),
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
            if let Some(active) = self.layout.active_window().cloned()
                && let Some(surface) = crate::layout::window_wl_surface(&active)
            {
                let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                if let Some(kb) = self.seat.get_keyboard() {
                    kb.set_focus(self, Some(surface), serial);
                }
            }
        } else {
            self.output_workspaces
                .insert(focused_output.name(), workspace);
            if let Some(layout) = self.output_layouts.get_mut(&focused_output.name()) {
                layout.switch_workspace(workspace);
                if let Some(active) = layout.active_window().cloned()
                    && let Some(surface) = crate::layout::window_wl_surface(&active)
                {
                    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                    if let Some(kb) = self.seat.get_keyboard() {
                        kb.set_focus(self, Some(surface), serial);
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
            if let Some(active) = self.layout.active_window().cloned()
                && let Some(surface) = crate::layout::window_wl_surface(&active)
            {
                let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                if let Some(kb) = self.seat.get_keyboard() {
                    kb.set_focus(self, Some(surface), serial);
                }
            }
        } else {
            self.output_workspaces
                .insert(output_name.to_string(), workspace);
            if let Some(layout) = self.output_layouts.get_mut(output_name) {
                layout.switch_workspace(workspace);
                if let Some(active) = layout.active_window().cloned()
                    && let Some(surface) = crate::layout::window_wl_surface(&active)
                {
                    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                    if let Some(kb) = self.seat.get_keyboard() {
                        kb.set_focus(self, Some(surface), serial);
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
            if self.accessibility.screen_reader || corten_services::a11y::is_screen_reader_enabled()
            {
                corten_services::a11y::announce_screen_reader("Pencereler Geri Yüklendi");
            }
            self.switch_workspace(prev_ws);
        } else {
            let current = self.active_workspace;
            if self.accessibility.screen_reader || corten_services::a11y::is_screen_reader_enabled()
            {
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
        self.usable_area_for_output(&self.output)
    }

    /// Resolves the output that currently owns `window`, falling back to the primary output.
    pub fn output_for_window(&self, window: &Window) -> Output {
        for out in &self.additional_outputs {
            if let Some(layout) = self.output_layouts.get(&out.name())
                && layout
                    .windows()
                    .iter()
                    .chain(layout.floating_windows().iter())
                    .any(|w| w == window)
            {
                return out.clone();
            }
        }
        self.output.clone()
    }

    /// Computes the non-exclusive usable work area for a specific output.
    pub fn usable_area_for_output(
        &self,
        output: &Output,
    ) -> Rectangle<i32, smithay::utils::Logical> {
        let mut map = smithay::desktop::layer_map_for_output(output);
        map.arrange();
        let zone = map.non_exclusive_zone();
        if zone.size.w > 0 && zone.size.h > 0 {
            return zone;
        }
        let scale = output.current_scale().fractional_scale().max(0.5);
        let size = output
            .current_mode()
            .map(|m| {
                Size::from((
                    (m.size.w as f64 / scale).round() as i32,
                    (m.size.h as f64 / scale).round() as i32,
                ))
            })
            .unwrap_or(self.output_size);
        let top_offset = self.panel_height.max(0);
        Rectangle::new(
            Point::from((0, top_offset)),
            Size::from((size.w.max(100), (size.h - top_offset).max(100))),
        )
    }

    /// Computes the usable work area on the output where `window` currently lives.
    pub fn usable_area_for_window(
        &self,
        window: &Window,
    ) -> Rectangle<i32, smithay::utils::Logical> {
        let out = self.output_for_window(window);
        self.usable_area_for_output(&out)
    }

    /// Clears any active interactive drag/resize grab referencing `window` (e.g. on destroy).
    pub fn clear_window_grabs(&mut self, window: &Window) {
        if self
            .dragged_window
            .as_ref()
            .is_some_and(|(w, _, _)| w == window)
        {
            self.dragged_window = None;
            self.snap_preview = None;
            self.drag_split_target = None;
        }
        if self
            .resized_window
            .as_ref()
            .is_some_and(|(w, _, _, _)| w == window)
        {
            self.resized_window = None;
        }
    }

    /// Prunes dead lock surfaces and respawns `corten-lock` (rate-limited to once every 2s)
    /// if the locker crashed while the session is locked, preventing the user from being stranded.
    pub fn recover_crashed_lock(&mut self) {
        if !self.session_locked {
            return;
        }
        self.lock_surfaces.retain(|(s, _)| s.alive());
        if self.lock_surfaces.is_empty() && self.pending_lock.is_none() {
            let should_respawn = self
                .last_lock_respawn
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(2));
            if should_respawn {
                self.last_lock_respawn = Some(Instant::now());
                let lock_bin = resolve_corten_bin("corten-lock");
                tracing::warn!(
                    "Compositor: ext-session-lock surface died while locked; respawning {:?}",
                    lock_bin
                );
                let _ = std::process::Command::new(lock_bin).spawn();
            }
        }
    }

    /// Resolves the fractional scale of the output a surface lives on (windows and layer
    /// surfaces on secondary outputs); falls back to the primary output.
    pub fn scale_for_surface(
        &self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) -> f64 {
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
            let scale = self
                .additional_outputs
                .iter()
                .find(|o| o.name() == *out_name)
                .map(|o| o.current_scale().fractional_scale())
                .unwrap_or(primary_scale);
            for window in layout.windows() {
                if let Some(surface) = crate::layout::window_wl_surface(window) {
                    smithay::wayland::compositor::with_states(&surface, |states| {
                        smithay::wayland::fractional_scale::with_fractional_scale(
                            states,
                            |fscale| {
                                fscale.set_preferred_scale(scale);
                            },
                        );
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
            self.a11y_bridge
                .sync_windows(&a11y_snaps, self.active_workspace as u32);
            guard.active_workspace = self.active_workspace as u32;
            guard.occupied_workspaces = occupied;
            guard.windows = windows;
            guard.output_active_workspaces = output_active_workspaces;
            guard.output_occupied_workspaces = output_occupied_workspaces;
            if let Some(ref tx) = self.event_broadcaster {
                let active_id = self
                    .layout
                    .active_window()
                    .map(crate::layout::window_stable_id)
                    .unwrap_or(0);
                let payload = format!(
                    r#"{{"event":"workspace:changed","data":{{"workspace":{},"active_window":{},"total_windows":{}}}}}"#,
                    self.active_workspace, active_id, total_windows
                );
                if let Ok(mut last) = self.last_broadcast_event.lock()
                    && last.as_deref() != Some(&payload)
                {
                    let _ = tx.send(payload.clone());
                    *last = Some(payload);
                }
            }
        }
    }

    pub fn remove_window_from_all_outputs(&mut self, window: &Window) {
        let wid = crate::layout::window_stable_id(window);
        if let Some((handle, _, _)) = self.foreign_toplevel_handles.remove(&wid) {
            handle.send_closed();
            self.foreign_toplevel_list_state.remove_toplevel(&handle);
        }
        self.layout.remove_window(window);
        for layout in self.output_layouts.values_mut() {
            layout.remove_window(window);
        }
        if !self.layout.windows().is_empty() || !self.layout.floating_windows().is_empty() {
            let _ = self.save_window_session_to_disk();
        }
        self.sync_ipc_snapshot();
    }

    /// Synchronizes open windows with `ext-foreign-toplevel-list-v1` protocol handles.
    pub fn sync_foreign_toplevels(&mut self) {
        let mut snaps = self.layout.window_snapshots();
        for layout in self.output_layouts.values() {
            for s in layout.window_snapshots() {
                if !snaps.iter().any(|w| w.id == s.id) {
                    snaps.push(s);
                }
            }
        }
        for snap in snaps {
            if snap.app_id.is_empty() && snap.title.is_empty() {
                continue;
            }
            let eff_title = if snap.title.is_empty() {
                "Corten Window"
            } else {
                &snap.title
            };
            let eff_app = if snap.app_id.is_empty() {
                "corten"
            } else {
                &snap.app_id
            };
            if let Some((handle, last_app, last_title)) =
                self.foreign_toplevel_handles.get_mut(&snap.id)
            {
                let mut changed = false;
                if last_title != eff_title {
                    handle.send_title(eff_title);
                    *last_title = eff_title.to_string();
                    changed = true;
                }
                if last_app != eff_app {
                    handle.send_app_id(eff_app);
                    *last_app = eff_app.to_string();
                    changed = true;
                }
                if changed {
                    handle.send_done();
                }
            } else {
                let handle = self
                    .foreign_toplevel_list_state
                    .new_toplevel::<Self>(eff_title.to_string(), eff_app.to_string());
                self.foreign_toplevel_handles.insert(
                    snap.id,
                    (handle, eff_app.to_string(), eff_title.to_string()),
                );
            }
        }
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
        tracing::warn!(
            "Rescuing orphaned windows from disconnected display '{}'...",
            removed_output_name
        );

        let mut removed_layout = self.output_layouts.remove(removed_output_name);
        self.output_workspaces.remove(removed_output_name);
        self.additional_outputs
            .retain(|o| o.name() != removed_output_name);

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
                self.layout
                    .rescue_window(ws, window.clone(), is_floating, geo);
            }
        }

        // Clamp rescued floating windows to the visible bounds of the primary display
        self.layout
            .clamp_floating_geometries_to_area(primary_usable_area);

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
            &format!(
                "Pencereler ana ekrana güvenle taşındı ({})",
                removed_output_name
            ),
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
        if let Some(fs_window) = layout.active_fullscreen_window()
            && let Some(surface) = crate::layout::window_wl_surface(&fs_window)
        {
            return self.tearing_state.is_tearing_requested(&surface);
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
        let xwayland_keyboard_grab_state = XWaylandKeyboardGrabState::new::<Self>(&dh);
        let text_input_state =
            smithay::wayland::text_input::TextInputManagerState::new::<Self>(&dh);
        let idle_event_loop = smithay::reexports::calloop::EventLoop::<Self>::try_new()
            .expect("calloop EventLoop for IdleNotifierState");
        let idle_notifier_state =
            smithay::wayland::idle_notify::IdleNotifierState::new(&dh, idle_event_loop.handle());
        let xdg_foreign_state = smithay::wayland::xdg_foreign::XdgForeignState::new::<Self>(&dh);
        let foreign_toplevel_list_state =
            smithay::wayland::foreign_toplevel_list::ForeignToplevelListState::new::<Self>(&dh);
        let virtual_keyboard_manager_state =
            smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState::new::<Self, _>(
                &dh,
                |_| true,
            );
        let input_method_manager_state =
            smithay::wayland::input_method::InputMethodManagerState::new::<Self, _>(&dh, |_| true);
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
            idle_notifier_state,
            idle_event_loop: Some(std::sync::Mutex::new(idle_event_loop)),
            xdg_foreign_state,
            foreign_toplevel_list_state,
            foreign_toplevel_handles: HashMap::new(),
            virtual_keyboard_manager_state,
            input_method_manager_state,
            tearing_state,
            screencopy_state,
            pending_screencopies: Vec::new(),
            pending_screenshot: None,
            xwayland_shell_state,
            xwayland_keyboard_grab_state,
            xwayland_keyboard_grab_surface: None,
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
            snap_flyout_window: None,
            drag_split_target: None,
            selection_box: None,
            alt_tab_active: false,
            alt_tab_instant: None,
            lock_surfaces: Vec::new(),
            pending_lock: None,
            session_locked: false,
            last_lock_respawn: None,
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
            accessibility:
                corten_config::ConfigManager::<corten_config::AccessibilityConfig>::load_or_default(
                    "accessibility",
                )
                .map(|m| m.get().clone())
                .unwrap_or_default(),
            a11y_bridge: corten_services::a11y::AtspiBridge::new("Corten Desktop"),
            game_mode: false,
            auto_fullscreen_game_mode: true,
            auto_game_mode_active: false,
            configured_vrr: false,
            configured_allow_tearing: false,
            vrr_dirty: false,
            vrr_hardware_capable: false,
            last_frame_direct_scanout: false,
            last_pointer_motion: Instant::now(),
            saved_power_profile: None,
            zoom_level: 1.0,
            target_zoom_level: 1.0,
            xwayland_force_zero_scaling: corten_config::ConfigManager::<
                corten_config::DisplaySettingsConfig,
            >::load_or_default("displays")
            .map(|m| m.get().xwayland_force_zero_scaling)
            .unwrap_or(true),
            tablet_pressure_curve:
                corten_config::ConfigManager::<corten_config::InputSettingsConfig>::load_or_default(
                    "input",
                )
                .map(|m| m.get().tablet_pressure_curve.clone())
                .unwrap_or_else(|_| "linear".to_string()),
            tablet_left_handed:
                corten_config::ConfigManager::<corten_config::InputSettingsConfig>::load_or_default(
                    "input",
                )
                .map(|m| m.get().tablet_left_handed)
                .unwrap_or(false),
        }
    }

    /// Records pointer activity so inactivity cursor hiding resets immediately on mouse movement.
    pub fn note_pointer_motion(&mut self) {
        self.last_pointer_motion = Instant::now();
    }

    /// Hides the cursor automatically after 4 seconds of pointer inactivity during Game Mode
    /// (prevents a stray mouse pointer from sitting in the middle of the screen during gamepad play).
    pub fn should_hide_idle_cursor(&self) -> bool {
        self.game_mode && self.last_pointer_motion.elapsed() >= Duration::from_secs(4)
    }

    /// Releases GPU VRAM held by local AI assistant (`corten-ai`) when entering Game Mode
    /// if `auto_gpu_release_on_game` is enabled in `ai.toml`.
    fn release_ai_vram_if_configured() {
        std::thread::spawn(|| {
            let should_release =
                corten_config::ConfigManager::<corten_config::AiSettingsConfig>::load_or_default(
                    "ai",
                )
                .map(|c| c.get().auto_gpu_release_on_game)
                .unwrap_or(true);
            if should_release {
                let _ = std::process::Command::new("pkill")
                    .args(["-TERM", "-x", "corten-ai"])
                    .status();
            }
        });
    }

    /// Switches `power-profiles-daemon` to `performance` when entering Game Mode and restores
    /// the previous profile on exit (executed on a detached thread so compositor never blocks).
    fn apply_game_mode_power_profile(&mut self, entering: bool) {
        if entering {
            Self::release_ai_vram_if_configured();
            if self.saved_power_profile.is_none() {
                let fallback = corten_config::ConfigManager::<corten_config::PowerSettingsConfig>::load_or_default("power")
                    .map(|c| c.get().power_profile.clone())
                    .unwrap_or_else(|_| "balanced".to_string());
                self.saved_power_profile = Some(fallback);
            }
            std::thread::spawn(|| {
                let _ = corten_services::power_profiles::set_active_profile("performance");
            });
        } else if let Some(prev) = self.saved_power_profile.take() {
            std::thread::spawn(move || {
                let _ = corten_services::power_profiles::set_active_profile(&prev);
            });
        }
    }

    /// Toggles One-Click Game Mode (disables blur, enables VRR/tearing, toggles DND & notifies user).
    pub fn toggle_game_mode(&mut self) -> bool {
        self.auto_game_mode_active = false;
        self.game_mode = !self.game_mode;
        self.allow_tearing = if self.game_mode {
            true
        } else {
            self.configured_allow_tearing
        };
        self.vrr_enabled = if self.game_mode {
            true
        } else {
            self.configured_vrr
        };
        self.vrr_dirty = true;
        self.apply_game_mode_power_profile(self.game_mode);
        let _ = std::process::Command::new("busctl")
            .args([
                "--user",
                "call",
                "org.freedesktop.Notifications",
                "/org/freedesktop/Notifications",
                "org.freedesktop.Notifications",
                "SetDoNotDisturb",
                "b",
                if self.game_mode { "true" } else { "false" },
            ])
            .spawn();
        let msg = if self.game_mode {
            "Oyun Modu Aktif (VRR + Tearing + Blur Bypass + DND)"
        } else {
            "Oyun Modu Kapatıldı (Normal Masaüstü Modu)"
        };
        crate::render::send_panel_toast(if self.game_mode {
            "Oyun Modu: Açık"
        } else {
            "Oyun Modu: Kapalı"
        });
        let _ = std::process::Command::new("notify-send")
            .args(["Corten Game Mode", msg])
            .spawn();
        self.game_mode
    }

    /// Returns `true` when the given surface explicitly advertises `wp_content_type_v1::Type::Game`
    /// (used by Vulkan/DXVK/Proton/native Wayland games even in borderless-windowed mode).
    pub fn surface_is_game_content(surface: &WlSurface) -> bool {
        use smithay::reexports::wayland_protocols::wp::content_type::v1::server::wp_content_type_v1::Type as WpContentType;
        use smithay::wayland::compositor::with_states;
        use smithay::wayland::content_type::ContentTypeSurfaceCachedState;
        with_states(surface, |states| {
            let mut guard = states.cached_state.get::<ContentTypeSurfaceCachedState>();
            *guard.current().content_type() == WpContentType::Game
        })
    }

    /// Recognizes common Steam/Proton/Gamescope/emulator game `app_id` patterns so games that do
    /// not emit `wp_content_type_v1::Type::Game` still trigger Auto Game Mode cleanly.
    pub fn is_known_game_app_id(app_id: &str) -> bool {
        let lower = app_id.trim().to_ascii_lowercase();
        if lower.is_empty() {
            return false;
        }
        lower.starts_with("steam_app_")
            || lower == "gamescope"
            || lower == "cs2"
            || lower == "dota2"
            || lower.contains("minecraft")
            || lower.ends_with(".exe")
            || matches!(
                lower.as_str(),
                "retroarch" | "rpcs3" | "dolphin-emu" | "pcsx2" | "ryujinx" | "cemu"
            )
    }

    /// Returns `true` if the currently focused window advertises `ContentType::Game`
    /// or matches a known Steam/Proton/emulator game `app_id`.
    pub fn active_window_is_game_content(&self) -> bool {
        let Some(active) = self.layout.active_window() else {
            return false;
        };
        let by_protocol = crate::layout::window_wl_surface(active)
            .is_some_and(|surf| Self::surface_is_game_content(&surf));
        if by_protocol {
            return true;
        }
        let (app_id, _) = crate::layout::window_metadata(active);
        Self::is_known_game_app_id(&app_id)
    }

    /// Automatically enables Game Mode (VRR + tearing + blur bypass + idle inhibit) while a
    /// fullscreen window or a `wp_content_type_v1::Type::Game` surface is active when
    /// `auto_fullscreen_game_mode` is enabled, and restores the user's configured baseline when
    /// leaving (never overriding manual `Super+G`).
    pub fn sync_fullscreen_game_mode(&mut self, has_fullscreen: bool) {
        if !self.auto_fullscreen_game_mode {
            return;
        }
        let game_active = has_fullscreen || self.active_window_is_game_content();
        if game_active && !self.game_mode {
            self.game_mode = true;
            self.auto_game_mode_active = true;
            self.vrr_enabled = true;
            self.allow_tearing = true;
            self.vrr_dirty = true;
            self.apply_game_mode_power_profile(true);
            tracing::info!(
                has_fullscreen,
                content_type_game = self.active_window_is_game_content(),
                "Auto Game Mode activated (VRR + tearing + blur bypass)"
            );
        } else if !game_active && self.auto_game_mode_active {
            self.game_mode = false;
            self.auto_game_mode_active = false;
            self.vrr_enabled = self.configured_vrr;
            self.allow_tearing = self.configured_allow_tearing;
            self.vrr_dirty = true;
            self.apply_game_mode_power_profile(false);
            tracing::info!("Auto Game Mode deactivated (restored baseline display settings)");
        }
    }

    /// Returns a human-readable multi-line summary of live compositor gaming readiness.
    pub fn gaming_status_report(&self) -> String {
        let mode = if !self.game_mode {
            "off"
        } else if self.auto_game_mode_active {
            "auto-fullscreen"
        } else {
            "manual"
        };
        let xkb_grab = self
            .xwayland_keyboard_grab_surface
            .as_ref()
            .is_some_and(|s| s.is_alive());
        format!(
            "explicit_sync={}\ndirect_scanout={}\nvrr_capable={}\nvrr_enabled={}\ntearing_allowed={}\ncontent_type_game={}\nxwayland_keyboard_grab_protocol=enabled\nxwayland_keyboard_grab_active={}\ngame_mode={}\nauto_fullscreen_game_mode={}",
            self.syncobj_state.is_some(),
            self.last_frame_direct_scanout,
            self.vrr_hardware_capable,
            self.vrr_enabled,
            self.allow_tearing,
            self.active_window_is_game_content(),
            xkb_grab,
            mode,
            self.auto_fullscreen_game_mode,
        )
    }

    /// Dispatches a user-configured 3-finger or 4-finger touchpad swipe action.
    pub fn dispatch_configured_swipe_action(&mut self, action: &str, go_next_or_up: bool) {
        match action {
            "overview" => {
                self.overview_mode = !self.overview_mode;
            }
            "maximize" => {
                if let Some(active) = self.layout.active_window().cloned() {
                    let is_fs = self.layout.toggle_fullscreen(&active);
                    if let Some(toplevel) = active.toplevel() {
                        toplevel.with_pending_state(|tstate| {
                            use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
                            if is_fs {
                                tstate.states.set(State::Fullscreen);
                            } else {
                                tstate.states.unset(State::Fullscreen);
                            }
                        });
                        toplevel.send_configure();
                    }
                }
            }
            "dashboard" => {
                let _ = std::process::Command::new("busctl")
                    .args([
                        "--user",
                        "call",
                        "org.corten.Panel",
                        "/org/corten/Panel",
                        "org.corten.Panel",
                        "TriggerModule",
                        "s",
                        "desktop_hub",
                    ])
                    .spawn();
            }
            "game-mode" => {
                self.toggle_game_mode();
            }
            "scrollable-strip" => {
                let usable = self.usable_area();
                self.layout
                    .scroll_column_step(if go_next_or_up { 1 } else { -1 }, usable);
            }
            _ => {
                let target_ws = if go_next_or_up {
                    (self.active_workspace + 1).min(9)
                } else {
                    self.active_workspace.saturating_sub(1).max(1)
                };
                if target_ws != self.active_workspace {
                    self.switch_workspace(target_ws);
                }
            }
        }
    }

    /// Adjusts GPU compositor zoom magnification (`1.0` to `4.0`) with smooth spring target.
    pub fn adjust_zoom(&mut self, delta: f64) -> f64 {
        self.target_zoom_level = (self.target_zoom_level + delta).clamp(1.0, 4.0);
        self.zoom_level = crate::render::step_smooth_zoom(self.zoom_level, self.target_zoom_level);
        crate::render::send_panel_toast(&format!(
            "Yakınlaştırma: %{}",
            (self.target_zoom_level * 100.0).round() as u32
        ));
        self.target_zoom_level
    }

    /// Resets compositor zoom back to `1.0`.
    pub fn reset_zoom(&mut self) {
        self.target_zoom_level = 1.0;
        self.zoom_level = 1.0;
        crate::render::send_panel_toast("Yakınlaştırma: %100");
    }

    /// Steps smooth per-frame compositor visual animations (zoom & scrollable strip).
    pub fn step_visual_animations(&mut self) {
        self.zoom_level = crate::render::step_smooth_zoom(self.zoom_level, self.target_zoom_level);
        self.layout.step_scroll_animation();
    }

    /// Computes physical-pixel viewport pan offset `(pan_x, pan_y)` for cursor-following GPU magnification.
    /// Returns `(0, 0)` when `zoom_level <= 1.001` so standard desktop rendering incurs zero shift.
    pub fn zoom_pan_offset(
        &self,
        output_phys_size: Size<i32, smithay::utils::Physical>,
        base_scale: f64,
    ) -> Point<i32, smithay::utils::Physical> {
        compute_zoom_pan_offset(
            self.cursor_location,
            output_phys_size,
            base_scale,
            self.zoom_level,
        )
    }

    /// Executes a named compositor action triggered via `cortenctl action <name>` or hotkeys.
    pub fn handle_custom_action(&mut self, action: &str) {
        let usable = self.usable_area();
        match action {
            "toggle-scratchpad" => {
                if let Some(win) = self.layout.toggle_scratchpad(usable)
                    && let Some(surface) = crate::layout::window_wl_surface(&win)
                {
                    let serial = SERIAL_COUNTER.next_serial();
                    if let Some(kb) = self.seat.get_keyboard() {
                        kb.set_focus(self, Some(surface), serial);
                    }
                }
            }
            "move-to-scratchpad" => {
                if let Some(active) = self.layout.active_window().cloned() {
                    self.layout.move_to_scratchpad(&active);
                }
            }
            "toggle-sticky" => {
                if let Some(active) = self.layout.active_window().cloned() {
                    let pinned = self.layout.toggle_sticky(&active);
                    let msg = if pinned {
                        "Pencere tüm çalışma alanlarına sabitlendi (Sticky / PiP)"
                    } else {
                        "Pencere sabitlemesi kaldırıldı"
                    };
                    let _ = std::process::Command::new("notify-send")
                        .args(["Corten Desktop", msg])
                        .spawn();
                }
            }
            "toggle-game-mode" => {
                self.toggle_game_mode();
            }
            "gaming-status" => {
                let report = self.gaming_status_report();
                if let Ok(home) = std::env::var("HOME") {
                    let dir = std::path::PathBuf::from(home).join(".cache/corten");
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(dir.join("gaming-status.txt"), &report);
                }
                tracing::info!("Gaming status snapshot:\n{}", report);
            }
            "scroll-left" => {
                self.layout.scroll_column_step(-1, usable);
            }
            "scroll-right" => {
                self.layout.scroll_column_step(1, usable);
            }
            "cycle-column-width" => {
                let frac = self.layout.cycle_scroll_column_fraction();
                for (w, r) in self.layout.arrange(usable) {
                    crate::layout::configure_window_geometry(&w, r);
                }
                crate::render::send_panel_toast(&format!(
                    "Sütun Genişliği: %{}",
                    (frac * 100.0).round() as u32
                ));
            }
            "zoom-in" => {
                self.adjust_zoom(0.25);
            }
            "zoom-out" => {
                self.adjust_zoom(-0.25);
            }
            "zoom-reset" => {
                self.reset_zoom();
            }
            "cycle-layout" => {
                let next = self.layout.cycle_mode();
                for (w, r) in self.layout.arrange(usable) {
                    crate::layout::configure_window_geometry(&w, r);
                }
                let _ = std::process::Command::new("notify-send")
                    .args(["Corten Yerleşim Modu", next.label()])
                    .spawn();
            }
            "set-layout-scrollable" => {
                self.layout.set_mode(corten_config::LayoutMode::Scrollable);
                for (w, r) in self.layout.arrange(usable) {
                    crate::layout::configure_window_geometry(&w, r);
                }
            }
            "color-picker" => {
                let x = self.cursor_location.x.round() as i32;
                let y = self.cursor_location.y.round() as i32;
                // Offload grim + wl-copy off the compositor main thread to prevent
                // Wayland screencopy deadlock!
                std::thread::spawn(move || {
                    let (r, g, b) = sample_screen_pixel_at(x, y);
                    let (hex, rgb_str) = format_color_hex_rgb(r, g, b);
                    if let Ok(mut child) = std::process::Command::new("wl-copy")
                        .stdin(std::process::Stdio::piped())
                        .spawn()
                    {
                        if let Some(mut stdin) = child.stdin.take() {
                            use std::io::Write;
                            let _ = stdin.write_all(hex.as_bytes());
                        }
                        let _ = child.wait();
                    }
                    let msg = format!("{} — {} panoya kopyalandı ({}, {})", hex, rgb_str, x, y);
                    let _ = std::process::Command::new("notify-send")
                        .args(["Corten Renk Seçici", &msg])
                        .spawn();
                });
            }
            "save-session" => {
                if let Ok(path) = self.save_window_session_to_disk() {
                    let msg = format!("Oturum pencere konumları kaydedildi ({})", path.display());
                    let _ = std::process::Command::new("notify-send")
                        .args(["Corten Oturum Yöneticisi", &msg])
                        .spawn();
                }
            }
            "restore-session" => {
                let count = self.apply_restored_session();
                let msg = format!("{} pencerenin çalışma alanı ve konumu geri yüklendi", count);
                let _ = std::process::Command::new("notify-send")
                    .args(["Corten Oturum Yöneticisi", &msg])
                    .spawn();
            }
            "toggle-xwayland-zero-scale" => {
                self.xwayland_force_zero_scaling = !self.xwayland_force_zero_scaling;
                let msg = if self.xwayland_force_zero_scaling {
                    "XWayland Keskin Ölçekleme (Force Zero Scaling) Aktif"
                } else {
                    "XWayland Standart Kompozitör Ölçeklemesi Aktif"
                };
                let _ = std::process::Command::new("notify-send")
                    .args(["Corten XWayland", msg])
                    .spawn();
            }
            "prepare-sleep" => {
                self.handle_prepare_for_sleep(true);
            }
            "resume-wake" => {
                self.handle_prepare_for_sleep(false);
            }
            _ => {
                tracing::debug!("Compositor: unhandled custom action '{}'", action);
            }
        }
        self.sync_ipc_snapshot();
    }

    /// Handles `org.freedesktop.login1.Manager.PrepareForSleep(going_to_sleep)` lifecycle:
    /// locks session prior to suspend and recovers DRM/output scales & input timers upon resume.
    pub fn handle_prepare_for_sleep(&mut self, going_to_sleep: bool) {
        if going_to_sleep {
            tracing::info!(
                "Compositor: PrepareForSleep(true) — locking session and saving window layout before suspend"
            );
            let _ = self.save_window_session_to_disk();
            if !self.session_locked {
                let lock_bin = resolve_corten_bin("corten-lock");
                let _ = std::process::Command::new(lock_bin).spawn();
            }
        } else {
            tracing::info!(
                "Compositor: PrepareForSleep(false) — system resumed; waking outputs and refreshing surface scales"
            );
            self.note_input();
            self.refresh_all_surface_scales();
            let usable = self.usable_area();
            for (w, r) in self.layout.arrange(usable) {
                crate::layout::configure_window_geometry(&w, r);
            }
            self.sync_ipc_snapshot();
        }
    }

    /// Captures all currently open windows' workspace and geometry into a serializable snapshot.
    pub fn capture_session_snapshot(&self) -> Vec<SavedWindowEntry> {
        let mut entries = Vec::new();
        for snap in self.layout.window_snapshots() {
            if snap.app_id.is_empty() {
                continue;
            }
            let win_opt = self
                .layout
                .windows()
                .iter()
                .chain(self.layout.floating_windows().iter())
                .find(|w| crate::layout::window_stable_id(w) == snap.id);
            let rect = win_opt
                .and_then(|w| self.layout.floating_geometry(w))
                .unwrap_or_else(|| Rectangle::new(Point::from((120, 90)), Size::from((960, 640))));
            entries.push(SavedWindowEntry {
                app_id: snap.app_id,
                title: snap.title,
                workspace: snap.workspace,
                is_floating: snap.is_floating,
                x: rect.loc.x,
                y: rect.loc.y,
                width: rect.size.w,
                height: rect.size.h,
            });
        }
        entries
    }

    /// Saves the active window session layout to `~/.config/corten/window_session.toml`.
    pub fn save_window_session_to_disk(&self) -> std::io::Result<std::path::PathBuf> {
        let entries = self.capture_session_snapshot();
        let path = window_session_file_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let body = serialize_window_session(&entries);
        std::fs::write(&path, body)?;
        Ok(path)
    }

    /// Applies saved workspace and floating geometry rules from `~/.config/corten/window_session.toml`
    /// to currently open windows.
    pub fn apply_restored_session(&mut self) -> usize {
        let saved = load_window_session_from_disk();
        if saved.is_empty() {
            return 0;
        }
        let mut restored = 0usize;
        let all_wins: Vec<Window> = self
            .layout
            .windows()
            .iter()
            .chain(self.layout.floating_windows().iter())
            .cloned()
            .collect();
        for win in all_wins {
            let (app_id, _) = crate::layout::window_metadata(&win);
            if let Some(entry) = saved.iter().find(|e| e.app_id == app_id) {
                if entry.is_floating {
                    let rect = Rectangle::new(
                        Point::from((entry.x, entry.y)),
                        Size::from((entry.width.max(160), entry.height.max(120))),
                    );
                    self.layout.set_floating_geometry(&win, rect);
                    crate::layout::configure_window_geometry(&win, rect);
                }
                if (1..=9).contains(&entry.workspace) {
                    self.layout.set_active_window(&win);
                    let _ = self.layout.move_active_to_workspace(entry.workspace);
                }
                restored += 1;
            }
        }
        restored
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedWindowEntry {
    pub app_id: String,
    pub title: String,
    pub workspace: usize,
    pub is_floating: bool,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

pub fn window_session_file_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
    base.join("corten").join("window_session.tsv")
}

pub fn serialize_window_session(entries: &[SavedWindowEntry]) -> String {
    let mut out = String::new();
    for e in entries {
        let safe_app = e.app_id.replace(['\t', '\n'], " ");
        let safe_title = e.title.replace(['\t', '\n'], " ");
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            safe_app,
            safe_title,
            e.workspace,
            if e.is_floating { 1 } else { 0 },
            e.x,
            e.y,
            e.width,
            e.height
        ));
    }
    out
}

pub fn parse_window_session(content: &str) -> Vec<SavedWindowEntry> {
    let mut list = Vec::new();
    for line in content.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() == 8
            && let (Ok(ws), Ok(fl), Ok(x), Ok(y), Ok(w), Ok(h)) = (
                parts[2].parse::<usize>(),
                parts[3].parse::<u8>(),
                parts[4].parse::<i32>(),
                parts[5].parse::<i32>(),
                parts[6].parse::<i32>(),
                parts[7].parse::<i32>(),
            )
        {
            list.push(SavedWindowEntry {
                app_id: parts[0].to_string(),
                title: parts[1].to_string(),
                workspace: ws,
                is_floating: fl != 0,
                x,
                y,
                width: w,
                height: h,
            });
        }
    }
    list
}

pub fn load_window_session_from_disk() -> Vec<SavedWindowEntry> {
    let path = window_session_file_path();
    std::fs::read_to_string(path)
        .map(|s| parse_window_session(&s))
        .unwrap_or_default()
}

/// Computes smooth cursor-following viewport pan offset in physical pixels when GPU magnifier zoom > 1.0.
/// Clamps pan strictly within `[output_dim * (1 - zoom), 0]` so the viewport never exposes black edges.
pub fn compute_zoom_pan_offset(
    cursor_logical: Point<f64, smithay::utils::Logical>,
    output_phys_size: Size<i32, smithay::utils::Physical>,
    base_scale: f64,
    zoom_level: f64,
) -> Point<i32, smithay::utils::Physical> {
    let z = zoom_level.clamp(1.0, 4.0);
    if z <= 1.001 {
        return Point::from((0, 0));
    }
    let out_w = output_phys_size.w.max(1) as f64;
    let out_h = output_phys_size.h.max(1) as f64;
    let zoomed_scale = base_scale.max(0.5) * z;
    let min_x = out_w * (1.0 - z);
    let min_y = out_h * (1.0 - z);
    let pan_x = (out_w * 0.5 - cursor_logical.x * zoomed_scale).clamp(min_x, 0.0);
    let pan_y = (out_h * 0.5 - cursor_logical.y * zoomed_scale).clamp(min_y, 0.0);
    Point::from((pan_x.round() as i32, pan_y.round() as i32))
}

/// Scales and translates a logical window rectangle for GLSL shadow/glow shader alignment under zoom.
pub fn compute_zoomed_glow_rect(
    rect: Rectangle<i32, smithay::utils::Logical>,
    zoom_level: f64,
    pan_phys: Point<i32, smithay::utils::Physical>,
    base_scale: f64,
) -> Rectangle<i32, smithay::utils::Logical> {
    let z = zoom_level.clamp(1.0, 4.0);
    if z <= 1.001 {
        return rect;
    }
    let inv_base = 1.0 / base_scale.max(0.5);
    let pan_log_x = (pan_phys.x as f64 * inv_base).round() as i32;
    let pan_log_y = (pan_phys.y as f64 * inv_base).round() as i32;
    Rectangle::new(
        Point::from((
            (rect.loc.x as f64 * z).round() as i32 + pan_log_x,
            (rect.loc.y as f64 * z).round() as i32 + pan_log_y,
        )),
        Size::from((
            (rect.size.w as f64 * z).round() as i32,
            (rect.size.h as f64 * z).round() as i32,
        )),
    )
}

/// Formats an RGB triplet into `("#RRGGBB", "rgb(R, G, B)")`.
pub fn format_color_hex_rgb(r: u8, g: u8, b: u8) -> (String, String) {
    (
        format!("#{:02X}{:02X}{:02X}", r, g, b),
        format!("rgb({}, {}, {})", r, g, b),
    )
}

/// Parses a 1x1 binary (P6) or ASCII (P3) PPM image produced by `grim -g "X,Y 1x1" -t ppm -`.
pub fn parse_ppm_pixel(ppm_bytes: &[u8]) -> Option<(u8, u8, u8)> {
    if ppm_bytes.starts_with(b"P6") && ppm_bytes.len() >= 3 {
        let n = ppm_bytes.len();
        return Some((ppm_bytes[n - 3], ppm_bytes[n - 2], ppm_bytes[n - 1]));
    }
    if ppm_bytes.starts_with(b"P3") {
        let text = std::str::from_utf8(ppm_bytes).ok()?;
        let tokens: Vec<u8> = text
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .flat_map(|l| l.split_whitespace())
            .skip(4) // P3, width, height, maxval
            .filter_map(|t| t.parse::<u8>().ok())
            .collect();
        if tokens.len() >= 3 {
            return Some((tokens[0], tokens[1], tokens[2]));
        }
    }
    None
}

/// Samples the pixel color at screen coordinates `(x, y)` via `grim` 1x1 PPM capture.
pub fn sample_screen_pixel_at(x: i32, y: i32) -> (u8, u8, u8) {
    let geom = format!("{},{} 1x1", x.max(0), y.max(0));
    if let Ok(out) = std::process::Command::new("grim")
        .args(["-g", &geom, "-t", "ppm", "-"])
        .output()
        && out.status.success()
        && let Some(rgb) = parse_ppm_pixel(&out.stdout)
    {
        return rgb;
    }
    // Fallback Corten Copper accent if grim is not active
    (217, 107, 67)
}

pub fn resolve_corten_bin(name: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(name);
    if p.is_file() {
        return p.to_path_buf();
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return candidate;
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
    let ws_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or(std::path::Path::new("."));
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

/// Toggles `corten-launcher` on a detached background thread so `pkill.status()`
/// never stalls the compositor main event loop.
pub fn toggle_corten_launcher_async(args: &[&str]) {
    let owned_args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    std::thread::spawn(move || {
        if let Ok(status) = std::process::Command::new("pkill")
            .args(["-x", "corten-launcher"])
            .status()
            && status.success()
        {
            return;
        }
        let launcher_path = resolve_corten_bin("corten-launcher");
        let _ = std::process::Command::new(launcher_path)
            .args(&owned_args)
            .spawn();
    });
}

/// Replaces any open `corten-launcher` instance and spawns the desktop context menu
/// at `(cur_x, cur_y)` without blocking the compositor main loop.
pub fn spawn_desktop_context_menu_async(cur_x: i32, cur_y: i32) {
    std::thread::spawn(move || {
        let _ = std::process::Command::new("pkill")
            .args(["-9", "-x", "corten-launcher"])
            .status();
        let launcher_path = resolve_corten_bin("corten-launcher");
        let _ = std::process::Command::new(launcher_path)
            .arg("--desktop-menu")
            .arg("--x")
            .arg(cur_x.to_string())
            .arg("--y")
            .arg(cur_y.to_string())
            .spawn();
    });
}

/// Calculates kinetic velocity (px/sec) from recent gesture samples.
pub fn calculate_gesture_velocity(samples: &[(u64, f64, f64)]) -> (f64, f64) {
    let Some(last) = samples.last() else {
        return (0.0, 0.0);
    };
    if samples.len() < 2 {
        return (0.0, 0.0);
    }
    let last_time = last.0;
    // Consider samples within the last 150ms (150,000 microseconds)
    let recent: Vec<_> = samples
        .iter()
        .filter(|(t, _, _)| last_time.saturating_sub(*t) <= 150_000)
        .collect();

    let Some(first) = recent.first() else {
        return (0.0, 0.0);
    };
    if recent.len() < 2 {
        return (0.0, 0.0);
    }
    let first_time = first.0;
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
        assert_eq!(
            calculate_gesture_velocity(&[(100_000, 0.0, 0.0)]),
            (0.0, 0.0)
        );

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

    #[test]
    fn test_color_picker_ppm_parsing_and_formatting() {
        let p6 = b"P6\n1 1\n255\n\xD9\x6B\x43";
        assert_eq!(parse_ppm_pixel(p6), Some((217, 107, 67)));
        let p3 = b"P3\n1 1\n255\n64 128 255\n";
        assert_eq!(parse_ppm_pixel(p3), Some((64, 128, 255)));
        let (hex, rgb) = format_color_hex_rgb(217, 107, 67);
        assert_eq!(hex, "#D96B43");
        assert_eq!(rgb, "rgb(217, 107, 67)");
    }

    #[test]
    fn test_window_session_serialize_and_parse_roundtrip() {
        let entries = vec![
            SavedWindowEntry {
                app_id: "corten-terminal".into(),
                title: "Terminal".into(),
                workspace: 1,
                is_floating: false,
                x: 100,
                y: 80,
                width: 960,
                height: 640,
            },
            SavedWindowEntry {
                app_id: "corten-files".into(),
                title: "Dosyalar".into(),
                workspace: 2,
                is_floating: true,
                x: 240,
                y: 160,
                width: 1100,
                height: 720,
            },
        ];
        let encoded = serialize_window_session(&entries);
        let decoded = parse_window_session(&encoded);
        assert_eq!(decoded, entries);
    }

    #[test]
    fn test_cursor_following_zoom_pan_and_glow_rect() {
        let out = Size::from((1920, 1080));
        // At 1.0x zoom, pan must be exactly (0, 0)
        assert_eq!(
            compute_zoom_pan_offset(Point::from((960.0, 540.0)), out, 1.0, 1.0),
            Point::from((0, 0))
        );
        // At 2.0x zoom with cursor at top-left, pan clamps to (0, 0) (no black borders)
        assert_eq!(
            compute_zoom_pan_offset(Point::from((0.0, 0.0)), out, 1.0, 2.0),
            Point::from((0, 0))
        );
        // At 2.0x zoom with cursor at center (960, 540), pan shifts by (-960, -540)
        let center_pan = compute_zoom_pan_offset(Point::from((960.0, 540.0)), out, 1.0, 2.0);
        assert_eq!(center_pan, Point::from((-960, -540)));
        // At 2.0x zoom with cursor at bottom-right (1920, 1080), pan clamps to (-1920, -1080)
        assert_eq!(
            compute_zoom_pan_offset(Point::from((1920.0, 1080.0)), out, 1.0, 2.0),
            Point::from((-1920, -1080))
        );
        // Glow rect matches zoomed and panned window coordinates
        let win = Rectangle::new(Point::from((500, 300)), Size::from((400, 200)));
        let glow = compute_zoomed_glow_rect(win, 2.0, center_pan, 1.0);
        assert_eq!(glow.loc, Point::from((40, 60)));
        assert_eq!(glow.size, Size::from((800, 400)));
    }

    #[test]
    fn test_auto_fullscreen_game_mode_and_status_report() {
        let mut display = Display::<CortenCompositor>::new().expect("display");
        let mut state = CortenCompositor::new(&mut display, Size::from((1920, 1080)));
        state.configured_vrr = false;
        state.configured_allow_tearing = false;
        state.auto_fullscreen_game_mode = true;

        // Entering fullscreen activates auto Game Mode + VRR + tearing
        state.sync_fullscreen_game_mode(true);
        assert!(state.game_mode);
        assert!(state.auto_game_mode_active);
        assert!(state.vrr_enabled);
        assert!(state.allow_tearing);
        assert!(state.vrr_dirty);
        assert!(
            state
                .gaming_status_report()
                .contains("game_mode=auto-fullscreen")
        );

        // Leaving fullscreen restores configured baseline
        state.vrr_dirty = false;
        state.sync_fullscreen_game_mode(false);
        assert!(!state.game_mode);
        assert!(!state.auto_game_mode_active);
        assert!(!state.vrr_enabled);
        assert!(!state.allow_tearing);
        assert!(state.vrr_dirty);
        assert!(state.gaming_status_report().contains("game_mode=off"));
        let _ = display.flush_clients();
    }

    #[test]
    fn test_game_mode_idle_cursor_hide() {
        let mut display = Display::<CortenCompositor>::new().expect("display");
        let mut state = CortenCompositor::new(&mut display, Size::from((1920, 1080)));
        // Cursor stays visible outside Game Mode even after inactivity
        state.last_pointer_motion = Instant::now() - Duration::from_secs(10);
        assert!(!state.should_hide_idle_cursor());

        // Inside Game Mode, cursor hides after 4s of inactivity and immediately reappears on motion
        state.game_mode = true;
        assert!(state.should_hide_idle_cursor());
        state.note_pointer_motion();
        assert!(!state.should_hide_idle_cursor());
        let _ = display.flush_clients();
    }

    #[test]
    fn test_known_game_app_id_matching() {
        assert!(CortenCompositor::is_known_game_app_id("steam_app_730"));
        assert!(CortenCompositor::is_known_game_app_id("gamescope"));
        assert!(CortenCompositor::is_known_game_app_id("cs2"));
        assert!(CortenCompositor::is_known_game_app_id("Cyberpunk2077.exe"));
        assert!(CortenCompositor::is_known_game_app_id("rpcs3"));
        assert!(!CortenCompositor::is_known_game_app_id("corten-files"));
        assert!(!CortenCompositor::is_known_game_app_id("firefox"));
        assert!(!CortenCompositor::is_known_game_app_id(""));
    }
}
