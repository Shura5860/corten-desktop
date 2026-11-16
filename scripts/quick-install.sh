#!/usr/bin/env bash
# ==============================================================================
# Corten Desktop Environment - Zero-Friction Automated Installer
# Pure Rust Wayland Desktop Environment with Native Local AI
# Usage: curl -fsSL https://raw.githubusercontent.com/Shura5860/corten-desktop/main/scripts/quick-install.sh | bash
# ==============================================================================

set -euo pipefail

CORTEN_VERSION="${CORTEN_VERSION:-latest}"
REPO="Shura5860/corten-desktop"
INSTALL_PREFIX="${INSTALL_PREFIX:-/usr/local}"
BIN_DIR="${INSTALL_PREFIX}/bin"
SHARE_DIR="${INSTALL_PREFIX}/share"

RED='\033[0;31m'
GREEN='\033[0;32m'
AMBER='\033[38;2;225;110;45m'
BOLD='\033[1m'
NC='\033[0m'

echo -e "${AMBER}${BOLD}"
cat << "EOF"
╔═══════════════════════════════════════════════════════════════════════╗
║        CORTEN DESKTOP ENVIRONMENT — AUTOMATED ZERO-FRICTION INSTALLER ║
║        Pure Rust Modular Wayland Desktop & Native Local AI            ║
╚═══════════════════════════════════════════════════════════════════════╝
EOF
echo -e "${NC}"

# 1. Detect Distribution
echo -e "${BOLD}==> [1/6] Dağıtım ve Paket Yöneticisi Algılanıyor...${NC}"
DISTRO="unknown"
if [[ -f /etc/os-release ]]; then
    # shellcheck disable=SC1091
    source /etc/os-release
    DISTRO="${ID:-unknown}"
fi

echo -e "    Algılanan Dağıtım: ${BOLD}${DISTRO}${NC}"

# 2. Install Runtime Dependencies
echo -e "${BOLD}==> [2/6] Gerekli Sistem Bağımlılıkları Kontrol Ediliyor...${NC}"

install_deps_arch() {
    sudo pacman -S --needed --noconfirm \
        wayland libxkbcommon libseat pipewire wireplumber \
        networkmanager udisks2 bluez brightnessctl wl-clipboard \
        cliphist xdg-utils mesa polkit foot alacritty
}

install_deps_fedora() {
    sudo dnf install -y \
        wayland-devel libxkbcommon-devel libseat-devel pipewire wireplumber \
        NetworkManager udisks2 bluez brightnessctl wl-clipboard \
        xdg-utils mesa-dri-drivers polkit foot alacritty
}

install_deps_debian() {
    sudo apt-get update -y
    sudo apt-get install -y \
        libwayland-client0 libwayland-cursor0 libwayland-egl1 \
        libxkbcommon0 libseat1 pipewire wireplumber network-manager \
        udisks2 bluez brightnessctl wl-clipboard xdg-utils \
        mesa-va-drivers polkitd foot || true
}

case "${DISTRO}" in
    arch|manjaro|endeavouros|garuda|cachyos)
        echo "    Arch Linux tabanlı paketler kuruluyor..."
        install_deps_arch
        ;;
    fedora|rhel|centos)
        echo "    Fedora tabanlı paketler kuruluyor..."
        install_deps_fedora
        ;;
    ubuntu|debian|pop|mint|elementary)
        echo "    Debian/Ubuntu tabanlı paketler kuruluyor..."
        install_deps_debian
        ;;
    *)
        echo -e "${AMBER}    Uyarı: Desteklenmeyen veya özel dağıtım (${DISTRO}). Bağımlılıkları el ile kurmanız gerekebilir.${NC}"
        ;;
esac

# 3. Download Latest Release Package
TMP_DIR=$(mktemp -d /tmp/corten_install_XXXXXX)
trap 'rm -rf "${TMP_DIR}"' EXIT

echo -e "${BOLD}==> [3/6] GitHub Releases Üzerinden En Son Sürüm İndiriliyor...${NC}"

RELEASE_API="https://api.github.com/repos/${REPO}/releases/latest"
TARBALL_URL=""

if command -v curl >/dev/null 2>&1; then
    RELEASE_JSON=$(curl -s -H "User-Agent: CortenInstaller" "${RELEASE_API}" || true)
    if [[ -n "${RELEASE_JSON}" ]]; then
        TARBALL_URL=$(echo "${RELEASE_JSON}" | grep -o 'https://[^"]*corten-desktop-[^"]*-x86_64-linux\.tar\.gz' | head -n 1 || true)
    fi
fi

if [[ -z "${TARBALL_URL}" ]]; then
    echo -e "    GitHub Releases paketi bulunamadı, yerel depodan paketleniyor veya kaynak kod fallback..."
    # Fallback: if script run inside repository, install directly
    if [[ -f "Cargo.toml" && -d "corten-compositor" ]]; then
        echo "    Yerel depo algılandı, yerel kurulum betiği çalıştırılıyor..."
        ./install.sh
        exit 0
    else
        TARBALL_URL="https://github.com/${REPO}/archive/refs/heads/main.tar.gz"
    fi
fi

echo -e "    İndiriliyor: ${TARBALL_URL}"
curl -L --progress-bar "${TARBALL_URL}" -o "${TMP_DIR}/corten.tar.gz"

# 4. Extract and Deploy Binaries
echo -e "${BOLD}==> [4/6] İkili Dosyalar Kuruluyor -> ${BIN_DIR}...${NC}"
tar -xzf "${TMP_DIR}/corten.tar.gz" -C "${TMP_DIR}"

EXTRACTED_BIN_DIR=""
for d in "${TMP_DIR}"/*; do
    if [[ -d "${d}/bin" ]]; then
        EXTRACTED_BIN_DIR="${d}/bin"
        break
    fi
done

sudo install -d "${BIN_DIR}"

if [[ -n "${EXTRACTED_BIN_DIR}" ]]; then
    for bin in "${EXTRACTED_BIN_DIR}"/corten*; do
        if [[ -f "${bin}" ]]; then
            sudo install -m 755 "${bin}" "${BIN_DIR}/"
        fi
    done
fi

# 5. Register Wayland Session (GDM, SDDM, LightDM, Ly)
echo -e "${BOLD}==> [5/6] Wayland Oturum Kaydı Oluşturuluyor...${NC}"
sudo install -d /usr/share/wayland-sessions
sudo tee /usr/share/wayland-sessions/corten.desktop > /dev/null << 'EOF'
[Desktop Entry]
Name=Corten Desktop
Comment=Pure Rust Wayland Desktop Environment with Native Local AI
Exec=/usr/local/bin/corten-session
Type=Application
DesktopNames=Corten;corten;
Keywords=wayland;desktop;compositor;rust;
EOF

# 6. Run System Doctor & Verification
echo -e "${BOLD}==> [6/6] Corten Sistem Hekimi Doğrulaması Çalıştırılıyor...${NC}"
if command -v "${BIN_DIR}/cortenctl" >/dev/null 2>&1; then
    "${BIN_DIR}/cortenctl" doctor || true
else
    cargo run --bin cortenctl -- doctor || true
fi

echo -e "\n${GREEN}${BOLD}✓ CORTEN DESKTOP KURULUMU BAŞARIYLA TAMAMLANDI!${NC}"
echo -e "Bilgisayarınızı yeniden başlattığınızda giriş ekranında (GDM / SDDM / LightDM) ${BOLD}Corten Desktop${NC}'ı seçebilirsiniz."
echo -e "Masaüstünü hemen TTY üzerinden başlatmak için:"
echo -e "  ${AMBER}${BOLD}corten-session${NC}\n"

