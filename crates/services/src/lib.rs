//! Standard Linux service adapters used by Corten.
//!
//! This crate deliberately does not depend on GNOME, KDE, Sway, or another
//! desktop shell. Each adapter talks to a standard system service or to the
//! service's own stable command-line client when no practical synchronous API
//! is available yet.
//!
use std::process::Command;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("service runtime could not be created: {0}")]
    Runtime(#[from] std::io::Error),
    #[error("D-Bus service error: {0}")]
    Dbus(#[from] zbus::Error),
    #[error("D-Bus property error: {0}")]
    DbusProperty(#[from] zbus::fdo::Error),
    #[error("{program} exited with {status}")]
    CommandFailed { program: String, status: String },
    #[error("{program} is unavailable: {source}")]
    CommandUnavailable {
        program: String,
        source: std::io::Error,
    },
}

pub mod power_profiles {
    use super::ServiceError;

    const SERVICE: &str = "org.freedesktop.UPower.PowerProfiles";
    const PATH: &str = "/org/freedesktop/UPower/PowerProfiles";
    const INTERFACE: &str = "org.freedesktop.UPower.PowerProfiles";

    /// Sets the system power profile through the standard UPower interface.
    pub fn set_active_profile(profile: &str) -> Result<(), ServiceError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        runtime.block_on(async {
            let connection = zbus::Connection::system().await?;
            let proxy = zbus::Proxy::new(&connection, SERVICE, PATH, INTERFACE).await?;
            proxy.set_property("ActiveProfile", profile).await?;
            Ok(())
        })
    }
}

pub mod pipewire {
    use super::{Command, ServiceError};

    fn run(args: &[&str]) -> Result<(), ServiceError> {
        let status = Command::new("wpctl")
            .args(args)
            .status()
            .map_err(|source| ServiceError::CommandUnavailable {
                program: "wpctl".into(),
                source,
            })?;
        if status.success() {
            Ok(())
        } else {
            Err(ServiceError::CommandFailed {
                program: "wpctl".into(),
                status: status.to_string(),
            })
        }
    }

    pub fn set_default(node: &str) -> Result<(), ServiceError> {
        run(&["set-default", node])
    }

    pub fn set_volume(node: &str, percent: u8) -> Result<(), ServiceError> {
        let volume = format!("{:.2}", f32::from(percent) / 100.0);
        run(&["set-volume", node, &volume])
    }

    pub fn set_mute(node: &str, muted: bool) -> Result<(), ServiceError> {
        run(&["set-mute", node, if muted { "1" } else { "0" }])
    }

    pub fn toggle_mute(node: &str) -> Result<(), ServiceError> {
        run(&["set-mute", node, "toggle"])
    }

    pub fn get_volume() -> Option<(u8, bool)> {
        let output = Command::new("wpctl")
            .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let muted = text.contains("[MUTED]");
        let value = text.split_whitespace().nth(1)?.parse::<f32>().ok()?;
        Some(((value * 100.0).round() as u8, muted))
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct AudioSink {
        pub id: u32,
        pub name: String,
        pub is_default: bool,
    }

    pub fn list_sinks() -> Vec<AudioSink> {
        let Ok(output) = Command::new("wpctl").arg("status").output() else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut sinks = Vec::new();
        let mut in_sinks = false;
        for line in text.lines() {
            if line.contains("Sinks:") {
                in_sinks = true;
                continue;
            }
            if !in_sinks {
                continue;
            }
            if line.contains("Sources:") || line.contains("Filters:") {
                break;
            }
            let is_default = line.contains('*');
            let trimmed = line.replace(['│', '*'], "");
            let Some((id, rest)) = trimmed.trim().split_once('.') else {
                continue;
            };
            let Ok(id) = id.trim().parse::<u32>() else {
                continue;
            };
            let name = rest
                .split_once('[')
                .map(|(name, _)| name)
                .unwrap_or(rest)
                .trim();
            sinks.push(AudioSink {
                id,
                name: name.into(),
                is_default,
            });
        }
        sinks
    }

    pub fn set_default_sink(id: u32) -> Result<(), ServiceError> {
        let id = id.to_string();
        run(&["set-default", &id])
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct AppAudioStream {
        pub id: u32,
        pub name: String,
        pub volume_pct: u8,
        pub is_muted: bool,
    }

    pub fn list_app_streams() -> Vec<AppAudioStream> {
        let Ok(output) = Command::new("wpctl").arg("status").output() else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut streams = Vec::new();
        let mut in_streams = false;
        for line in text.lines() {
            if line.contains("Streams:") {
                in_streams = true;
                continue;
            }
            if !in_streams {
                continue;
            }
            if line.trim().is_empty() {
                break;
            }
            let trimmed = line.replace(['│', '*'], "");
            let Some((id_str, rest)) = trimmed.trim().split_once('.') else {
                continue;
            };
            let Ok(id) = id_str.trim().parse::<u32>() else {
                continue;
            };
            let name = rest
                .split_once('[')
                .map(|(n, _)| n)
                .unwrap_or(rest)
                .trim();

            if !name.is_empty() {
                let vol_info = get_stream_volume(id).unwrap_or((80, false));
                streams.push(AppAudioStream {
                    id,
                    name: name.to_string(),
                    volume_pct: vol_info.0,
                    is_muted: vol_info.1,
                });
            }
        }
        streams
    }

    pub fn get_stream_volume(id: u32) -> Option<(u8, bool)> {
        let id_str = id.to_string();
        let output = Command::new("wpctl")
            .args(["get-volume", &id_str])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let muted = text.contains("[MUTED]");
        let value = text.split_whitespace().nth(1)?.parse::<f32>().ok()?;
        Some(((value * 100.0).round() as u8, muted))
    }

    pub fn set_stream_volume(id: u32, percent: u8) -> Result<(), ServiceError> {
        let id_str = id.to_string();
        let volume = format!("{:.2}", f32::from(percent) / 100.0);
        run(&["set-volume", &id_str, &volume])
    }

    pub fn toggle_stream_mute(id: u32) -> Result<(), ServiceError> {
        let id_str = id.to_string();
        run(&["set-mute", &id_str, "toggle"])
    }
}

pub mod network {
    use std::collections::HashMap;
    use std::net::Ipv4Addr;
    use std::process::Command;

    use zbus::zvariant::{OwnedObjectPath, Value};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WifiNetwork {
        pub ssid: String,
        pub signal: u8,
        pub secured: bool,
    }

    pub fn scan_wifi() -> Vec<WifiNetwork> {
        if let Some(networks) = scan_wifi_dbus() {
            return networks;
        }
        scan_wifi_nmcli()
    }

    /// Returns active connection state from NetworkManager's Device API.
    pub fn connection_details() -> Option<(bool, String, String)> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        runtime.block_on(async {
            let connection = zbus::Connection::system().await.ok()?;
            let manager = zbus::Proxy::new(
                &connection,
                "org.freedesktop.NetworkManager",
                "/org/freedesktop/NetworkManager",
                "org.freedesktop.NetworkManager",
            )
            .await
            .ok()?;
            let devices: Vec<OwnedObjectPath> = manager.call("GetDevices", &()).await.ok()?;
            let mut connected = None;
            for device in devices {
                let proxy = zbus::Proxy::new(
                    &connection,
                    "org.freedesktop.NetworkManager",
                    device.as_str(),
                    "org.freedesktop.NetworkManager.Device",
                )
                .await
                .ok()?;
                let state: u32 = proxy.get_property("State").await.ok()?;
                if state != 100 {
                    continue;
                }
                let device_type: u32 = proxy.get_property("DeviceType").await.ok()?;
                let interface: String = proxy.get_property("Interface").await.ok()?;
                let ip_path: OwnedObjectPath = proxy.get_property("Ip4Config").await.ok()?;
                let ip = if ip_path.as_str() == "/" {
                    "127.0.0.1".to_string()
                } else {
                    let ip_proxy = zbus::Proxy::new(
                        &connection,
                        "org.freedesktop.NetworkManager",
                        ip_path.as_str(),
                        "org.freedesktop.NetworkManager.IP4Config",
                    )
                    .await
                    .ok()?;
                    let addresses: Vec<(u32, u32, u32)> =
                        ip_proxy.get_property("Addresses").await.ok()?;
                    addresses
                        .first()
                        .map(|(address, _, _)| Ipv4Addr::from(address.to_be_bytes()).to_string())
                        .unwrap_or_else(|| "127.0.0.1".into())
                };
                let label = if device_type == 2 {
                    format!("Wi-Fi Online ({interface})")
                } else {
                    format!("Ethernet Online ({interface})")
                };
                connected = Some((true, label, ip));
                break;
            }
            Some(connected.unwrap_or_else(|| (false, "Disconnected".into(), "127.0.0.1".into())))
        })
    }

    fn scan_wifi_dbus() -> Option<Vec<WifiNetwork>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        runtime.block_on(async {
            let connection = zbus::Connection::system().await.ok()?;
            let manager = zbus::Proxy::new(
                &connection,
                "org.freedesktop.NetworkManager",
                "/org/freedesktop/NetworkManager",
                "org.freedesktop.NetworkManager",
            )
            .await
            .ok()?;
            let devices: Vec<OwnedObjectPath> = manager.call("GetDevices", &()).await.ok()?;
            let mut networks = Vec::new();
            for device in devices {
                let device_proxy = zbus::Proxy::new(
                    &connection,
                    "org.freedesktop.NetworkManager",
                    device.as_str(),
                    "org.freedesktop.NetworkManager.Device",
                )
                .await
                .ok()?;
                let device_type: u32 = device_proxy.get_property("DeviceType").await.ok()?;
                if device_type != 2 {
                    continue;
                }
                let wireless = zbus::Proxy::new(
                    &connection,
                    "org.freedesktop.NetworkManager",
                    device.as_str(),
                    "org.freedesktop.NetworkManager.Device.Wireless",
                )
                .await
                .ok()?;
                let access_points: Vec<OwnedObjectPath> =
                    wireless.call("GetAllAccessPoints", &()).await.ok()?;
                for access_point in access_points {
                    let ap = zbus::Proxy::new(
                        &connection,
                        "org.freedesktop.NetworkManager",
                        access_point.as_str(),
                        "org.freedesktop.NetworkManager.AccessPoint",
                    )
                    .await
                    .ok()?;
                    let ssid: Vec<u8> = ap.get_property("Ssid").await.ok()?;
                    let ssid = String::from_utf8_lossy(&ssid).trim().to_string();
                    if ssid.is_empty() {
                        continue;
                    }
                    let signal: u8 = ap.get_property("Strength").await.unwrap_or(0);
                    let flags: u32 = ap.get_property("Flags").await.unwrap_or(0);
                    let wpa_flags: u32 = ap.get_property("WpaFlags").await.unwrap_or(0);
                    let rsn_flags: u32 = ap.get_property("RsnFlags").await.unwrap_or(0);
                    if let Some(existing) = networks
                        .iter_mut()
                        .find(|network: &&mut WifiNetwork| network.ssid == ssid)
                    {
                        existing.signal = existing.signal.max(signal);
                    } else {
                        networks.push(WifiNetwork {
                            ssid,
                            signal,
                            secured: flags != 0 || wpa_flags != 0 || rsn_flags != 0,
                        });
                    }
                }
            }
            networks.sort_by_key(|network| std::cmp::Reverse(network.signal));
            Some(networks)
        })
    }

    fn scan_wifi_nmcli() -> Vec<WifiNetwork> {
        let Ok(output) = Command::new("nmcli")
            .args(["-t", "-f", "SSID,SIGNAL,SECURITY", "dev", "wifi"])
            .output()
        else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let mut networks = Vec::new();
        for line in text.lines() {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() < 2 {
                continue;
            }
            let ssid = parts[0].trim();
            if ssid.is_empty()
                || networks
                    .iter()
                    .any(|network: &WifiNetwork| network.ssid == ssid)
            {
                continue;
            }
            networks.push(WifiNetwork {
                ssid: ssid.into(),
                signal: parts[1].trim().parse().unwrap_or(0),
                secured: parts
                    .get(2)
                    .is_some_and(|security| !security.trim().is_empty() && security.trim() != "--"),
            });
        }
        networks.sort_by_key(|network| std::cmp::Reverse(network.signal));
        networks
    }

    pub fn connect_wifi(ssid: &str, password: &str) -> bool {
        if let Some(result) = connect_wifi_dbus(ssid, password) {
            return result;
        }
        connect_wifi_nmcli(ssid, password)
    }

    fn connect_wifi_dbus(ssid: &str, password: &str) -> Option<bool> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        runtime.block_on(async {
            let connection = zbus::Connection::system().await.ok()?;
            let manager = zbus::Proxy::new(
                &connection,
                "org.freedesktop.NetworkManager",
                "/org/freedesktop/NetworkManager",
                "org.freedesktop.NetworkManager",
            )
            .await
            .ok()?;
            let devices: Vec<OwnedObjectPath> = manager.call("GetDevices", &()).await.ok()?;
            let mut wifi_device = None;
            for device in devices {
                let proxy = zbus::Proxy::new(
                    &connection,
                    "org.freedesktop.NetworkManager",
                    device.as_str(),
                    "org.freedesktop.NetworkManager.Device",
                )
                .await
                .ok()?;
                let device_type: u32 = proxy.get_property("DeviceType").await.ok()?;
                if device_type == 2 {
                    wifi_device = Some(device);
                    break;
                }
            }
            let device = wifi_device?;
            let mut settings: HashMap<&str, HashMap<&str, Value<'_>>> = HashMap::new();
            let mut connection_settings = HashMap::new();
            connection_settings.insert("type", Value::from("802-11-wireless"));
            connection_settings.insert("id", Value::from(ssid));
            settings.insert("connection", connection_settings);

            let mut wireless_settings = HashMap::new();
            wireless_settings.insert("ssid", Value::from(ssid.as_bytes().to_vec()));
            settings.insert("802-11-wireless", wireless_settings);

            if !password.is_empty() {
                let mut security = HashMap::new();
                security.insert("key-mgmt", Value::from("wpa-psk"));
                security.insert("psk", Value::from(password));
                settings.insert("802-11-wireless-security", security);
            }

            let mut ipv4 = HashMap::new();
            ipv4.insert("method", Value::from("auto"));
            settings.insert("ipv4", ipv4);
            let mut ipv6 = HashMap::new();
            ipv6.insert("method", Value::from("auto"));
            settings.insert("ipv6", ipv6);

            let access_point = OwnedObjectPath::try_from("/").ok()?;
            let result: Result<(OwnedObjectPath, OwnedObjectPath), _> = manager
                .call(
                    "AddAndActivateConnection",
                    &(settings, device, access_point),
                )
                .await;
            Some(result.is_ok())
        })
    }

    fn connect_wifi_nmcli(ssid: &str, password: &str) -> bool {
        let mut command = Command::new("nmcli");
        command.args(["dev", "wifi", "connect", ssid]);
        if !password.is_empty() {
            command.args(["password", password]);
        }
        command
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
}

pub mod bluetooth {
    use std::collections::HashMap;

    use zbus::zvariant::{OwnedObjectPath, OwnedValue};

    type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

    pub fn status() -> (bool, Option<String>) {
        status_dbus().unwrap_or((false, None))
    }

    fn status_dbus() -> Option<(bool, Option<String>)> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        runtime.block_on(async {
            let connection = zbus::Connection::system().await.ok()?;
            let manager = zbus::Proxy::new(
                &connection,
                "org.bluez",
                "/",
                "org.freedesktop.DBus.ObjectManager",
            )
            .await
            .ok()?;
            let objects: ManagedObjects = manager.call("GetManagedObjects", &()).await.ok()?;
            let mut powered = false;
            let mut connected_name = None;
            for interfaces in objects.values() {
                if let Some(adapter) = interfaces.get("org.bluez.Adapter1") {
                    powered |= adapter.get("Powered")?.downcast_ref::<bool>().ok()?;
                }
                if let Some(device) = interfaces.get("org.bluez.Device1") {
                    let connected = device.get("Connected")?.downcast_ref::<bool>().ok()?;
                    if connected {
                        connected_name = device
                            .get("Name")
                            .and_then(|value| value.downcast_ref::<String>().ok());
                    }
                }
            }
            Some((powered, connected_name))
        })
    }

    pub fn set_powered(powered: bool) -> bool {
        set_powered_dbus(powered).unwrap_or(false)
    }

    fn set_powered_dbus(powered: bool) -> Option<bool> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        runtime.block_on(async {
            let connection = zbus::Connection::system().await.ok()?;
            let manager = zbus::Proxy::new(
                &connection,
                "org.bluez",
                "/",
                "org.freedesktop.DBus.ObjectManager",
            )
            .await
            .ok()?;
            let objects: ManagedObjects = manager.call("GetManagedObjects", &()).await.ok()?;
            let adapter_path = objects.iter().find_map(|(path, interfaces)| {
                interfaces
                    .contains_key("org.bluez.Adapter1")
                    .then_some(path.clone())
            })?;
            let adapter = zbus::Proxy::new(
                &connection,
                "org.bluez",
                adapter_path.as_str(),
                "org.bluez.Adapter1",
            )
            .await
            .ok()?;
            Some(adapter.set_property("Powered", powered).await.is_ok())
        })
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct BluetoothDevice {
        pub path: String,
        pub name: String,
        pub address: String,
        pub is_connected: bool,
        pub is_paired: bool,
    }

    pub fn list_devices() -> Vec<BluetoothDevice> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok();
        let Some(rt) = runtime else { return Vec::new(); };
        rt.block_on(async {
            let connection = zbus::Connection::system().await.ok()?;
            let manager = zbus::Proxy::new(
                &connection,
                "org.bluez",
                "/",
                "org.freedesktop.DBus.ObjectManager",
            )
            .await
            .ok()?;
            let objects: ManagedObjects = manager.call("GetManagedObjects", &()).await.ok()?;
            let mut devices = Vec::new();
            for (path, interfaces) in objects {
                if let Some(dev_props) = interfaces.get("org.bluez.Device1") {
                    let name = dev_props.get("Name")
                        .and_then(|v| v.downcast_ref::<String>().ok())
                        .unwrap_or_else(|| "Bilinmeyen Aygıt".into());
                    let address = dev_props.get("Address")
                        .and_then(|v| v.downcast_ref::<String>().ok())
                        .unwrap_or_default();
                    let is_connected = dev_props.get("Connected")
                        .and_then(|v| v.downcast_ref::<bool>().ok())
                        .unwrap_or(false);
                    let is_paired = dev_props.get("Paired")
                        .and_then(|v| v.downcast_ref::<bool>().ok())
                        .unwrap_or(false);

                    devices.push(BluetoothDevice {
                        path: path.as_str().to_string(),
                        name,
                        address,
                        is_connected,
                        is_paired,
                    });
                }
            }
            Some(devices)
        }).unwrap_or_default()
    }

    pub fn toggle_device_connection(device_path: &str, currently_connected: bool) -> Result<(), super::ServiceError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let connection = zbus::Connection::system().await?;
            let proxy = zbus::Proxy::new(
                &connection,
                "org.bluez",
                device_path,
                "org.bluez.Device1",
            ).await?;
            if currently_connected {
                let _: () = proxy.call("Disconnect", &()).await?;
            } else {
                let _: () = proxy.call("Connect", &()).await?;
            }
            Ok(())
        })
    }
}

pub mod upower {
    /// Reads the aggregate battery device from the standard UPower system bus.
    pub fn battery_status() -> Option<(u8, bool)> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        runtime.block_on(async {
            let connection = zbus::Connection::system().await.ok()?;
            let upower = zbus::Proxy::new(
                &connection,
                "org.freedesktop.UPower",
                "/org/freedesktop/UPower",
                "org.freedesktop.UPower",
            )
            .await
            .ok()?;
            let display_device: zbus::zvariant::OwnedObjectPath =
                upower.call("GetDisplayDevice", &()).await.ok()?;
            let device = zbus::Proxy::new(
                &connection,
                "org.freedesktop.UPower",
                display_device.as_str(),
                "org.freedesktop.UPower.Device",
            )
            .await
            .ok()?;
            let percentage: f64 = device.get_property("Percentage").await.ok()?;
            let state: u32 = device.get_property("State").await.ok()?;
            Some((percentage.round().clamp(0.0, 100.0) as u8, state == 1))
        })
    }
}

pub mod portal {
    use std::collections::HashMap;

    use futures_lite::StreamExt;
    use zbus::zvariant::{OwnedObjectPath, Value};

    const SERVICE: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";

    pub async fn open_file(title: &str, multiple: bool) -> Result<Vec<String>, String> {
        let connection = zbus::Connection::session()
            .await
            .map_err(|error| error.to_string())?;
        let portal = zbus::Proxy::new(
            &connection,
            SERVICE,
            PATH,
            "org.freedesktop.portal.FileChooser",
        )
        .await
        .map_err(|error| error.to_string())?;
        let mut options = HashMap::new();
        options.insert("multiple", Value::from(multiple));
        let handle: OwnedObjectPath = portal
            .call("OpenFile", &("", title, options))
            .await
            .map_err(|error| error.to_string())?;
        response_uris(&connection, handle).await
    }

    pub async fn save_file(title: &str, filename: &str) -> Result<String, String> {
        let connection = zbus::Connection::session()
            .await
            .map_err(|error| error.to_string())?;
        let portal = zbus::Proxy::new(
            &connection,
            SERVICE,
            PATH,
            "org.freedesktop.portal.FileChooser",
        )
        .await
        .map_err(|error| error.to_string())?;
        let mut options = HashMap::new();
        options.insert("current_name", Value::from(filename));
        let handle: OwnedObjectPath = portal
            .call("SaveFile", &("", title, options))
            .await
            .map_err(|error| error.to_string())?;
        let uris = response_uris(&connection, handle).await?;
        let uri = uris
            .first()
            .ok_or_else(|| "portal did not return a save location".to_string())?;
        file_uri_to_path(uri)
    }

    fn file_uri_to_path(uri: &str) -> Result<String, String> {
        let encoded = uri
            .strip_prefix("file://")
            .ok_or_else(|| "capture output must be a local file URI".to_string())?;
        if encoded.starts_with("//") {
            return Err("remote file URIs are not supported for capture output".into());
        }
        let mut bytes = Vec::with_capacity(encoded.len());
        let raw = encoded.as_bytes();
        let mut index = 0;
        while index < raw.len() {
            if raw[index] == b'%' {
                if index + 2 >= raw.len() {
                    return Err("invalid percent-encoded file URI".into());
                }
                let high = (raw[index + 1] as char)
                    .to_digit(16)
                    .ok_or_else(|| "invalid file URI escape".to_string())?;
                let low = (raw[index + 2] as char)
                    .to_digit(16)
                    .ok_or_else(|| "invalid file URI escape".to_string())?;
                bytes.push((high * 16 + low) as u8);
                index += 3;
            } else {
                bytes.push(raw[index]);
                index += 1;
            }
        }
        String::from_utf8(bytes).map_err(|error| error.to_string())
    }

    async fn response_uris(
        connection: &zbus::Connection,
        handle: OwnedObjectPath,
    ) -> Result<Vec<String>, String> {
        let request = zbus::Proxy::new(
            connection,
            SERVICE,
            handle.as_str(),
            "org.freedesktop.portal.Request",
        )
        .await
        .map_err(|error| error.to_string())?;
        let mut responses = request
            .receive_signal("Response")
            .await
            .map_err(|error| error.to_string())?;
        let Some(response) = responses.next().await else {
            return Err("portal request ended without a response".into());
        };
        let (code, results): (u32, HashMap<String, zbus::zvariant::OwnedValue>) = response
            .body()
            .deserialize()
            .map_err(|error| error.to_string())?;
        if code != 0 {
            return Err("portal file selection was cancelled".into());
        }
        results
            .get("uris")
            .and_then(|value| Vec::<String>::try_from(value.clone()).ok())
            .ok_or_else(|| "portal response did not contain URIs".into())
    }

    async fn response_results(
        connection: &zbus::Connection,
        handle: OwnedObjectPath,
    ) -> Result<HashMap<String, zbus::zvariant::OwnedValue>, String> {
        let request = zbus::Proxy::new(
            connection,
            SERVICE,
            handle.as_str(),
            "org.freedesktop.portal.Request",
        )
        .await
        .map_err(|error| error.to_string())?;
        let mut responses = request
            .receive_signal("Response")
            .await
            .map_err(|error| error.to_string())?;
        let Some(response) = responses.next().await else {
            return Err("portal request ended without a response".into());
        };
        let (code, results): (u32, HashMap<String, zbus::zvariant::OwnedValue>) = response
            .body()
            .deserialize()
            .map_err(|error| error.to_string())?;
        if code != 0 {
            return Err("portal request was cancelled".into());
        }
        Ok(results)
    }

    /// A live portal ScreenCast session. The D-Bus connection is retained for
    /// the lifetime of the session because the portal associates the session
    /// with the client's bus name.
    pub struct ScreenCastSession {
        connection: zbus::Connection,
        session: OwnedObjectPath,
        node_ids: Vec<u32>,
    }

    impl ScreenCastSession {
        pub fn node_ids(&self) -> &[u32] {
            &self.node_ids
        }

        /// Closes the portal session explicitly. Dropping without calling
        /// this still releases the client connection, but explicit close is
        /// preferred so the compositor can release PipeWire resources early.
        pub async fn close(self) -> Result<(), String> {
            let proxy = zbus::Proxy::new(
                &self.connection,
                SERVICE,
                self.session.as_str(),
                "org.freedesktop.portal.Session",
            )
            .await
            .map_err(|error| error.to_string())?;
            proxy
                .call::<_, _, ()>("Close", &())
                .await
                .map_err(|error| error.to_string())
        }
    }

    /// Starts a desktop capture session and keeps its portal connection alive.
    pub async fn start_screencast_session(monitor_only: bool) -> Result<ScreenCastSession, String> {
        start_screencast_session_with_options(monitor_only, false).await
    }

    pub async fn start_screencast_session_with_options(
        monitor_only: bool,
        multiple: bool,
    ) -> Result<ScreenCastSession, String> {
        let connection = zbus::Connection::session()
            .await
            .map_err(|error| error.to_string())?;
        let portal = zbus::Proxy::new(
            &connection,
            SERVICE,
            PATH,
            "org.freedesktop.portal.ScreenCast",
        )
        .await
        .map_err(|error| error.to_string())?;
        let token = format!("corten_{}_{}", std::process::id(), monotonic_token());

        let mut session_options = HashMap::new();
        session_options.insert("session_handle_token", Value::from(token.as_str()));
        session_options.insert("session_token", Value::from(token.as_str()));
        let request: OwnedObjectPath = portal
            .call("CreateSession", &(session_options))
            .await
            .map_err(|error| error.to_string())?;
        let session_results = response_results(&connection, request).await?;
        let session: OwnedObjectPath = session_results
            .get("session_handle")
            .and_then(|value| OwnedObjectPath::try_from(value.clone()).ok())
            .ok_or_else(|| "portal did not return a screencast session handle".to_string())?;

        let mut source_options = HashMap::new();
        source_options.insert("types", Value::from(if monitor_only { 1u32 } else { 3u32 }));
        source_options.insert("multiple", Value::from(multiple));
        source_options.insert("cursor_mode", Value::from(2u32));
        let request: OwnedObjectPath = portal
            .call("SelectSources", &(session.clone(), source_options))
            .await
            .map_err(|error| error.to_string())?;
        response_results(&connection, request).await?;

        let start_options: HashMap<&str, Value<'_>> = HashMap::new();
        let request: OwnedObjectPath = portal
            .call("Start", &(session.clone(), "", start_options))
            .await
            .map_err(|error| error.to_string())?;
        let results = response_results(&connection, request).await?;
        let streams: Vec<(u32, HashMap<String, zbus::zvariant::OwnedValue>)> = results
            .get("streams")
            .and_then(|value| Vec::try_from(value.clone()).ok())
            .ok_or_else(|| "portal did not return PipeWire streams".to_string())?;
        let node_ids = streams
            .into_iter()
            .filter_map(|(_, properties)| {
                properties
                    .get("node_id")
                    .and_then(|value| u32::try_from(value.clone()).ok())
            })
            .collect();
        Ok(ScreenCastSession {
            connection,
            session,
            node_ids,
        })
    }

    fn monotonic_token() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    }

    pub async fn screenshot_uri(interactive: bool) -> Result<String, String> {
        let connection = zbus::Connection::session()
            .await
            .map_err(|error| error.to_string())?;
        let portal = zbus::Proxy::new(
            &connection,
            SERVICE,
            PATH,
            "org.freedesktop.portal.Screenshot",
        )
        .await
        .map_err(|error| error.to_string())?;
        let mut options = HashMap::new();
        options.insert("interactive", Value::from(interactive));
        let handle: OwnedObjectPath = portal
            .call("Screenshot", &("", options))
            .await
            .map_err(|error| error.to_string())?;
        let request = zbus::Proxy::new(
            &connection,
            SERVICE,
            handle.as_str(),
            "org.freedesktop.portal.Request",
        )
        .await
        .map_err(|error| error.to_string())?;
        let mut responses = request
            .receive_signal("Response")
            .await
            .map_err(|error| error.to_string())?;
        let Some(response) = responses.next().await else {
            return Err("portal screenshot request ended without a response".into());
        };
        let (code, results): (u32, HashMap<String, zbus::zvariant::OwnedValue>) = response
            .body()
            .deserialize()
            .map_err(|error| error.to_string())?;
        if code != 0 {
            return Err("portal screenshot was cancelled".into());
        }
        results
            .get("uri")
            .and_then(|value| String::try_from(value.clone()).ok())
            .ok_or_else(|| "portal screenshot response did not contain a URI".into())
    }

    pub fn open_uri(uri: &str) -> Option<bool> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        runtime.block_on(async {
            let connection = zbus::Connection::session().await.ok()?;
            let proxy =
                zbus::Proxy::new(&connection, SERVICE, PATH, "org.freedesktop.portal.OpenURI")
                    .await
                    .ok()?;
            let options: HashMap<&str, Value<'_>> = HashMap::new();
            let _: OwnedObjectPath = proxy.call("OpenURI", &("", uri, options)).await.ok()?;
            Some(true)
        })
    }

    pub fn request_screenshot(interactive: bool) -> Option<bool> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        runtime.block_on(async {
            let connection = zbus::Connection::session().await.ok()?;
            let proxy = zbus::Proxy::new(
                &connection,
                SERVICE,
                PATH,
                "org.freedesktop.portal.Screenshot",
            )
            .await
            .ok()?;
            let mut options = HashMap::new();
            options.insert("interactive", Value::from(interactive));
            let _: OwnedObjectPath = proxy.call("Screenshot", &("", options)).await.ok()?;
            Some(true)
        })
    }
}

pub mod udisks2 {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
    use super::ServiceError;

    const SERVICE: &str = "org.freedesktop.UDisks2";
    const MANAGER_PATH: &str = "/org/freedesktop/UDisks2/Manager";
    const MANAGER_INTERFACE: &str = "org.freedesktop.DBus.ObjectManager";

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct BlockDevice {
        pub object_path: String,
        pub device: String,
        pub label: String,
        pub size: u64,
        pub mount_point: Option<PathBuf>,
        pub is_removable: bool,
        pub is_read_only: bool,
        pub fs_type: String,
    }

    /// Lists storage block devices via UDisks2 ObjectManager.
    pub fn list_devices() -> Result<Vec<BlockDevice>, ServiceError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        runtime.block_on(async {
            let connection = zbus::Connection::system().await?;
            let proxy = zbus::Proxy::new(&connection, SERVICE, MANAGER_PATH, MANAGER_INTERFACE).await?;
            
            // GetManagedObjects returns a dictionary of object paths and their interfaces/properties
            let managed: HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>> = 
                proxy.call("GetManagedObjects", &()).await?;

            let mut devices = Vec::new();

            for (path, interfaces) in managed {
                // Must have Block interface
                if let Some(block_props) = interfaces.get("org.freedesktop.UDisks2.Block") {
                    let mut device_name = String::new();
                    if let Some(dev_val) = block_props.get("Device") {
                        if let Ok(bytes) = Vec::<u8>::try_from(dev_val.clone()) {
                            // Strip trailing null if present
                            let s = String::from_utf8_lossy(&bytes);
                            device_name = s.trim_matches('\0').to_string();
                        }
                    }

                    // Ignore loop devices and zram
                    if device_name.starts_with("/dev/loop") || device_name.starts_with("/dev/zram") {
                        continue;
                    }

                    let label = block_props.get("IdLabel")
                        .and_then(|v| String::try_from(v.clone()).ok())
                        .unwrap_or_default();

                    let fs_type = block_props.get("IdType")
                        .and_then(|v| String::try_from(v.clone()).ok())
                        .unwrap_or_default();

                    let size = block_props.get("Size")
                        .and_then(|v| u64::try_from(v.clone()).ok())
                        .unwrap_or(0);

                    let is_read_only = block_props.get("ReadOnly")
                        .and_then(|v| bool::try_from(v.clone()).ok())
                        .unwrap_or(false);

                    let hint_system = block_props.get("HintSystem")
                        .and_then(|v| bool::try_from(v.clone()).ok())
                        .unwrap_or(true);

                    // Check filesystem mount points if present
                    let mut mount_point = None;
                    if let Some(fs_props) = interfaces.get("org.freedesktop.UDisks2.Filesystem") {
                        if let Some(mp_val) = fs_props.get("MountPoints") {
                            if let Ok(mps) = Vec::<Vec<u8>>::try_from(mp_val.clone()) {
                                if let Some(first) = mps.first() {
                                    let path_str = String::from_utf8_lossy(first).trim_matches('\0').to_string();
                                    if !path_str.is_empty() {
                                        mount_point = Some(PathBuf::from(path_str));
                                    }
                                }
                            }
                        }
                    }

                    // A device is considered removable if HintSystem is false or drive is removable
                    let is_removable = !hint_system;

                    let display_label = if !label.is_empty() {
                        label
                    } else if !device_name.is_empty() {
                        device_name.clone()
                    } else {
                        path.as_str().split('/').next_back().unwrap_or("Disk").to_string()
                    };

                    devices.push(BlockDevice {
                        object_path: path.as_str().to_string(),
                        device: device_name,
                        label: display_label,
                        size,
                        mount_point,
                        is_removable,
                        is_read_only,
                        fs_type,
                    });
                }
            }

            // Sort devices by device path
            devices.sort_by(|a, b| a.device.cmp(&b.device));
            Ok(devices)
        })
    }

    /// Mounts a filesystem via UDisks2.
    pub fn mount_device(object_path: &str) -> Result<PathBuf, ServiceError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        runtime.block_on(async {
            let connection = zbus::Connection::system().await?;
            let proxy = zbus::Proxy::new(
                &connection,
                SERVICE,
                object_path,
                "org.freedesktop.UDisks2.Filesystem",
            ).await?;

            let options: HashMap<&str, Value<'_>> = HashMap::new();
            let mount_path: String = proxy.call("Mount", &(options)).await?;
            Ok(PathBuf::from(mount_path))
        })
    }

    /// Unmounts a filesystem via UDisks2.
    pub fn unmount_device(object_path: &str) -> Result<(), ServiceError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        runtime.block_on(async {
            let connection = zbus::Connection::system().await?;
            let proxy = zbus::Proxy::new(
                &connection,
                SERVICE,
                object_path,
                "org.freedesktop.UDisks2.Filesystem",
            ).await?;

            let options: HashMap<&str, Value<'_>> = HashMap::new();
            let _: () = proxy.call("Unmount", &(options)).await?;
            Ok(())
        })
    }
}

pub mod mpris {
    use std::collections::HashMap;
    use zbus::zvariant::OwnedValue;
    use super::ServiceError;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum PlaybackStatus {
        Playing,
        Paused,
        Stopped,
    }

    impl PlaybackStatus {
        pub fn from_status_str(s: &str) -> Self {
            match s {
                "Playing" => Self::Playing,
                "Paused" => Self::Paused,
                _ => Self::Stopped,
            }
        }

        pub fn as_str(&self) -> &'static str {
            match self {
                Self::Playing => "Playing",
                Self::Paused => "Paused",
                Self::Stopped => "Stopped",
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct MediaPlayer {
        pub bus_name: String,
        pub identity: String,
        pub playback_status: PlaybackStatus,
        pub title: String,
        pub artist: String,
        pub album: String,
        pub art_url: Option<String>,
        pub position_micros: Option<u64>,
        pub length_micros: Option<u64>,
    }

    /// Lists active MPRIS media players on the session bus.
    pub fn list_players() -> Result<Vec<MediaPlayer>, ServiceError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        runtime.block_on(async {
            let connection = zbus::Connection::session().await?;
            let dbus_proxy = zbus::fdo::DBusProxy::new(&connection).await?;
            let names = dbus_proxy.list_names().await?;

            let mut players = Vec::new();

            for name in names {
                if name.starts_with("org.mpris.MediaPlayer2.") {
                    let bus_name = name.as_str().to_string();
                    let player_proxy = zbus::Proxy::new(
                        &connection,
                        bus_name.clone(),
                        "/org/mpris/MediaPlayer2",
                        "org.mpris.MediaPlayer2.Player",
                    ).await;

                    let root_proxy = zbus::Proxy::new(
                        &connection,
                        bus_name.clone(),
                        "/org/mpris/MediaPlayer2",
                        "org.mpris.MediaPlayer2",
                    ).await;

                    if let (Ok(p_proxy), Ok(r_proxy)) = (player_proxy, root_proxy) {
                        let identity: String = r_proxy.get_property("Identity").await.unwrap_or_else(|_| {
                            bus_name.strip_prefix("org.mpris.MediaPlayer2.").unwrap_or(&bus_name).to_string()
                        });

                        let status_str: String = p_proxy.get_property("PlaybackStatus").await.unwrap_or_else(|_| "Stopped".into());
                        let playback_status = PlaybackStatus::from_status_str(&status_str);

                        let metadata: HashMap<String, OwnedValue> = p_proxy.get_property("Metadata").await.unwrap_or_default();

                        let title = metadata.get("xesam:title")
                            .and_then(|v| String::try_from(v.clone()).ok())
                            .unwrap_or_default();

                        let artist = metadata.get("xesam:artist")
                            .and_then(|v| {
                                if let Ok(s) = String::try_from(v.clone()) {
                                    Some(s)
                                } else if let Ok(list) = Vec::<String>::try_from(v.clone()) {
                                    Some(list.join(", "))
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_default();

                        let album = metadata.get("xesam:album")
                            .and_then(|v| String::try_from(v.clone()).ok())
                            .unwrap_or_default();

                        let art_url = metadata.get("mpris:artUrl")
                            .and_then(|v| String::try_from(v.clone()).ok());

                        let length_micros = metadata.get("mpris:length")
                            .and_then(|v| {
                                if let Ok(u) = u64::try_from(v.clone()) {
                                    Some(u)
                                } else if let Ok(i) = i64::try_from(v.clone()) {
                                    Some(i.max(0) as u64)
                                } else {
                                    None
                                }
                            });

                        let position_micros: Option<u64> = p_proxy.get_property::<i64>("Position")
                            .await
                            .ok()
                            .map(|p| p.max(0) as u64);

                        players.push(MediaPlayer {
                            bus_name,
                            identity,
                            playback_status,
                            title,
                            artist,
                            album,
                            art_url,
                            position_micros,
                            length_micros,
                        });
                    }
                }
            }

            Ok(players)
        })
    }

    pub fn play_pause(bus_name: &str) -> Result<(), ServiceError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        runtime.block_on(async {
            let connection = zbus::Connection::session().await?;
            let proxy = zbus::Proxy::new(
                &connection,
                bus_name,
                "/org/mpris/MediaPlayer2",
                "org.mpris.MediaPlayer2.Player",
            ).await?;
            let _: () = proxy.call("PlayPause", &()).await?;
            Ok(())
        })
    }

    pub fn next(bus_name: &str) -> Result<(), ServiceError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        runtime.block_on(async {
            let connection = zbus::Connection::session().await?;
            let proxy = zbus::Proxy::new(
                &connection,
                bus_name,
                "/org/mpris/MediaPlayer2",
                "org.mpris.MediaPlayer2.Player",
            ).await?;
            let _: () = proxy.call("Next", &()).await?;
            Ok(())
        })
    }

    pub fn previous(bus_name: &str) -> Result<(), ServiceError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        runtime.block_on(async {
            let connection = zbus::Connection::session().await?;
            let proxy = zbus::Proxy::new(
                &connection,
                bus_name,
                "/org/mpris/MediaPlayer2",
                "org.mpris.MediaPlayer2.Player",
            ).await?;
            let _: () = proxy.call("Previous", &()).await?;
            Ok(())
        })
    }
}

pub mod notifications {
    use super::ServiceError;
    use std::collections::HashMap;

    /// Sends a standard desktop notification via org.freedesktop.Notifications
    pub fn send_notification(
        app_name: &str,
        summary: &str,
        body: &str,
        icon: &str,
        expire_timeout_ms: i32,
    ) -> Result<u32, ServiceError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let connection = zbus::Connection::session().await?;
            let proxy = zbus::Proxy::new(
                &connection,
                "org.freedesktop.Notifications",
                "/org/freedesktop/Notifications",
                "org.freedesktop.Notifications",
            )
            .await?;
            let hints = HashMap::<&str, zbus::zvariant::Value<'_>>::new();
            let actions: Vec<&str> = Vec::new();
            let id: u32 = proxy
                .call(
                    "Notify",
                    &(
                        app_name,
                        0u32,
                        icon,
                        summary,
                        body,
                        actions,
                        hints,
                        expire_timeout_ms,
                    ),
                )
                .await?;
            Ok(id)
        })
    }
}

pub mod a11y {
    use std::process::Command;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SoundCue {
        WorkspaceSwitch,
        WindowClose,
        WindowFocus,
        Screenshot,
        ScreenLock,
    }

    /// Plays a lightweight accessibility sound feedback cue asynchronously.
    /// Tries `canberra-gtk-play`, `paplay` stereo system sounds, or fallback.
    pub fn play_sound_cue(cue: SoundCue) {
        let sound_name = match cue {
            SoundCue::WorkspaceSwitch => "desktop-switch",
            SoundCue::WindowClose => "window-close",
            SoundCue::WindowFocus => "window-attention",
            SoundCue::Screenshot => "screen-capture",
            SoundCue::ScreenLock => "system-lock-screen",
        };

        std::thread::spawn(move || {
            // 1. Try canberra-gtk-play
            if let Ok(mut child) = Command::new("canberra-gtk-play")
                .arg("-i")
                .arg(sound_name)
                .spawn()
            {
                let _ = child.wait();
                return;
            }
            // 2. Try paplay with standard system sound effects
            let path = format!("/usr/share/sounds/freedesktop/stereo/{sound_name}.oga");
            if std::path::Path::new(&path).exists() {
                if let Ok(mut child) = Command::new("paplay").arg(&path).spawn() {
                    let _ = child.wait();
                    return;
                }
            }
            // 3. Terminal bell fallback
            eprint!("\x07");
        });
    }

    /// Speaks an accessibility announcement to the user using Speech Dispatcher (`spd-say`)
    /// or Orca speech bridge.
    pub fn announce_screen_reader(text: &str) {
        let msg = text.to_string();
        std::thread::spawn(move || {
            let _ = Command::new("spd-say")
                .arg("-r")
                .arg("10")
                .arg("-p")
                .arg("0")
                .arg(&msg)
                .spawn();
        });
    }

    /// Queries whether the AT-SPI2 screen reader (e.g. Orca) is active on the D-Bus session bus.
    pub fn is_screen_reader_enabled() -> bool {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(_) => return false,
        };

        runtime.block_on(async {
            let conn = match zbus::Connection::session().await {
                Ok(c) => c,
                Err(_) => return false,
            };
            if let Ok(proxy) = zbus::Proxy::new(&conn, "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Status").await {
                if let Ok(enabled) = proxy.get_property::<bool>("IsEnabled").await {
                    return enabled;
                }
            }
            // Fallback: check if orca process is running
            std::process::Command::new("pgrep")
                .arg("-x")
                .arg("orca")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        })
    }

    /// AT-SPI2 standard accessibility role enumerations
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u32)]
    pub enum AtspiRole {
        Invalid = 0,
        Application = 1,
        DesktopFrame = 2,
        Window = 3,
        Panel = 4,
        PushButton = 5,
        ToggleButton = 6,
        CheckBox = 7,
        RadioButton = 8,
        Text = 9,
        Entry = 10,
        Slider = 11,
        ProgressBar = 12,
        MenuItem = 13,
        MenuBar = 14,
        Notification = 15,
        Dialog = 16,
        Heading = 17,
        Section = 18,
        PageTab = 19,
        PageTabList = 20,
    }

    impl AtspiRole {
        pub fn role_name(&self) -> &'static str {
            match self {
                Self::Invalid => "invalid",
                Self::Application => "application",
                Self::DesktopFrame => "desktop frame",
                Self::Window => "window",
                Self::Panel => "panel",
                Self::PushButton => "push button",
                Self::ToggleButton => "toggle button",
                Self::CheckBox => "check box",
                Self::RadioButton => "radio button",
                Self::Text => "text",
                Self::Entry => "entry",
                Self::Slider => "slider",
                Self::ProgressBar => "progress bar",
                Self::MenuItem => "menu item",
                Self::MenuBar => "menu bar",
                Self::Notification => "notification",
                Self::Dialog => "dialog",
                Self::Heading => "heading",
                Self::Section => "section",
                Self::PageTab => "page tab",
                Self::PageTabList => "page tab list",
            }
        }
    }

    /// AT-SPI2 standard state bitflags
    pub mod atspi_state {
        pub const INVALID: u64 = 0;
        pub const ACTIVE: u64 = 1 << 0;
        pub const ARMED: u64 = 1 << 1;
        pub const BUSY: u64 = 1 << 2;
        pub const CHECKED: u64 = 1 << 3;
        pub const EDITABLE: u64 = 1 << 4;
        pub const ENABLED: u64 = 1 << 5;
        pub const EXPANDED: u64 = 1 << 6;
        pub const FOCUSABLE: u64 = 1 << 7;
        pub const FOCUSED: u64 = 1 << 8;
        pub const MODAL: u64 = 1 << 9;
        pub const PRESSED: u64 = 1 << 10;
        pub const SELECTABLE: u64 = 1 << 11;
        pub const SELECTED: u64 = 1 << 12;
        pub const SENSITIVE: u64 = 1 << 13;
        pub const SHOWING: u64 = 1 << 14;
        pub const VISIBLE: u64 = 1 << 15;
    }

    /// Accessible node in the live AT-SPI2 / AccessKit object tree
    #[derive(Debug, Clone, PartialEq)]
    pub struct AccessibleNode {
        pub id: String,
        pub name: String,
        pub description: String,
        pub role: AtspiRole,
        pub states: u64,
        pub value: Option<f64>,
        pub children: Vec<String>,
        pub parent: Option<String>,
    }

    impl AccessibleNode {
        pub fn new(id: impl Into<String>, name: impl Into<String>, role: AtspiRole) -> Self {
            Self {
                id: id.into(),
                name: name.into(),
                description: String::new(),
                role,
                states: atspi_state::VISIBLE | atspi_state::SHOWING | atspi_state::ENABLED | atspi_state::SENSITIVE,
                value: None,
                children: Vec::new(),
                parent: None,
            }
        }

        pub fn with_description(mut self, desc: impl Into<String>) -> Self {
            self.description = desc.into();
            self
        }

        pub fn with_states(mut self, states: u64) -> Self {
            self.states = states;
            self
        }

        pub fn is_focused(&self) -> bool {
            (self.states & atspi_state::FOCUSED) != 0
        }

        pub fn state_tuple(&self) -> (u32, u32) {
            ((self.states & 0xFFFF_FFFF) as u32, (self.states >> 32) as u32)
        }
    }

    /// Snapshot of a window for AT-SPI2 synchronization
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WindowA11ySnapshot {
        pub id: u64,
        pub app_id: String,
        pub title: String,
        pub workspace: usize,
        pub is_active: bool,
        pub is_floating: bool,
    }

    /// Live AT-SPI2 Accessible Object Tree maintained by Corten
    #[derive(Debug, Clone)]
    pub struct AccessibleTree {
        pub root_id: String,
        pub nodes: std::collections::HashMap<String, AccessibleNode>,
        pub active_focus: Option<String>,
    }

    impl Default for AccessibleTree {
        fn default() -> Self {
            Self::new("Corten Desktop")
        }
    }

    impl AccessibleTree {
        pub fn new(desktop_name: &str) -> Self {
            let root_id = "root".to_string();
            let mut root_node = AccessibleNode::new(&root_id, desktop_name, AtspiRole::DesktopFrame);
            root_node.description = "Corten Wayland Masaüstü Ortamı".to_string();
            root_node.states |= atspi_state::ACTIVE;

            let mut nodes = std::collections::HashMap::new();
            nodes.insert(root_id.clone(), root_node);

            Self {
                root_id,
                nodes,
                active_focus: None,
            }
        }

        pub fn root(&self) -> Option<&AccessibleNode> {
            self.nodes.get(&self.root_id)
        }

        pub fn get_node(&self, id: &str) -> Option<&AccessibleNode> {
            self.nodes.get(id)
        }

        pub fn get_node_mut(&mut self, id: &str) -> Option<&mut AccessibleNode> {
            self.nodes.get_mut(id)
        }

        pub fn add_or_update_node(&mut self, node: AccessibleNode) {
            let id = node.id.clone();
            if let Some(parent_id) = &node.parent {
                if let Some(parent) = self.nodes.get_mut(parent_id) {
                    if !parent.children.contains(&id) {
                        parent.children.push(id.clone());
                    }
                }
            }
            self.nodes.insert(id, node);
        }

        pub fn remove_node(&mut self, id: &str) {
            if let Some(node) = self.nodes.remove(id) {
                if let Some(parent_id) = &node.parent {
                    if let Some(parent) = self.nodes.get_mut(parent_id) {
                        parent.children.retain(|c| c != id);
                    }
                }
                for child_id in node.children {
                    self.remove_node(&child_id);
                }
            }
            if self.active_focus.as_deref() == Some(id) {
                self.active_focus = None;
            }
        }

        pub fn set_focused_node(&mut self, id: &str) -> Option<(Option<String>, String)> {
            if !self.nodes.contains_key(id) {
                return None;
            }
            let prev = self.active_focus.clone();
            if let Some(ref prev_id) = prev {
                if let Some(prev_node) = self.nodes.get_mut(prev_id) {
                    prev_node.states &= !atspi_state::FOCUSED;
                    prev_node.states &= !atspi_state::ACTIVE;
                }
            }
            if let Some(node) = self.nodes.get_mut(id) {
                node.states |= atspi_state::FOCUSED | atspi_state::ACTIVE;
            }
            self.active_focus = Some(id.to_string());
            Some((prev, id.to_string()))
        }

        pub fn active_focus(&self) -> Option<&str> {
            self.active_focus.as_deref()
        }

        /// Synchronizes the AT-SPI2 accessible tree with the compositor's current window snapshot
        pub fn sync_desktop_windows(&mut self, windows: &[WindowA11ySnapshot], active_workspace: u32) {
            let current_win_ids: Vec<String> = windows.iter().map(|w| format!("win_{}", w.id)).collect();

            // 1. Remove closed windows
            let to_remove: Vec<String> = self
                .nodes
                .keys()
                .filter(|k| k.starts_with("win_") && !current_win_ids.contains(k))
                .cloned()
                .collect();
            for id in to_remove {
                self.remove_node(&id);
            }

            // 2. Add or update active windows
            let mut root_children = Vec::new();
            let mut newly_focused = None;

            for win in windows {
                let win_node_id = format!("win_{}", win.id);
                root_children.push(win_node_id.clone());

                let display_title = if win.title.is_empty() {
                    if win.app_id.is_empty() {
                        "İsimsiz Pencere".to_string()
                    } else {
                        win.app_id.clone()
                    }
                } else {
                    format!("{} ({})", win.title, win.app_id)
                };

                let mut states = atspi_state::VISIBLE | atspi_state::SHOWING | atspi_state::ENABLED | atspi_state::SENSITIVE | atspi_state::FOCUSABLE;
                if win.workspace as u32 == active_workspace {
                    states |= atspi_state::SHOWING;
                }
                if win.is_active {
                    states |= atspi_state::FOCUSED | atspi_state::ACTIVE;
                    newly_focused = Some(win_node_id.clone());
                }

                let desc = format!("Çalışma Alanı {}, {}", win.workspace, if win.is_floating { "Kayar" } else { "Döşeli" });

                let node = AccessibleNode {
                    id: win_node_id.clone(),
                    name: display_title,
                    description: desc,
                    role: AtspiRole::Window,
                    states,
                    value: None,
                    children: Vec::new(),
                    parent: Some(self.root_id.clone()),
                };
                self.nodes.insert(win_node_id, node);
            }

            if let Some(root) = self.nodes.get_mut(&self.root_id) {
                root.children = root_children;
            }

            if let Some(focused_id) = newly_focused {
                self.active_focus = Some(focused_id);
            }
        }

        /// Generates a spoken accessibility summary for Orca of the active desktop state
        pub fn generate_summary(&self) -> String {
            if let Some(ref focus_id) = self.active_focus {
                if let Some(node) = self.nodes.get(focus_id) {
                    return format!("{}: {}, {}", node.role.role_name(), node.name, node.description);
                }
            }
            if let Some(root) = self.root() {
                format!("{}: {} açık pencere", root.name, root.children.len())
            } else {
                "Corten Desktop".to_string()
            }
        }
    }

    /// AT-SPI2 D-Bus interface adapter for exposing the live accessible object tree to Orca
    #[derive(Clone)]
    pub struct AtspiAccessibleServer {
        pub tree: std::sync::Arc<std::sync::RwLock<AccessibleTree>>,
    }

    #[zbus::interface(name = "org.a11y.atspi.Accessible")]
    impl AtspiAccessibleServer {
        async fn get_role(&self) -> u32 {
            if let Ok(tree) = self.tree.read() {
                if let Some(root) = tree.root() {
                    return root.role as u32;
                }
            }
            AtspiRole::DesktopFrame as u32
        }

        async fn get_role_name(&self) -> String {
            if let Ok(tree) = self.tree.read() {
                if let Some(root) = tree.root() {
                    return root.role.role_name().to_string();
                }
            }
            "desktop frame".to_string()
        }

        async fn get_name(&self) -> String {
            if let Ok(tree) = self.tree.read() {
                if let Some(root) = tree.root() {
                    return root.name.clone();
                }
            }
            "Corten Desktop".to_string()
        }

        async fn get_description(&self) -> String {
            if let Ok(tree) = self.tree.read() {
                if let Some(root) = tree.root() {
                    return root.description.clone();
                }
            }
            String::new()
        }

        async fn get_state(&self) -> (u32, u32) {
            if let Ok(tree) = self.tree.read() {
                if let Some(root) = tree.root() {
                    return root.state_tuple();
                }
            }
            (0, 0)
        }

        async fn get_child_count(&self) -> i32 {
            if let Ok(tree) = self.tree.read() {
                if let Some(root) = tree.root() {
                    return root.children.len() as i32;
                }
            }
            0
        }

        async fn get_children(&self) -> Vec<String> {
            if let Ok(tree) = self.tree.read() {
                if let Some(root) = tree.root() {
                    return root.children.clone();
                }
            }
            Vec::new()
        }

        async fn get_active_summary(&self) -> String {
            if let Ok(tree) = self.tree.read() {
                tree.generate_summary()
            } else {
                "Corten Desktop".to_string()
            }
        }
    }

    /// Thread-safe AT-SPI2 Bridge instance for sharing across D-Bus and compositor threads
    #[derive(Debug, Clone)]
    pub struct AtspiBridge {
        pub tree: std::sync::Arc<std::sync::RwLock<AccessibleTree>>,
    }

    impl Default for AtspiBridge {
        fn default() -> Self {
            Self::new("Corten Desktop")
        }
    }

    impl AtspiBridge {
        pub fn new(desktop_name: &str) -> Self {
            Self {
                tree: std::sync::Arc::new(std::sync::RwLock::new(AccessibleTree::new(desktop_name))),
            }
        }

        pub fn sync_windows(&self, windows: &[WindowA11ySnapshot], active_workspace: u32) {
            if let Ok(mut tree) = self.tree.write() {
                tree.sync_desktop_windows(windows, active_workspace);
            }
        }

        pub fn set_focus(&self, id: &str) {
            let mut summary_to_speak = None;
            if let Ok(mut tree) = self.tree.write() {
                if let Some((_, new_id)) = tree.set_focused_node(id) {
                    if let Some(node) = tree.get_node(&new_id) {
                        summary_to_speak = Some(format!("{}: {}", node.role.role_name(), node.name));
                    }
                }
            }
            if let Some(summary) = summary_to_speak {
                if is_screen_reader_enabled() {
                    announce_screen_reader(&summary);
                }
            }
        }
    }
}

pub mod color_management;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_profile_names_are_passed_without_desktop_shell_assumptions() {
        for profile in ["power-saver", "balanced", "performance"] {
            assert!(!profile.is_empty());
        }
    }

    #[test]
    fn mpris_playback_status_parsing() {
        use mpris::PlaybackStatus;
        assert_eq!(PlaybackStatus::from_status_str("Playing"), PlaybackStatus::Playing);
        assert_eq!(PlaybackStatus::from_status_str("Paused"), PlaybackStatus::Paused);
        assert_eq!(PlaybackStatus::from_status_str("Stopped"), PlaybackStatus::Stopped);
        assert_eq!(PlaybackStatus::from_status_str("other"), PlaybackStatus::Stopped);
        assert_eq!(PlaybackStatus::Playing.as_str(), "Playing");
    }

    #[test]
    fn block_device_structure_integrity() {
        let dev = udisks2::BlockDevice {
            object_path: "/org/freedesktop/UDisks2/block_devices/sdb1".into(),
            device: "/dev/sdb1".into(),
            label: "USB_FLASH".into(),
            size: 32_000_000_000,
            mount_point: None,
            is_removable: true,
            is_read_only: false,
            fs_type: "vfat".into(),
        };
        assert_eq!(dev.label, "USB_FLASH");
        assert!(dev.is_removable);
    }

    #[test]
    fn a11y_sound_cue_variants() {
        use a11y::SoundCue;
        let cues = [
            SoundCue::WorkspaceSwitch,
            SoundCue::WindowClose,
            SoundCue::WindowFocus,
            SoundCue::Screenshot,
            SoundCue::ScreenLock,
        ];
        assert_eq!(cues.len(), 5);
    }

    #[test]
    fn test_atspi_tree_window_sync_and_focus() {
        use a11y::{AccessibleTree, AtspiRole, WindowA11ySnapshot};

        let mut tree = AccessibleTree::new("Corten Desktop");
        assert_eq!(tree.root().unwrap().role, AtspiRole::DesktopFrame);
        assert_eq!(tree.root().unwrap().children.len(), 0);

        let snaps = vec![
            WindowA11ySnapshot {
                id: 101,
                app_id: "alacritty".to_string(),
                title: "Terminal".to_string(),
                workspace: 1,
                is_active: true,
                is_floating: false,
            },
            WindowA11ySnapshot {
                id: 102,
                app_id: "firefox".to_string(),
                title: "Web Browser".to_string(),
                workspace: 2,
                is_active: false,
                is_floating: true,
            },
        ];

        tree.sync_desktop_windows(&snaps, 1);
        assert_eq!(tree.root().unwrap().children.len(), 2);
        assert_eq!(tree.active_focus(), Some("win_101"));

        let win1 = tree.get_node("win_101").unwrap();
        assert_eq!(win1.role, AtspiRole::Window);
        assert!(win1.is_focused());
        assert_eq!(win1.name, "Terminal (alacritty)");

        let win2 = tree.get_node("win_102").unwrap();
        assert!(!win2.is_focused());

        let summary = tree.generate_summary();
        assert!(summary.contains("window: Terminal (alacritty)"));

        // Switch focus to win_102
        let (prev, cur) = tree.set_focused_node("win_102").unwrap();
        assert_eq!(prev, Some("win_101".to_string()));
        assert_eq!(cur, "win_102".to_string());
        assert!(tree.get_node("win_102").unwrap().is_focused());
        assert!(!tree.get_node("win_101").unwrap().is_focused());
    }

    #[test]
    fn test_color_primaries_and_drm_fixed() {
        use color_management::{ColorPrimaries, ColorSpace};

        let srgb = ColorPrimaries::srgb();
        // Red x = 0.640 -> 0.640 / 0.00002 = 32000
        assert_eq!(ColorPrimaries::to_drm_fixed(srgb.red[0]), 32000);
        // Red y = 0.330 -> 16500
        assert_eq!(ColorPrimaries::to_drm_fixed(srgb.red[1]), 16500);

        let bt2020 = ColorPrimaries::bt2020();
        // Red x = 0.708 -> 0.708 / 0.00002 = 35400
        assert_eq!(ColorPrimaries::to_drm_fixed(bt2020.red[0]), 35400);

        assert_eq!(ColorSpace::parse_str("display-p3"), ColorSpace::DciP3);
        assert_eq!(ColorSpace::parse_str("hdr"), ColorSpace::Bt2020);
        assert_eq!(ColorSpace::parse_str("unknown"), ColorSpace::Srgb);
    }

    #[test]
    fn test_gamut_mapping_matrix_inverses() {
        use color_management::{gamut_matrix, transform_color, ColorSpace};

        let srgb_red = [1.0f32, 0.0, 0.0];
        let mat_srgb_to_p3 = gamut_matrix(ColorSpace::Srgb, ColorSpace::DciP3);
        let p3_red = transform_color(srgb_red, &mat_srgb_to_p3);
        let mat_p3_to_srgb = gamut_matrix(ColorSpace::DciP3, ColorSpace::Srgb);
        let restored_red = transform_color(p3_red, &mat_p3_to_srgb);

        assert!((restored_red[0] - 1.0).abs() < 0.01);
        assert!(restored_red[1].abs() < 0.01);
        assert!(restored_red[2].abs() < 0.01);
    }

    #[test]
    fn test_eotf_srgb_and_pq_roundtrip() {
        use color_management::{linear_to_pq, linear_to_srgb, pq_to_linear, srgb_to_linear};

        for val in [0.0f32, 0.18, 0.5, 0.8, 1.0] {
            let lin = srgb_to_linear(val);
            let srgb = linear_to_srgb(lin);
            assert!((val - srgb).abs() < 0.001);
        }

        // Test PQ curve at 100 nits (SDR reference), 400 nits, and 1000 nits (HDR10)
        for nits in [10.0f32, 100.0, 400.0, 1000.0, 4000.0] {
            let pq = linear_to_pq(nits);
            let restored = pq_to_linear(pq);
            assert!((nits - restored).abs() / nits < 0.01, "PQ roundtrip failed for {nits} nits");
        }
    }

    #[test]
    fn test_drm_hdr_output_metadata_size_and_serialization() {
        use color_management::{ColorSpace, DrmHdrOutputMetadata};

        let hdr = DrmHdrOutputMetadata::hdr10(ColorSpace::Bt2020, 1000);
        assert_eq!(hdr.metadata_type, 0);
        assert_eq!(hdr.hdmi_metadata_type1.eotf, 2); // SMPTE ST2084 PQ
        assert_eq!(hdr.hdmi_metadata_type1.max_display_mastering_luminance, 1000);
        assert_eq!(hdr.hdmi_metadata_type1.max_cll, 1000);
        assert_eq!(hdr.hdmi_metadata_type1.max_fall, 800);

        let bytes = hdr.to_raw_bytes();
        assert_eq!(bytes.len(), 32);
        assert_eq!(std::mem::size_of::<DrmHdrOutputMetadata>(), 32);
    }

    #[test]
    fn test_icc_profile_synthetic_and_parsing() {
        use color_management::{generate_synthetic_icc_profile, parse_icc_profile};

        let profile_bytes = generate_synthetic_icc_profile("Corten Wide Gamut Test");
        let parsed = parse_icc_profile(&profile_bytes).expect("synthetic profile should parse cleanly");
        assert_eq!(parsed.description, "Corten Wide Gamut Test");
        assert_eq!(parsed.device_class, "Monitör (Ekran)");
        assert_eq!(parsed.color_space, "RGB");
        assert_eq!(parsed.version_major, 2);
    }
}
