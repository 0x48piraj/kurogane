{
  pkgs,
  kurogane,
}:
let
  owner = "kurogane-rs";

  # Each template is pinned to the commit used to compute its hashes
  templates = {
    starter-minimal = {
      rev = "2beecfd19c69e9b0e74944a96b2d9c8c444536d1";
      srcHash = "sha256-50794DZCWXH+lqZGA96+h9TzwDrbF8DTNkLf1IKCVR4=";
      outputHash = "sha256-gwUR92YfK3l7lTo1qyc6ykHgnCn4eiinJkNeIg0vcew=";
    };

    starter-vue = {
      rev = "199a6e0ec7847266f127f8ed4fbef9c0aa0e84bd";
      srcHash = "sha256-Gg6YRYw7aT5avPrbraBOC2ewS552iTJWgqrKpN9QbTE=";
      outputHash = "sha256-gwUR92YfK3l7lTo1qyc6ykHgnCn4eiinJkNeIg0vcew=";
    };

    starter-svelte = {
      rev = "866bd9232f03ee414810fa1eb40c4368e7c024f8";
      srcHash = "sha256-1xd2MoL33k8kOt1qN98vwf8WUqNWDAGDJhypNHWUsRg=";
      outputHash = "sha256-gwUR92YfK3l7lTo1qyc6ykHgnCn4eiinJkNeIg0vcew=";
    };

    starter-react = {
      rev = "f2883e5f1d434baacb82f7f68f063768d42bed2b";
      srcHash = "sha256-N25nEretOGNjYfVl9beFIgr1pGD3s/EXgzav5N2OoKU=";
      outputHash = "sha256-gwUR92YfK3l7lTo1qyc6ykHgnCn4eiinJkNeIg0vcew=";
    };
  };

  mkTest =
    tName: tCfg:
    pkgs.stdenv.mkDerivation {
      name = "test-kurogane-${tName}";
      src = pkgs.fetchFromGitHub {
        inherit owner;
        inherit (tCfg) rev;
        repo = "kurogane-${tName}";
        hash = tCfg.srcHash;
      };

      nativeBuildInputs = with pkgs; [
        kurogane
        cargo # cargo generate (internal to kurogane build)
        bun # bun ci
        stdenv.cc # linker
        cacert # avoid SSL error
      ];

      env.USER = "Kurogane Tests"; # Fallback when git.user and git.email aren't set

      configurePhase = ''
        runHook preConfigure

        workdir="$TMPDIR/${tName}"
        mkdir -p "$workdir/template"
        cp -R --no-preserve=mode,ownership "$src"/. "$workdir/template"
        cd "$workdir"

        kurogane new --template "$workdir/template" --name "ts" --language typescript --yes --ci
        kurogane new --template "$workdir/template" --name "js" --language javascript --yes --ci

        cd "$workdir/ts/frontend"
        bun ci
        cd "$workdir/js/frontend"
        bun ci

        export CARGO_HOME="$TMPDIR/cargo-home"
        mkdir -p "$CARGO_HOME"

        runHook postConfigure
      '';

      buildPhase = ''
        runHook preBuild

        cd "$workdir/ts"
        kurogane build --ci
        cd "$workdir/js"
        kurogane build --ci

        runHook postBuild
      '';

      installPhase = ''
        runHook preInstall

        mkdir -p "$out"
        touch $out/pass

        runHook postInstall
      '';
#         cp -R "$workdir/ts" "$out/ts"
#         cp -R "$workdir/js" "$out/js"

      outputHashMode = "recursive";
      outputHashAlgo = "sha256";
      outputHash = tCfg.outputHash;
    };


in
pkgs.lib.mapAttrs' (tName: tCfg: {
  name = "test-${tName}";
  value = mkTest tName tCfg;
}) templates
