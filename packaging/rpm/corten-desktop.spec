Name:           corten-desktop
Version:        0.5.8
Release:        1%{?dist}
Summary:        Pure-Rust Wayland desktop environment powered by Smithay, Iced & AURA-CORE
License:        GPL-3.0-or-later AND MIT
URL:            https://github.com/Shura5860/corten-desktop
Source0:        %{url}/archive/refs/tags/v%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  clang
BuildRequires:  pkgconfig(wayland-server)
BuildRequires:  pkgconfig(xkbcommon)
BuildRequires:  pkgconfig(libinput)
BuildRequires:  pkgconfig(libdrm)
BuildRequires:  pkgconfig(gbm)
BuildRequires:  pkgconfig(egl)
BuildRequires:  pkgconfig(libseat)
BuildRequires:  pkgconfig(libudev)
BuildRequires:  pkgconfig(libpipewire-0.3)
BuildRequires:  pam-devel

Requires:       wayland
Requires:       libxkbcommon
Requires:       libinput
Requires:       mesa-dri-drivers
Requires:       libseat
Requires:       pam
Requires:       pipewire
Requires:       wireplumber
Requires:       polkit
Requires:       xdg-desktop-portal
Requires:       wl-clipboard
Requires:       grim
Recommends:     xorg-x11-server-Xwayland
Recommends:     NetworkManager
Recommends:     bluez
Recommends:     power-profiles-daemon

%description
Corten Desktop is a fast, memory-safe Wayland desktop environment written
in 100% Rust using Smithay and Iced. Includes Floating, Master-Stack,
Dwindle, BSP, Monocle, and Niri-style Scrollable Tiling modes, full XDG
Desktop Portal integration, and optional local AURA-CORE AI assistant.

%prep
%autosetup -n corten-desktop-%{version}

%build
cargo build --release --locked

%install
install -dm755 %{buildroot}%{_bindir}
for bin in corten-compositor corten-session corten-panel corten-launcher corten-settings \
           corten-files corten-notify corten-lock corten-greeter corten-monitor \
           corten-snipper corten-widgets corten-wallpaper corten-portal corten-polkit \
           corten-ai corten-welcome corten-forge corten-update cortenctl; do
    install -Dm755 target/release/${bin} %{buildroot}%{_bindir}/${bin}
done
install -Dm755 scripts/corten-terminal %{buildroot}%{_bindir}/corten-terminal

install -Dm644 corten-session/corten.desktop \
    %{buildroot}%{_datadir}/wayland-sessions/corten.desktop
install -Dm644 corten-portal/corten.portal \
    %{buildroot}%{_datadir}/xdg-desktop-portal/portals/corten.portal
install -Dm644 corten-portal/corten-portals.conf \
    %{buildroot}%{_datadir}/xdg-desktop-portal/corten-portals.conf

%files
%license LICENSE
%{_bindir}/corten-*
%{_bindir}/cortenctl
%{_datadir}/wayland-sessions/corten.desktop
%{_datadir}/xdg-desktop-portal/portals/corten.portal
%{_datadir}/xdg-desktop-portal/corten-portals.conf

%changelog
* Thu Nov 19 2026 Corten Desktop Contributors - 0.5.0-1
- Corten Desktop v0.5.0 release with 39 community-requested features across 6 phases

