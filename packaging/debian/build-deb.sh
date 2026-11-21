#!/usr/bin/env bash
set -euo pipefail

VERSION="${1:-0.5.6}"
ARCH="$(dpkg --print-architecture 2>/dev/null || echo amd64)"
PKG_DIR="target/debian/corten-desktop_${VERSION}_${ARCH}"

echo "==> Building Corten Desktop v${VERSION} (${ARCH}) release binaries..."
cargo build --release

rm -rf "${PKG_DIR}"
mkdir -p "${PKG_DIR}/DEBIAN"
mkdir -p "${PKG_DIR}/usr/bin"
mkdir -p "${PKG_DIR}/usr/share/wayland-sessions"
mkdir -p "${PKG_DIR}/usr/share/xdg-desktop-portal/portals"
mkdir -p "${PKG_DIR}/usr/share/backgrounds/corten"

cat > "${PKG_DIR}/DEBIAN/control" <<EOF
Package: corten-desktop
Version: ${VERSION}
Section: x11
Priority: optional
Architecture: ${ARCH}
Maintainer: Corten Desktop Contributors <https://github.com/Shura5860/corten-desktop>
Depends: libc6, libwayland-server0, libxkbcommon0, libinput10, libdrm2, libgbm1, libseat1, libpam0g, wl-clipboard, grim
Recommends: xwayland, pipewire, wireplumber, xdg-desktop-portal, network-manager
Description: Pure-Rust Wayland desktop environment powered by Smithay, Iced & AURA-CORE
EOF

BINS=(
    corten-compositor
    corten-session
    corten-panel
    corten-launcher
    corten-settings
    corten-files
    corten-notify
    corten-lock
    corten-greeter
    corten-monitor
    corten-snipper
    corten-widgets
    corten-wallpaper
    corten-portal
    corten-polkit
    corten-ai
    corten-welcome
    corten-forge
    corten-update
    cortenctl
)

for bin in "${BINS[@]}"; do
    if [ -f "target/release/${bin}" ]; then
        install -Dm755 "target/release/${bin}" "${PKG_DIR}/usr/bin/${bin}"
    fi
done
install -Dm755 "scripts/corten-terminal" "${PKG_DIR}/usr/bin/corten-terminal"

install -Dm644 "corten-session/corten.desktop" "${PKG_DIR}/usr/share/wayland-sessions/corten.desktop"
install -Dm644 "corten-portal/corten.portal" "${PKG_DIR}/usr/share/xdg-desktop-portal/portals/corten.portal"
install -Dm644 "corten-portal/corten-portals.conf" "${PKG_DIR}/usr/share/xdg-desktop-portal/corten-portals.conf"

if [ -d "assets/wallpapers" ]; then
    cp -r assets/wallpapers/* "${PKG_DIR}/usr/share/backgrounds/corten/" || true
fi

dpkg-deb --build --root-owner-group "${PKG_DIR}"
echo "==> Built Debian package: ${PKG_DIR}.deb"

