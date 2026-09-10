{
  pkgs,
  kurogane,
}:
let
  owner = "kurogane-rs";
  rev = "master";

  templates = {
    starter-minimal = pkgs.fetchFromGitHub {
      inherit owner rev;
      repo = "kurogane-starter-minimal";
      hash = "sha256-50794DZCWXH+lqZGA96+h9TzwDrbF8DTNkLf1IKCVR4=";
    };

    starter-vue = pkgs.fetchFromGitHub {
      inherit owner rev;
      repo = "kurogane-starter-vue";
      hash = "sha256-Gg6YRYw7aT5avPrbraBOC2ewS552iTJWgqrKpN9QbTE=";
    };

    starter-svelte = pkgs.fetchFromGitHub {
      inherit owner rev;
      repo = "kurogane-starter-svelte";
      hash = "sha256-1xd2MoL33k8kOt1qN98vwf8WUqNWDAGDJhypNHWUsRg=";
    };

    starter-react = pkgs.fetchFromGitHub {
      inherit owner rev;
      repo = "kurogane-starter-react";
      hash = "sha256-N25nEretOGNjYfVl9beFIgr1pGD3s/EXgzav5N2OoKU=";
    };
  };

  templateHashes = {
    starter-minimal = "sha256-gwUR92YfK3l7lTo1qyc6ykHgnCn4eiinJkNeIg0vcew=";
    starter-vue = "sha256-gwUR92YfK3l7lTo1qyc6ykHgnCn4eiinJkNeIg0vcew=";
    starter-svelte = "sha256-gwUR92YfK3l7lTo1qyc6ykHgnCn4eiinJkNeIg0vcew=";
    starter-react = "sha256-gwUR92YfK3l7lTo1qyc6ykHgnCn4eiinJkNeIg0vcew=";
  };

  mkTest =
    tName: tHash:
    pkgs.stdenv.mkDerivation {
      name = "test-kurogane-${tName}";
      src = templates.${tName};

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
      outputHash = tHash;
    };


in
pkgs.lib.mapAttrs' (tName: tHash: {
  name = "test-${tName}";
  value = mkTest tName tHash;
}) templateHashes
