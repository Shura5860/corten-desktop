use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use smithay::backend::input::{
    ButtonState, TabletToolCapabilities, TabletToolDescriptor, TabletToolType,
};
use smithay::reexports::input::event::tablet_tool::{
    TabletTool as LibinputTabletTool, TabletToolAxisEvent, TabletToolButtonEvent,
    TabletToolEventTrait, TabletToolProximityEvent, TabletToolTipEvent,
};
use smithay::reexports::input::event::EventTrait;
use smithay::utils::{Logical, Point, Serial};
use smithay::wayland::tablet_manager::{
    TabletDescriptor, TabletHandle, TabletSeatTrait, TabletToolHandle,
};
use wayland_server::protocol::wl_surface::WlSurface;

use crate::state::CortenCompositor;

/// Transforms raw stylus pressure (`0.0..=1.0`) according to user-selected curve (`"soft"`, `"linear"`, `"firm"`).
pub fn apply_tablet_pressure_curve(raw: f64, curve: &str) -> f64 {
    let p = raw.clamp(0.0, 1.0);
    match curve {
        "soft" => p.powf(0.65),
        "firm" => p.powf(1.55),
        _ => p,
    }
}

/// Applies optional 180-degree left-handed tablet coordinate inversion.
pub fn transform_tablet_coords(
    x: f64,
    y: f64,
    screen_w: u32,
    screen_h: u32,
    left_handed: bool,
) -> (f64, f64) {
    if left_handed {
        (
            (screen_w as f64 - x).clamp(0.0, screen_w as f64),
            (screen_h as f64 - y).clamp(0.0, screen_h as f64),
        )
    } else {
        (x, y)
    }
}

/// Converts a libinput physical tablet tool into Smithay's `TabletToolDescriptor`.
pub fn libinput_tool_to_descriptor(tool: &LibinputTabletTool) -> TabletToolDescriptor {
    let tool_type = match tool.tool_type() {
        Some(smithay::reexports::input::event::tablet_tool::TabletToolType::Pen) => {
            TabletToolType::Pen
        }
        Some(smithay::reexports::input::event::tablet_tool::TabletToolType::Eraser) => {
            TabletToolType::Eraser
        }
        Some(smithay::reexports::input::event::tablet_tool::TabletToolType::Brush) => {
            TabletToolType::Brush
        }
        Some(smithay::reexports::input::event::tablet_tool::TabletToolType::Pencil) => {
            TabletToolType::Pencil
        }
        Some(smithay::reexports::input::event::tablet_tool::TabletToolType::Airbrush) => {
            TabletToolType::Airbrush
        }
        Some(smithay::reexports::input::event::tablet_tool::TabletToolType::Mouse) => {
            TabletToolType::Mouse
        }
        Some(smithay::reexports::input::event::tablet_tool::TabletToolType::Lens) => {
            TabletToolType::Lens
        }
        _ => TabletToolType::Unknown,
    };

    let mut capabilities = TabletToolCapabilities::empty();
    capabilities.set(TabletToolCapabilities::TILT, tool.has_tilt());
    capabilities.set(TabletToolCapabilities::PRESSURE, tool.has_pressure());
    capabilities.set(TabletToolCapabilities::DISTANCE, tool.has_distance());
    capabilities.set(TabletToolCapabilities::ROTATION, tool.has_rotation());
    capabilities.set(TabletToolCapabilities::SLIDER, tool.has_slider());
    capabilities.set(TabletToolCapabilities::WHEEL, tool.has_wheel());

    TabletToolDescriptor {
        tool_type,
        hardware_serial: tool.serial(),
        hardware_id_wacom: tool.tool_id(),
        capabilities,
    }
}

/// Manages active drawing tablets and stylus tools for the compositor seat.
#[derive(Debug, Default, Clone)]
pub struct TabletState {
    tablets: Arc<Mutex<HashMap<String, TabletHandle>>>,
    tools: Arc<Mutex<HashMap<u64, TabletToolHandle>>>,
}

impl TabletState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Gets or registers a tablet device on the compositor seat.
    pub fn get_or_create_tablet(
        &self,
        state: &CortenCompositor,
        name: &str,
    ) -> TabletHandle {
        let mut tablets = self.tablets.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tablet) = tablets.get(name) {
            return tablet.clone();
        }

        let desc = TabletDescriptor {
            name: name.to_string(),
            usb_id: None,
            syspath: None,
        };

        let tablet = state
            .seat
            .tablet_seat()
            .add_tablet::<CortenCompositor>(&state.display_handle, &desc);
        tablets.insert(name.to_string(), tablet.clone());
        tracing::info!(?name, "Registered graphics tablet with zwp_tablet_v2 protocol");
        tablet
    }

    /// Gets or registers a physical stylus tool on the compositor seat.
    pub fn get_or_create_tool(
        &self,
        state: &mut CortenCompositor,
        desc: &TabletToolDescriptor,
    ) -> TabletToolHandle {
        let mut tools = self.tools.lock().unwrap_or_else(|e| e.into_inner());
        let key = if desc.hardware_serial != 0 {
            desc.hardware_serial
        } else {
            desc.hardware_id_wacom
        };

        if let Some(tool) = tools.get(&key) {
            return tool.clone();
        }

        let dh = state.display_handle.clone();
        let tool = state
            .seat
            .tablet_seat()
            .add_tool::<CortenCompositor>(state, &dh, desc);
        tools.insert(key, tool.clone());
        tracing::info!(
            tool_type = ?desc.tool_type,
            serial = desc.hardware_serial,
            "Registered graphics stylus tool with zwp_tablet_tool_v2 protocol"
        );
        tool
    }

    /// Handles a tablet axis event (motion, pressure, tilt, distance, rotation).
    pub fn handle_axis(
        &self,
        state: &mut CortenCompositor,
        axis: &TabletToolAxisEvent,
        focused_surface: Option<(WlSurface, Point<f64, Logical>)>,
        screen_w: u32,
        screen_h: u32,
        serial: Serial,
    ) {
        let tool_desc = libinput_tool_to_descriptor(&axis.tool());
        let tool = self.get_or_create_tool(state, &tool_desc);
        let device = axis.device();
        let device_name = device.name();
        let tablet = self.get_or_create_tablet(state, device_name);

        let (x, y) = transform_tablet_coords(
            axis.x_transformed(screen_w),
            axis.y_transformed(screen_h),
            screen_w,
            screen_h,
            state.tablet_left_handed,
        );
        let loc = Point::<f64, Logical>::from((x, y));
        state.cursor_location = loc;

        let time = (axis.time_usec() / 1000) as u32;

        if axis.pressure_has_changed() {
            let curved = apply_tablet_pressure_curve(axis.pressure(), &state.tablet_pressure_curve);
            tool.pressure(curved);
        }
        if axis.distance_has_changed() {
            tool.distance(axis.distance());
        }
        if axis.tilt_x_has_changed() || axis.tilt_y_has_changed() {
            tool.tilt((axis.tilt_x(), axis.tilt_y()));
        }
        if axis.rotation_has_changed() {
            tool.rotation(axis.rotation());
        }
        if axis.slider_has_changed() {
            tool.slider_position(axis.slider_position());
        }
        if axis.wheel_has_changed() {
            tool.wheel(axis.wheel_delta(), axis.wheel_delta_discrete() as i32);
        }

        tool.motion(loc, focused_surface, &tablet, serial, time);
    }

    /// Handles a tablet tip event (contact with tablet drawing surface).
    pub fn handle_tip(
        &self,
        state: &mut CortenCompositor,
        tip: &TabletToolTipEvent,
        serial: Serial,
    ) {
        let tool_desc = libinput_tool_to_descriptor(&tip.tool());
        let tool = self.get_or_create_tool(state, &tool_desc);
        let time = (tip.time_usec() / 1000) as u32;

        match tip.tip_state() {
            smithay::reexports::input::event::tablet_tool::TipState::Down => {
                tool.tip_down(serial, time);
            }
            smithay::reexports::input::event::tablet_tool::TipState::Up => {
                tool.tip_up(time);
            }
        }
    }

    /// Handles a tablet proximity event (stylus coming in or out of sensor range).
    pub fn handle_proximity(
        &self,
        state: &mut CortenCompositor,
        prox: &TabletToolProximityEvent,
        focused_surface: Option<(WlSurface, Point<f64, Logical>)>,
        screen_w: u32,
        screen_h: u32,
        serial: Serial,
    ) {
        let tool_desc = libinput_tool_to_descriptor(&prox.tool());
        let tool = self.get_or_create_tool(state, &tool_desc);
        let device = prox.device();
        let device_name = device.name();
        let tablet = self.get_or_create_tablet(state, device_name);

        let time = (prox.time_usec() / 1000) as u32;

        match prox.proximity_state() {
            smithay::reexports::input::event::tablet_tool::ProximityState::In => {
                let (x, y) = transform_tablet_coords(
                    prox.x_transformed(screen_w),
                    prox.y_transformed(screen_h),
                    screen_w,
                    screen_h,
                    state.tablet_left_handed,
                );
                let loc = Point::<f64, Logical>::from((x, y));
                state.cursor_location = loc;

                if let Some(focus) = focused_surface {
                    tool.proximity_in(loc, focus, &tablet, serial, time);
                }
            }
            smithay::reexports::input::event::tablet_tool::ProximityState::Out => {
                tool.proximity_out(time);
            }
        }
    }

    /// Handles stylus barrel / eraser buttons.
    pub fn handle_button(
        &self,
        state: &mut CortenCompositor,
        btn: &TabletToolButtonEvent,
        serial: Serial,
    ) {
        let tool_desc = libinput_tool_to_descriptor(&btn.tool());
        let tool = self.get_or_create_tool(state, &tool_desc);
        let time = (btn.time_usec() / 1000) as u32;

        let button_state = match btn.button_state() {
            smithay::reexports::input::event::pointer::ButtonState::Pressed => {
                ButtonState::Pressed
            }
            smithay::reexports::input::event::pointer::ButtonState::Released => {
                ButtonState::Released
            }
        };

        tool.button(btn.button(), button_state, serial, time);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tablet_state_creation() {
        let state = TabletState::new();
        assert!(state.tablets.lock().unwrap().is_empty());
        assert!(state.tools.lock().unwrap().is_empty());
    }

    #[test]
    fn test_tablet_descriptor_properties() {
        let desc = TabletDescriptor {
            name: "Wacom Intuos Pro".into(),
            usb_id: Some((0x056a, 0x0357)),
            syspath: None,
        };
        assert_eq!(desc.name, "Wacom Intuos Pro");
        assert_eq!(desc.usb_id, Some((0x056a, 0x0357)));
    }

    #[test]
    fn test_tablet_pressure_curve_and_left_handed_rotation() {
        assert!(apply_tablet_pressure_curve(0.5, "soft") > 0.5);
        assert!(apply_tablet_pressure_curve(0.5, "firm") < 0.5);
        assert!((apply_tablet_pressure_curve(0.5, "linear") - 0.5).abs() < 1e-6);
        assert_eq!(
            transform_tablet_coords(200.0, 100.0, 1920, 1080, true),
            (1720.0, 980.0)
        );
    }
}
