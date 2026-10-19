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
        "aarch64-darwin"
      ];

      cefVersion = "154.0.33";
    in
    inputs.flake-utils.lib.eachSystem supportedSystems (
      system:
      let

        pkgs = import nixpkgs { inherit system; };
        craneLib = inputs.crane.mkLib pkgs;

        # nixpkgs packages glxinfo inside mesa-demos beside its demo programs.
        # A copy of the one binary keeps mesa-demos out of the closure.
        glxinfo = pkgs.runCommand "glxinfo" { } ''
          install -Dm755 ${pkgs.mesa-demos}/bin/glxinfo $out/bin/glxinfo
        '';

        commonArgs = {
          src = craneLib.cleanCargoSource ./.;

          strictDeps = true;

          buildInputs =
            with pkgs;
            [
              openssl
              nss
              nspr
              pango
              cairo
              glib
              expat
              fontconfig
            ]
            ++ lib.optionals stdenv.hostPlatform.isLinux [
              dbus
              at-spi2-core
              libGL
              vulkan-loader
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
              alsa-lib
              at-spi2-atk
              atk
              cups
              gdk-pixbuf
              libva
              libgbm
              libvdpau
              systemd
            ]
            # macOS Rust binaries link libiconv which apple-sdk omits
            ++ lib.optionals stdenv.hostPlatform.isDarwin [
              libiconv
            ];

          nativeBuildInputs =
            with pkgs;
            [
              rustc
              cargo
              pkg-config
              stdenv.cc
            ]
            # kurogane doctor reads the GPU through glxinfo
            ++ lib.optionals stdenv.hostPlatform.isLinux [
              glxinfo
            ]
            # macOS bundles link CEF with otool and install_name_tool
            ++ lib.optionals stdenv.hostPlatform.isDarwin [
              cctools
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

            # Cargo runs outside stdenv; specifically provide its build dependencies
            # On macOS rustc otherwise targets an older macOS than Nix's Rust standard library
            postInstall = ''
              wrapProgram $out/bin/kurogane \
                --prefix LIBRARY_PATH : ${pkgs.lib.makeLibraryPath commonArgs.buildInputs} \
                --set CEF_PATH ${cef} \
                --prefix PATH : ${pkgs.lib.makeBinPath commonArgs.nativeBuildInputs} \
                --prefix LD_LIBRARY_PATH : ${
                  pkgs.lib.makeLibraryPath (commonArgs.buildInputs ++ [ pkgs.stdenv.cc.cc ])
                } \
                --prefix PKG_CONFIG_PATH : ${pkgs.lib.makeSearchPath "lib/pkgconfig" commonArgs.buildInputs} \
                ${pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isDarwin "--set-default MACOSX_DEPLOYMENT_TARGET ${pkgs.stdenv.hostPlatform.darwinMinVersion}"}
            '';

            meta = {
              description = "Composable Chromium runtime for Rust";
              longDescription = ''
                Kurogane is a Rust-native runtime built on [Chromium Embedded Framework (CEF)](https://en.wikipedia.org/wiki/Chromium_Embedded_Framework), bringing Chromium to desktop applications while giving you control over windowing, event loops and lifecycle when you need it.
              '';
              homepage = "https://github.com/0x48piraj/kurogane";
              changelog = "https://github.com/0x48piraj/kurogane/releases";
              # Mirrors the allow-list in cargo-deny
              license = with pkgs.lib.licenses; [
                asl20
                llvm-exception
                bsd2
                bsd3
                isc
                mit
                zlib
                mpl20
                cc0
                ofl
                unicode-30
                bzip2
                {
                  free = true;
                  shortName = "LicenseRef-UFL-1.0";
                }
                {
                  free = true;
                  shortName = "CDLA-Permissive-2.0";
                }
              ];
              sourceProvenance =
                with pkgs.lib.sourceTypes;
                [ fromSource ] ++ pkgs.cef-binary.meta.sourceProvenance; # cef is binaryNativeCode
              # maintainers = with pkgs.lib.maintainers; [ _0x48piraj R0M-A ]; # TODO: get on maintainers list
              platforms = supportedSystems;
              mainProgram = "kurogane";
            };
          }
        );

        bundleFixture = import ./nix/bundleFixture.nix {
          inherit pkgs craneLib kurogane;
          src = ./.;
        };

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
          packages = with pkgs; [
            rustc
            cargo
            clippy
            rustfmt
          ];

          # Applications started from the shell load the Nix store's CEF
          env.CEF_PATH = "${cef}";
        };

        checks = pkgs.lib.mergeAttrsList [
          {
            inherit kurogane;
            bundle-fixture = bundleFixture;
          }
          cef.tests
        ];
      }
    );
}
