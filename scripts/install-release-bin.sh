#!/usr/bin/env bash
set -euo pipefail

# Corten Desktop — Precompiled Binary Release Installer
# Downloads the latest x86_64 GitHub Release tarball, verifies SHA256,
# installs binaries, icons, wallpapers, portals, systemd user units & PAM,
# and optionally enables Corten Greeter (greetd + cage).

REPO="Shura5860/corten-desktop"
FALLBACK_VERSION="v0.5.9"
ENABLE_GREETER=0

for arg in "$@"; do
    case "${arg}" in
        --enable-greeter) ENABLE_GREETER=1 ;;
    esac
done

if [[ "${EUID}" -ne 0 ]]; then
    echo "Lütfen root yetkisiyle çalıştırın: curl -fsSL https://raw.githubusercontent.com/${REPO}/main/scripts/install-release-bin.sh | sudo bash"
    exit 1
fi

echo "==> En son Corten Desktop sürümü kontrol ediliyor..."
VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1 || true)"
if [[ -z "${VERSION}" ]]; then
    VERSION="${FALLBACK_VERSION}"
fi
echo "==> Kurulacak sürüm: ${VERSION}"

PKG_NAME="corten-desktop-${VERSION}-x86_64-linux"
TARBALL_URL="https://github.com/${REPO}/releases/download/${VERSION}/${PKG_NAME}.tar.gz"
SHA_URL="${TARBALL_URL}.sha256"

TMP_DIR="$(mktemp -d /tmp/corten-install.XXXXXX)"
trap 'rm -rf "${TMP_DIR}"' EXIT

echo "==> İndiriliyor: ${TARBALL_URL}"
curl -fL --progress-bar "${TARBALL_URL}" -o "${TMP_DIR}/${PKG_NAME}.tar.gz"

if curl -fsSL "${SHA_URL}" -o "${TMP_DIR}/${PKG_NAME}.tar.gz.sha256" 2>/dev/null; then
    echo "==> SHA256 doğrulanıyor..."
    (cd "${TMP_DIR}" && sha256sum -c "${PKG_NAME}.tar.gz.sha256")
fi

echo "==> Arşiv çıkarılıyor..."
tar -xzf "${TMP_DIR}/${PKG_NAME}.tar.gz" -C "${TMP_DIR}"
ROOT="${TMP_DIR}/${PKG_NAME}"

echo "==> Binary dosyaları /usr/bin altına kuruluyor..."
install -dm755 /usr/bin
for bin in "${ROOT}/bin/"*; do
    if [[ -f "${bin}" ]]; then
        install -Dm755 "${bin}" "/usr/bin/$(basename "${bin}")"
    fi
done

if [[ -f "${ROOT}/scripts/corten-enable-greeter" ]]; then
    install -Dm755 "${ROOT}/scripts/corten-enable-greeter" /usr/bin/corten-enable-greeter
fi

echo "==> Özel Corten ikonları, duvar kağıtları ve .desktop kısayolları kuruluyor..."
if [[ -d "${ROOT}/assets/wallpapers" ]]; then
    install -dm755 /usr/share/backgrounds/corten
    cp -r "${ROOT}/assets/wallpapers/"* /usr/share/backgrounds/corten/
fi
if [[ -d "${ROOT}/assets/icons/hicolor" ]]; then
    install -dm755 /usr/share/icons/hicolor
    cp -r "${ROOT}/assets/icons/hicolor/"* /usr/share/icons/hicolor/
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -f -t /usr/share/icons/hicolor || true
    fi
fi
if [[ -d "${ROOT}/assets/applications" ]]; then
    install -dm755 /usr/share/applications
    for desk in "${ROOT}/assets/applications/"*.desktop; do
        if [[ -f "${desk}" ]]; then
            install -Dm644 "${desk}" "/usr/share/applications/$(basename "${desk}")"
        fi
    done
fi

echo "==> XDG Desktop Portal, systemd ve PAM yapılandırılıyor..."
if [[ -f "${ROOT}/corten-portal/corten.portal" ]]; then
    install -Dm644 "${ROOT}/corten-portal/corten.portal" \
        /usr/share/xdg-desktop-portal/portals/corten.portal
fi
if [[ -f "${ROOT}/corten-portal/corten-portals.conf" ]]; then
    install -Dm644 "${ROOT}/corten-portal/corten-portals.conf" \
        /usr/share/xdg-desktop-portal/corten-portals.conf
fi
if [[ -d "${ROOT}/systemd" ]]; then
    install -dm755 /usr/lib/systemd/user
    for unit in "${ROOT}/systemd/"*.service "${ROOT}/systemd/"*.target; do
        if [[ -f "${unit}" ]]; then
            install -Dm644 "${unit}" "/usr/lib/systemd/user/$(basename "${unit}")"
        fi
    done
fi

install -dm755 /usr/share/wayland-sessions
cat > /usr/share/wayland-sessions/corten.desktop << 'EOF'
[Desktop Entry]
Name=Corten Desktop
Comment=Pure-Rust Wayland Desktop Environment
Exec=/usr/bin/corten-session
Type=Application
DesktopNames=corten
EOF

install -dm755 /etc/pam.d
if [[ ! -f /etc/pam.d/corten-lock ]]; then
    cat > /etc/pam.d/corten-lock << 'EOF'
#%PAM-1.0
auth include system-auth
EOF
fi

if [[ "${ENABLE_GREETER}" -eq 1 && -x /usr/bin/corten-enable-greeter ]]; then
    echo "==> Corten Greeter (greetd) etkinleştiriliyor..."
    /usr/bin/corten-enable-greeter
fi

echo "✓ Corten Desktop ${VERSION} başarıyla kuruldu!"
echo "  Giriş ekranını etkinleştirmek için: sudo corten-enable-greeter"
echo "  Veya TTY'den başlatmak için: corten-session"
