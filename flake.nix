{
  description = "Kurogane";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    { self, nixpkgs, ... }@inputs:
    let
      cefVersion = "150.0.10";
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      craneLib = inputs.crane.mkLib pkgs;

      commonArgs = {
        src = ./.; #TODO: should be src = craneLib.cleanCargoSource ./.; but doesn't include templates/
        strictDeps = true;

        buildInputs = with pkgs; [
          openssl
          dbus
          at-spi2-core
          glib
          libGL
          libxkbcommon
          wayland
          libX11
          libXcomposite
          libXcursor
          libXdamage
          libXext
          libXfixes
          libXi
          libXrandr
          libXrender
          libXScrnSaver
          libXtst
          libxcb
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

        nativeBuildInputs = with pkgs; [
          rustc
          cargo
          pkg-config
          cmake
          ninja
        ];
      };

      cefIntermediate = pkgs.cef-binary.override {
        version = "150.0.10";
        gitRevision = "8042e43";
        chromiumVersion = "150.0.7871.101";
        srcHashes = {
          aarch64-linux = "";
          x86_64-linux = "sha256-bB1Ike84huPM9l0JKI2DBOP343JKR8kyk+K9Y+dlKOQ=";
        };
      };

      cef = cefIntermediate.overrideAttrs (old: {
        postInstall = (old.postInstall or "") + ''
          cat > "$out/archive.json" <<EOF
          {
            "type": "minimal",
            "name": "cef_binary_${cefVersion}",
            "sha1": "0000000000000000000000000000000000000000"
          }
          EOF

          ln -sf $out/Release/* $out/
          ln -sf $out/Resources/* $out/
        '';
      });

      cargoArtifacts = craneLib.vendorCargoDeps (commonArgs // { pname = "kuroganeDeps"; });

      kurogane = craneLib.buildPackage (
        commonArgs
        // {
          inherit cargoArtifacts;

          pname = "kurogane";
          version = "0.0.4";

          cargoExtraArgs = "-p kurogane-cli";

          nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.makeWrapper ];

          # TODO: Avoid envvars
          postInstall = ''
            wrapProgram $out/bin/kurogane \
              --set KUROGANE_CEF_VERSION ${cefVersion} \
              --set CEF_PATH ${cef} \
              --prefix PATH : ${pkgs.lib.makeBinPath commonArgs.nativeBuildInputs} \
              --prefix LD_LIBRARY_PATH : ${
                pkgs.lib.makeLibraryPath (commonArgs.buildInputs ++ [ pkgs.stdenv.cc.cc ])
              }:${cef} \
              --prefix PKG_CONFIG_PATH : ${pkgs.lib.makeSearchPath "lib/pkgconfig" commonArgs.buildInputs}
          '';
        }
      );
    in
    {
      packages.${system}.default = kurogane;

      apps.${system}.default = {
        type = "app";
        program = "${kurogane}/bin/kurogane";
      };

      devShells.${system}.default = craneLib.devShell {
        packages = [ kurogane ];
      };
    };
}
