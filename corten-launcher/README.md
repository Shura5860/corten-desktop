# corten-launcher

Modern, modular, keyboard-driven application launcher and clipboard manager for Wayland, built with Rust and [iced](https://github.com/iced-rs/iced) layer-shell.

Part of the **Corten Desktop Project**.

---

## Features

- **Raycast / Spotlight Architecture**: Fast, centered/top-anchored modal interface with dark Anthracite styling and Corten Rust accents.
- **Fuzzy Search**: Powered by `nucleo`, the same algorithm backing modern terminal fuzzy finders.
- **Desktop Entry Discovery**: Automatically parses XDG application desktop files across `/usr/share/applications`, `/usr/local/share/applications`, and `~/.local/share/applications`. Respects `NoDisplay=true`.
- **Clipboard History Mode (`cliphist`)**:
  - Seamlessly switch between Application search and Clipboard history using `Tab`, top tabs, or `:clip` prefix.
  - Copies entries back into the Wayland clipboard via `cliphist decode | wl-copy`.
  - Graceful fallback: displays helpful hints when `cliphist` or `wl-clipboard` is missing.
- **Ultra-Minimalist Scrollable**: 4px slim scrollbar with automatic proportional scroll synchronization (`snap_to`) on arrow key navigation.
- **D-Bus Control (`org.corten.Launcher`)**:
  - `Toggle`: Opens or closes the launcher (perfect for compositor shortcut binds in Sway, Niri, Hyprland).
  - `Open` / `Close`: Explicit modal state control.

---

## Installation & Build

```bash
cargo build --release -p corten-launcher
```

The resulting binary will be at `target/release/corten-launcher`.

---

## Configuration

Configuration is automatically created at `~/.config/corten/launcher.toml` on first run:

```toml
width = 600
max_results = 8
placeholder = "Search applications, commands, or clipboard..."
enable_clipboard_history = true
```

The launcher supports live config reloading using `corten-config`.

---

## Usage & Keybindings

| Key / Action | Description |
| :--- | :--- |
| `↑` / `↓` | Navigate through search results (with automatic smooth scroll tracking) |
| `Tab` | Toggle between **Applications** and **Clipboard History** modes |
| `↵ (Enter)` | Launch selected application or copy selected clipboard item |
| `Esc` | Close launcher |
| `:clip <query>` | Quickly jump into clipboard mode with filter |
| `:app <query>` | Quickly jump into applications mode with filter |

### Compositor Binding Example (Sway / Niri)

```bash
# Bind super/mod key or shortcut to toggle launcher via D-Bus CLI or direct exec
bindsym Mod4+d exec corten-launcher
```

Or via D-Bus if the process is running as a daemon:
```bash
gdbus call --session --dest org.corten.Launcher --object-path /org/corten/Launcher --method org.corten.Launcher.Toggle
```

---

## License

GPL-3.0. See LICENSE for details.
