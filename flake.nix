{
  description = "Kurogane";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        runtimeDeps = with pkgs; [
          openssl
          dbus
          at-spi2-core
          glib
          libGL
          libxkbcommon
          wayland
          xorg.libX11
          xorg.libXcomposite
          xorg.libXcursor
          xorg.libXdamage
          xorg.libXext
          xorg.libXfixes
          xorg.libXi
          xorg.libXrandr
          xorg.libXrender
          xorg.libXScrnSaver
          xorg.libXtst
          xorg.libxcb
          gtk3
          nss
          nspr
          pango
          cairo
          alsa-lib
          at-spi2-atk
          atk
          cups
          expat
          fontconfig
          gdk-pixbuf
          libva
          libgbm
          libvdpau
          systemd
        ];

        buildDeps = with pkgs; [ rustc cargo pkg-config cmake ninja ];
      in {
        devShells.default = pkgs.mkShell {
          buildInputs = buildDeps ++ runtimeDeps;

          shellHook = ''
            export PKG_CONFIG_PATH="${
              pkgs.lib.makeSearchPath "lib/pkgconfig" runtimeDeps
            }"

            export CEF_PATH="$HOME/.local/share/cef"
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath runtimeDeps}:$CEF_PATH:$LD_LIBRARY_PATH"
            export PATH="$HOME/.cargo/bin:$PATH"

            alias kurogane-setup='
              echo "Installing kurogane-cli...";
              cargo install --path ./kurogane-cli --force;
              echo "";
              echo "Downloading CEF binaries...";
              cargo install --git https://github.com/tauri-apps/cef-rs export-cef-dir && "$HOME/.cargo/bin/export-cef-dir" --force "$CEF_PATH";
              echo "";
              echo "Setup complete!"
              echo "IMPORTANT: Do NOT run this inside the Kurogane repository."
              echo "Create a new project directory and run:"
              echo "  kurogane init"
            '

            echo ""
            echo "First time setup:"
            echo "  1. Run: kurogane-setup"
            echo "     (This installs CLI and downloads CEF)"
            echo ""
            echo "Regular development:"
            echo "  - kurogane init   - Create new project"
            echo "  - kurogane dev    - Run the project"
            echo "  - kurogane build  - Build for production"
            echo ""
            echo "CEF will be installed to: $CEF_PATH"
            echo ""
          '';

        };

      });
}
