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

        buildDeps = with pkgs; [
          rustc
          cargo
          pkg-config
          cmake
          ninja
        ];
        kurogane-cli = pkgs.writeShellScriptBin "kurogane" ''
          set -e

          SRC=${./.}
          BUILD_DIR="$HOME/.kurogane-build"

          mkdir -p "$BUILD_DIR"

          # Sync source
          if [ ! -f "$BUILD_DIR/Cargo.toml" ]; then
            echo "[kurogane] Preparing source..."
            cp -r "$SRC"/* "$BUILD_DIR/"
          fi

          # Build CLI
          if [ ! -f "$BUILD_DIR/target/debug/kurogane" ]; then
            echo "[kurogane] Building Kurogane CLI..."
            (cd "$BUILD_DIR" && cargo build -p kurogane-cli)
          fi

          # Run CLI
          exec "$BUILD_DIR/target/debug/kurogane" "$@"
        '';
      in {
        devShells.default = pkgs.mkShell {
          buildInputs =
            buildDeps
            ++ runtimeDeps
            ++ [ kurogane-cli ];

          shellHook = ''
            export CEF_PATH="$HOME/.local/share/cef"

            export PKG_CONFIG_PATH="${
              pkgs.lib.makeSearchPath "lib/pkgconfig" runtimeDeps
            }"
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath runtimeDeps}:$CEF_PATH:$LD_LIBRARY_PATH"

            echo "Kurogane Dev Shell"
            echo "    kurogane init   - Create new project"
            echo "    kurogane dev    - Run the project"
            echo "    kurogane bundle - Package for production"
            echo ""
          '';
        };
      }
    );
}
