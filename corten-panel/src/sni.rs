use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use futures::StreamExt;
use tracing::{info, warn};
use zbus::interface;
use zbus::object_server::SignalEmitter;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SniItem {
    pub bus_name: String,
    pub obj_path: String,
    pub id: String,
    pub title: String,
    pub icon_name: String,
    pub icon_display: String,
    pub menu_path: Option<String>,
}

pub struct StatusNotifierWatcherServer {
    items: Arc<Mutex<HashMap<String, SniItem>>>,
    registered_keys: Arc<Mutex<Vec<String>>>,
    tx: tokio::sync::mpsc::UnboundedSender<crate::PanelIpcCommand>,
}

impl StatusNotifierWatcherServer {
    pub fn new(
        tx: tokio::sync::mpsc::UnboundedSender<crate::PanelIpcCommand>,
    ) -> (Self, Arc<Mutex<HashMap<String, SniItem>>>) {
        let items = Arc::new(Mutex::new(HashMap::new()));
        let registered_keys = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                items: items.clone(),
                registered_keys,
                tx,
            },
            items,
        )
    }

    pub fn start_monitor(&self) {
        Self::start_owner_changed_monitor(
            self.items.clone(),
            self.registered_keys.clone(),
            self.tx.clone(),
        );
    }

    pub fn start_owner_changed_monitor(
        items: Arc<Mutex<HashMap<String, SniItem>>>,
        registered_keys: Arc<Mutex<Vec<String>>>,
        tx: tokio::sync::mpsc::UnboundedSender<crate::PanelIpcCommand>,
    ) {
        tokio::spawn(async move {
            let Ok(conn) = zbus::Connection::session().await else {
                return;
            };
            let Ok(dbus_proxy) = zbus::fdo::DBusProxy::new(&conn).await else {
                return;
            };
            let Ok(mut stream) = dbus_proxy.receive_name_owner_changed().await else {
                return;
            };

            while let Some(signal) = stream.next().await {
                if let Ok(args) = signal.args() {
                    let new_owner = args.new_owner();
                    if new_owner.is_none() || new_owner.as_deref() == Some("") {
                        let dead_name = args.name();
                        let mut items_lock = items.lock().unwrap();
                        let before_len = items_lock.len();
                        items_lock.retain(|_, item| item.bus_name != dead_name.as_str());

                        let mut reg_lock = registered_keys.lock().unwrap();
                        reg_lock.retain(|key| !key.starts_with(dead_name.as_str()));

                        if items_lock.len() != before_len {
                            info!("SNI: Removed tray items for disconnected bus name: {}", dead_name);
                            let remaining: Vec<SniItem> = items_lock.values().cloned().collect();
                            drop(items_lock);
                            drop(reg_lock);
                            let _ = tx.send(crate::PanelIpcCommand::UpdateSniItems(remaining));
                        }
                    }
                }
            }
        });
    }
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl StatusNotifierWatcherServer {
    #[zbus(property)]
    async fn registered_status_notifier_items(&self) -> Vec<String> {
        self.registered_keys.lock().unwrap().clone()
    }

    #[zbus(property)]
    async fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    async fn protocol_version(&self) -> i32 {
        0
    }

    async fn register_status_notifier_item(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        service: &str,
        #[zbus(signal_emitter)] ctxt: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let sender_str = header.sender().map(|s| s.as_str());
        let (bus_name, obj_path) = parse_service_target(sender_str, service);

        let key = format!("{bus_name}{obj_path}");
        info!("SNI Watcher: RegisterStatusNotifierItem received for key: {}", key);

        {
            let mut reg = self.registered_keys.lock().unwrap();
            if !reg.contains(&key) {
                reg.push(key.clone());
            }
        }

        let _ = Self::status_notifier_item_registered(&ctxt, &key).await;

        // Query item properties asynchronously
        let items_arc = self.items.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Ok(conn) = zbus::Connection::session().await {
                if let Ok(proxy) = zbus::Proxy::new(
                    &conn,
                    bus_name.as_str(),
                    obj_path.as_str(),
                    "org.kde.StatusNotifierItem",
                )
                .await
                {
                    let id = proxy.get_property::<String>("Id").await.unwrap_or_else(|_| bus_name.clone());
                    let title = proxy.get_property::<String>("Title").await.unwrap_or_else(|_| id.clone());
                    let icon_name = proxy.get_property::<String>("IconName").await.unwrap_or_default();
                    let menu_path = proxy.get_property::<zbus::zvariant::OwnedObjectPath>("Menu").await.ok().map(|p| p.to_string());

                    let icon_display = derive_tray_icon(&id, &title, &icon_name);

                    let item = SniItem {
                        bus_name,
                        obj_path,
                        id: id.clone(),
                        title,
                        icon_name,
                        icon_display,
                        menu_path,
                    };

                    let mut items_lock = items_arc.lock().unwrap();
                    items_lock.insert(key, item);
                    let all_items: Vec<SniItem> = items_lock.values().cloned().collect();
                    drop(items_lock);

                    let _ = tx.send(crate::PanelIpcCommand::UpdateSniItems(all_items));
                } else {
                    warn!("SNI Watcher: Failed to create proxy for item {}:{}", bus_name, obj_path);
                }
            }
        });

        Ok(())
    }

    async fn register_status_notifier_host(
        &self,
        service: &str,
        #[zbus(signal_emitter)] ctxt: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        info!("SNI Watcher: RegisterStatusNotifierHost received for: {}", service);
        let _ = Self::status_notifier_host_registered(&ctxt).await;
        Ok(())
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        ctxt: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        ctxt: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;
}

pub fn derive_tray_icon(id: &str, title: &str, icon_name: &str) -> String {
    let lower = format!("{id} {title} {icon_name}").to_lowercase();
    if lower.contains("steam") {
        "🎮".to_string()
    } else if lower.contains("discord") || lower.contains("vesktop") {
        "💬".to_string()
    } else if lower.contains("telegram") {
        "✈".to_string()
    } else if lower.contains("obs") {
        "🔴".to_string()
    } else if lower.contains("spotify") {
        "🎵".to_string()
    } else if lower.contains("vlc") {
        "🎬".to_string()
    } else if lower.contains("dropbox") {
        "📦".to_string()
    } else if lower.contains("network") || lower.contains("nm-") {
        "📶".to_string()
    } else if lower.contains("bluetooth") {
        "ᛒ".to_string()
    } else {
        "🔹".to_string()
    }
}

pub fn parse_service_target(sender: Option<&str>, service: &str) -> (String, String) {
    if service.starts_with('/') {
        let bus = sender.unwrap_or(service).to_string();
        (bus, service.to_string())
    } else if let Some((b, p)) = service.split_once('/') {
        (b.to_string(), format!("/{p}"))
    } else {
        (service.to_string(), "/StatusNotifierItem".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sni_service_target_parsing() {
        let (bus, path) = parse_service_target(Some(":1.42"), "/org/ayatana/NotificationItem/steam");
        assert_eq!(bus, ":1.42");
        assert_eq!(path, "/org/ayatana/NotificationItem/steam");

        let (bus, path) = parse_service_target(None, "org.kde.discord/StatusNotifierItem");
        assert_eq!(bus, "org.kde.discord");
        assert_eq!(path, "/StatusNotifierItem");

        let (bus, path) = parse_service_target(None, "org.kde.telegram");
        assert_eq!(bus, "org.kde.telegram");
        assert_eq!(path, "/StatusNotifierItem");
    }

    #[test]
    fn test_sni_icon_derivation() {
        assert_eq!(derive_tray_icon("steam", "Steam", "steam_tray"), "🎮");
        assert_eq!(derive_tray_icon("vesktop", "Discord", "discord-tray"), "💬");
        assert_eq!(derive_tray_icon("telegram-desktop", "Telegram", "telegram"), "✈");
        assert_eq!(derive_tray_icon("obs", "OBS Studio", "obs-tray"), "🔴");
        assert_eq!(derive_tray_icon("spotify", "Spotify", ""), "🎵");
        assert_eq!(derive_tray_icon("nm-applet", "Network", "network-wireless-connected"), "📶");
        assert_eq!(derive_tray_icon("custom-app", "My App", "custom-icon"), "🔹");
    }
}
