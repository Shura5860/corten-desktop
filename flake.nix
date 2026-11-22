{
  description = "Corten Desktop — Pure-Rust Wayland Desktop Environment powered by Smithay, Iced & AURA-CORE";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "corten-desktop";
            version = "0.5.8";
            src = ./.;

            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            nativeBuildInputs = with pkgs; [
              pkg-config
              clang
              llvmPackages.libclang
            ];

            buildInputs = with pkgs; [
              wayland
              libxkbcommon
              libinput
              mesa
              libdrm
              libgbm
              libseat
              udev
              linux-pam
              pipewire
              dbus
            ];

            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
            doCheck = false;

            postInstall = ''
              install -Dm755 scripts/corten-terminal \
                $out/bin/corten-terminal
              install -Dm644 corten-session/corten.desktop \
                $out/share/wayland-sessions/corten.desktop
              install -Dm644 corten-portal/corten.portal \
                $out/share/xdg-desktop-portal/portals/corten.portal
              install -Dm644 corten-portal/corten-portals.conf \
                $out/share/xdg-desktop-portal/corten-portals.conf
            '';

            meta = with pkgs.lib; {
              description = "Pure-Rust Wayland desktop environment with dynamic tiling, floating, scrollable strip & local AI";
              homepage = "https://github.com/Shura5860/corten-desktop";
              license = with licenses; [ gpl3Plus mit ];
              platforms = platforms.linux;
              mainProgram = "corten-session";
            };
          };
        });

      devShells = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.default ];
            packages = with pkgs; [
              cargo
              rustc
              rustfmt
              clippy
              wl-clipboard
              grim
            ];
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          };
        });

      nixosModules.corten-desktop = { config, lib, pkgs, ... }:
        let
          cfg = config.services.desktopManager.corten;
        in
        {
          options.services.desktopManager.corten.enable = lib.mkEnableOption "Corten Wayland Desktop Environment";

          config = lib.mkIf cfg.enable {
            environment.systemPackages = [
              self.packages.${pkgs.stdenv.hostPlatform.system}.default
              pkgs.wl-clipboard
              pkgs.grim
            ];
            services.displayManager.sessionPackages = [
              self.packages.${pkgs.stdenv.hostPlatform.system}.default
            ];
            security.polkit.enable = true;
            services.pipewire.enable = lib.mkDefault true;
          };
        };
    };
}

