use std::process::Command;
use tracing::{info, warn};

pub struct PowerActions;

impl PowerActions {
    pub fn shutdown() {
        info!("PowerAction: Shutdown requested");
        if Command::new("systemctl").arg("poweroff").spawn().is_err()
            && Command::new("loginctl").arg("poweroff").spawn().is_err()
        {
            warn!("Failed to execute poweroff via systemctl and loginctl");
        }
    }

    pub fn reboot() {
        info!("PowerAction: Reboot requested");
        if Command::new("systemctl").arg("reboot").spawn().is_err()
            && Command::new("loginctl").arg("reboot").spawn().is_err()
        {
            warn!("Failed to execute reboot via systemctl and loginctl");
        }
    }

    pub fn suspend() {
        info!("PowerAction: Suspend requested");
        if Command::new("systemctl").arg("suspend").spawn().is_err()
            && Command::new("loginctl").arg("suspend").spawn().is_err()
        {
            warn!("Failed to execute suspend via systemctl and loginctl");
        }
    }
}
