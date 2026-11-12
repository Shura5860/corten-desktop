use std::os::unix::io::OwnedFd;

use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::desktop::{layer_map_for_output, PopupKind, Window};
use smithay::input::pointer::CursorImageStatus;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::utils::{Point, Rectangle, Size, Serial};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    add_blocker, add_pre_commit_hook, with_states, BufferAssignment, CompositorClientState,
    CompositorHandler, CompositorState, SurfaceAttributes,
};
use smithay::wayland::dmabuf::get_dmabuf;
use smithay::wayland::drm_syncobj::{DrmSyncobjCachedState, DrmSyncobjHandler, DrmSyncobjState};
use smithay::reexports::calloop::Interest;
use smithay::wayland::fractional_scale::{
    with_fractional_scale, FractionalScaleHandler,
};
use smithay::wayland::idle_inhibit::IdleInhibitHandler;
use smithay::wayland::keyboard_shortcuts_inhibit::{
    KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor,
};
use smithay::wayland::pointer_constraints::{
    with_pointer_constraint, PointerConstraintsHandler,
};
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};
use smithay::wayland::selection::primary_selection::{
    PrimarySelectionHandler, PrimarySelectionState,
};
use smithay::wayland::selection::wlr_data_control::{DataControlHandler, DataControlState};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};
use smithay::wayland::tablet_manager::TabletSeatHandler;
use smithay::wayland::session_lock::{
    LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker,
};
use smithay::wayland::shell::wlr_layer::{
    Layer, LayerSurface as WlrLayerSurface, WlrLayerShellHandler, WlrLayerShellState,
};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::wayland::xdg_activation::{
    XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
};
use tracing::info;
use wayland_protocols::xdg::shell::server::xdg_toplevel;
use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::protocol::wl_buffer::WlBuffer;
use wayland_server::protocol::wl_output::WlOutput;
use wayland_server::protocol::wl_seat::WlSeat;
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::Client;
use wayland_server::Resource;

use crate::state::CortenCompositor;

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

// 1. Compositor Handler
impl CompositorHandler for CortenCompositor {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        if let Some(state) = client.get_data::<smithay::xwayland::XWaylandClientData>() {
            return &state.compositor_state;
        }
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn new_surface(&mut self, surface: &WlSurface) {
        // Hold a commit until the client's GPU work on the attached dmabuf is finished:
        // explicit-sync acquire point if provided, otherwise the dmabuf's implicit fence.
        add_pre_commit_hook::<Self, _>(surface, |state, _dh, surface| {
            let mut acquire_point = None;
            let maybe_dmabuf = with_states(surface, |data| {
                acquire_point.clone_from(
                    &data
                        .cached_state
                        .get::<DrmSyncobjCachedState>()
                        .pending()
                        .acquire_point,
                );
                data.cached_state
                    .get::<SurfaceAttributes>()
                    .pending()
                    .buffer
                    .as_ref()
                    .and_then(|assignment| match assignment {
                        BufferAssignment::NewBuffer(buffer) => get_dmabuf(buffer).cloned().ok(),
                        _ => None,
                    })
            });
            let Some(dmabuf) = maybe_dmabuf else { return };
            let Some(client) = surface.client() else { return };
            let Some(loop_handle) = state.loop_handle.clone() else { return };

            if let Some(acquire_point) = acquire_point {
                if let Ok((blocker, source)) = acquire_point.generate_blocker() {
                    let client = client.clone();
                    let inserted = loop_handle.insert_source(source, move |_, _, data| {
                        let dh = data.display_handle.clone();
                        data.client_compositor_state(&client)
                            .blocker_cleared(data, &dh);
                        Ok(())
                    });
                    if inserted.is_ok() {
                        add_blocker(surface, blocker);
                        return;
                    }
                }
            }
            if let Ok((blocker, source)) = dmabuf.generate_blocker(Interest::READ) {
                let inserted = loop_handle.insert_source(source, move |_, _, data| {
                    let dh = data.display_handle.clone();
                    data.client_compositor_state(&client)
                        .blocker_cleared(data, &dh);
                    Ok(())
                });
                if inserted.is_ok() {
                    add_blocker(surface, blocker);
                }
            }
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        self.popup_manager.commit(surface);

        let mut matched_windows = Vec::new();
        for window in self.layout.windows() {
            if crate::layout::window_wl_surface(window).as_ref() == Some(surface) {
                window.on_commit();
                matched_windows.push(window.clone());
            }
        }
        for window in self.layout.floating_windows() {
            if crate::layout::window_wl_surface(window).as_ref() == Some(surface) {
                window.on_commit();
                matched_windows.push(window.clone());
            }
        }
        let mut metadata_changed = false;
        for win in matched_windows {
            if self
                .layout
                .evaluate_window_on_commit(&win, &self.window_rules)
            {
                metadata_changed = true;
            }
        }

        let scale = self.scale_for_surface(surface);
        with_states(surface, |states| {
            with_fractional_scale(states, |fractional_scale| {
                fractional_scale.set_preferred_scale(scale);
            });
        });
        if metadata_changed {
            self.sync_ipc_snapshot();
        }

        // While session is locked, always ensure keyboard focus belongs to the active lock surface.
        if self.session_locked {
            if let Some((lock_surface, _)) = self
                .lock_surfaces
                .iter()
                .find(|(ls, _)| ls.wl_surface() == surface)
                .or_else(|| self.lock_surfaces.first())
            {
                let target_surface = lock_surface.wl_surface().clone();
                let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                if let Some(kb) = self.seat.get_keyboard() {
                    if kb.current_focus().as_ref() != Some(&target_surface) {
                        kb.set_focus(self, Some(target_surface), serial);
                        info!("Compositor: Granted keyboard focus to ext-session-lock surface");
                    }
                }
            }
            return;
        }

        let should_focus_exclusive = {
            let mut is_exclusive = false;
            if let Some(ls) = self
                .layer_surfaces
                .iter()
                .find(|l| l.wl_surface() == surface)
                .cloned()
            {
                let requested_size = ls.cached_state().size;
                let mut arranged_any = false;
                for output in &self.additional_outputs {
                    let mut map = layer_map_for_output(output);
                    if map.layers().any(|l| l.wl_surface() == surface) {
                        let current_size = map.layer_geometry(&ls).map(|g| g.size);
                        if requested_size.w > 0 && requested_size.h > 0 && current_size != Some(requested_size) {
                            ls.layer_surface().with_pending_state(|state| {
                                state.size = Some(requested_size);
                            });
                            let _ = ls.layer_surface().send_configure();
                        }
                        map.arrange();
                        arranged_any = true;
                    }
                }
                let mut primary_map = layer_map_for_output(&self.output);
                if primary_map.layers().any(|l| l.wl_surface() == surface) {
                    let current_size = primary_map.layer_geometry(&ls).map(|g| g.size);
                    if requested_size.w > 0 && requested_size.h > 0 && current_size != Some(requested_size) {
                        ls.layer_surface().with_pending_state(|state| {
                            state.size = Some(requested_size);
                        });
                        let _ = ls.layer_surface().send_configure();
                    }
                    primary_map.arrange();
                } else if !arranged_any {
                    let _ = primary_map.map_layer(&ls);
                    primary_map.arrange();
                }

                use smithay::wayland::shell::wlr_layer::KeyboardInteractivity;
                is_exclusive =
                    ls.cached_state().keyboard_interactivity == KeyboardInteractivity::Exclusive;
            }
            is_exclusive
        };

        if should_focus_exclusive {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            if let Some(kb) = self.seat.get_keyboard() {
                if kb.current_focus().as_ref() != Some(surface) {
                    kb.set_focus(self, Some(surface.clone()), serial);
                    info!("Compositor: Exclusive layer surface committed, automatically granted keyboard focus!");
                }
            }
        }
    }
}

// 2. Buffer & SHM Handler
impl BufferHandler for CortenCompositor {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for CortenCompositor {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

// 3. XDG Shell Handler
impl XdgShellHandler for CortenCompositor {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        info!("Compositor: New toplevel window requested");
        let window = Window::new_wayland_window(surface.clone());

        let (rule_float, _, _) = crate::layout::evaluate_window_rules(&window, &self.window_rules);
        let auto_float = crate::layout::should_auto_float(&window);
        let is_floating = rule_float.unwrap_or_else(|| {
            if self.layout.mode() == corten_config::LayoutMode::Floating {
                true
            } else {
                auto_float
            }
        });

        if is_floating {
            self.add_floating_window_to_primary_output(window.clone());
        } else {
            self.add_window_to_primary_output(window.clone());
        }

        let usable_area = {
            let mut map = layer_map_for_output(&self.output);
            map.arrange();
            map.non_exclusive_zone()
        };

        // Determine smart initial geometry so the window never starts with a 0x0 buffer or overlaps
        let initial_rect = if is_floating {
            let geo = self.layout.floating_geometry(&window);
            let rect = geo.unwrap_or_else(|| {
                let existing = self.layout.floating_windows();
                let count = existing.len();
                let (min_w, min_h) = crate::layout::window_min_size(&window);
                let w = 960.min(usable_area.size.w - 48).max(min_w);
                let h = 640.min(usable_area.size.h - 48).max(min_h + 36);
                let max_cascade_x = (usable_area.size.w.saturating_sub(w) - 64).max(0);
                let max_cascade_y = (usable_area.size.h.saturating_sub(h) - 64).max(0);
                let cascade_x = if max_cascade_x > 0 { (count.saturating_sub(1) as i32 * 52) % max_cascade_x } else { 0 };
                let cascade_y = if max_cascade_y > 0 { (count.saturating_sub(1) as i32 * 44) % max_cascade_y } else { 0 };

                let mut candidate_x = usable_area.loc.x + 36 + cascade_x;
                let mut candidate_y = usable_area.loc.y + 36 + cascade_y;

                // Titlebar anti-occlusion: ensure titlebars never overlap within 40px of any existing window
                for other in existing {
                    if other != &window {
                        if let Some(other_geo) = self.layout.floating_geometry(other) {
                            if (candidate_x - other_geo.loc.x).abs() < 40 && (candidate_y - other_geo.loc.y).abs() < 40 {
                                candidate_x = (candidate_x + 52).min(usable_area.loc.x + max_cascade_x);
                                candidate_y = (candidate_y + 44).min(usable_area.loc.y + max_cascade_y);
                            }
                        }
                    }
                }

                Rectangle::new(
                    Point::from((candidate_x, candidate_y)),
                    Size::from((w, h)),
                )
            });
            self.layout.set_floating_geometry(&window, rect);
            rect
        } else {
            let tiled = self.layout.arrange(usable_area);
            for (w, r) in &tiled {
                if w != &window {
                    crate::layout::configure_window_geometry(w, *r);
                }
            }
            tiled
                .into_iter()
                .find(|(w, _)| w == &window)
                .map(|(_, r)| r)
                .unwrap_or_else(|| Rectangle::new(usable_area.loc, usable_area.size))
        };

        surface.with_pending_state(|state| {
            let client_h = (initial_rect.size.h - 36).max(100);
            state.size = Some((initial_rect.size.w.max(160), client_h).into());
            state.states.set(xdg_toplevel::State::Activated);
            state.decoration_mode = Some(wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode::ServerSide);
        });
        surface.send_configure();
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        info!("Compositor: Toplevel window destroyed");
        let to_remove = self
            .layout
            .windows()
            .iter()
            .chain(self.layout.floating_windows().iter())
            .find(|w| w.toplevel().map(|t| t == &surface).unwrap_or(false))
            .cloned();

        if let Some(window) = to_remove {
            self.remove_window_from_all_outputs(&window);
            if let Some(new_active) = self.layout.active_window().cloned() {
                if let Some(surface) = crate::layout::window_wl_surface(&new_active) {
                    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                    if let Some(kb) = self.seat.get_keyboard() {
                        kb.set_focus(self, Some(surface), serial);
                    }
                }
            }
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        if let Err(err) = self
            .popup_manager
            .track_popup(PopupKind::Xdg(surface.clone()))
        {
            tracing::warn!("Failed to track popup: {:?}", err);
        }
        let _ = surface.send_configure();
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {}

    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        info!("Compositor: Maximize requested for toplevel");
        let to_max = self
            .layout
            .windows()
            .iter()
            .chain(self.layout.floating_windows().iter())
            .find(|w| w.toplevel().map(|t| t == &surface).unwrap_or(false))
            .cloned();

        let usable_area = {
            let mut map = layer_map_for_output(&self.output);
            map.arrange();
            map.non_exclusive_zone()
        };

        if let Some(win) = to_max {
            self.layout.set_maximized(&win, usable_area, true);
            self.sync_ipc_snapshot();
        } else {
            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Maximized);
            });
            surface.send_configure();
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        info!("Compositor: Unmaximize requested for toplevel");
        let to_unmax = self
            .layout
            .windows()
            .iter()
            .chain(self.layout.floating_windows().iter())
            .find(|w| w.toplevel().map(|t| t == &surface).unwrap_or(false))
            .cloned();

        let usable_area = {
            let mut map = layer_map_for_output(&self.output);
            map.arrange();
            map.non_exclusive_zone()
        };

        if let Some(win) = to_unmax {
            self.layout.set_maximized(&win, usable_area, false);
            self.sync_ipc_snapshot();
        } else {
            surface.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
            });
            surface.send_configure();
        }
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        info!("Compositor: Fullscreen requested for toplevel");
        let win = Window::new_wayland_window(surface.clone());
        self.layout.set_fullscreen(&win, true);
        let full_size = self.output_size;
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Fullscreen);
            state.size = Some(full_size);
        });
        surface.send_configure();
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        info!("Compositor: Unfullscreen requested for toplevel");
        let win = Window::new_wayland_window(surface.clone());
        self.layout.set_fullscreen(&win, false);
        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Fullscreen);
        });
        surface.send_configure();
    }

    fn move_request(&mut self, surface: ToplevelSurface, _seat: WlSeat, _serial: Serial) {
        info!("Compositor: Move requested for toplevel via XDG Shell");
        let win = self.layout.windows().iter().chain(self.layout.floating_windows().iter())
            .find(|w| w.toplevel().map(|t| t == &surface).unwrap_or(false))
            .cloned();
        if let Some(window) = win {
            let rect = self.layout.floating_geometry(&window)
                .unwrap_or_else(|| window.geometry());
            self.dragged_window = Some((window, self.cursor_location, rect));
        }
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        _seat: WlSeat,
        _serial: Serial,
        _edges: xdg_toplevel::ResizeEdge,
    ) {
        info!("Compositor: Resize requested for toplevel via XDG Shell");
        let win = self.layout.windows().iter().chain(self.layout.floating_windows().iter())
            .find(|w| w.toplevel().map(|t| t == &surface).unwrap_or(false))
            .cloned();
        if let Some(window) = win {
            let rect = self.layout.floating_geometry(&window)
                .unwrap_or_else(|| window.geometry());
            self.resized_window = Some((window, self.cursor_location, rect));
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        info!("Compositor: Minimize requested for toplevel");
        let to_minimize = self
            .layout
            .windows()
            .iter()
            .chain(self.layout.floating_windows().iter())
            .find(|w| w.toplevel().map(|t| t == &surface).unwrap_or(false))
            .cloned();

        if let Some(window) = to_minimize {
            self.minimize_window(&window);
        }
    }
}

// 4. WLR Layer Shell Handler
impl WlrLayerShellHandler for CortenCompositor {
    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        {
            let mut layer_map = layer_map_for_output(&self.output);
            let found = layer_map.layers().find(|l| l.layer_surface() == &surface).cloned();
            if let Some(ref ls) = found {
                layer_map.unmap_layer(ls);
                layer_map.arrange();
            }
        }
        self.layer_surfaces.retain(|l| l.layer_surface() != &surface);
        if let Some(kb) = self.seat.get_keyboard() {
            if let Some(focus) = kb.current_focus() {
                if &focus == surface.wl_surface() {
                    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                    let target_surface = self
                        .layout
                        .active_window()
                        .and_then(crate::layout::window_wl_surface);
                    kb.set_focus(self, target_surface, serial);
                }
            }
        }
    }


    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        _output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        info!("Compositor: New layer shell surface created: {}", namespace);

        let target_output = _output
            .as_ref()
            .and_then(smithay::output::Output::from_resource)
            .unwrap_or_else(|| self.output.clone());

        let output_size: smithay::utils::Size<i32, smithay::utils::Logical> = target_output
            .current_mode()
            .map(|m| (m.size.w, m.size.h).into())
            .unwrap_or_else(|| (1280, 800).into());

        let mut configure_size = match namespace.as_str() {
            "corten-wallpaper"
            | "corten-lock"
            | "corten-snipper"
            | "corten-welcome"
            | "corten-launcher" => output_size,
            "corten-panel" => (output_size.w.min(760), 78).into(),
            "corten-widgets" => (340, 420).into(),
            "corten-notification" => (360, 100).into(),
            "corten-polkit" => (460, 340).into(),
            "corten-ai" => (560, 420).into(),
            _ => (output_size.w.min(600), 80).into(),
        };

        let desktop_layer = smithay::desktop::LayerSurface::new(surface.clone(), namespace.clone());
        let cached = desktop_layer.cached_state();

        surface.with_pending_state(|state| {
            if let Some(req) = state.size {
                if req.w > 0 {
                    configure_size.w = req.w;
                }
                if req.h > 0 {
                    configure_size.h = req.h;
                }
            }
            if configure_size.w == 0 || (cached.anchor.anchored_horizontally() && state.size.map(|s| s.w == 0).unwrap_or(true)) {
                configure_size.w = output_size.w;
            }
            if configure_size.h == 0 || (cached.anchor.anchored_vertically() && state.size.map(|s| s.h == 0).unwrap_or(true)) {
                configure_size.h = output_size.h;
            }

            // Guarantee non-zero dimensions for wgpu surface creation
            configure_size.w = configure_size.w.max(1);
            configure_size.h = configure_size.h.max(1);

            state.size = Some(configure_size);
        });

        let serial = surface.send_configure();

        // Now map the layer surface to output layer map
        self.layer_surfaces.push(desktop_layer.clone());
        let mut layer_map = layer_map_for_output(&target_output);
        let _ = layer_map.map_layer(&desktop_layer);
        layer_map.arrange();

        info!(
            "Compositor: Sent initial layer configure for {} with serial {:?}, configured size: {:?}",
            namespace, serial, configure_size
        );
    }
}

impl SessionLockHandler for CortenCompositor {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.session_lock_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        info!("Compositor: ext-session-lock request received");
        self.pending_lock = Some(confirmation);
        self.session_locked = true;
    }

    fn unlock(&mut self) {
        info!("Compositor: ext-session-lock released");
        self.session_locked = false;
        self.pending_lock = None;
        self.lock_surfaces.clear();

        // Restore keyboard focus to the currently active window on unlock.
        let target_surface = self
            .layout
            .active_window()
            .and_then(crate::layout::window_wl_surface);
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        if let Some(kb) = self.seat.get_keyboard() {
            kb.set_focus(self, target_surface, serial);
        }
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        info!("Compositor: ext-session-lock surface attached");
        surface.with_pending_state(|state| {
            let output_size = smithay::output::Output::from_resource(&output)
                .unwrap_or_else(|| self.output.clone());
            let size = output_size
                .current_mode()
                .map(|mode| (mode.size.w.max(1) as u32, mode.size.h.max(1) as u32).into());
            state.size = size;
        });
        surface.send_configure();
        let wl_surface = surface.wl_surface().clone();
        self.lock_surfaces.push((surface, output));

        // Immediately grant keyboard focus to the lock surface so password entry works on first keystroke.
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        if let Some(kb) = self.seat.get_keyboard() {
            kb.set_focus(self, Some(wl_surface), serial);
        }

        // Confirm only after every registered output has a lock surface. A
        // first surface is insufficient on multi-monitor setups: confirming
        // early could leave another output visible and unlocked.
        let expected_outputs = self.additional_outputs.len() + 1;
        if self.lock_surfaces.len() >= expected_outputs {
            if let Some(confirmation) = self.pending_lock.take() {
                confirmation.lock();
                info!(
                    "Compositor: ext-session-lock confirmed for {} output(s)",
                    expected_outputs
                );
            }
        }
    }
}

// 5. Selection & Data Device Handler
impl SelectionHandler for CortenCompositor {
    type SelectionUserData = ();

    fn new_selection(&mut self, ty: SelectionTarget, source: Option<SelectionSource>, _seat: Seat<Self>) {
        if let Some(xwm) = self.xwm.as_mut() {
            let mime_types = source.as_ref().map(|s| s.mime_types());
            let _ = xwm.new_selection(ty, mime_types);
        }
    }

    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        _user_data: &Self::SelectionUserData,
    ) {
        if let Some(xwm) = self.xwm.as_mut() {
            if let Some(ref loop_handle) = self.loop_handle {
                if let Err(err) = xwm.send_selection(ty, mime_type.clone(), fd, loop_handle.clone()) {
                    tracing::debug!(?err, %mime_type, "Failed to send selection from X11 to Wayland client");
                }
            }
        }
    }
}

impl DataDeviceHandler for CortenCompositor {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for CortenCompositor {
    fn started(
        &mut self,
        _source: Option<wayland_server::protocol::wl_data_source::WlDataSource>,
        icon: Option<WlSurface>,
        _seat: Seat<Self>,
    ) {
        self.dnd_icon = icon;
    }

    fn dropped(
        &mut self,
        _target: Option<WlSurface>,
        _validated: bool,
        _seat: Seat<Self>,
    ) {
        self.dnd_icon = None;
    }
}
impl ServerDndGrabHandler for CortenCompositor {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {}
}

impl DataControlHandler for CortenCompositor {
    fn data_control_state(&self) -> &DataControlState {
        &self.data_control_state
    }
}

impl TabletSeatHandler for CortenCompositor {}

// 6. Seat Handler
impl SeatHandler for CortenCompositor {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_status = image;
    }
}

// 7. Output Handler
impl smithay::wayland::output::OutputHandler for CortenCompositor {}

// 8. Dmabuf Handler
impl smithay::wayland::dmabuf::DmabufHandler for CortenCompositor {
    fn dmabuf_state(&mut self) -> &mut smithay::wayland::dmabuf::DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &smithay::wayland::dmabuf::DmabufGlobal,
        _dmabuf: smithay::backend::allocator::dmabuf::Dmabuf,
        notifier: smithay::wayland::dmabuf::ImportNotifier,
    ) {
        // Notify client that buffer was successfully imported
        let _ = notifier.successful::<Self>();
    }
}

// 9. XDG Decoration Handler (Advertises Client/Server decoration so windows render close/minimize/maximize buttons)
impl smithay::wayland::shell::xdg::decoration::XdgDecorationHandler for CortenCompositor {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        use wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ServerSide);
        });
        toplevel.send_configure();
    }

    fn request_mode(
        &mut self,
        toplevel: ToplevelSurface,
        _mode: wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
    ) {
        use wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ServerSide);
        });
        let _ = toplevel.send_configure();
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        use wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ServerSide);
        });
        let _ = toplevel.send_configure();
    }
}

impl PrimarySelectionHandler for CortenCompositor {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

impl FractionalScaleHandler for CortenCompositor {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let scale = self.scale_for_surface(&surface);
        with_states(&surface, |states| {
            with_fractional_scale(states, |fractional_scale| {
                fractional_scale.set_preferred_scale(scale);
            });
        });
    }
}

impl PointerConstraintsHandler for CortenCompositor {
    fn new_constraint(
        &mut self,
        surface: &WlSurface,
        pointer: &smithay::input::pointer::PointerHandle<Self>,
    ) {
        if pointer
            .current_focus()
            .as_ref()
            .is_none_or(|focused| focused == surface)
        {
            with_pointer_constraint(surface, pointer, |constraint| {
                if let Some(constraint) = constraint {
                    if !constraint.is_active() {
                        info!("Compositor: Activating zwp_pointer_constraints_v1 for surface");
                        constraint.activate();
                    }
                }
            });
        }
    }

    fn cursor_position_hint(
        &mut self,
        _surface: &WlSurface,
        _pointer: &smithay::input::pointer::PointerHandle<Self>,
        _location: smithay::utils::Point<f64, smithay::utils::Logical>,
    ) {
    }
}

impl IdleInhibitHandler for CortenCompositor {
    fn inhibit(&mut self, surface: WlSurface) {
        info!("Compositor: Surface requested idle inhibition (video/fullscreen active)");
        if !self.idle_inhibiting_surfaces.contains(&surface) {
            self.idle_inhibiting_surfaces.push(surface);
        }
        self.note_input();
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        info!("Compositor: Surface released idle inhibition");
        self.idle_inhibiting_surfaces.retain(|s| s != &surface);
    }
}

impl KeyboardShortcutsInhibitHandler for CortenCompositor {
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState {
        &mut self.keyboard_shortcuts_inhibit_state
    }

    fn new_inhibitor(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        info!("Compositor: Activating keyboard shortcuts inhibitor for client surface");
        inhibitor.activate();
    }

    fn inhibitor_destroyed(&mut self, _inhibitor: KeyboardShortcutsInhibitor) {
        info!("Compositor: Keyboard shortcuts inhibitor destroyed");
    }
}

impl XdgActivationHandler for CortenCompositor {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        if token_data.timestamp.elapsed().as_secs() < 10 {
            let target_window = self
                .layout
                .windows()
                .iter()
                .find(|w| {
                    w.toplevel()
                        .is_some_and(|t| t.wl_surface() == &surface)
                })
                .cloned();
            if let Some(window) = target_window {
                self.layout.set_active_window(&window);
                let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                if let Some(kb) = self.seat.get_keyboard() {
                    kb.set_focus(self, Some(surface), serial);
                }
                self.sync_ipc_snapshot();
            }
        }
        self.xdg_activation_state.remove_token(&token);
    }
}

// Delegate macros
smithay::delegate_compositor!(CortenCompositor);
smithay::delegate_shm!(CortenCompositor);
smithay::delegate_dmabuf!(CortenCompositor);
smithay::delegate_xdg_shell!(CortenCompositor);
smithay::delegate_xdg_decoration!(CortenCompositor);
smithay::delegate_layer_shell!(CortenCompositor);
smithay::delegate_session_lock!(CortenCompositor);
smithay::delegate_output!(CortenCompositor);
smithay::delegate_data_device!(CortenCompositor);
smithay::delegate_primary_selection!(CortenCompositor);
smithay::delegate_data_control!(CortenCompositor);
smithay::delegate_cursor_shape!(CortenCompositor);
impl DrmSyncobjHandler for CortenCompositor {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> {
        self.syncobj_state.as_mut()
    }
}

smithay::delegate_drm_syncobj!(CortenCompositor);
smithay::delegate_viewporter!(CortenCompositor);
smithay::delegate_fractional_scale!(CortenCompositor);
smithay::delegate_relative_pointer!(CortenCompositor);
smithay::delegate_pointer_constraints!(CortenCompositor);
smithay::delegate_pointer_gestures!(CortenCompositor);
smithay::delegate_idle_inhibit!(CortenCompositor);
smithay::delegate_keyboard_shortcuts_inhibit!(CortenCompositor);
smithay::delegate_single_pixel_buffer!(CortenCompositor);
smithay::delegate_presentation!(CortenCompositor);
smithay::delegate_alpha_modifier!(CortenCompositor);
smithay::delegate_content_type!(CortenCompositor);
smithay::delegate_xdg_activation!(CortenCompositor);
smithay::delegate_text_input_manager!(CortenCompositor);
smithay::delegate_tablet_manager!(CortenCompositor);
smithay::delegate_seat!(CortenCompositor);
