{
  pkgs,
  kurogane,
}:
let
  lib = pkgs.lib;

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
      hash = "sha256-pQpattmS9VmO3ZIQUFn66az8GSmB4IvYhTTCFn6SUmo=";
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

#   kuroganeSrc = pkgs.fetchFromGitHub {
#     owner = "0x48piraj";
#     repo = "kurogane";
#     rev = "master";
#     hash = "sha256-/ugr7CvQvHinwX9uQL51kdbwdEOqAfKVlFXVQtkVqpc=";
#   };

  templateHashes = {
    starter-minimal = "sha256-50794DZCWXH+lqZGA96+h9TzwDrbF8DTNkLf1IKCVR4=";
    starter-vue = "sha256-Gg6YRYw7aT5avPrbraBOC2ewS552iTJWgqrKpN9QbTE=";
    starter-svelte = "sha256-pQpattmS9VmO3ZIQUFn66az8GSmB4IvYhTTCFn6SUmo=";
    starter-react =  "sha256-N25nEretOGNjYfVl9beFIgr1pGD3s/EXgzav5N2OoKU=";
  };

  mkTest =
  tName: tHash:
  pkgs.runCommand "test-kurogane-${tName}"
    {
      src = templates.${tName};

      nativeBuildInputs = [
        kurogane
        pkgs.bun
        pkgs.cargo
        pkgs.rustc
        pkgs.git
        pkgs.stdenv.cc
      ];

      env.USER = "Kurogane Tests"; # Fallback when git.user and git.email aren't set
#       env.GIT_SSL_CAINFO = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
      env.CARGO_HTTP_CAINFO = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
      outputHashMode = "recursive";
      outputHashAlgo = "sha256";
      outputHash = tHash;
    }
    ''
      set -euo pipefail
      mkdir -p "$out"
      workdir="$TMPDIR/${tName}"
      mkdir -p "$workdir/template"
      cp -R --no-preserve=mode,ownership "$src"/. "$workdir/template"
      export NIX_SSL_CERT_FILE=$NIX_SSL_CERT_FILE

      echo "Fetched source:"
    find "$src" -maxdepth 2 -print | sort

    echo "Template copy:"
    find "$workdir/template" -maxdepth 2 -print | sort

      echo "test-${tName} init ts and js template"
      cd "$workdir"
      kurogane new --template "$workdir/template" --name "ts" --language typescript --yes --ci
      kurogane new --template "$workdir/template" --name "js" --language javascript --yes --ci

      echo "test-${tName} bun install"
      if [[ -d "$workdir/ts/frontend" ]]; then
        cd "$workdir/ts/frontend"
        bun ci
      fi
      if [[ -d "$workdir/js/frontend" ]]; then
        cd "$workdir/js/frontend"
        bun ci
      fi

      echo "test-${tName} kurogane build"
      export CARGO_HOME="$TMPDIR/cargo-home"
      mkdir -p "$CARGO_HOME"

      cd "$workdir/ts"
      kurogane build --ci
      cd "$workdir/js"
      kurogane build --ci

      echo "test-${tName} SUCCESS"
    '';

#       cat > "$CARGO_HOME/config.toml" <<'EOF'
#       [net]
#       git-fetch-with-cli = true
#       EOF


in
lib.mapAttrs' (tName: tHash: {
  name = "test-${tName}";
  value = mkTest tName tHash;
}) templateHashes
