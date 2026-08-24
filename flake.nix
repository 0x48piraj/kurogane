{
  description = "Kurogane";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    { self, nixpkgs, ... }@inputs:
    let
      supportedSystems = [
        "x86_64-linux"
      ];

      cefVersion = "150.0.10";
    in
    inputs.flake-utils.lib.eachSystem supportedSystems ( system:
      let

      pkgs = import nixpkgs { inherit system; };
      craneLib = inputs.crane.mkLib pkgs;

      includeTemplates = path: _type: builtins.match ".*\/kurogane-cli\/templates.*" (toString path) != null;
      commonArgs = {
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = path: type: (includeTemplates path type) || (craneLib.filterCargoSources path type);
          name = "source";
        };

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

      cef = pkgs.callPackage ./nix/cef.nix { inherit pkgs cefVersion; };

      cargoArtifacts = craneLib.vendorCargoDeps (commonArgs // { pname = "kuroganeDeps"; });
      crateInfo = craneLib.crateNameFromCargoToml { cargoToml = ./Cargo.toml; };

      kurogane = craneLib.buildPackage (
        commonArgs
        // {
          inherit cargoArtifacts;

          pname = "kurogane";
          version = crateInfo.version;

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
      packages = {
        default = kurogane;
        kurogane = kurogane;
      };

      apps.default = {
        type = "app";
        program = "${kurogane}/bin/kurogane";
      };

      devShells.default = craneLib.devShell {
        packages = [ kurogane ];
      };
    }
  );
}
