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
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath runtimeDeps}"

            export CEF_PATH="$HOME/.local/share/cef"
            export LD_LIBRARY_PATH="$LD_LIBRARY_PATH:$CEF_PATH"
            export PATH="$HOME/.cargo/bin:$PATH"

            # Use cargo install and call via full path or rely on PATH update
            alias setup-cef='cargo install --git https://github.com/tauri-apps/cef-rs export-cef-dir && "$HOME/.cargo/bin/export-cef-dir" --force "$CEF_PATH"'

            echo "Kurogane dev shell ready"
            echo "Run 'setup-cef' to download/install CEF binaries (if not already done)."
          '';

        };

      });
}
