//! XWayland (`X11Wm` + `xwayland_shell`) integration for running X11 applications natively.

use std::os::unix::io::OwnedFd;
use std::process::Stdio;

use smithay::desktop::Window;
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::selection::data_device::{
    clear_data_device_selection, request_data_device_client_selection, set_data_device_selection,
};
use smithay::wayland::selection::primary_selection::{
    clear_primary_selection, request_primary_client_selection, set_primary_selection,
};
use smithay::wayland::selection::SelectionTarget;
use smithay::wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState};
use smithay::xwayland::xwm::{
    Reorder, ResizeEdge as X11ResizeEdge, X11Wm, XwmHandler, XwmId,
};
use smithay::xwayland::{X11Surface, XWayland, XWaylandEvent};
use tracing::{info, warn};

use crate::layout::{configure_window_geometry, window_stable_id};
use crate::state::CortenCompositor;

impl XWaylandShellHandler for CortenCompositor {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }

    fn surface_associated(
        &mut self,
        _xwm: XwmId,
        wl_surface: WlSurface,
        window: X11Surface,
    ) {
        let output_scale = self.output.current_scale().fractional_scale();
        let scale = compute_xwayland_surface_scale(output_scale, self.xwayland_force_zero_scaling);
        smithay::wayland::compositor::with_states(&wl_surface, |states| {
            smithay::wayland::fractional_scale::with_fractional_scale(states, |fscale| {
                fscale.set_preferred_scale(scale);
            });
        });
        if !window.is_override_redirect() {
            let wid = (1u64 << 32) | u64::from(window.window_id());
            let already_tracked = self
                .layout
                .windows()
                .iter()
                .chain(self.layout.floating_windows().iter())
                .any(|w| window_stable_id(w) == wid);
            if !already_tracked && window.is_mapped() {
                let smithay_window = Window::new_x11_window(window.clone());
                self.layout.add_window(smithay_window.clone());
                let usable = self.usable_area();
                let geo = window.geometry();
                let titlebar_h = crate::layout::window_titlebar_height(&smithay_window, false);
                let max_w = (usable.size.w - 32).max(200);
                let max_h = (usable.size.h - 32).max(160);
                let w = geo.size.w.clamp(160, max_w);
                let h = (geo.size.h + titlebar_h).clamp(100 + titlebar_h, max_h);
                let x = geo
                    .loc
                    .x
                    .clamp(usable.loc.x + 16, (usable.loc.x + usable.size.w - w).max(usable.loc.x));
                let y = geo
                    .loc
                    .y
                    .clamp(usable.loc.y + 16, (usable.loc.y + usable.size.h - h).max(usable.loc.y));
                let rect = Rectangle::new((x, y).into(), (w, h).into());
                self.layout.set_floating_geometry(&smithay_window, rect);
                configure_window_geometry(&smithay_window, rect);
            }
            let _ = window.set_activated(true);
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            if let Some(keyboard) = self.seat.get_keyboard() {
                keyboard.set_focus(self, Some(wl_surface), serial);
            }
            self.sync_ipc_snapshot();
        }
    }
}

smithay::delegate_xwayland_shell!(CortenCompositor);

impl XwmHandler for CortenCompositor {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm.as_mut().expect("X11Wm must be initialized")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Err(err) = window.set_mapped(true) {
            warn!("XWayland: failed to set_mapped(true): {err:?}");
        }

        let smithay_window = Window::new_x11_window(window.clone());
        let (rule_float, _, _) =
            crate::layout::evaluate_window_rules(&smithay_window, &self.window_rules);
        let should_float = rule_float.unwrap_or_else(|| {
            if self.layout.mode() == corten_config::LayoutMode::Floating {
                true
            } else {
                crate::layout::should_auto_float(&smithay_window)
            }
        });

        self.layout.add_window(smithay_window.clone());
        let _ = self
            .layout
            .evaluate_window_on_commit(&smithay_window, &self.window_rules);
        if should_float {
            let usable = self.usable_area();
            let geo = window.geometry();
            let titlebar_h = crate::layout::window_titlebar_height(&smithay_window, false);
            let max_w = (usable.size.w - 32).max(200);
            let max_h = (usable.size.h - 32).max(160);
            let w = geo.size.w.clamp(160, max_w);
            let h = (geo.size.h + titlebar_h).clamp(100 + titlebar_h, max_h);
            let x = geo.loc.x.clamp(usable.loc.x + 16, (usable.loc.x + usable.size.w - w).max(usable.loc.x));
            let y = geo.loc.y.clamp(usable.loc.y + 16, (usable.loc.y + usable.size.h - h).max(usable.loc.y));
            let rect = Rectangle::new((x, y).into(), (w, h).into());
            self.layout.set_floating_geometry(&smithay_window, rect);
            configure_window_geometry(&smithay_window, rect);
        } else {
            let usable = self.usable_area();
            for (w, rect) in self.layout.arrange(usable) {
                configure_window_geometry(&w, rect);
            }
        }

        let _ = window.set_activated(true);
        if let Some(wl_surface) = window.wl_surface() {
            let output_scale = self.output.current_scale().fractional_scale();
            let scale = compute_xwayland_surface_scale(output_scale, self.xwayland_force_zero_scaling);
            smithay::wayland::compositor::with_states(&wl_surface, |states| {
                smithay::wayland::fractional_scale::with_fractional_scale(states, |fscale| {
                    fscale.set_preferred_scale(scale);
                });
            });
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            if let Some(keyboard) = self.seat.get_keyboard() {
                keyboard.set_focus(self, Some(wl_surface), serial);
            }
        }
        self.sync_ipc_snapshot();
    }

    fn map_window_notify(&mut self, _xwm: XwmId, window: X11Surface) {
        let wid = (1u64 << 32) | u64::from(window.window_id());
        let already_tracked = self
            .layout
            .windows()
            .iter()
            .any(|w| window_stable_id(w) == wid);
        if !already_tracked {
            let smithay_window = Window::new_x11_window(window.clone());
            self.layout.add_window(smithay_window.clone());
            let usable = self.usable_area();
            let geo = window.geometry();
            let titlebar_h = crate::layout::window_titlebar_height(&smithay_window, false);
            let max_w = (usable.size.w - 32).max(200);
            let max_h = (usable.size.h - 32).max(160);
            let w = geo.size.w.clamp(160, max_w);
            let h = (geo.size.h + titlebar_h).clamp(100 + titlebar_h, max_h);
            let x = geo
                .loc
                .x
                .clamp(usable.loc.x + 16, (usable.loc.x + usable.size.w - w).max(usable.loc.x));
            let y = geo
                .loc
                .y
                .clamp(usable.loc.y + 16, (usable.loc.y + usable.size.h - h).max(usable.loc.y));
            let rect = Rectangle::new((x, y).into(), (w, h).into());
            self.layout.set_floating_geometry(&smithay_window, rect);
            configure_window_geometry(&smithay_window, rect);
        }
        let _ = window.set_activated(true);
        if let Some(wl_surface) = window.wl_surface() {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            if let Some(keyboard) = self.seat.get_keyboard() {
                keyboard.set_focus(self, Some(wl_surface), serial);
            }
        }
        self.sync_ipc_snapshot();
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        if !self
            .override_redirect_windows
            .iter()
            .any(|w| w.window_id() == window.window_id())
        {
            self.override_redirect_windows.push(window);
        }
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.override_redirect_windows
            .retain(|w| w.window_id() != window.window_id());
        self.remove_x11_window(&window);
        if !window.is_override_redirect() {
            let _ = window.set_mapped(false);
        }
    }

    fn destroyed_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.override_redirect_windows
            .retain(|w| w.window_id() != window.window_id());
        self.remove_x11_window(&window);
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        let mut geo = window.geometry();
        if let Some(x) = x {
            geo.loc.x = x;
        }
        if let Some(y) = y {
            geo.loc.y = y;
        }
        if let Some(w) = w {
            geo.size.w = (w as i32).max(1);
        }
        if let Some(h) = h {
            geo.size.h = (h as i32).max(1);
        }

        let wid = (1u64 << 32) | u64::from(window.window_id());
        let is_floating = self
            .layout
            .floating_windows()
            .iter()
            .find(|win| window_stable_id(win) == wid)
            .cloned();

        if window.is_override_redirect() || is_floating.is_some() || !window.is_mapped() {
            if let Some(win) = is_floating {
                let titlebar_h = crate::layout::window_titlebar_height(&win, window.is_fullscreen());
                // Convert client geometry into outer frame geometry (avoiding 36px drift)
                let frame_geo = Rectangle::new(
                    (geo.loc.x, (geo.loc.y - titlebar_h).max(0)).into(),
                    (geo.size.w.max(160), (geo.size.h + titlebar_h).max(100 + titlebar_h)).into(),
                );
                self.layout.set_floating_geometry(&win, frame_geo);
                configure_window_geometry(&win, frame_geo);
            } else {
                let _ = window.configure(geo);
            }
        } else {
            let _ = window.configure(None);
        }
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        let wid = (1u64 << 32) | u64::from(window.window_id());
        if let Some(win) = self
            .layout
            .floating_windows()
            .iter()
            .find(|win| window_stable_id(win) == wid)
            .cloned()
        {
            let titlebar_h = crate::layout::window_titlebar_height(&win, window.is_fullscreen());
            // Reconstruct outer frame geometry so X11 client y+titlebar_h does not drift downwards!
            let frame_geo = Rectangle::new(
                (geometry.loc.x, (geometry.loc.y - titlebar_h).max(0)).into(),
                (geometry.size.w.max(160), (geometry.size.h + titlebar_h).max(100 + titlebar_h)).into(),
            );
            self.layout.set_floating_geometry(&win, frame_geo);
        }
    }

    fn maximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let wid = (1u64 << 32) | u64::from(window.window_id());
        let target = self
            .layout
            .windows()
            .iter()
            .chain(self.layout.floating_windows().iter())
            .find(|w| window_stable_id(w) == wid)
            .cloned();
        if let Some(win) = target {
            let area = self.usable_area_for_window(&win);
            self.layout.set_maximized(&win, area, true);
            self.sync_ipc_snapshot();
        } else {
            let _ = window.set_maximized(true);
            let area = self.usable_area();
            let _ = window.configure(area);
        }
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let wid = (1u64 << 32) | u64::from(window.window_id());
        let target = self
            .layout
            .windows()
            .iter()
            .chain(self.layout.floating_windows().iter())
            .find(|w| window_stable_id(w) == wid)
            .cloned();
        if let Some(win) = target {
            let area = self.usable_area_for_window(&win);
            self.layout.set_maximized(&win, area, false);
            self.sync_ipc_snapshot();
        } else {
            let _ = window.set_maximized(false);
            let usable = self.usable_area();
            for (w, rect) in self.layout.arrange(usable) {
                configure_window_geometry(&w, rect);
            }
        }
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let _ = window.set_fullscreen(true);
        let smithay_win = Window::new_x11_window(window.clone());
        self.layout.set_fullscreen(&smithay_win, true);
        let target_output = self.output_for_window(&smithay_win);
        let scale = target_output.current_scale().fractional_scale().max(0.5);
        let full_size = target_output
            .current_mode()
            .map(|m| {
                smithay::utils::Size::from((
                    (m.size.w as f64 / scale).round() as i32,
                    (m.size.h as f64 / scale).round() as i32,
                ))
            })
            .unwrap_or(self.output_size);
        let full = Rectangle::from_size(full_size);
        let _ = window.configure(full);
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let _ = window.set_fullscreen(false);
        let smithay_win = Window::new_x11_window(window);
        self.layout.set_fullscreen(&smithay_win, false);
        let usable = self.usable_area_for_window(&smithay_win);
        if let Some(geo) = self.layout.floating_geometry(&smithay_win) {
            configure_window_geometry(&smithay_win, geo);
        } else {
            for (w, rect) in self.layout.arrange(usable) {
                configure_window_geometry(&w, rect);
            }
        }
    }

    fn minimize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let wid = (1u64 << 32) | u64::from(window.window_id());
        if let Some(win) = self
            .layout
            .windows()
            .iter()
            .chain(self.layout.floating_windows().iter())
            .find(|w| window_stable_id(w) == wid)
            .cloned()
        {
            self.minimize_window(&win);
        }
    }

    fn unminimize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let wid = (1u64 << 32) | u64::from(window.window_id());
        let _ = self.focus_window_by_id(wid);
    }

    fn resize_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        _button: u32,
        resize_edge: X11ResizeEdge,
    ) {
        let wid = (1u64 << 32) | u64::from(window.window_id());
        let target = self
            .layout
            .windows()
            .iter()
            .chain(self.layout.floating_windows().iter())
            .find(|w| window_stable_id(w) == wid)
            .cloned();
        if let Some(win) = target {
            let rect = self.layout.floating_geometry(&win).unwrap_or_else(|| {
                let usable = self.usable_area_for_window(&win);
                self.layout
                    .arrange(usable)
                    .into_iter()
                    .find(|(w, _)| w == &win)
                    .map(|(_, r)| r)
                    .unwrap_or_else(|| window.geometry())
            });
            let edge = crate::layout::ResizeEdge::from_x11(resize_edge);
            self.resized_window = Some((win, self.cursor_location, rect, edge));
        }
    }

    fn move_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32) {
        let wid = (1u64 << 32) | u64::from(window.window_id());
        let target = self
            .layout
            .windows()
            .iter()
            .chain(self.layout.floating_windows().iter())
            .find(|w| window_stable_id(w) == wid)
            .cloned();
        if let Some(win) = target {
            let rect = self.layout.floating_geometry(&win).unwrap_or_else(|| {
                let usable = self.usable_area_for_window(&win);
                self.layout
                    .arrange(usable)
                    .into_iter()
                    .find(|(w, _)| w == &win)
                    .map(|(_, r)| r)
                    .unwrap_or_else(|| window.geometry())
            });
            self.dragged_window = Some((win, self.cursor_location, rect));
        }
    }

    fn allow_selection_access(&mut self, _xwm: XwmId, _selection: SelectionTarget) -> bool {
        true
    }

    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        match selection {
            SelectionTarget::Clipboard => {
                if let Err(err) = request_data_device_client_selection(
                    &self.seat,
                    mime_type.clone(),
                    fd,
                ) {
                    tracing::debug!(?err, %mime_type, "Clipboard selection request from X11 not satisfied by Wayland client");
                }
            }
            SelectionTarget::Primary => {
                if let Err(err) = request_primary_client_selection(
                    &self.seat,
                    mime_type.clone(),
                    fd,
                ) {
                    tracing::debug!(?err, %mime_type, "Primary selection request from X11 not satisfied by Wayland client");
                }
            }
        }
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        let dh = self.display_handle.clone();
        match selection {
            SelectionTarget::Clipboard => {
                set_data_device_selection::<Self>(
                    &dh,
                    &self.seat,
                    mime_types,
                    (),
                );
            }
            SelectionTarget::Primary => {
                set_primary_selection::<Self>(
                    &dh,
                    &self.seat,
                    mime_types,
                    (),
                );
            }
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        let dh = self.display_handle.clone();
        match selection {
            SelectionTarget::Clipboard => {
                clear_data_device_selection::<Self>(&dh, &self.seat);
            }
            SelectionTarget::Primary => {
                clear_primary_selection::<Self>(&dh, &self.seat);
            }
        }
    }
}

impl CortenCompositor {
    pub fn remove_x11_window(&mut self, x11: &X11Surface) {
        let wid = (1u64 << 32) | u64::from(x11.window_id());
        if self
            .dragged_window
            .as_ref()
            .is_some_and(|(w, _, _)| window_stable_id(w) == wid)
        {
            self.dragged_window = None;
            self.snap_preview = None;
            self.drag_split_target = None;
        }
        if self
            .resized_window
            .as_ref()
            .is_some_and(|(w, _, _, _)| window_stable_id(w) == wid)
        {
            self.resized_window = None;
        }
        let mut removed = self.layout.remove_x11_surface(wid);
        for layout in self.output_layouts.values_mut() {
            if layout.remove_x11_surface(wid) {
                removed = true;
            }
        }
        if removed {
            let usable = self.usable_area();
            for (w, rect) in self.layout.arrange(usable) {
                configure_window_geometry(&w, rect);
            }
            self.sync_ipc_snapshot();
        }
    }

    /// Return the underlying `WlSurface` for all mapped XWayland override-redirect windows
    /// (menus, tooltips, splash screens) along with their logical coordinates.
    pub fn override_redirect_surfaces(&self) -> Vec<(WlSurface, Rectangle<i32, Logical>)> {
        self.override_redirect_windows
            .iter()
            .filter(|w| w.is_mapped())
            .filter_map(|w| w.wl_surface().map(|s| (s, w.geometry())))
            .collect()
    }
}

/// Spawn the `XWayland` server on the given Calloop event loop if the `Xwayland` binary is installed.
pub fn start_xwayland(
    display: &DisplayHandle,
    loop_handle: &LoopHandle<'static, CortenCompositor>,
) -> Option< wl_client_guard::XWaylandInstance > {
    let (xwayland, client) = match XWayland::spawn(
        display,
        None,
        std::iter::empty::<(String, String)>(),
        true,
        Stdio::null(),
        Stdio::null(),
        |_| (),
    ) {
        Ok(res) => res,
        Err(err) => {
            warn!("XWayland spawn skipped or unavailable: {err:?}");
            return None;
        }
    };

    let handle_for_wm = loop_handle.clone();
    let client_for_wm = client.clone();

    let insert_res = loop_handle.insert_source(xwayland, move |event, _, state: &mut CortenCompositor| {
        match event {
            XWaylandEvent::Ready {
                x11_socket,
                display_number,
            } => {
                let display_value = format!(":{display_number}");
                // SAFETY: Compositor owns the process environment and updates DISPLAY when XWayland becomes ready.
                unsafe {
                    std::env::set_var("DISPLAY", &display_value);
                }
                if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
                    let _ = std::fs::write(
                        std::path::PathBuf::from(runtime_dir).join("corten-x11-display"),
                        &display_value,
                    );
                }
                let _ = std::process::Command::new("dbus-update-activation-environment")
                    .args(["--systemd", "DISPLAY"])
                    .env("DISPLAY", &display_value)
                    .spawn();
                let scale = state.output.current_scale().fractional_scale();
                sync_x11_scale(scale);
                match X11Wm::start_wm(handle_for_wm.clone(), x11_socket, client_for_wm.clone()) {
                    Ok(wm) => {
                        if let Some(data) =
                            client_for_wm.get_data::<smithay::xwayland::XWaylandClientData>()
                        {
                            data.user_data().insert_if_missing(|| wm.id());
                        }
                        info!(
                            "XWayland ready on DISPLAY={display_value} with active X11Wm (id={:?})",
                            wm.id()
                        );
                        state.xwm = Some(wm);
                        state.xwayland_client = Some(client_for_wm.clone());
                    }
                    Err(err) => {
                        warn!("XWayland ready on {display_value}, but failed to start X11Wm: {err:?}");
                    }
                }
            }
            XWaylandEvent::Error => {
                warn!("XWayland exited with an error before becoming ready");
                state.xwm = None;
            }
        }
    });

    if let Err(err) = insert_res {
        warn!("Failed to register XWayland source in Calloop event loop: {err:?}");
        None
    } else {
        Some(wl_client_guard::XWaylandInstance)
    }
}

pub mod wl_client_guard {
    pub struct XWaylandInstance;
}

/// Computes the effective Wayland buffer scale to report to an XWayland surface.
/// When `force_zero_scaling` is true (Hyprland `force_zero_scaling` style), XWayland
/// renders 1:1 unscaled buffers and relies on `Xft.dpi` (`96 * output_scale`), preventing blurry
/// bilinear upscaling on fractional HiDPI monitors.
pub fn compute_xwayland_surface_scale(output_scale: f64, force_zero_scaling: bool) -> f64 {
    if force_zero_scaling {
        1.0
    } else {
        output_scale
    }
}

/// Synchronizes the X11 DPI, cursor size and toolkit scaling variables
/// with the compositor's active output scale to ensure razor-sharp rendering on XWayland.
pub fn sync_x11_scale(scale: f64) {
    let dpi = (96.0 * scale).round() as u32;
    let cursor_size = (24.0 * scale).round() as u32;
    let gdk_scale = scale.round().max(1.0) as u32;
    let gdk_dpi_scale = 1.0 / (gdk_scale as f64);

    // Update process environment for spawned child processes
    unsafe {
        std::env::set_var("GDK_SCALE", gdk_scale.to_string());
        std::env::set_var("GDK_DPI_SCALE", format!("{:.2}", gdk_dpi_scale));
        std::env::set_var("QT_SCALE_FACTOR", format!("{:.2}", scale));
        std::env::set_var("XCURSOR_SIZE", cursor_size.to_string());
    }

    // Pass Xft.dpi to running XWayland instance via xrdb if available in a detached thread
    let xrdb_content = format!("Xft.dpi: {}\nXcursor.size: {}\n", dpi, cursor_size);
    std::thread::spawn(move || {
        if let Ok(mut child) = std::process::Command::new("xrdb")
            .arg("-merge")
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(xrdb_content.as_bytes());
            }
            let _ = child.wait();
            tracing::info!("X11 DPI synchronized: {} DPI (scale {:.2})", dpi, scale);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_x11_scale_math() {
        let cases: [(f64, u32, u32, u32); 4] = [
            (1.0, 96, 24, 1),
            (1.25, 120, 30, 1),
            (1.5, 144, 36, 2),
            (2.0, 192, 48, 2),
        ];
        for (scale, expected_dpi, expected_cursor, expected_gdk) in cases {
            let dpi = (96.0 * scale).round() as u32;
            let cursor_size = (24.0 * scale).round() as u32;
            let gdk_scale = scale.round().max(1.0) as u32;
            assert_eq!(dpi, expected_dpi);
            assert_eq!(cursor_size, expected_cursor);
            assert_eq!(gdk_scale, expected_gdk);
        }
        assert_eq!(compute_xwayland_surface_scale(1.5, true), 1.0);
        assert_eq!(compute_xwayland_surface_scale(1.5, false), 1.5);
    }

    #[test]
    fn test_xwayland_selection_target_equality() {
        assert_ne!(SelectionTarget::Clipboard, SelectionTarget::Primary);
    }
}
