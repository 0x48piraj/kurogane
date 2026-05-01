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
          rsync
          pkg-config
          cmake
          ninja
        ];
        kurogane-cli = pkgs.writeShellScriptBin "kurogane" ''
          set -e

          SRC=${./.}
          BUILD_DIR="$HOME/.kurogane-build"
          HASH_FILE="$BUILD_DIR/.src-hash"

          mkdir -p "$BUILD_DIR"

          # Hash Rust-relevant source files
          CURRENT_HASH=$(find "$SRC" \
            \( -name "*.rs" -o -name "Cargo.toml" -o -name "Cargo.lock" \) \
            -not -path "*/target/*" \
            | sort | xargs sha256sum 2>/dev/null | sha256sum | cut -d' ' -f1)

          STORED_HASH=$(cat "$HASH_FILE" 2>/dev/null || echo "none")

          if [ "$CURRENT_HASH" != "$STORED_HASH" ]; then
            echo "[kurogane] Source changed, syncing..."
            rsync -a --delete --chmod=Du+rwx,Fu+rw \
              --exclude="target/" \
              "$SRC/" "$BUILD_DIR/"

            echo "[kurogane] Building Kurogane CLI..."
            (cd "$BUILD_DIR" && cargo build -p kurogane-cli)

            echo "$CURRENT_HASH" > "$HASH_FILE"
          fi

          exec env LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath runtimeDeps}:$CEF_PATH" \
            "$BUILD_DIR/target/debug/kurogane" "$@"
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
