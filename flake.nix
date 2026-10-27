{
  description = "Kurogane";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    kurogane-nixpkgs.url = "github:0x48piraj/nixpkgs?ref=cef-binary-optional-angle-libs";

    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    { self, nixpkgs, ... }@inputs:

    # While we only support platforms defined in kurogane.meta.platforms,
    # we expose other systems for developing and for users who want to
    # have fun on exotic systems
    inputs.flake-utils.lib.eachSystem nixpkgs.lib.systems.flakeExposed (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        craneLib = inputs.crane.mkLib pkgs;
        kurogane-nixpkgs = import inputs.kurogane-nixpkgs { inherit system; };
        kurogane = pkgs.callPackage ./nix/package.nix { inherit kurogane-nixpkgs; };

        commonArgs = {
          src = craneLib.cleanCargoSource ./.;
          version = "dev";
          cargoExtraArgs = "-p kurogane-cli";

          inherit (kurogane)
            pname
            buildInputs
            nativeBuildInputs
            strictDeps
            USER
            CEF_PATH
            meta
            ;
        };

        kurogane-dev = craneLib.buildPackage (
          commonArgs
          // {
            cargoArtifacts = craneLib.buildDepsOnly commonArgs;
            inherit (kurogane) postInstall;
          }
        );
      in
      {
        packages = {
          default = kurogane-dev;
          inherit kurogane-dev kurogane;
          inherit (kurogane) cef-binary;
        };

        apps.default = {
          type = "app";
          program = "${kurogane}/bin/kurogane";
          inherit (kurogane) meta;
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ kurogane-dev ];
          packages = with pkgs; [
            rustc
            cargo
            clippy
            rustfmt
          ];

          inherit (kurogane) CEF_PATH;
        };

        checks = pkgs.lib.mergeAttrsList [
          {
            inherit kurogane-dev;
            bundle-fixture = pkgs.callPackage ./nix/bundleFixture.nix {
              inherit craneLib kurogane;
              src = ./.;
            };
          }
          kurogane.cef-binary.tests
        ];
      }
    );
}
