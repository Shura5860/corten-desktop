Name:           corten-desktop
Version:        0.1.0
Release:        1%{?dist}
Summary:        Pure Rust, Modular, Modern Wayland Desktop Environment with Native Local AI

License:        GPL-3.0-only
URL:            https://github.com/Shura5860/corten-desktop

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  pkgconfig(wayland-server)
BuildRequires:  pkgconfig(wayland-client)
BuildRequires:  pkgconfig(xkbcommon)
BuildRequires:  pkgconfig(libseat)
BuildRequires:  pkgconfig(systemd)

Requires:       wayland
Requires:       pipewire
Requires:       wireplumber
Requires:       NetworkManager
Requires:       udisks2
Requires:       bluez
Requires:       brightnessctl
Requires:       wl-clipboard
Requires:       cliphist

%description
Corten Desktop is a high-performance, modular, zero-bloat Wayland desktop environment
written in pure Rust. It features an integrated Wayland compositor (Smithay),
status panel, spotlight launcher, notification daemon, local AI assistant HUD,
system settings hub, and native file manager.

%prep
%autosetup -n %{name}-%{version}

%build
cargo build --release --offline

%check
cargo test --workspace --offline

%install
rm -rf %{buildroot}
install -d %{buildroot}%{_bindir}
install -m 755 target/release/corten-compositor %{buildroot}%{_bindir}/corten-compositor
install -m 755 target/release/corten-session %{buildroot}%{_bindir}/corten-session
install -m 755 target/release/corten-panel %{buildroot}%{_bindir}/corten-panel
install -m 755 target/release/corten-launcher %{buildroot}%{_bindir}/corten-launcher
install -m 755 target/release/corten-notify %{buildroot}%{_bindir}/corten-notify
install -m 755 target/release/corten-settings %{buildroot}%{_bindir}/corten-settings
install -m 755 target/release/corten-lock %{buildroot}%{_bindir}/corten-lock
install -m 755 target/release/corten-greeter %{buildroot}%{_bindir}/corten-greeter
install -m 755 target/release/corten-widgets %{buildroot}%{_bindir}/corten-widgets
install -m 755 target/release/corten-files %{buildroot}%{_bindir}/corten-files
install -m 755 target/release/corten-monitor %{buildroot}%{_bindir}/corten-monitor
install -m 755 target/release/corten-snipper %{buildroot}%{_bindir}/corten-snipper
install -m 755 target/release/corten-wallpaper %{buildroot}%{_bindir}/corten-wallpaper
install -m 755 target/release/corten-polkit %{buildroot}%{_bindir}/corten-polkit
install -m 755 target/release/corten-portal %{buildroot}%{_bindir}/corten-portal
install -m 755 target/release/corten-ai %{buildroot}%{_bindir}/corten-ai

# Wayland Session file
install -d %{buildroot}%{_datadir}/wayland-sessions
cat << 'EOF' > %{buildroot}%{_datadir}/wayland-sessions/corten.desktop
[Desktop Entry]
Name=Corten Desktop
Comment=Pure Rust Modular Wayland Desktop Environment
Exec=corten-session
Type=Application
DesktopNames=Corten
Keywords=wayland;compositor;rust;desktop;
EOF

# Desktop files
install -d %{buildroot}%{_datadir}/applications
cat << 'EOF' > %{buildroot}%{_datadir}/applications/corten-settings.desktop
[Desktop Entry]
Name=Corten Settings
Comment=System and Desktop configuration for Corten
Exec=corten-settings
Icon=preferences-system
Terminal=false
Type=Application
Categories=Settings;DesktopSettings;
EOF

cat << 'EOF' > %{buildroot}%{_datadir}/applications/corten-files.desktop
[Desktop Entry]
Name=Corten Files
Comment=Modern Pure Rust File Manager
Exec=corten-files
Icon=system-file-manager
Terminal=false
Type=Application
Categories=System;FileManager;Utility;
EOF

cat << 'EOF' > %{buildroot}%{_datadir}/applications/corten-monitor.desktop
[Desktop Entry]
Name=Corten Monitor
Comment=Hardware, Network and Process Monitor
Exec=corten-monitor
Icon=utilities-system-monitor
Terminal=false
Type=Application
Categories=System;Monitor;
EOF

cat << 'EOF' > %{buildroot}%{_datadir}/applications/corten-ai.desktop
[Desktop Entry]
Name=Corten AI
Comment=Local AI Assistant HUD
Exec=corten-ai
Icon=help-browser
Terminal=false
Type=Application
Categories=Utility;
EOF

%files
%{_bindir}/corten-*
%{_datadir}/wayland-sessions/corten.desktop
%{_datadir}/applications/corten-*.desktop

%changelog
* Wed Nov 04 2026 Corten Desktop Contributors <corten@corten-desktop.org> - 0.1.0-1
- Initial production packaging release of Corten Desktop Environment

