{
  lib,
  stdenv,
  cef-binary,
  cefVersion,
  symlinkJoin,
  writeTextFile,
}:

let
  version = cefVersion;
  gitRevision = "8042e43";
  chromiumVersion = "150.0.7871.101";
  platform =
    {
      "x86_64-linux" = "linux64";
      "aarch64-linux" = "linuxarm64";
      "armv7l-linux" = "linuxarm";

      "x86_64-darwin" = "macosx64";
      "aarch64-darwin" = "macosarm64";

      "x86_64-windows" = "windows64";
      "aarch64-windows" = "windowsarm64";
      "i686-windows" = "windows32";
    }
    .${stdenv.hostPlatform.system} or (throw "Unsupported platform: ${stdenv.hostPlatform.system}");

  cef = cef-binary.override {
    inherit version gitRevision chromiumVersion;

    srcHashes = {
      aarch64-linux = "sha256-+5U2KskaH3GuaoyLpNBkHK0IN1kExOfdHMPO67Gi2HU=";
      x86_64-linux = "sha256-bB1Ike84huPM9l0JKI2DBOP343JKR8kyk+K9Y+dlKOQ=";
    };
  };

  archiveJson = writeTextFile {
    name = "cef-archive.json";
    destination = "/archive.json";
    text = builtins.toJSON {
      type = "minimal";
      name = "cef_binary_${cefVersion}+g${gitRevision}+chromium-${chromiumVersion}_${platform}_minimal.tar.bz2";
      sha1 = lib.fakeHash;
    };
  };
in
symlinkJoin {
  name = "cef-with-archive-${cefVersion}";
  paths = [
    cef
    archiveJson
  ];

  postBuild = ''
    ln -s "$out"/Release/* "$out"/Resources/* "$out"
  '';
}
