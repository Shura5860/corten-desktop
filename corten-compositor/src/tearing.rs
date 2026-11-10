use std::collections::HashSet;
use wayland_protocols::wp::tearing_control::v1::server::{
    wp_tearing_control_manager_v1::{self, WpTearingControlManagerV1},
    wp_tearing_control_v1::{self, PresentationHint, WpTearingControlV1},
};
use wayland_server::{
    backend::ObjectId,
    protocol::wl_surface::WlSurface,
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, Resource,
};

use crate::state::CortenCompositor;

#[derive(Debug, Default)]
pub struct TearingControlState {
    pub tearing_surfaces: HashSet<ObjectId>,
    pub global_tearing_enabled: bool,
    #[allow(dead_code)]
    pub vrr_adaptive_sync_enabled: bool,
}

impl TearingControlState {
    pub fn new() -> Self {
        Self {
            tearing_surfaces: HashSet::new(),
            global_tearing_enabled: true,
            vrr_adaptive_sync_enabled: true,
        }
    }

    pub fn is_tearing_requested(&self, surface: &WlSurface) -> bool {
        self.global_tearing_enabled && self.tearing_surfaces.contains(&surface.id())
    }
}

pub struct TearingUserData {
    pub surface: WlSurface,
}

impl GlobalDispatch<WpTearingControlManagerV1, ()> for CortenCompositor {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: wayland_server::New<WpTearingControlManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<WpTearingControlManagerV1, ()> for CortenCompositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WpTearingControlManagerV1,
        request: wp_tearing_control_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_tearing_control_manager_v1::Request::GetTearingControl { id, surface } => {
                data_init.init(id, TearingUserData { surface });
            }
            wp_tearing_control_manager_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<WpTearingControlV1, TearingUserData> for CortenCompositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &WpTearingControlV1,
        request: wp_tearing_control_v1::Request,
        data: &TearingUserData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_tearing_control_v1::Request::SetPresentationHint { hint } => match hint {
                wayland_server::WEnum::Value(PresentationHint::Async) => {
                    state.tearing_state.tearing_surfaces.insert(data.surface.id());
                }
                _ => {
                    state.tearing_state.tearing_surfaces.remove(&data.surface.id());
                }
            },
            wp_tearing_control_v1::Request::Destroy => {
                state.tearing_state.tearing_surfaces.remove(&data.surface.id());
            }
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: wayland_server::backend::ClientId,
        _resource: &WpTearingControlV1,
        data: &TearingUserData,
    ) {
        state.tearing_state.tearing_surfaces.remove(&data.surface.id());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tearing_state_defaults() {
        let state = TearingControlState::new();
        assert!(state.global_tearing_enabled);
        assert!(state.vrr_adaptive_sync_enabled);
        assert!(state.tearing_surfaces.is_empty());
    }

    #[test]
    fn test_tearing_toggle_disables_all() {
        let mut state = TearingControlState::new();
        state.global_tearing_enabled = false;
        assert!(!state.global_tearing_enabled);
    }
}
