use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoredApp {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub workspace: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    pub apps: Vec<RestoredApp>,
}

fn session_state_file_path() -> Option<PathBuf> {
    directories::BaseDirs::new()
        .map(|dirs| dirs.config_dir().join("corten").join("session_state.json"))
}

pub fn load_session_state() -> Option<SessionState> {
    let path = session_state_file_path()?;
    if !path.exists() {
        return None;
    }
    let data = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn save_session_state(state: &SessionState) -> Result<()> {
    if let Some(path) = session_state_file_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let json = serde_json::to_string_pretty(state)?;
        std::fs::write(path, json)?;
    }
    Ok(())
}

fn restore_session_apps(socket_name: &str) {
    if let Some(state) = load_session_state() {
        info!("Restoring {} session applications...", state.apps.len());
        for app in state.apps {
            info!(
                "Restoring app '{}' with args {:?} on workspace {}",
                app.command, app.args, app.workspace
            );
            let mut cmd = Command::new(&app.command);
            cmd.args(&app.args)
                .env("WAYLAND_DISPLAY", socket_name)
                .env("CORTEN_INITIAL_WORKSPACE", app.workspace.to_string())
                .stderr(std::process::Stdio::null());
            match cmd.spawn() {
                Ok(_) => info!("Successfully launched restored app: {}", app.command),
                Err(e) => warn!("Failed to restore app {}: {}", app.command, e),
            }
        }
    }
}
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::{error, info, warn};

/// Managed child process within the Corten session.
struct SessionProcess {
    name: &'static str,
    child: Child,
    factory: Box<dyn Fn() -> Command + Send + Sync>,
    restart_count: u8,
}

impl SessionProcess {
    fn spawn(
        name: &'static str,
        mut cmd: Command,
        factory: Box<dyn Fn() -> Command + Send + Sync>,
    ) -> Option<Self> {
        info!("Spawning session component: {}", name);
        cmd.stderr(std::process::Stdio::inherit());
        cmd.stdout(std::process::Stdio::inherit());
        match cmd.spawn() {
            Ok(child) => Some(Self {
                name,
                child,
                factory,
                restart_count: 0,
            }),
            Err(e) => {
                warn!("Failed to start component {}: {}", name, e);
                None
            }
        }
    }

    fn supervise(&mut self) {
        let exited = match self.child.try_wait() {
            Ok(Some(status)) => Some(status),
            Ok(None) => None,
            Err(error) => {
                warn!("Failed to poll {}: {}", self.name, error);
                Some(std::process::ExitStatus::default())
            }
        };
        let Some(status) = exited else { return };

        if self.restart_count >= 3 {
            warn!(
                "Component {} exited with {} and reached restart limit; leaving it stopped.",
                self.name, status
            );
            return;
        }

        self.restart_count += 1;
        warn!(
            "Component {} exited with {}; restarting ({}/3).",
            self.name, status, self.restart_count
        );
        let mut command = (self.factory)();
        command.stderr(std::process::Stdio::inherit());
        command.stdout(std::process::Stdio::inherit());
        match command.spawn() {
            Ok(child) => self.child = child,
            Err(error) => warn!("Failed to restart {}: {}", self.name, error),
        }
    }

    fn terminate(&mut self) {
        let pid = self.child.id() as i32;
        info!("Terminating {} (PID: {})...", self.name, pid);
        let _ = kill(Pid::from_raw(pid), Signal::SIGTERM);
    }

    fn wait_timeout(&mut self, timeout: Duration) {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    info!("Component {} exited with status: {}", self.name, status);
                    return;
                }
                Ok(None) => thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
    }
}

use std::io::IsTerminal;

static RUNNING_FLAG: AtomicBool = AtomicBool::new(true);

extern "C" fn sig_termination_handler(_: std::os::raw::c_int) {
    RUNNING_FLAG.store(false, Ordering::SeqCst);
}

fn register_signals(running: Arc<AtomicBool>) {
    unsafe {
        let _ = nix::sys::signal::signal(
            nix::sys::signal::Signal::SIGINT,
            nix::sys::signal::SigHandler::Handler(sig_termination_handler),
        );
        let _ = nix::sys::signal::signal(
            nix::sys::signal::Signal::SIGTERM,
            nix::sys::signal::SigHandler::Handler(sig_termination_handler),
        );
        let _ = nix::sys::signal::signal(
            nix::sys::signal::Signal::SIGHUP,
            nix::sys::signal::SigHandler::Handler(sig_termination_handler),
        );
    }

    // Only listen on stdin if attached to an interactive terminal,
    // and only exit on real input (never on EOF/closed pipe).
    let r = running.clone();
    if std::io::stdin().is_terminal() {
        let _ = std::thread::spawn(move || {
            let mut buffer = String::new();
            if let Ok(bytes) = std::io::stdin().read_line(&mut buffer) {
                if bytes > 0 {
                    r.store(false, Ordering::SeqCst);
                }
            }
        });
    }
}

/// Locate a binary either in current PATH or in the target release/debug build folder.
fn find_binary(bin_name: &str) -> PathBuf {
    // 1. Check if it's in target/release or target/debug relative to current exe
    if let Ok(current_exe) = env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            let candidate = parent.join(bin_name);
            if candidate.exists() {
                return candidate;
            }
        }
    }

    // 2. Check local repo build folders (target/release, target/debug, etc.)
    for candidate_dir in ["target/release", "target/debug", "."] {
        let candidate = Path::new(candidate_dir).join(bin_name);
        if candidate.exists() {
            return candidate;
        }
    }

    // 3. Fallback to binary name (found via PATH)
    PathBuf::from(bin_name)
}

fn component_command(binary: PathBuf, socket_name: String) -> Command {
    let mut command = Command::new(binary);
    command.env("WAYLAND_DISPLAY", socket_name);
    command
}

fn wait_for_wayland_socket(socket_name: &str, timeout: Duration) -> bool {
    let runtime_dir = env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let socket_path = Path::new(&runtime_dir).join(socket_name);
    let start = std::time::Instant::now();

    info!("Waiting for Wayland socket {:?}...", socket_path);
    while start.elapsed() < timeout {
        if socket_path.exists() {
            // Check if socket actually accepts connections
            if std::os::unix::net::UnixStream::connect(&socket_path).is_ok() {
                // Allow compositor loop a brief moment to finish advertising globals (wl_output, wlr_layer_shell)
                thread::sleep(Duration::from_millis(300));
                info!(
                    "Wayland socket {:?} is ready and accepting connections!",
                    socket_path
                );
                return true;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}

#[derive(Debug, PartialEq, Eq)]
pub struct AutostartEntry {
    pub name: String,
    pub exec: Vec<String>,
}

pub fn parse_autostart_desktop(content: &str) -> Option<AutostartEntry> {
    let mut name = None;
    let mut exec = None;
    let mut hidden = false;
    let mut only_show_in: Option<Vec<String>> = None;
    let mut not_show_in: Option<Vec<String>> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("Hidden=") {
            if line.trim_start_matches("Hidden=").trim().eq_ignore_ascii_case("true") {
                hidden = true;
            }
        } else if line.starts_with("Name=") && name.is_none() {
            name = Some(line.trim_start_matches("Name=").trim().to_string());
        } else if line.starts_with("Exec=") && exec.is_none() {
            exec = Some(line.trim_start_matches("Exec=").trim().to_string());
        } else if line.starts_with("OnlyShowIn=") {
            let list = line
                .trim_start_matches("OnlyShowIn=")
                .split(';')
                .filter(|s| !s.is_empty())
                .map(|s| s.trim().to_ascii_lowercase())
                .collect();
            only_show_in = Some(list);
        } else if line.starts_with("NotShowIn=") {
            let list = line
                .trim_start_matches("NotShowIn=")
                .split(';')
                .filter(|s| !s.is_empty())
                .map(|s| s.trim().to_ascii_lowercase())
                .collect();
            not_show_in = Some(list);
        }
    }

    if hidden {
        return None;
    }

    if let Some(ref list) = only_show_in {
        let matched = list.iter().any(|d| d == "corten" || d == "corten desktop" || d == "x-corten");
        if !matched {
            return None;
        }
    }

    if let Some(ref list) = not_show_in {
        let matched = list.iter().any(|d| d == "corten" || d == "corten desktop" || d == "x-corten");
        if matched {
            return None;
        }
    }

    let exec_str = exec?;
    let tokens: Vec<String> = exec_str
        .split_whitespace()
        .filter(|t| !t.starts_with('%'))
        .map(|t| t.to_string())
        .collect();

    if tokens.is_empty() {
        return None;
    }

    Some(AutostartEntry {
        name: name.unwrap_or_else(|| tokens[0].clone()),
        exec: tokens,
    })
}

fn spawn_xdg_autostart(socket_name: &str) {
    let mut dirs = Vec::new();
    if let Some(base_dirs) = directories::BaseDirs::new() {
        dirs.push(base_dirs.config_dir().join("autostart"));
    }
    dirs.push(PathBuf::from("/etc/xdg/autostart"));

    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("desktop") {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Some(item) = parse_autostart_desktop(&content) {
                            if let Some((prog, args)) = item.exec.split_first() {
                                info!("Launching XDG Autostart app '{}' ({:?})", item.name, prog);
                                let _ = Command::new(prog)
                                    .args(args)
                                    .env("WAYLAND_DISPLAY", socket_name)
                                    .stderr(std::process::Stdio::null())
                                    .spawn();
                            }
                        }
                    }
                }
            }
        }
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "corten_session=debug,info".into()),
        )
        .init();

    info!("==========================================");
    info!("   CORTEN DESKTOP ENVIRONMENT v0.1.0      ");
    info!("   Pure Rust Wayland Session Orchestrator ");
    info!("==========================================");

    // Set standard XDG desktop environment variables
    env::set_var("XDG_CURRENT_DESKTOP", "Corten");
    env::set_var("XDG_SESSION_DESKTOP", "corten");
    env::set_var("XDG_SESSION_TYPE", "wayland");

    // Prepend the release/debug directory to PATH so direct component spawns
    // and compositor shortcuts can find the freshly built binaries.
    if let Ok(current_exe) = env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            if let Ok(old_path) = env::var("PATH") {
                env::set_var("PATH", format!("{}:{}", parent.display(), old_path));
            }
        }
    }

    // 1. Start the Compositor
    let compositor_bin = find_binary("corten-compositor");
    info!("Launching Compositor: {:?}", compositor_bin);

    let mut compositor_cmd = Command::new(&compositor_bin);
    let mut compositor_child = compositor_cmd
        .spawn()
        .with_context(|| format!("Failed to spawn compositor at {:?}", compositor_bin))?;

    let mut compositor_pid = compositor_child.id() as i32;
    info!("Corten Compositor started (PID: {})", compositor_pid);

    // 2. Wait for corten-0 Wayland socket to be advertised
    let socket_name = "corten-0";
    if !wait_for_wayland_socket(socket_name, Duration::from_secs(5)) {
        warn!(
            "Socket {:?} was not found in time, proceeding anyway...",
            socket_name
        );
    }

    // Export WAYLAND_DISPLAY and graphic toolkit drivers for child processes
    env::set_var("WAYLAND_DISPLAY", socket_name);
    env::set_var("GDK_BACKEND", "wayland");
    env::set_var("QT_QPA_PLATFORM", "wayland");
    env::set_var("WINIT_UNIX_BACKEND", "wayland");
    let cursor_theme = if std::path::Path::new("/usr/share/icons/breeze_cursors").exists() {
        "breeze_cursors"
    } else if std::path::Path::new("/usr/share/icons/Adwaita").exists() {
        "Adwaita"
    } else {
        "default"
    };

    env::set_var("GTK_THEME", "Adwaita:dark");
    env::set_var("XCURSOR_THEME", cursor_theme);
    env::set_var("XCURSOR_SIZE", "24");

    // Apply dark color scheme preference, modern cursor, and Corten orange accent to GTK / GNOME settings if gsettings exists
    let _ = Command::new("gsettings")
        .args(["set", "org.gnome.desktop.interface", "color-scheme", "prefer-dark"])
        .status();
    let _ = Command::new("gsettings")
        .args(["set", "org.gnome.desktop.interface", "gtk-theme", "Adwaita-dark"])
        .status();
    let _ = Command::new("gsettings")
        .args(["set", "org.gnome.desktop.interface", "cursor-theme", cursor_theme])
        .status();
    let _ = Command::new("gsettings")
        .args(["set", "org.gnome.desktop.interface", "accent-color", "orange"])
        .status();

    // Synchronize WAYLAND_DISPLAY and XDG_CURRENT_DESKTOP with systemd/D-Bus user environment
    // so xdg-desktop-portal and xdg-desktop-portal-wlr can attach to the active Wayland session.
    let _ = Command::new("dbus-update-activation-environment")
        .args([
            "--systemd",
            "WAYLAND_DISPLAY",
            "XDG_CURRENT_DESKTOP",
            "XDG_SESSION_DESKTOP",
            "XDG_SESSION_TYPE",
            "GTK_THEME",
            "XCURSOR_THEME",
            "XCURSOR_SIZE",
        ])
        .status();

    // Respect the system's graphics stack. NVIDIA-specific overrides must be
    // supplied by the administrator/environment, never forced on AMD/Intel.
    if env::var_os("WGPU_BACKEND").is_none() {
        env::set_var("WGPU_BACKEND", "vulkan");
    }

    // Setup POSIX signals (SIGINT, SIGTERM, SIGHUP) handler and optional interactive TTY stdin
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    register_signals(r);

    // 4. Optional Authentication Stage: By default, greetd/corten-greeter has already
    // authenticated the user before launching corten-session. Only run corten-lock
    // at session startup when explicitly requested (--lock-on-start or CORTEN_LOCK_ON_START=1).
    let lock_on_start = env::args().any(|arg| arg == "--lock-on-start")
        || env::var("CORTEN_LOCK_ON_START").as_deref() == Ok("1");

    if lock_on_start {
        info!("Running Corten Lock: Authenticate to enter desktop environment...");
        let lock_bin = find_binary("corten-lock");
        let mut lock_cmd = Command::new(&lock_bin);
        lock_cmd.env("WAYLAND_DISPLAY", socket_name);

        let mut lock_child = match lock_cmd.spawn() {
            Ok(child) => child,
            Err(error) => {
                error!(%error, "corten-lock could not be started; refusing an unlocked session");
                let _ = kill(Pid::from_raw(compositor_pid), Signal::SIGTERM);
                let _ = compositor_child.wait();
                return Err(error).context("failed to start mandatory corten-lock");
            }
        };
        let lock_status = match lock_child.wait() {
            Ok(status) => status,
            Err(error) => {
                error!(%error, "failed while waiting for corten-lock");
                let _ = kill(Pid::from_raw(compositor_pid), Signal::SIGTERM);
                let _ = compositor_child.wait();
                return Err(error).context("failed while waiting for corten-lock");
            }
        };
        info!("Lock screen finished with status: {:?}", lock_status);
        if !lock_status.success() {
            info!("Authentication aborted or failed. Exiting session.");
            let _ = kill(Pid::from_raw(compositor_pid), Signal::SIGTERM);
            let _ = compositor_child.wait();
            return Ok(());
        }
    }

    // 5. Desktop Initialization Pipeline
    let mut session_processes = Vec::new();

    // Always start corten-wallpaper first so monitors have a clean backdrop
    let wallpaper_bin = find_binary("corten-wallpaper");
    let wallpaper_socket = socket_name.to_string();
    let wallpaper_factory_bin = wallpaper_bin.clone();
    let wallpaper_factory = Box::new(move || {
        component_command(wallpaper_factory_bin.clone(), wallpaper_socket.clone())
    });
    let wallpaper_cmd = wallpaper_factory();
    if let Some(proc) = SessionProcess::spawn("corten-wallpaper", wallpaper_cmd, wallpaper_factory)
    {
        session_processes.push(proc);
    }

    // STAGE 1: First-Boot Setup & Welcome Wizard
    // If ~/.config/corten/.welcome_completed does not exist (or --welcome flag was supplied),
    // present only the onboarding setup wizard. Do not spawn dock, widgets or autostart yet.
    let show_welcome = env::args().any(|arg| arg == "--welcome") || {
        directories::BaseDirs::new()
            .map(|d| !d.config_dir().join("corten").join(".welcome_completed").exists())
            .unwrap_or(false)
    };

    if show_welcome {
        let welcome_bin = find_binary("corten-welcome");
        if welcome_bin.exists() {
            info!("=======================================================");
            info!("  STAGE 1: First-Boot Setup Wizard (corten-welcome)   ");
            info!("=======================================================");
            let mut welcome_cmd = Command::new(&welcome_bin);
            welcome_cmd.env("WAYLAND_DISPLAY", socket_name);
            match welcome_cmd.spawn() {
                Ok(mut child) => {
                    info!("Waiting for user to complete setup wizard...");
                    while running.load(Ordering::SeqCst) && RUNNING_FLAG.load(Ordering::SeqCst) {
                        for process in &mut session_processes {
                            process.supervise();
                        }
                        if let Ok(Some(status)) = child.try_wait() {
                            info!("Setup wizard completed with status: {:?}", status);
                            break;
                        }
                        if let Ok(Some(comp_status)) = compositor_child.try_wait() {
                            info!("Compositor exited during setup: {:?}", comp_status);
                            let _ = child.kill();
                            for proc in &mut session_processes {
                                proc.terminate();
                            }
                            return Ok(());
                        }
                        thread::sleep(Duration::from_millis(50));
                    }
                }
                Err(e) => warn!("Failed to spawn corten-welcome: {}", e),
            }
        }
    }

    if !running.load(Ordering::SeqCst) || !RUNNING_FLAG.load(Ordering::SeqCst) {
        for proc in &mut session_processes {
            proc.terminate();
        }
        let _ = kill(Pid::from_raw(compositor_pid), Signal::SIGTERM);
        let _ = compositor_child.wait();
        return Ok(());
    }

    // STAGE 2: Full Desktop Environment Launch
    info!("=======================================================");
    info!("  STAGE 2: Spawning Corten Desktop Environment         ");
    info!("=======================================================");

    // A. corten-notify (Notification Daemon)
    let notify_bin = find_binary("corten-notify");
    let notify_socket = socket_name.to_string();
    let notify_factory_bin = notify_bin.clone();
    let notify_factory =
        Box::new(move || component_command(notify_factory_bin.clone(), notify_socket.clone()));
    let notify_cmd = notify_factory();
    if let Some(proc) = SessionProcess::spawn("corten-notify", notify_cmd, notify_factory) {
        session_processes.push(proc);
    }

    // B. corten-panel (Centered Dock)
    let panel_bin = find_binary("corten-panel");
    let panel_socket = socket_name.to_string();
    let panel_factory_bin = panel_bin.clone();
    let panel_factory =
        Box::new(move || component_command(panel_factory_bin.clone(), panel_socket.clone()));
    let panel_cmd = panel_factory();
    if let Some(proc) = SessionProcess::spawn("corten-panel", panel_cmd, panel_factory) {
        session_processes.push(proc);
    }

    // Optional capture service. Its PipeWire/FFmpeg dependencies are kept out
    // of the base desktop build and are enabled explicitly at installation.
    if env::var("CORTEN_ENABLE_CAPTURE").as_deref() == Ok("1") {
        let capture_bin = find_binary("corten-capture-service");
        let capture_factory_bin = capture_bin.clone();
        let capture_factory = Box::new(move || Command::new(capture_factory_bin.clone()));
        let capture_cmd = capture_factory();
        if let Some(proc) =
            SessionProcess::spawn("corten-capture-service", capture_cmd, capture_factory)
        {
            session_processes.push(proc);
        } else {
            warn!("Capture service was requested but could not be started");
        }
    }

    // C. corten-widgets (Desktop Desklets / Visualizer / Notes)
    let widgets_bin = find_binary("corten-widgets");
    let widgets_socket = socket_name.to_string();
    let widgets_factory_bin = widgets_bin.clone();
    let widgets_factory =
        Box::new(move || component_command(widgets_factory_bin.clone(), widgets_socket.clone()));
    let widgets_cmd = widgets_factory();
    if let Some(proc) = SessionProcess::spawn("corten-widgets", widgets_cmd, widgets_factory) {
        session_processes.push(proc);
    }

    // D. Clipboard watcher (wl-paste -> cliphist)
    let cliphist_socket = socket_name.to_string();
    let cliphist_factory = Box::new(move || {
        let mut command = Command::new("wl-paste");
        command
            .args(["--watch", "cliphist", "store"])
            .env("WAYLAND_DISPLAY", cliphist_socket.clone());
        command
    });
    let cliphist_cmd = cliphist_factory();
    if let Some(proc) = SessionProcess::spawn("cliphist-watcher", cliphist_cmd, cliphist_factory) {
        session_processes.push(proc);
    }

    // E. Polkit Authentication Agent (auto-detect corten-polkit or installed system agent)
    let polkit_bin = find_binary("corten-polkit");
    if polkit_bin.exists() {
        let p_socket = socket_name.to_string();
        let p_factory_bin = polkit_bin.clone();
        let p_factory =
            Box::new(move || component_command(p_factory_bin.clone(), p_socket.clone()));
        let p_cmd = p_factory();
        if let Some(proc) = SessionProcess::spawn("corten-polkit", p_cmd, p_factory) {
            session_processes.push(proc);
        }
    } else {
        for agent_candidate in [
            "/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1",
            "/usr/libexec/polkit-gnome-authentication-agent-1",
            "/usr/lib/polkit-kde-authentication-agent-1",
            "/usr/libexec/polkit-kde-authentication-agent-1",
            "/usr/bin/lxqt-policykit-agent",
            "/usr/libexec/polkit-mate-authentication-agent-1",
        ] {
            let agent_path = Path::new(agent_candidate);
            if agent_path.exists() {
                let p_path = agent_path.to_path_buf();
                let p_socket = socket_name.to_string();
                let p_factory =
                    Box::new(move || component_command(p_path.clone(), p_socket.clone()));
                let p_cmd = p_factory();
                if let Some(proc) = SessionProcess::spawn("polkit-agent", p_cmd, p_factory) {
                    session_processes.push(proc);
                }
                break;
            }
        }
    }

    // E.2. XDG Desktop Portal Backend (`corten-portal` — org.freedesktop.impl.portal.ScreenCast)
    let portal_bin = find_binary("corten-portal");
    if portal_bin.exists() {
        let portal_socket = socket_name.to_string();
        let portal_factory_bin = portal_bin.clone();
        let portal_factory =
            Box::new(move || component_command(portal_factory_bin.clone(), portal_socket.clone()));
        let portal_cmd = portal_factory();
        if let Some(proc) = SessionProcess::spawn("corten-portal", portal_cmd, portal_factory) {
            session_processes.push(proc);
        }
    }

    // F. User Autostart script (`~/.config/corten/autostart.sh`) and commands (`~/.config/corten/autostart.conf`)
    if let Some(base_dirs) = directories::BaseDirs::new() {
        let corten_cfg_dir = base_dirs.config_dir().join("corten");
        let autostart_sh = corten_cfg_dir.join("autostart.sh");
        if autostart_sh.exists() {
            info!("Executing user autostart script: {:?}", autostart_sh);
            let _ = Command::new("sh")
                .arg(&autostart_sh)
                .env("WAYLAND_DISPLAY", socket_name)
                .stderr(std::process::Stdio::null())
                .spawn();
        }
        let autostart_conf = corten_cfg_dir.join("autostart.conf");
        if let Ok(contents) = std::fs::read_to_string(&autostart_conf) {
            for line in contents.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                let mut parts = trimmed.split_whitespace();
                if let Some(prog) = parts.next() {
                    info!("Spawning autostart command: {}", trimmed);
                    let _ = Command::new(prog)
                        .args(parts)
                        .env("WAYLAND_DISPLAY", socket_name)
                        .stderr(std::process::Stdio::null())
                        .spawn();
                }
            }
        }
    }

    // G. Standard XDG Autostart (.desktop files in ~/.config/autostart and /etc/xdg/autostart)
    spawn_xdg_autostart(socket_name);

    // H. Restore Saved Session Applications (State & Workspace mapping)
    restore_session_apps(socket_name);

    info!("All session components initialized successfully.");
    info!("Desktop shortcuts:");
    info!("  - Super+D or Super+Space : Corten Spotlight Launcher");
    info!("  - Super+Return          : Terminal");
    info!("  - Super+E               : Corten Files");
    info!("  - Super+I / Super+,      : Corten Settings");
    info!("  - Super+Q                : Close Active Window");
    info!("  - Super+J / Super+K      : Focus Next / Previous Window");

    // 5. Main Session Supervision Loop with Compositor Crash Resilience
    let mut compositor_restart_count: u8 = 0;
    const MAX_COMPOSITOR_RESTARTS: u8 = 3;

    while running.load(Ordering::SeqCst) && RUNNING_FLAG.load(Ordering::SeqCst) {
        for process in &mut session_processes {
            process.supervise();
        }

        // Compositor supervision: Auto-recover on unexpected crash!
        match compositor_child.try_wait() {
            Ok(Some(status)) => {
                let session_active =
                    running.load(Ordering::SeqCst) && RUNNING_FLAG.load(Ordering::SeqCst);
                if session_active
                    && !status.success()
                    && compositor_restart_count < MAX_COMPOSITOR_RESTARTS
                {
                    compositor_restart_count += 1;
                    warn!(
                        "⚠️ CRITICAL: Corten Compositor crashed with status: {}! Initiating auto-recovery ({}/{})...",
                        status, compositor_restart_count, MAX_COMPOSITOR_RESTARTS
                    );

                    // Wait 500ms before respawning to allow DRM/KMS clean drop
                    thread::sleep(Duration::from_millis(500));

                    let mut new_cmd = Command::new(&compositor_bin);
                    match new_cmd.spawn() {
                        Ok(new_child) => {
                            compositor_child = new_child;
                            compositor_pid = compositor_child.id() as i32;
                            info!(
                                "Successfully revived Corten Compositor (New PID: {})",
                                compositor_pid
                            );

                            if wait_for_wayland_socket(socket_name, Duration::from_secs(5)) {
                                info!("Restored Wayland socket. Re-supervising session components...");
                                for proc in &mut session_processes {
                                    proc.supervise();
                                }
                                restore_session_apps(socket_name);
                                continue;
                            }
                        }
                        Err(e) => {
                            error!("Failed to respawn compositor during crash recovery: {}", e);
                            break;
                        }
                    }
                } else {
                    info!(
                        "Compositor exited with status: {}. Ending session...",
                        status
                    );
                    break;
                }
            }
            Ok(None) => {}
            Err(e) => {
                error!("Error waiting on compositor: {}", e);
                break;
            }
        }

        thread::sleep(Duration::from_millis(100));
    }

    // 6. Teardown All Child Processes
    info!("Shutting down Corten desktop session components...");
    for proc in &mut session_processes {
        proc.terminate();
    }
    for proc in &mut session_processes {
        proc.wait_timeout(Duration::from_millis(500));
    }

    // Terminate compositor if still running
    let _ = kill(Pid::from_raw(compositor_pid), Signal::SIGTERM);
    let _ = compositor_child.wait();

    info!("Corten Session ended cleanly. Returning to TTY.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_state_serde_roundtrip() {
        let state = SessionState {
            apps: vec![
                RestoredApp {
                    command: "corten-files".into(),
                    args: vec!["/home/user".into()],
                    workspace: 2,
                },
                RestoredApp {
                    command: "alacritty".into(),
                    args: vec![],
                    workspace: 1,
                },
            ],
        };

        let json = serde_json::to_string(&state).expect("serialize");
        let restored: SessionState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.apps.len(), 2);
        assert_eq!(restored.apps[0].command, "corten-files");
        assert_eq!(restored.apps[0].workspace, 2);
        assert_eq!(restored.apps[1].command, "alacritty");
    }

    #[test]
    fn test_parse_autostart_desktop_standard() {
        let content = r#"[Desktop Entry]
Type=Application
Name=Nextcloud Sync Client
Exec=nextcloud --background %u
OnlyShowIn=X-CORTEN;GNOME;
Hidden=false
"#;
        let entry = parse_autostart_desktop(content).expect("should parse");
        assert_eq!(entry.name, "Nextcloud Sync Client");
        assert_eq!(entry.exec, vec!["nextcloud", "--background"]);
    }

    #[test]
    fn test_parse_autostart_desktop_hidden_returns_none() {
        let content = r#"[Desktop Entry]
Type=Application
Name=Disabled App
Exec=disabled-bin
Hidden=true
"#;
        assert_eq!(parse_autostart_desktop(content), None);
    }

    #[test]
    fn test_parse_autostart_desktop_not_show_in_corten() {
        let content = r#"[Desktop Entry]
Type=Application
Name=KDE Plasma Daemon
Exec=kded5
NotShowIn=corten;XFCE;
"#;
        assert_eq!(parse_autostart_desktop(content), None);
    }

    #[test]
    fn test_parse_autostart_desktop_only_show_in_other_desktop() {
        let content = r#"[Desktop Entry]
Type=Application
Name=GNOME Keyring
Exec=gnome-keyring-daemon
OnlyShowIn=GNOME;Unity;
"#;
        assert_eq!(parse_autostart_desktop(content), None);
    }

    #[test]
    fn test_find_binary_fallback_to_path() {
        let bin = find_binary("bash");
        assert!(bin.to_string_lossy().contains("bash"));
    }
}
