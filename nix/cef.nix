{ pkgs, cefVersion }:

let
  version = cefVersion;
  gitRevision = "8042e43";
  chromiumVersion = "150.0.7871.101";

  cef = pkgs.cef-binary.override {
    inherit version gitRevision chromiumVersion;

    srcHashes = {
      aarch64-linux = "sha256-+5U2KskaH3GuaoyLpNBkHK0IN1kExOfdHMPO67Gi2HU=";
      x86_64-linux = "sha256-bB1Ike84huPM9l0JKI2DBOP343JKR8kyk+K9Y+dlKOQ=";
    };
  };

  archiveJson = pkgs.writeTextFile {
    name = "cef-archive.json";
    destination = "/archive.json";
    text = builtins.toJSON {
      type = "minimal";
      name = "cef_binary_${cefVersion}+g${gitRevision}+chromium-${chromiumVersion}_NixIsLoveNixIsLife_minimal.tar.bz2";
      sha1 = pkgs.lib.fakeHash;
    };
  };
in
pkgs.symlinkJoin {
  name = "cef-with-archive-${cefVersion}";
  paths = [
    cef
    archiveJson
  ];

  postBuild = ''
    ln -s "$out"/Release/* "$out"/Resources/* "$out"
  '';
}
