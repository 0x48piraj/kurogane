{ pkgs }:

let
  cefVersion = "150.0.10";

  cefIntermediate = pkgs.cef-binary.override {
    version = cefVersion;
    gitRevision = "8042e43";
    chromiumVersion = "150.0.7871.101";

    srcHashes = {
      aarch64-linux = "";
      x86_64-linux = "sha256-bB1Ike84huPM9l0JKI2DBOP343JKR8kyk+K9Y+dlKOQ=";
    };
  };
in
cefIntermediate.overrideAttrs (oldAttrs: {
  pname = "cef";
  postInstall = (oldAttrs.postInstall or "") + ''
    cat > "$out/archive.json" <<EOF
    {
      "type": "minimal",
      "name": "cef_binary_${cefVersion}",
      "sha1": "0000000000000000000000000000000000000000"
    }
    EOF

    for file in "$out"/Release/* "$out"/Resources/*; do
      ln -sf "$file" "$out/$(basename "$file")"
    done
  '';
})
