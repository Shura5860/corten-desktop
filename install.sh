#!/usr/bin/env bash
# ==============================================================================
# Corten Desktop Environment - Installation & Packaging Script
# Pure Rust Wayland Desktop Environment
# ==============================================================================

set -euo pipefail

echo "==> Building Corten Desktop Environment (Release Profile)..."
if [[ "${CORTEN_ENABLE_CAPTURE:-0}" == "1" ]]; then
    cargo build --release --features corten-capture-session/ffmpeg
else
    cargo build --release
fi

PREFIX="${PREFIX:-/usr/local}"
BIN_DIR="${PREFIX}/bin"
SHARE_DIR="${PREFIX}/share"
WAYLAND_SESSIONS_DIR="${SHARE_DIR}/wayland-sessions"
APPLICATIONS_DIR="${SHARE_DIR}/applications"
SYSTEMD_USER_DIR="${PREFIX}/lib/systemd/user"

echo "==> Installing binaries to ${BIN_DIR}..."
sudo install -d "${BIN_DIR}"
sudo install -m 755 target/release/corten-compositor "${BIN_DIR}/corten-compositor"
sudo install -m 755 target/release/corten-session "${BIN_DIR}/corten-session"
sudo install -m 755 target/release/corten-welcome "${BIN_DIR}/corten-welcome"
sudo install -m 755 target/release/corten-panel "${BIN_DIR}/corten-panel"
sudo install -m 755 target/release/corten-launcher "${BIN_DIR}/corten-launcher"
sudo install -m 755 target/release/corten-notify "${BIN_DIR}/corten-notify"
sudo install -m 755 target/release/corten-settings "${BIN_DIR}/corten-settings"
sudo install -m 755 target/release/corten-lock "${BIN_DIR}/corten-lock"
sudo install -m 755 target/release/corten-greeter "${BIN_DIR}/corten-greeter"
sudo install -m 755 target/release/corten-widgets "${BIN_DIR}/corten-widgets"
sudo install -m 755 target/release/corten-files "${BIN_DIR}/corten-files"
sudo install -m 755 target/release/corten-monitor "${BIN_DIR}/corten-monitor"
sudo install -m 755 target/release/corten-snipper "${BIN_DIR}/corten-snipper"
sudo install -m 755 target/release/corten-wallpaper "${BIN_DIR}/corten-wallpaper"
sudo install -m 755 target/release/corten-polkit "${BIN_DIR}/corten-polkit"
sudo install -m 755 target/release/corten-portal "${BIN_DIR}/corten-portal"
sudo install -m 755 target/release/cortenctl "${BIN_DIR}/cortenctl"
sudo install -m 755 target/release/corten-ai "${BIN_DIR}/corten-ai"
if [[ "${CORTEN_ENABLE_CAPTURE:-0}" == "1" ]]; then
    sudo install -m 755 target/release/corten-capture-service "${BIN_DIR}/corten-capture-service"
fi

echo "==> Installing corten-terminal wrapper to ${BIN_DIR}/corten-terminal..."
sudo tee "${BIN_DIR}/corten-terminal" > /dev/null << 'EOF'
#!/usr/bin/env sh
# Corten Desktop unified terminal launcher
if command -v foot >/dev/null 2>&1; then
    exec foot --app-id=corten-terminal "$@"
elif command -v alacritty >/dev/null 2>&1; then
    exec alacritty --class corten-terminal,corten-terminal "$@"
elif command -v kitty >/dev/null 2>&1; then
    exec kitty --class corten-terminal "$@"
elif command -v wezterm >/dev/null 2>&1; then
    exec wezterm start --class corten-terminal "$@"
elif command -v ghostty >/dev/null 2>&1; then
    exec ghostty --class=corten-terminal "$@"
elif command -v konsole >/dev/null 2>&1; then
    exec konsole "$@"
elif command -v gnome-terminal >/dev/null 2>&1; then
    exec gnome-terminal "$@"
else
    exec xterm "$@"
fi
EOF
sudo chmod 755 "${BIN_DIR}/corten-terminal"

echo "==> Installing system wallpapers and icons..."
sudo install -d /usr/share/backgrounds/corten
if [[ -d assets/wallpapers ]]; then
    sudo install -m 644 assets/wallpapers/* /usr/share/backgrounds/corten/
fi
sudo install -d /usr/share/pixmaps
if [[ -f assets/logo.png ]]; then
    sudo install -m 644 assets/logo.png /usr/share/pixmaps/corten.png
fi
if [[ -f assets/logos/ai_concept1_aura_core.png ]]; then
    sudo install -m 644 assets/logos/ai_concept1_aura_core.png /usr/share/pixmaps/corten-ai.png
fi

echo "==> Installing XDG Desktop Portal configuration..."
sudo install -d /usr/share/xdg-desktop-portal/portals
sudo tee /usr/share/xdg-desktop-portal/portals/corten.portal > /dev/null << 'EOF'
[portal]
DBusName=org.freedesktop.impl.portal.desktop.corten
Interfaces=org.freedesktop.impl.portal.ScreenCast;
UseIn=Corten;corten;
EOF
sudo tee /usr/share/xdg-desktop-portal/corten-portals.conf > /dev/null << 'EOF'
[preferred]
default=gtk;wlr;
org.freedesktop.impl.portal.ScreenCast=corten;wlr;
EOF

echo "==> Installing optional systemd user service..."
sudo install -d "${SYSTEMD_USER_DIR}"
sudo install -m 644 systemd/corten-session.service "${SYSTEMD_USER_DIR}/corten-session.service"
sudo install -m 644 systemd/corten-aura.service "${SYSTEMD_USER_DIR}/corten-aura.service"
if [[ "${CORTEN_ENABLE_CAPTURE:-0}" == "1" ]]; then
    sudo install -m 644 systemd/corten-capture.service "${SYSTEMD_USER_DIR}/corten-capture.service"
    sudo sed -i '/Environment=XDG_SESSION_TYPE=wayland/a Environment=CORTEN_ENABLE_CAPTURE=1' \
        "${SYSTEMD_USER_DIR}/corten-session.service"
fi
if [[ "${PREFIX}" != "/usr/local" ]]; then
    sudo sed -i "s#^ExecStart=.*#ExecStart=${BIN_DIR}/corten-session#" \
        "${SYSTEMD_USER_DIR}/corten-session.service"
    sudo sed -i "s#^ExecStart=.*#ExecStart=${BIN_DIR}/corten-ai --daemon#" \
        "${SYSTEMD_USER_DIR}/corten-aura.service"
fi

echo "==> Installing Wayland session entry..."
sudo install -d "${WAYLAND_SESSIONS_DIR}"
sudo install -d /usr/share/wayland-sessions
sudo tee "${WAYLAND_SESSIONS_DIR}/corten.desktop" > /dev/null << 'EOF'
[Desktop Entry]
Name=Corten Desktop
Comment=Pure Rust Modular Wayland Desktop Environment
Exec=corten-session
Type=Application
DesktopNames=Corten
Keywords=wayland;compositor;rust;desktop;
EOF
if [[ "${WAYLAND_SESSIONS_DIR}" != "/usr/share/wayland-sessions" ]]; then
    sudo install -m 644 "${WAYLAND_SESSIONS_DIR}/corten.desktop" /usr/share/wayland-sessions/corten.desktop
fi

echo "==> Installing desktop files for applications..."
sudo install -d "${APPLICATIONS_DIR}"
sudo tee "${APPLICATIONS_DIR}/corten-settings.desktop" > /dev/null << 'EOF'
[Desktop Entry]
Name=Corten Settings
Comment=System and Desktop configuration for Corten
Exec=corten-settings
Icon=preferences-system
Terminal=false
Type=Application
Categories=Settings;DesktopSettings;
EOF

sudo tee "${APPLICATIONS_DIR}/corten-files.desktop" > /dev/null << 'EOF'
[Desktop Entry]
Name=Corten Files
Comment=Modern Pure Rust File Manager
Exec=corten-files
Icon=system-file-manager
Terminal=false
Type=Application
Categories=System;FileManager;Utility;
EOF

sudo tee "${APPLICATIONS_DIR}/corten-monitor.desktop" > /dev/null << 'EOF'
[Desktop Entry]
Name=Corten Monitor
Comment=Hardware, Network and Process Monitor
Exec=corten-monitor
Icon=utilities-system-monitor
Terminal=false
Type=Application
Categories=System;Monitor;
EOF

sudo tee "${APPLICATIONS_DIR}/corten-ai.desktop" > /dev/null << 'EOF'
[Desktop Entry]
Name=Corten AI
Comment=Local AI Assistant HUD
Exec=corten-ai
Icon=help-browser
Terminal=false
Type=Application
Categories=Utility;
EOF

echo "==> Corten Desktop Environment installed successfully!"
echo "You can now log in via any Display Manager (GDM, SDDM, LightDM) by selecting 'Corten Desktop' or launch directly from TTY using: corten-session"
