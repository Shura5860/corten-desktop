use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use tracing::info;
use zbus::interface;
use zbus::zvariant::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    Low = 0,
    Normal = 1,
    Critical = 2,
}

impl From<u8> for Urgency {
    fn from(val: u8) -> Self {
        match val {
            0 => Urgency::Low,
            2 => Urgency::Critical,
            _ => Urgency::Normal,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NotificationItem {
    pub id: u32,
    pub app_name: String,
    pub summary: String,
    pub body: String,
    pub icon: String,
    pub urgency: Urgency,
    pub actions: Vec<(String, String)>, // (key, label)
    pub timeout_ms: i32,
    pub sync_tag: Option<String>,
    pub progress: Option<u8>,
}

#[derive(Debug, Clone)]
pub enum NotifyEvent {
    Add(NotificationItem),
    Close(u32),
    ClearAll,
    ClearHistory,
    ToggleHistory,
    ReloadConfig,
    SetDnd(bool),
}

static DBUS_CONN: std::sync::OnceLock<zbus::Connection> = std::sync::OnceLock::new();

pub fn set_dbus_connection(conn: zbus::Connection) {
    let _ = DBUS_CONN.set(conn);
}

pub fn get_dbus_connection() -> Option<zbus::Connection> {
    DBUS_CONN.get().cloned()
}

pub fn emit_notification_closed(id: u32, reason: u32) {
    if let Some(conn) = get_dbus_connection() {
        tokio::spawn(async move {
            let _ = conn
                .emit_signal(
                    None::<()>,
                    "/org/freedesktop/Notifications",
                    "org.freedesktop.Notifications",
                    "NotificationClosed",
                    &(id, reason),
                )
                .await;
        });
    }
}

pub fn emit_action_invoked(id: u32, action_key: &str) {
    if let Some(conn) = get_dbus_connection() {
        let key = action_key.to_string();
        tokio::spawn(async move {
            let _ = conn
                .emit_signal(
                    None::<()>,
                    "/org/freedesktop/Notifications",
                    "org.freedesktop.Notifications",
                    "ActionInvoked",
                    &(id, &key),
                )
                .await;
        });
    }
}

pub fn strip_html_markup(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut inside_tag = false;

    for ch in input.chars() {
        if ch == '<' {
            inside_tag = true;
        } else if ch == '>' {
            inside_tag = false;
        } else if !inside_tag {
            output.push(ch);
        }
    }

    output
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
}

pub struct NotificationServer {
    next_id: AtomicU32,
    dnd_enabled: Arc<AtomicBool>,
    event_tx: UnboundedSender<NotifyEvent>,
}

impl NotificationServer {
    pub fn new(event_tx: UnboundedSender<NotifyEvent>, dnd_enabled: Arc<AtomicBool>) -> Self {
        Self {
            next_id: AtomicU32::new(1),
            dnd_enabled,
            event_tx,
        }
    }
}

#[interface(name = "org.freedesktop.Notifications")]
impl NotificationServer {
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, Value<'_>>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            self.next_id.fetch_add(1, Ordering::SeqCst)
        };

        // Determine urgency from hints
        let urgency_byte = hints
            .get("urgency")
            .and_then(|v| v.downcast_ref::<u8>().ok())
            .unwrap_or(1);
        let urgency = Urgency::from(urgency_byte);

        // Convert actions pairs [key1, label1, key2, label2] -> Vec<(key, label)>
        let mut action_pairs = Vec::new();
        let mut iter = actions.into_iter();
        while let (Some(k), Some(lbl)) = (iter.next(), iter.next()) {
            action_pairs.push((k, lbl));
        }

        // Extract optional synchronous OSD tag (`x-canonical-private-synchronous` or `synchronous`)
        let sync_tag = hints
            .get("x-canonical-private-synchronous")
            .or_else(|| hints.get("synchronous"))
            .and_then(|v| v.downcast_ref::<&str>().ok().map(|s| s.to_string()))
            .filter(|s| !s.is_empty());

        // Extract optional OSD progress value (`value`: 0..=100)
        let progress = hints.get("value").and_then(|v| {
            v.downcast_ref::<u8>()
                .ok()
                .or_else(|| v.downcast_ref::<i32>().ok().map(|n| n.clamp(0, 100) as u8))
                .or_else(|| v.downcast_ref::<u32>().ok().map(|n| n.min(100) as u8))
        });

        let clean_summary = strip_html_markup(&summary);
        let clean_body = strip_html_markup(&body);

        let item = NotificationItem {
            id,
            app_name,
            summary: clean_summary,
            body: clean_body,
            icon: app_icon,
            urgency,
            actions: action_pairs,
            timeout_ms: expire_timeout,
            sync_tag,
            progress,
        };

        // Do-Not-Disturb check: critical notifications still pass through
        if self.dnd_enabled.load(Ordering::SeqCst) && urgency != Urgency::Critical {
            info!("Notification {} suppressed by Do-Not-Disturb mode", id);
            return id;
        }

        let _ = self.event_tx.send(NotifyEvent::Add(item));
        id
    }

    async fn close_notification(&self, id: u32) {
        let _ = self.event_tx.send(NotifyEvent::Close(id));
    }

    async fn get_capabilities(&self) -> Vec<&'static str> {
        vec!["body", "actions", "persistence", "icon-static"]
    }

    async fn get_server_information(
        &self,
    ) -> (&'static str, &'static str, &'static str, &'static str) {
        ("corten-notify", "corten-desktop", "0.1.0", "1.2")
    }

    async fn set_do_not_disturb(&self, enabled: bool) {
        info!("D-Bus: set_do_not_disturb: {}", enabled);
        self.dnd_enabled.store(enabled, Ordering::SeqCst);
        let _ = self.event_tx.send(NotifyEvent::SetDnd(enabled));
    }

    async fn toggle_do_not_disturb(&self) -> bool {
        let new_val = !self.dnd_enabled.load(Ordering::SeqCst);
        info!("D-Bus: toggle_do_not_disturb -> {}", new_val);
        self.dnd_enabled.store(new_val, Ordering::SeqCst);
        let _ = self.event_tx.send(NotifyEvent::SetDnd(new_val));
        new_val
    }

    async fn get_do_not_disturb(&self) -> bool {
        self.dnd_enabled.load(Ordering::SeqCst)
    }

    async fn reload_config(&self) {
        info!("D-Bus: Notification reload_config received");
        let _ = self.event_tx.send(NotifyEvent::ReloadConfig);
    }

    async fn toggle_history(&self) {
        info!("D-Bus: Notification toggle_history received");
        let _ = self.event_tx.send(NotifyEvent::ToggleHistory);
    }

    async fn clear_history(&self) {
        info!("D-Bus: Notification clear_history received");
        let _ = self.event_tx.send(NotifyEvent::ClearHistory);
    }
}
