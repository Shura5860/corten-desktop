use crate::config::{
    DisplaySettingsConfig, InputSettingsConfig, PowerSettingsConfig, SoundSettingsConfig,
};
use corten_services::{pipewire, power_profiles};

fn apply_power_profile(profile: &str) -> Result<(), String> {
    power_profiles::set_active_profile(profile).map_err(|error| error.to_string())
}

pub fn apply_sound(config: &SoundSettingsConfig) -> Result<(), String> {
    let mut errors = Vec::new();
    let output_node = config.output_device.trim();
    if !output_node.is_empty() && output_node != "Default Audio Device" {
        if let Err(error) = pipewire::set_default(output_node) {
            errors.push(error.to_string());
        }
    }
    for node in ["@DEFAULT_AUDIO_SINK@", "@DEFAULT_AUDIO_SOURCE@"] {
        let percent = if node == "@DEFAULT_AUDIO_SINK@" {
            config.output_volume
        } else {
            config.input_volume
        };
        if let Err(error) = pipewire::set_volume(node, percent) {
            errors.push(error.to_string());
        }
    }
    if let Err(error) = pipewire::set_mute("@DEFAULT_AUDIO_SINK@", config.mute_output) {
        errors.push(error.to_string());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub fn apply_power(config: &PowerSettingsConfig) -> Result<(), String> {
    apply_power_profile(&config.power_profile)?;
    // Idle screen blanking is handled by corten-compositor from power.toml;
    // no desktop-specific settings daemon is required.
    Ok(())
}

pub fn apply_display(config: &DisplaySettingsConfig) -> Result<(), String> {
    let _ = config;
    // Output scale, arrangement, and mode are owned by corten-compositor and
    // applied through its D-Bus reload path. No external compositor utility is
    // required here.
    Ok(())
}

pub fn validate_input(config: &InputSettingsConfig) -> Result<(), String> {
    if config.keyboard_layout.trim().is_empty() {
        return Err("keyboard layout cannot be empty".into());
    }
    if !(-100..=100).contains(&config.pointer_speed) {
        return Err("pointer speed must be between -100 and 100".into());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectedDisplayInfo {
    pub name: String,
    pub is_connected: bool,
    pub modes: Vec<String>,
}

pub fn detect_drm_outputs() -> Vec<ConnectedDisplayInfo> {
    let mut outputs = Vec::new();
    let drm_path = std::path::Path::new("/sys/class/drm");
    if let Ok(entries) = std::fs::read_dir(drm_path) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("card") && name.contains('-') {
                let status_path = entry.path().join("status");
                if let Ok(status) = std::fs::read_to_string(&status_path) {
                    let is_connected = status.trim() == "connected";
                    let port_name = name.split_once('-').map(|x| x.1).unwrap_or(&name).to_string();
                    let mut modes = Vec::new();
                    let modes_path = entry.path().join("modes");
                    if let Ok(modes_str) = std::fs::read_to_string(&modes_path) {
                        for m in modes_str.lines() {
                            let trimmed = m.trim().to_string();
                            if !trimmed.is_empty() && !modes.contains(&trimmed) {
                                modes.push(trimmed);
                            }
                        }
                    }
                    outputs.push(ConnectedDisplayInfo {
                        name: port_name,
                        is_connected,
                        modes,
                    });
                }
            }
        }
    }
    outputs.sort_by(|a, b| {
        // Connected outputs first
        match (a.is_connected, b.is_connected) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        }
    });
    outputs
}

#[cfg(test)]
mod tests {
    use super::validate_input;
    use crate::config::InputSettingsConfig;

    #[test]
    fn rejects_empty_keyboard_layout() {
        let mut config = InputSettingsConfig::default();
        config.keyboard_layout.clear();
        assert!(validate_input(&config).is_err());
    }

    #[test]
    fn accepts_supported_pointer_speed() {
        let config = InputSettingsConfig {
            pointer_speed: 100,
            ..Default::default()
        };
        assert!(validate_input(&config).is_ok());
    }
}
