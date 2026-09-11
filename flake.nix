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
        "aarch64-linux"
      ];

      cefVersion = "150.0.10";
    in
    inputs.flake-utils.lib.eachSystem supportedSystems (
      system:
      let

        pkgs = import nixpkgs { inherit system; };
        craneLib = inputs.crane.mkLib pkgs;

        commonArgs = {
          src = craneLib.cleanCargoSource ./.;

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

        cef = pkgs.callPackage ./nix/cef.nix { inherit cefVersion; };

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

            # Fallback when git.user and git.email aren't set
            env.USER = "Kurogane Tests";

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

            meta = {
              description = "Composable Chromium runtime for Rust";
              longDescription = ''
                Kurogane is a Rust-native runtime built on [Chromium Embedded Framework (CEF)](https://en.wikipedia.org/wiki/Chromium_Embedded_Framework), bringing Chromium to desktop applications while giving you control over windowing, event loops and lifecycle when you need it.
              '';
              homepage = "https://github.com/0x48piraj/kurogane";
              changelog = "https://github.com/0x48piraj/kurogane/releases";
              license = with pkgs.lib.licenses; [ mit ]; # TODO: add licenses from all packages used
              sourceProvenance =
                with pkgs.lib.sourceTypes;
                [ fromSource ] ++ pkgs.cef-binary.meta.sourceProvenance; # cef is binaryNativeCode
              # maintainers = with pkgs.lib.maintainers; [ _0x48piraj R0M-A ]; # TODO: get on maintainers list
              platforms = supportedSystems;
              mainProgram = "kurogane";
            };
          }
        );

        testTemplates = import ./nix/testTemplates.nix { inherit pkgs kurogane; };

      in
      {
        packages = {
          default = kurogane;
          kurogane = kurogane;
        };

        apps.default = {
          type = "app";
          program = "${kurogane}/bin/kurogane";
          meta = kurogane.meta;
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ kurogane ];
          packages = with pkgs; [ rustc cargo clippy rustfmt ];

          # Reuse the Nix store CEF instead of letting cef-dll-sys re-download
          env.CEF_PATH = "${cef}";
          env.KUROGANE_CEF_VERSION = cefVersion;
        };

        checks = pkgs.lib.mergeAttrsList [
          testTemplates
        ];
      }
    );
}
