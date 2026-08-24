{ pkgs, cefVersion }:

let
  cef = pkgs.cef-binary.override {
    version = cefVersion;
    gitRevision = "8042e43";
    chromiumVersion = "150.0.7871.101";

    srcHashes = {
      aarch64-linux = "";
      x86_64-linux = "sha256-bB1Ike84huPM9l0JKI2DBOP343JKR8kyk+K9Y+dlKOQ=";
    };
  };

  archiveJson = pkgs.writeTextFile {
    name = "cef-archive.json";
    destination = "/archive.json";
    text = builtins.toJSON {
      type = "minimal";
      name = "cef_binary_${cefVersion}";
      sha1 = "0000000000000000000000000000000000000000";
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
