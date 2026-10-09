{
  pkgs,
  craneLib,
  kurogane,
  src,
}:
let
  # Vendor fixture dependencies for offline builds
  vendored = craneLib.vendorCargoDeps { cargoLock = ./fixture/Cargo.lock; };
in
# Build the fixture offline with the packaged CLI and CEF
pkgs.stdenvNoCC.mkDerivation {
  name = "kurogane-bundle-fixture";
  inherit src;

  dontConfigure = true;

  buildPhase = ''
    runHook preBuild

    export HOME="$TMPDIR/home"
    export CARGO_HOME="$TMPDIR/cargo-home"
    mkdir -p "$HOME" "$CARGO_HOME"
    cp ${vendored}/config.toml "$CARGO_HOME/config.toml"

    cd nix/fixture

    # Test the wrapper without stdenv's build environment
    env -i HOME="$HOME" USER=nixbld TMPDIR="$TMPDIR" CARGO_HOME="$CARGO_HOME" \
      CARGO_NET_OFFLINE=true PATH="${pkgs.lib.makeBinPath [ pkgs.coreutils ]}" \
      ${kurogane}/bin/kurogane bundle --ci

    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall

    # Check CEF notices and record the bundle contents
    for notice in LICENSE.txt CREDITS.html; do
      if [ -z "$(find dist -name "$notice" -print -quit)" ]; then
        echo "the bundle lacks CEF's $notice" >&2
        exit 1
      fi
    done

    mkdir -p "$out"
    (cd dist && find . | sort) > "$out/files.txt"

    runHook postInstall
  '';
}
