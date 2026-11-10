# Corten Notify (`corten-notify`)

Lightweight, beautiful, and native Wayland notification daemon for the **Corten Desktop Environment**, built with Rust and `iced` (`wlr-layer-shell`).

![Corten Notification](https://raw.githubusercontent.com/corten-desktop/corten-desktop/main/docs/assets/notify-preview.png)

## Features
- **Native Wayland (`wlr-layer-shell`):** Runs natively on Sway, Niri, Hyprland, and Wayland compositors with zero X11 dependency.
- **Zero Screen Blockage:** Dynamic surface sizing (1x1 min size when idle) ensures completely transparent background and zero click interception when no notifications are present.
- **Freedesktop Standard:** Implements the `org.freedesktop.Notifications` D-Bus specification (`Notify`, `CloseNotification`, `GetCapabilities`, `GetServerInformation`).
- **Urgency Levels & Timeouts:** Handles `Low`, `Normal`, and persistent `Critical` alerts with distinct Corten rust/danger accents.
- **Built-in Test Mode:** Test notifications without killing existing desktop daemons using `--test`.
- **Configurable & Live Reloading:** Configured via `~/.config/corten/notify.toml` with support for screen position changes (`bottom-right`, `top-right`, `bottom-left`, `top-left`).
- **Embedded Lucide Icons:** Vector icons rendered natively without external font package dependencies.

## Configuration (`~/.config/corten/notify.toml`)

```toml
# Notification placement on screen
position = "bottom-right"

# Auto-dismiss timeout in milliseconds
default_timeout_ms = 5000

# Maximum concurrently visible notification cards
max_visible = 5

# Card width in pixels
card_width = 360

# Margin from screen edges in pixels
margin = 16
```

## Running

```bash
# Run daemon
corten-notify

# Run in standalone test mode (displays sample card)
corten-notify --test
```

## License
GPL-3.0 - Part of the Corten Desktop Project.
