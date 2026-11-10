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
            let geo = window.geometry();
            let rect = Rectangle::new(
                (
                    geo.loc.x.max(40),
                    geo.loc.y.max((self.panel_height + 24).max(40)),
                )
                    .into(),
                (geo.size.w.clamp(120, 1600), geo.size.h.clamp(80, 1000)).into(),
            );
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
            let scale = self.output.current_scale().fractional_scale();
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
                self.layout.set_floating_geometry(&win, geo);
            }
            let _ = window.configure(geo);
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
            self.layout.set_floating_geometry(&win, geometry);
        }
    }

    fn maximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let _ = window.set_maximized(true);
        let area = self.usable_area();
        let _ = window.configure(area);
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let _ = window.set_maximized(false);
        let usable = self.usable_area();
        for (w, rect) in self.layout.arrange(usable) {
            configure_window_geometry(&w, rect);
        }
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let _ = window.set_fullscreen(true);
        let smithay_win = Window::new_x11_window(window.clone());
        self.layout.set_fullscreen(&smithay_win, true);
        let full = Rectangle::from_size(self.output_size);
        let _ = window.configure(full);
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let _ = window.set_fullscreen(false);
        let smithay_win = Window::new_x11_window(window);
        self.layout.set_fullscreen(&smithay_win, false);
        let usable = self.usable_area();
        for (w, rect) in self.layout.arrange(usable) {
            configure_window_geometry(&w, rect);
        }
    }

    fn minimize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        self.remove_x11_window(&window);
    }

    fn unminimize_request(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn resize_request(
        &mut self,
        _xwm: XwmId,
        _window: X11Surface,
        _button: u32,
        _resize_edge: X11ResizeEdge,
    ) {
    }

    fn move_request(&mut self, _xwm: XwmId, _window: X11Surface, _button: u32) {}

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
                let scale = state.output.current_scale().fractional_scale();
                sync_x11_scale(scale);
                match X11Wm::start_wm(handle_for_wm.clone(), x11_socket, client_for_wm.clone()) {
                    Ok(wm) => {
                        info!("XWayland ready on DISPLAY={display_value} with active X11Wm");
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
    }

    #[test]
    fn test_xwayland_selection_target_equality() {
        assert_ne!(SelectionTarget::Clipboard, SelectionTarget::Primary);
    }
}
