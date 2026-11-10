//! # Corten IPC (`corten-ipc`)
//!
//! Standard D-Bus interfaces and CLI communication helpers for Corten Desktop.

use thiserror::Error;
use tracing::info;
use zbus::connection::Builder;
use zbus::{proxy, Connection};

#[derive(Error, Debug)]
pub enum IpcError {
    #[error("D-Bus connection error: {0}")]
    Zbus(#[from] zbus::Error),
    #[error("Target service not running or method call failed: {0}")]
    CallFailed(String),
}

pub const BUS_NAME_PREFIX: &str = "org.corten";

/// D-Bus Proxy for controlling the Application Launcher
#[proxy(
    default_service = "org.corten.Launcher",
    default_path = "/org/corten/Launcher",
    interface = "org.corten.Launcher"
)]
pub trait LauncherControl {
    /// Toggles visibility of the launcher
    fn toggle(&self) -> zbus::Result<()>;
    /// Explicitly opens the launcher
    fn open(&self) -> zbus::Result<()>;
    /// Explicitly closes the launcher
    fn close(&self) -> zbus::Result<()>;
    /// Reloads configuration from disk immediately
    fn reload_config(&self) -> zbus::Result<()>;
}

/// D-Bus Proxy for controlling the Notification Server
#[proxy(
    default_service = "org.corten.Notification",
    default_path = "/org/corten/Notification",
    interface = "org.corten.Notification"
)]
pub trait NotificationControl {
    /// Toggles Do-Not-Disturb mode
    fn toggle_dnd(&self) -> zbus::Result<bool>;
    /// Gets current Do-Not-Disturb status
    fn get_dnd(&self) -> zbus::Result<bool>;
    /// Clears all visible notification popups
    fn clear_all(&self) -> zbus::Result<()>;
    /// Reloads configuration from disk immediately
    fn reload_config(&self) -> zbus::Result<()>;
}

/// D-Bus Proxy for controlling the Panel / Status Bar
#[proxy(
    default_service = "org.corten.Panel",
    default_path = "/org/corten/Panel",
    interface = "org.corten.Panel"
)]
pub trait PanelControl {
    /// Toggles visibility of the status bar
    fn toggle_visibility(&self) -> zbus::Result<()>;
    /// Triggers a specific module (e.g. "calendar", "clipboard", "power")
    fn trigger_module(&self, module_name: &str) -> zbus::Result<()>;
    /// Reloads configuration from disk immediately
    fn reload_config(&self) -> zbus::Result<()>;
}

/// D-Bus Proxy for controlling the Compositor
#[proxy(
    default_service = "org.corten.Compositor",
    default_path = "/org/corten/Compositor",
    interface = "org.corten.Compositor"
)]
pub trait CompositorControl {
    /// Reloads configuration from disk immediately
    fn reload_config(&self) -> zbus::Result<()>;
    /// Switches the compositor workspace without a desktop-environment API.
    fn switch_workspace(&self, workspace: u32) -> zbus::Result<()>;
    /// Focuses a specific window by its stable Wayland surface protocol ID.
    fn focus_window(&self, window_id: u64) -> zbus::Result<()>;
    /// Toggles showing the desktop (minimizing/restoring or switching to an empty workspace).
    fn toggle_show_desktop(&self) -> zbus::Result<()>;
    /// Toggles the Exposé / Window Overview mode.
    fn toggle_overview(&self) -> zbus::Result<()>;
    /// Captures a screenshot directly from the compositor framebuffer (or region via slurp) to PNG and clipboard.
    fn take_screenshot(&self, region: bool) -> zbus::Result<()>;
    /// Returns the currently active workspace number (1..=9).
    fn get_active_workspace(&self) -> zbus::Result<u32>;
    /// Returns the active workspace number (1..=9) for a specific display output.
    fn get_active_workspace_for_output(&self, output_name: &str) -> zbus::Result<u32>;
    /// Returns the list of workspaces (1..=9) that currently contain at least one window.
    fn get_occupied_workspaces(&self) -> zbus::Result<Vec<u32>>;
    /// Returns the occupied workspaces for a specific display output.
    fn get_occupied_workspaces_for_output(&self, output_name: &str) -> zbus::Result<Vec<u32>>;
    /// Returns map of display output names to active workspaces.
    fn get_output_workspaces(&self) -> zbus::Result<std::collections::HashMap<String, u32>>;
    /// Switches the workspace on a specific display output.
    fn switch_workspace_on_output(&self, output_name: &str, workspace: u32) -> zbus::Result<()>;
    /// Returns snapshots of all managed windows: (id, app_id, title, workspace, is_active, is_floating).
    fn get_windows(&self) -> zbus::Result<Vec<(u64, String, String, u32, bool, bool)>>;
    /// High-performance atomic snapshot: returns (active_workspace, occupied_workspaces, windows) in a single D-Bus RPC.
    fn get_full_state(&self, output_name: &str) -> zbus::Result<(u32, Vec<u32>, Vec<(u64, String, String, u32, bool, bool)>)>;
}

/// D-Bus proxy for the independent Corten capture service.
#[proxy(
    default_service = "org.corten.Capture",
    default_path = "/org/corten/Capture",
    interface = "org.corten.Capture"
)]
pub trait CaptureControl {
    fn start_recording(&self, output: &str, fps: u32, monitor_only: bool) -> zbus::Result<()>;
    fn stop_recording(&self) -> zbus::Result<()>;
    fn get_recording(&self) -> zbus::Result<bool>;
}

/// Helper to connect to the user session D-Bus
pub async fn get_session_connection() -> Result<Connection, IpcError> {
    let connection = Connection::session().await?;
    Ok(connection)
}

/// Registers an object on the user session bus under a specific service name
pub async fn serve_interface<T: zbus::object_server::Interface>(
    service_name: &str,
    object_path: &str,
    interface: T,
) -> Result<Connection, IpcError> {
    let full_service_name = format!("{}.{}", BUS_NAME_PREFIX, service_name);
    let connection = Builder::session()?
        .name(full_service_name.as_str())?
        .serve_at(object_path, interface)?
        .build()
        .await?;

    info!(
        "D-Bus service {} registered at {}",
        full_service_name, object_path
    );
    Ok(connection)
}

/// Standard UNIX Domain Event Socket path for instant push notifications from Compositor to Shell
pub fn event_socket_path() -> std::path::PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        std::path::PathBuf::from(runtime_dir).join("corten-events.sock")
    } else {
        let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
        std::path::PathBuf::from(format!("/tmp/corten-events-{}.sock", user))
    }
}
