# Corten Desktop

A Wayland compositor and desktop shell written in Rust using [Smithay](https://github.com/Smithay/smithay) and [Iced](https://github.com/iced-rs/iced).

![Corten Desktop running on Wayland](assets/screenshots/corten-desktop.jpg)

## About

Corten Desktop is a single-workspace Wayland desktop environment. The compositor (`corten-compositor`) is built on Smithay 0.7 with DRM/KMS and nested winit backends, XWayland support, and both floating and tiling layouts. Shell components and built-in utilities use Iced 0.13 with the `tiny-skia` software renderer to keep idle GPU memory usage low, communicating over D-Bus (`zbus`) and a Unix domain socket (`cortenctl`).

This is an independent personal project under active development.

### Current Status

- Tested on Linux with DRM/KMS (TTY session via `corten-session`) and nested inside existing Wayland sessions.
- Floating, Master-Stack, BSP, Monocle, and Scrollable tiling layouts are implemented.
- Shell components (3-island dock, control center, app drawer, notifications, MPRIS/telemetry desktop widgets, lock screen, file manager, settings, package manager front-end) run as separate processes supervised by `corten-session`.
- UI strings are currently primarily in Turkish; full i18n coverage is still in progress.
- Expect rough edges and breaking config changes between minor releases.

## Workspace Components

| Crate | Description |
| :--- | :--- |
| `corten-compositor` | Wayland compositor (Smithay 0.7, DRM/KMS + winit, XWayland, floating & tiling) |
| `corten-session` | Session supervisor and startup orchestrator |
| `corten-panel` | Bottom/top 3-island dock, app drawer, SNI system tray, calendar, control center |
| `corten-widgets` | Desktop side panel (clock, weather, CPU/RAM/GPU telemetry, D-Bus MPRIS player, network) |
| `corten-launcher` | Application launcher, calculator, clipboard history, and file search |
| `corten-files` | File manager with list/grid/column views, split pane, quick preview, and terminal drawer |
| `corten-settings` | Settings app (appearance, panel, widgets, displays, audio, network/Bluetooth, input, shortcuts) |
| `corten-notify` | `org.freedesktop.Notifications` daemon |
| `corten-lock` | `ext-session-lock-v1` lock screen (PAM) |
| `corten-greeter` | `greetd` login greeter |
| `corten-wallpaper` | Multi-monitor wallpaper daemon |
| `corten-snipper` | Region screenshot tool |
| `corten-monitor` | System and process monitor |
| `corten-forge` | Front-end for Flatpak and Arch package management |
| `corten-portal` / `corten-polkit` | XDG Desktop Portal backend and Polkit authentication agent |
| `corten-ai` | Optional local LLM client (Ollama / GGUF) |
| `crates/ipc` (`cortenctl`) | CLI for compositor IPC queries, actions, and `cortenctl doctor` |

## Building from Source

### Dependencies

**Arch Linux:**

```bash
sudo pacman -S base-devel rust clang wayland libxkbcommon libseat systemd mesa libinput pipewire wireplumber pam brightnessctl wl-clipboard
```

**Debian / Ubuntu:**

```bash
sudo apt install build-essential pkg-config cargo clang libclang-dev llvm-dev libwayland-dev libxkbcommon-dev libseat-dev libudev-dev libgbm-dev libdrm-dev libegl1-mesa-dev libgles2-mesa-dev libinput-dev libpam0g-dev libpipewire-0.3-dev libspa-0.2-dev libdbus-1-dev wl-clipboard
```

**Fedora:**

```bash
sudo dnf install @development-tools rust cargo clang clang-devel wayland-devel libxkbcommon-devel libseat-devel systemd-devel mesa-libEGL-devel mesa-libGLES-devel libinput-devel pam-devel pipewire-devel wl-clipboard
```

### Build & Install

```bash
git clone https://github.com/Shura5860/corten-desktop.git
cd corten-desktop

cargo build --release --workspace
sudo ./install.sh
```

After installation, pick **Corten Desktop** from your display manager, or start directly from a TTY:

```bash
corten-session
```

To test a local build from a TTY without installing system-wide:

```bash
PATH="$PWD/target/release:$PATH" ./target/release/corten-session
```

Check environment and hardware readiness:

```bash
cortenctl doctor
```

### Packaging

Packaging definitions are available in [`packaging/`](packaging/):
- Arch Linux: `cd packaging/arch && makepkg -si`
- Nix: `nix build github:Shura5860/corten-desktop`
- Debian: `./packaging/debian/build-deb.sh 0.7.0`
- Fedora: `rpmbuild -ba packaging/rpm/corten-desktop.spec`

## Default Keybindings

Configured in `~/.config/corten/compositor.toml`:

| Shortcut | Action |
| :--- | :--- |
| `Super + Return` | Open terminal |
| `Super + Space` | Open application launcher |
| `Super + E` | Open file manager (`corten-files`) |
| `Super + I` | Open settings (`corten-settings`) |
| `Super + W` | Window overview |
| `Super + D` | Show desktop |
| `Super + Shift + S` | Region screenshot (`corten-snipper`) |
| `Super + 1..9` | Switch workspace |
| `Super + Shift + 1..9` | Move window to workspace |
| `Super + G` | Toggle Game Mode (VRR / tearing / DND) |
| `Ctrl + Alt + Backspace` | Exit session to TTY (when unlocked) |

## Development Note

I use AI coding assistants while working on this project, mostly for UI boilerplate and refactoring, while handling the architecture, compositor integration, and TTY hardware testing myself. Bug reports, code reviews, and pull requests are welcome.

## License

Licensed under the GNU General Public License v3.0. See [LICENSE](LICENSE).
