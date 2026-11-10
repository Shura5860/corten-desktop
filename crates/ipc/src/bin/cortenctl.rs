//! `cortenctl` - Official command-line control and event streaming tool for Corten Desktop.

use corten_ipc::{
    event_socket_path, get_session_connection, CompositorControlProxy,
    NotificationControlProxy, PanelControlProxy,
};
use tokio::io::AsyncBufReadExt;

fn print_help() {
    println!(
        r#"cortenctl - Corten Desktop Command-Line Control & Event Tool

USAGE:
    cortenctl subscribe                     Stream compositor events in real time (JSON-RPC)
    cortenctl action workspace <1..9>       Switch to workspace number
    cortenctl action toggle-overview        Toggle Exposé / Window Overview mode
    cortenctl action toggle-desktop         Toggle Show Desktop mode
    cortenctl action focus <window_id>      Focus window by protocol ID
    cortenctl action screenshot [--region]  Capture full screen or region
    cortenctl action reload                 Reload compositor configuration
    cortenctl action windows                List managed windows (JSON format)
    cortenctl action state                  Output full compositor state snapshot (JSON format)

    cortenctl panel toggle                  Toggle panel dock visibility
    cortenctl panel trigger <module>        Trigger module (calendar, control-center, power, clipboard)
    cortenctl panel reload                  Reload panel configuration

    cortenctl notify dnd                    Toggle Do-Not-Disturb mode
    cortenctl notify clear                  Dismiss all active notifications

OPTIONS:
    -h, --help                              Print this help message
    -v, --version                           Print version information
"#
    );
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 || args[1] == "--help" || args[1] == "-h" || args[1] == "help" {
        print_help();
        return Ok(());
    }

    if args[1] == "--version" || args[1] == "-v" {
        println!("cortenctl {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    match args[1].as_str() {
        "subscribe" => {
            let sock_path = event_socket_path();
            if !sock_path.exists() {
                eprintln!(
                    "Error: Compositor event socket not found at {:?}. Is corten-compositor running?",
                    sock_path
                );
                std::process::exit(1);
            }

            let stream = tokio::net::UnixStream::connect(&sock_path).await?;
            let mut reader = tokio::io::BufReader::new(stream).lines();
            while let Some(line) = reader.next_line().await? {
                println!("{}", line);
            }
        }
        "action" => {
            if args.len() < 3 {
                eprintln!("Error: Missing action command. See 'cortenctl --help'.");
                std::process::exit(1);
            }
            let conn = get_session_connection().await?;
            let comp = CompositorControlProxy::new(&conn).await?;

            match args[2].as_str() {
                "workspace" => {
                    let ws: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
                    comp.switch_workspace(ws).await?;
                    println!("Switched to workspace {}", ws);
                }
                "toggle-overview" | "overview" => {
                    comp.toggle_overview().await?;
                    println!("Toggled Exposé / Overview");
                }
                "toggle-desktop" | "desktop" => {
                    comp.toggle_show_desktop().await?;
                    println!("Toggled Show Desktop");
                }
                "focus" => {
                    let win_id: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
                    comp.focus_window(win_id).await?;
                    println!("Focused window {}", win_id);
                }
                "screenshot" => {
                    let region = args.iter().any(|a| a == "--region" || a == "-r");
                    comp.take_screenshot(region).await?;
                    println!("Screenshot triggered (region={})", region);
                }
                "reload" => {
                    comp.reload_config().await?;
                    println!("Compositor config reloaded");
                }
                "windows" => {
                    let windows = comp.get_windows().await?;
                    let json_windows: Vec<serde_json::Value> = windows
                        .into_iter()
                        .map(|(id, app_id, title, ws, is_active, is_floating)| {
                            serde_json::json!({
                                "id": id,
                                "app_id": app_id,
                                "title": title,
                                "workspace": ws,
                                "is_active": is_active,
                                "is_floating": is_floating,
                            })
                        })
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&json_windows)?);
                }
                "state" => {
                    let (active_ws, occupied_ws, windows) = comp.get_full_state("").await?;
                    let json_state = serde_json::json!({
                        "active_workspace": active_ws,
                        "occupied_workspaces": occupied_ws,
                        "windows_count": windows.len(),
                        "windows": windows.into_iter().map(|(id, app_id, title, ws, is_active, is_floating)| {
                            serde_json::json!({
                                "id": id,
                                "app_id": app_id,
                                "title": title,
                                "workspace": ws,
                                "is_active": is_active,
                                "is_floating": is_floating,
                            })
                        }).collect::<Vec<_>>(),
                    });
                    println!("{}", serde_json::to_string_pretty(&json_state)?);
                }
                other => {
                    eprintln!("Unknown action '{}'. See 'cortenctl --help'.", other);
                    std::process::exit(1);
                }
            }
        }
        "panel" => {
            if args.len() < 3 {
                eprintln!("Error: Missing panel command. See 'cortenctl --help'.");
                std::process::exit(1);
            }
            let conn = get_session_connection().await?;
            let panel = PanelControlProxy::new(&conn).await?;

            match args[2].as_str() {
                "toggle" => {
                    panel.toggle_visibility().await?;
                    println!("Toggled panel visibility");
                }
                "trigger" => {
                    let module = args.get(3).map(|s| s.as_str()).unwrap_or("calendar");
                    panel.trigger_module(module).await?;
                    println!("Triggered panel module '{}'", module);
                }
                "reload" => {
                    panel.reload_config().await?;
                    println!("Panel config reloaded");
                }
                other => {
                    eprintln!("Unknown panel command '{}'. See 'cortenctl --help'.", other);
                    std::process::exit(1);
                }
            }
        }
        "notify" => {
            if args.len() < 3 {
                eprintln!("Error: Missing notify command. See 'cortenctl --help'.");
                std::process::exit(1);
            }
            let conn = get_session_connection().await?;
            let notify = NotificationControlProxy::new(&conn).await?;

            match args[2].as_str() {
                "dnd" => {
                    let state = notify.toggle_dnd().await?;
                    println!("Do-Not-Disturb is now: {}", if state { "ON" } else { "OFF" });
                }
                "clear" => {
                    notify.clear_all().await?;
                    println!("Cleared all notification popups");
                }
                other => {
                    eprintln!("Unknown notify command '{}'. See 'cortenctl --help'.", other);
                    std::process::exit(1);
                }
            }
        }
        other => {
            eprintln!("Unknown command '{}'. See 'cortenctl --help'.", other);
            std::process::exit(1);
        }
    }

    Ok(())
}

