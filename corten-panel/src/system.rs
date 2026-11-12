use chrono::Local;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

static COMPOSITOR_RUNTIME: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new();
static COMPOSITOR_CONN: OnceLock<Option<zbus::Connection>> = OnceLock::new();

fn get_or_init_dbus() -> Option<(&'static tokio::runtime::Runtime, &'static zbus::Connection)> {
    let rt = COMPOSITOR_RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok()
        })
        .as_ref()?;

    let conn = COMPOSITOR_CONN
        .get_or_init(|| {
            rt.block_on(async {
                zbus::Connection::session().await.ok()
            })
        })
        .as_ref()?;

    Some((rt, conn))
}

/// Simple, robust system status collectors for panel modules
pub struct SystemInfo;

impl SystemInfo {
    pub async fn start_recording(
        output: String,
        fps: u32,
        monitor_only: bool,
    ) -> Result<(), String> {
        let connection = zbus::Connection::session()
            .await
            .map_err(|error| error.to_string())?;
        let proxy = zbus::Proxy::new(
            &connection,
            "org.corten.Capture",
            "/org/corten/Capture",
            "org.corten.Capture",
        )
        .await
        .map_err(|error| error.to_string())?;
        proxy
            .call::<_, _, ()>("start_recording", &(output.as_str(), fps, monitor_only))
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn stop_recording() -> Result<(), String> {
        let connection = zbus::Connection::session()
            .await
            .map_err(|error| error.to_string())?;
        let proxy = zbus::Proxy::new(
            &connection,
            "org.corten.Capture",
            "/org/corten/Capture",
            "org.corten.Capture",
        )
        .await
        .map_err(|error| error.to_string())?;
        proxy
            .call::<_, _, ()>("stop_recording", &())
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn recording_active() -> Result<bool, String> {
        let connection = zbus::Connection::session()
            .await
            .map_err(|error| error.to_string())?;
        let proxy = zbus::Proxy::new(
            &connection,
            "org.corten.Capture",
            "/org/corten/Capture",
            "org.corten.Capture",
        )
        .await
        .map_err(|error| error.to_string())?;
        proxy
            .call::<_, _, bool>("get_recording", &())
            .await
            .map_err(|error| error.to_string())
    }

    /// Formats current time and date
    pub fn get_datetime(time_fmt: &str, date_fmt: &str) -> (String, String) {
        let now = Local::now();
        (
            now.format(time_fmt).to_string(),
            now.format(date_fmt).to_string(),
        )
    }

    /// Checks network connection state, SSID and local IP via sysfs (zero fork)
    pub fn get_network_details() -> (bool, String, String) {
        let mut connected = false;
        let mut iface_name = String::from("Disconnected");
        let mut ip_addr = String::from("127.0.0.1");

        if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str == "lo" {
                    continue;
                }
                let operstate_path = entry.path().join("operstate");
                if let Ok(state) = std::fs::read_to_string(operstate_path) {
                    if state.trim() == "up" {
                        connected = true;
                        iface_name = if name_str.starts_with('w') {
                            "Wi-Fi Online".to_string()
                        } else {
                            "Ethernet Online".to_string()
                        };
                        break;
                    }
                }
            }
        }

        // Get local IP address instantly without forking processes
        if connected {
            if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
                if socket.connect("1.1.1.1:80").is_ok() {
                    if let Ok(addr) = socket.local_addr() {
                        ip_addr = addr.ip().to_string();
                    }
                }
            }
        }

        (connected, iface_name, ip_addr)
    }

    /// Fetches battery percentage (0..100) and AC charging state from sysfs (zero fork)
    pub fn get_battery_info() -> Option<(u8, bool)> {
        // Read directly from /sys/class/power_supply/BAT* (instant kernel sysfs, zero fork)
        if let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("BAT") {
                    let cap_path = entry.path().join("capacity");
                    let status_path = entry.path().join("status");

                    if let Ok(cap_str) = std::fs::read_to_string(cap_path) {
                        if let Ok(pct) = cap_str.trim().parse::<u8>() {
                            let is_charging = std::fs::read_to_string(status_path)
                                .map(|s| s.trim().to_lowercase().contains("charging"))
                                .unwrap_or(false);
                            return Some((pct, is_charging));
                        }
                    }
                }
            }
        }

        None
    }

    /// Checks network connection state (e.g. "Online", "Offline") via sysfs (zero fork/exec)
    #[allow(dead_code)]
    pub fn get_network_status() -> (bool, String) {
        let (conn, label, _) = Self::get_network_details();
        (conn, label)
    }

    /// Fetches current active workspace number (1-based) from active compositor
    #[allow(dead_code)]
    pub fn get_active_workspace() -> usize {
        Self::get_compositor_state().0
    }

    /// Queries the live Corten Compositor D-Bus service for `(active_workspace, occupied_workspaces, windows)`.
    /// Each window entry is `(id, app_id, title, workspace, is_active, is_floating)`.
    #[allow(clippy::type_complexity)]
    pub fn get_compositor_state() -> (
        usize,
        Vec<u32>,
        Vec<(u64, String, String, u32, bool, bool)>,
    ) {
        Self::get_compositor_state_for_output(None)
    }

    /// Queries the live Corten Compositor D-Bus service for a specific display output.
    #[allow(clippy::type_complexity)]
    pub fn get_compositor_state_for_output(output_name: Option<&str>) -> (
        usize,
        Vec<u32>,
        Vec<(u64, String, String, u32, bool, bool)>,
    ) {
        let Some((runtime, connection)) = get_or_init_dbus() else {
            return (1, Vec::new(), Vec::new());
        };
        runtime.block_on(async {
            let Ok(proxy) = corten_ipc::CompositorControlProxy::new(connection).await else {
                return (1, Vec::new(), Vec::new());
            };
            let out_str = output_name.unwrap_or("");
            if let Ok((ws, occ, windows)) = proxy.get_full_state(out_str).await {
                return (ws.max(1) as usize, occ, windows);
            }
            // Fallback for older interface versions
            let (active_ws, occupied) = if let Some(out) = output_name {
                let ws = proxy.get_active_workspace_for_output(out).await.unwrap_or(1).max(1) as usize;
                let occ = proxy.get_occupied_workspaces_for_output(out).await.unwrap_or_default();
                (ws, occ)
            } else {
                let ws = proxy.get_active_workspace().await.unwrap_or(1).max(1) as usize;
                let occ = proxy.get_occupied_workspaces().await.unwrap_or_default();
                (ws, occ)
            };
            let windows = proxy.get_windows().await.unwrap_or_default();
            (active_ws, occupied, windows)
        })
    }

    /// Switches the workspace in the current compositor
    pub fn set_active_workspace(ws: usize) -> bool {
        Self::set_active_workspace_on_output(None, ws)
    }

    /// Switches the workspace on a specific output or globally
    pub fn set_active_workspace_on_output(output_name: Option<&str>, ws: usize) -> bool {
        let Some((runtime, connection)) = get_or_init_dbus() else {
            return false;
        };
        runtime.block_on(async {
            let Ok(proxy) = corten_ipc::CompositorControlProxy::new(connection).await else {
                return false;
            };
            if let Some(out) = output_name {
                proxy.switch_workspace_on_output(out, ws as u32).await.is_ok()
            } else {
                proxy.switch_workspace(ws as u32).await.is_ok()
            }
        })
    }

    /// Focuses a specific window in the compositor by its stable Wayland surface ID
    pub fn focus_window(window_id: u64) -> bool {
        let Some((runtime, connection)) = get_or_init_dbus() else {
            return false;
        };
        runtime.block_on(async {
            let Ok(proxy) = corten_ipc::CompositorControlProxy::new(connection).await else {
                return false;
            };
            proxy.focus_window(window_id).await.is_ok()
        })
    }

    /// Toggles showing the desktop in the compositor
    pub fn toggle_show_desktop() -> bool {
        let Some((runtime, connection)) = get_or_init_dbus() else {
            return false;
        };
        runtime.block_on(async {
            let Ok(proxy) = corten_ipc::CompositorControlProxy::new(connection).await else {
                return false;
            };
            proxy.toggle_show_desktop().await.is_ok()
        })
    }

    /// Checks which standard applications are currently running on the system,
    /// preferring live compositor window metadata before falling back to `/proc`.
    pub fn get_running_apps() -> (bool, bool, bool, bool) {
        let (_, _, windows) = Self::get_compositor_state();
        Self::get_running_apps_from_windows(&windows)
    }

    /// Pure in-memory calculation of running apps from existing window metadata (0 disk/D-Bus I/O).
    pub fn get_running_apps_from_windows(
        windows: &[(u64, String, String, u32, bool, bool)],
    ) -> (bool, bool, bool, bool) {
        if !windows.is_empty() {
            let mut term = false;
            let mut files = false;
            let mut browser = false;
            let mut editor = false;
            for (_, app_id, title, _, _, _) in windows {
                let hay = format!("{} {}", app_id.to_lowercase(), title.to_lowercase());
                if hay.contains("alacritty")
                    || hay.contains("kitty")
                    || hay.contains("konsole")
                    || hay.contains("foot")
                    || hay.contains("wezterm")
                    || hay.contains("xterm")
                    || hay.contains("terminal")
                {
                    term = true;
                }
                if hay.contains("dolphin")
                    || hay.contains("nautilus")
                    || hay.contains("thunar")
                    || hay.contains("files")
                {
                    files = true;
                }
                if hay.contains("firefox")
                    || hay.contains("chrome")
                    || hay.contains("brave")
                    || hay.contains("chromium")
                    || hay.contains("zen")
                {
                    browser = true;
                }
                if hay.contains("code")
                    || hay.contains("antigravity")
                    || hay.contains("kate")
                    || hay.contains("gedit")
                    || hay.contains("nvim")
                    || hay.contains("zed")
                {
                    editor = true;
                }
            }
            return (term, files, browser, editor);
        }

        // Check running processes: (Terminal, Files, Browser, Editor)
        let mut term = false;
        let mut files = false;
        let mut browser = false;
        let mut editor = false;

        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.chars().all(|c| c.is_ascii_digit()) {
                    let comm_path = entry.path().join("comm");
                    if let Ok(comm) = std::fs::read_to_string(comm_path) {
                        let comm = comm.trim().to_lowercase();
                        if comm.contains("alacritty")
                            || comm.contains("kitty")
                            || comm.contains("konsole")
                            || comm.contains("foot")
                            || comm.contains("xterm")
                        {
                            term = true;
                        }
                        if comm.contains("dolphin")
                            || comm.contains("nautilus")
                            || comm.contains("thunar")
                        {
                            files = true;
                        }
                        if comm.contains("firefox")
                            || comm.contains("chrome")
                            || comm.contains("brave")
                            || comm.contains("chromium")
                        {
                            browser = true;
                        }
                        if comm.contains("code")
                            || comm.contains("antigravity")
                            || comm.contains("kate")
                            || comm.contains("gedit")
                            || comm.contains("nvim")
                        {
                            editor = true;
                        }
                    }
                }
            }
        }

        (term, files, browser, editor)
    }

    /// Scans /proc to discover other running desktop apps with their process names
    pub fn get_other_running_apps() -> Vec<(String, String)> {
        let (_, _, windows) = Self::get_compositor_state();
        Self::get_other_running_apps_from_windows(&windows)
    }

    /// Pure in-memory calculation of other running apps from existing window metadata (0 disk/D-Bus I/O).
    pub fn get_other_running_apps_from_windows(
        windows: &[(u64, String, String, u32, bool, bool)],
    ) -> Vec<(String, String)> {
        if !windows.is_empty() {
            let mut others = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for (_, app_id, title, _, _, _) in windows {
                let id_lower = app_id.to_lowercase();
                if id_lower.is_empty()
                    || id_lower.contains("alacritty")
                    || id_lower.contains("kitty")
                    || id_lower.contains("konsole")
                    || id_lower.contains("foot")
                    || id_lower.contains("dolphin")
                    || id_lower.contains("nautilus")
                    || id_lower.contains("thunar")
                    || id_lower.contains("firefox")
                    || id_lower.contains("chrome")
                    || id_lower.contains("brave")
                    || id_lower.contains("chromium")
                    || id_lower.contains("code")
                    || id_lower.contains("corten")
                {
                    continue;
                }
                if seen.insert(app_id.clone()) {
                    others.push((app_id.clone(), title.clone()));
                }
            }
            if !others.is_empty() {
                return others;
            }
        }

        let mut apps = Vec::new();
        let known = [
            "systemd",
            "corten",
            "pipewire",
            "wireplumber",
            "dbus",
            "bash",
            "zsh",
            "sh",
            "cargo",
            "rustc",
            "sleep",
            "grep",
            "sed",
            "awk",
        ];

        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.chars().all(|c| c.is_ascii_digit()) {
                    let comm_path = entry.path().join("comm");
                    if let Ok(comm) = std::fs::read_to_string(comm_path) {
                        let comm = comm.trim().to_string();
                        let comm_lower = comm.to_lowercase();
                        if !known.iter().any(|k| comm_lower.contains(k))
                            && !comm_lower.is_empty()
                            && !apps.iter().any(|(n, _)| n == &comm)
                        {
                            // Detect candidate GUI app
                            if comm_lower.contains("gimp")
                                || comm_lower.contains("inkscape")
                                || comm_lower.contains("vlc")
                                || comm_lower.contains("spotify")
                                || comm_lower.contains("discord")
                                || comm_lower.contains("obs")
                                || comm_lower.contains("blender")
                                || comm_lower.contains("thunderbird")
                                || comm_lower.contains("steam")
                            {
                                apps.push((comm.clone(), comm));
                            }
                        }
                    }
                }
            }
        }
        apps
    }

    /// Reads active listening TCP ports from /proc/net/tcp and /proc/net/tcp6 without spawning external processes
    pub fn get_listening_dev_ports() -> Vec<u16> {
        let mut ports = Vec::new();
        for file in ["/proc/net/tcp", "/proc/net/tcp6"] {
            if let Ok(content) = std::fs::read_to_string(file) {
                for line in content.lines().skip(1) {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 4 {
                        let state = parts[3];
                        // "0A" is TCP_LISTEN in Linux kernel
                        if state == "0A" {
                            if let Some(local_addr) = parts.get(1) {
                                if let Some((_, port_hex)) = local_addr.split_once(':') {
                                    if let Ok(port) = u16::from_str_radix(port_hex, 16) {
                                        // Include developer-friendly listening ports (ignore OS ephemeral/low system daemons if preferred, or include all user dev ports)
                                        if !ports.contains(&port) {
                                            ports.push(port);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        ports.sort_unstable();
        ports
    }

    /// Native screenshot fallback used when the portal is unavailable.
    pub fn take_screenshot_fallback(region: bool) {
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
        let pictures_dir = std::env::var("XDG_PICTURES_DIR")
            .or_else(|_| std::env::var("HOME").map(|h| format!("{}/Pictures", h)))
            .unwrap_or_else(|_| ".".to_string());
        let _ = std::fs::create_dir_all(&pictures_dir);
        let target_file = format!("{}/Screenshot_{}.png", pictures_dir, timestamp);

        let geometry = if region {
            match Command::new("slurp").output() {
                Ok(output) if output.status.success() => {
                    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
                }
                _ => None,
            }
        } else {
            None
        };

        let mut grim = Command::new("grim");
        if let Some(geometry) = geometry.as_deref() {
            grim.args(["-g", geometry]);
        }
        grim.arg(&target_file);
        if let Ok(status) = grim.status() {
            if status.success() {
                if let Ok(bytes) = std::fs::read(&target_file) {
                    if let Ok(mut wl_copy) = Command::new("wl-copy").stdin(Stdio::piped()).spawn() {
                        if let Some(stdin) = wl_copy.stdin.as_mut() {
                            let _ = std::io::Write::write_all(stdin, &bytes);
                        }
                        let _ = wl_copy.wait();
                    }
                }
                let message = if region {
                    "Region saved and copied to clipboard"
                } else {
                    "Full screen saved and copied to clipboard"
                };
                let _ = Command::new("notify-send")
                    .args(["Corten Screenshot", message, "-i", &target_file])
                    .spawn();
            }
        }
    }

    /// Detects active background tray / StatusNotifierItem (SNI) services via
    /// `org.kde.StatusNotifierWatcher` D-Bus protocol first, with `/proc` fallback.
    pub fn get_active_tray_items() -> Vec<TrayItem> {
        let mut items = Vec::new();

        if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            let sni_items: Vec<TrayItem> = runtime.block_on(async {
                let mut discovered = Vec::new();
                let Ok(connection) = zbus::Connection::session().await else {
                    return discovered;
                };
                let Ok(watcher) = zbus::Proxy::new(
                    &connection,
                    "org.kde.StatusNotifierWatcher",
                    "/StatusNotifierWatcher",
                    "org.kde.StatusNotifierWatcher",
                )
                .await
                else {
                    return discovered;
                };
                let Ok(registered) = watcher
                    .get_property::<Vec<String>>("RegisteredStatusNotifierItems")
                    .await
                else {
                    return discovered;
                };
                for service_spec in registered {
                    let (bus_name, obj_path) = service_spec
                        .split_once('/')
                        .map(|(b, p)| (b, format!("/{p}")))
                        .unwrap_or((service_spec.as_str(), "/StatusNotifierItem".to_string()));
                    let title = if let Ok(item_proxy) = zbus::Proxy::new(
                        &connection,
                        bus_name,
                        obj_path.as_str(),
                        "org.kde.StatusNotifierItem",
                    )
                    .await
                    {
                        match item_proxy.get_property::<String>("Title").await {
                            Ok(s) if !s.is_empty() => s,
                            _ => item_proxy
                                .get_property::<String>("Id")
                                .await
                                .unwrap_or_else(|_| bus_name.to_string()),
                        }
                    } else {
                        bus_name.to_string()
                    };
                    let lower = title.to_lowercase();
                    let (id, icon) = if lower.contains("steam") {
                        ("steam".to_string(), "🎮".to_string())
                    } else if lower.contains("discord") || lower.contains("vesktop") {
                        ("discord".to_string(), "💬".to_string())
                    } else if lower.contains("telegram") {
                        ("telegram".to_string(), "✈".to_string())
                    } else if lower.contains("obs") {
                        ("obs".to_string(), "🔴".to_string())
                    } else {
                        (service_spec.clone(), "•".to_string())
                    };
                    if !discovered.iter().any(|existing: &TrayItem| existing.id == id) {
                        discovered.push(TrayItem { id, title, icon });
                    }
                }
                discovered
            });
            items.extend(sni_items);
        }

        // Scan proc and active D-Bus session services
        if let Ok(entries) = std::fs::read_dir("/proc") {
            let mut steam = items.iter().any(|i| i.id == "steam");
            let mut discord = items.iter().any(|i| i.id == "discord");
            let mut telegram = items.iter().any(|i| i.id == "telegram");
            let mut obs = items.iter().any(|i| i.id == "obs");

            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.chars().all(|c| c.is_ascii_digit()) {
                    let comm_path = entry.path().join("comm");
                    if let Ok(comm) = std::fs::read_to_string(comm_path) {
                        let comm = comm.trim().to_lowercase();
                        if comm.contains("steam") && !steam {
                            steam = true;
                            items.push(TrayItem {
                                id: "steam".into(),
                                title: "Steam".into(),
                                icon: "🎮".into(),
                            });
                        }
                        if (comm.contains("discord") || comm.contains("vesktop")) && !discord {
                            discord = true;
                            items.push(TrayItem {
                                id: "discord".into(),
                                title: "Discord".into(),
                                icon: "💬".into(),
                            });
                        }
                        if comm.contains("telegram") && !telegram {
                            telegram = true;
                            items.push(TrayItem {
                                id: "telegram".into(),
                                title: "Telegram".into(),
                                icon: "✈".into(),
                            });
                        }
                        if comm.contains("obs") && !obs {
                            obs = true;
                            items.push(TrayItem {
                                id: "obs".into(),
                                title: "OBS Studio".into(),
                                icon: "🔴".into(),
                            });
                        }
                    }
                }
            }
        }

        items
    }

    /// Reads plain text from Wayland clipboard via wl-paste
    pub fn read_clipboard_text() -> Option<String> {
        let output = Command::new("wl-paste")
            .args(["-t", "text/plain", "-n"])
            .output()
            .ok()?;
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
        None
    }

    /// Copies text to the Wayland clipboard via wl-copy
    pub fn copy_to_clipboard(text: &str) {
        if let Ok(mut child) = Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }

    /// Gets current screen brightness percentage (0..=100) via kernel sysfs (zero-fork) with brightnessctl fallback
    pub fn get_brightness_percent() -> u8 {
        if let Ok(entries) = std::fs::read_dir("/sys/class/backlight") {
            for entry in entries.flatten() {
                let path = entry.path();
                let cur_path = path.join("brightness");
                let max_path = path.join("max_brightness");
                if let (Ok(cur_str), Ok(max_str)) = (std::fs::read_to_string(&cur_path), std::fs::read_to_string(&max_path)) {
                    if let (Ok(cur), Ok(max)) = (cur_str.trim().parse::<f32>(), max_str.trim().parse::<f32>()) {
                        if max > 0.0 {
                            return ((cur / max) * 100.0).round().clamp(0.0, 100.0) as u8;
                        }
                    }
                }
            }
        }
        if let Ok(output) = Command::new("brightnessctl").args(["g"]).output() {
            if let Ok(max_out) = Command::new("brightnessctl").args(["m"]).output() {
                let cur: f32 = String::from_utf8_lossy(&output.stdout).trim().parse().unwrap_or(0.0);
                let max: f32 = String::from_utf8_lossy(&max_out.stdout).trim().parse().unwrap_or(1.0);
                if max > 0.0 {
                    return ((cur / max) * 100.0).round().clamp(0.0, 100.0) as u8;
                }
            }
        }
        100
    }

    /// Sets screen brightness percentage (5..=100) via kernel sysfs or brightnessctl
    pub fn set_brightness_percent(percent: u8) {
        let p = percent.clamp(5, 100);
        let mut written = false;
        if let Ok(entries) = std::fs::read_dir("/sys/class/backlight") {
            for entry in entries.flatten() {
                let path = entry.path();
                let max_path = path.join("max_brightness");
                let cur_path = path.join("brightness");
                if let Ok(max_str) = std::fs::read_to_string(&max_path) {
                    if let Ok(max) = max_str.trim().parse::<f32>() {
                        let target = ((max * (p as f32) / 100.0).round() as u64).max(1);
                        if std::fs::write(&cur_path, target.to_string()).is_ok() {
                            written = true;
                            break;
                        }
                    }
                }
            }
        }
        if !written {
            let _ = Command::new("brightnessctl")
                .args(["s", &format!("{}%", p)])
                .spawn();
        }
    }

    /// Checks if the microphone or any audio capture stream is currently active (recording)
    pub fn is_mic_active() -> bool {
        let mut checked_procfs = false;
        // 1. Zero-fork hardware check in ALSA procfs (sub0/status == RUNNING)
        if let Ok(entries) = std::fs::read_dir("/proc/asound") {
            for entry in entries.flatten() {
                let card_path = entry.path();
                if card_path.is_dir()
                    && card_path
                        .file_name()
                        .map(|n| n.to_string_lossy().starts_with("card"))
                        .unwrap_or(false)
                {
                    checked_procfs = true;
                    if let Ok(pcms) = std::fs::read_dir(&card_path) {
                        for pcm in pcms.flatten() {
                            let pcm_name = pcm.file_name().to_string_lossy().to_string();
                            if pcm_name.starts_with("pcm") && pcm_name.ends_with('c') {
                                let sub_status = pcm.path().join("sub0").join("status");
                                if let Ok(status) = std::fs::read_to_string(&sub_status) {
                                    if status.trim().eq_ignore_ascii_case("RUNNING") {
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2. Only fallback to pactl if procfs was inaccessible or has no ALSA card directories
        if !checked_procfs {
            if let Ok(output) = Command::new("pactl")
                .args(["list", "short", "source-outputs"])
                .output()
            {
                if !output.stdout.is_empty() {
                    return true;
                }
            }
        }

        false
    }

    pub fn get_quick_memory_summary() -> (u64, u64, f32) {
        let Ok(content) = std::fs::read_to_string("/proc/meminfo") else {
            return (0, 0, 0.0);
        };
        let mut total_kb = 0_u64;
        let mut avail_kb = 0_u64;
        for line in content.lines() {
            if line.starts_with("MemTotal:") {
                if let Some(val) = line.split_whitespace().nth(1) {
                    total_kb = val.parse().unwrap_or(0);
                }
            } else if line.starts_with("MemAvailable:") {
                if let Some(val) = line.split_whitespace().nth(1) {
                    avail_kb = val.parse().unwrap_or(0);
                }
            }
        }
        let total_mb = total_kb / 1024;
        let used_kb = total_kb.saturating_sub(avail_kb);
        let used_mb = used_kb / 1024;
        let percent = if total_kb > 0 {
            (used_kb as f32 / total_kb as f32) * 100.0
        } else {
            0.0
        };
        (used_mb, total_mb, percent)
    }

    pub fn get_top_memory_processes(limit: usize) -> Vec<QuickProcessInfo> {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        let mut procs = Vec::new();
        for entry in entries.flatten() {
            let name_os = entry.file_name();
            let Some(name_str) = name_os.to_str() else { continue };
            let Ok(pid) = name_str.parse::<u32>() else { continue };

            let statm_path = entry.path().join("statm");
            let comm_path = entry.path().join("comm");

            let Ok(statm) = std::fs::read_to_string(statm_path) else { continue };
            let Ok(comm) = std::fs::read_to_string(comm_path) else { continue };

            let resident_pages: u64 = statm
                .split_whitespace()
                .nth(1)
                .and_then(|p| p.parse().ok())
                .unwrap_or(0);

            let mem_mb = (resident_pages * 4096) / (1024 * 1024);
            let proc_name = comm.trim().to_string();
            if mem_mb > 0 && !proc_name.is_empty() {
                procs.push(QuickProcessInfo {
                    pid,
                    name: proc_name,
                    mem_mb,
                });
            }
        }
        procs.sort_by_key(|p| std::cmp::Reverse(p.mem_mb));
        procs.truncate(limit);
        procs
    }

    pub fn kill_process_by_pid(pid: u32) -> bool {
        Command::new("kill")
            .args(["-15", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    pub fn kill_process_by_name(name: &str) -> bool {
        Command::new("pkill")
            .args(["-15", "-f", name])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickProcessInfo {
    pub pid: u32,
    pub name: String,
    pub mem_mb: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayItem {
    pub id: String,
    pub title: String,
    pub icon: String,
}

#[derive(Debug, Clone, Default)]
pub struct NetworkSpeedTracker {
    pub prev_rx: u64,
    pub prev_tx: u64,
    pub last_sample: Option<std::time::Instant>,
}

impl NetworkSpeedTracker {
    pub fn sample_speed(&mut self) -> (u64, u64, String, String) {
        let (rx, tx) = read_net_bytes();
        let now = std::time::Instant::now();
        let (rx_speed, tx_speed) = if let Some(last) = self.last_sample {
            let dt = now.duration_since(last).as_secs_f32().max(0.1);
            let d_rx = (rx.saturating_sub(self.prev_rx) as f32 / dt) as u64;
            let d_tx = (tx.saturating_sub(self.prev_tx) as f32 / dt) as u64;
            (d_rx, d_tx)
        } else {
            (0, 0)
        };

        self.prev_rx = rx;
        self.prev_tx = tx;
        self.last_sample = Some(now);

        (rx_speed, tx_speed, format_speed(rx_speed), format_speed(tx_speed))
    }
}

pub fn read_net_bytes() -> (u64, u64) {
    let Ok(content) = std::fs::read_to_string("/proc/net/dev") else {
        return (0, 0);
    };
    let mut total_rx = 0;
    let mut total_tx = 0;
    for line in content.lines().skip(2) {
        let Some((iface, rest)) = line.split_once(':') else {
            continue;
        };
        let iface = iface.trim();
        if iface == "lo" {
            continue;
        }
        let mut parts = rest.split_whitespace();
        let rx: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let tx: u64 = parts.nth(7).and_then(|s| s.parse().ok()).unwrap_or(0);
        total_rx += rx;
        total_tx += tx;
    }
    (total_rx, total_tx)
}

pub fn format_speed(bytes_per_sec: u64) -> String {
    if bytes_per_sec >= 1024 * 1024 {
        format!("{:.1} MB/s", bytes_per_sec as f32 / (1024.0 * 1024.0))
    } else if bytes_per_sec >= 1024 {
        format!("{:.0} KB/s", bytes_per_sec as f32 / 1024.0)
    } else {
        format!("{} B/s", bytes_per_sec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_summary_integrity() {
        let (used, total, pct) = SystemInfo::get_quick_memory_summary();
        if total > 0 {
            assert!(used <= total);
            assert!((0.0..=100.0).contains(&pct));
        }
    }

    #[test]
    fn test_top_memory_processes_bounded() {
        let procs = SystemInfo::get_top_memory_processes(5);
        assert!(procs.len() <= 5);
        for p in &procs {
            assert!(p.pid > 0);
            assert!(!p.name.is_empty());
        }
    }

    #[test]
    fn test_speed_formatting() {
        assert_eq!(format_speed(500), "500 B/s");
        assert_eq!(format_speed(2048), "2 KB/s");
        assert_eq!(format_speed(5 * 1024 * 1024), "5.0 MB/s");
    }
}

