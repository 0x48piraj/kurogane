{
  lib,
  stdenv,
  fetchurl,
  cef-binary,
  cefVersion,
  linkFarm,
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

  # Archive SHA-1 published in the CEF build index
  sha1 =
    {
      "x86_64-linux" = "74a1186c566cbbac38c6b0f5298fc0bcfc1b9606";
      "aarch64-linux" = "03e7a836ee73326280b8a3032e9741898133447e";
      "aarch64-darwin" = "e73f7ce767420791b1965e15816a955d88cf1f9a";
    }
    .${stdenv.hostPlatform.system};

  cef =
    if stdenv.hostPlatform.isDarwin then
      stdenv.mkDerivation {
        pname = "cef-binary";
        inherit version;

        src = fetchurl {
          url = "https://cef-builds.spotifycdn.com/cef_binary_${version}+g${gitRevision}+chromium-${chromiumVersion}_${platform}_minimal.tar.bz2";

          hash =
            {
              "aarch64-darwin" = "sha256-71/kZBhOLgA4GizHPpEbtLjMIZ8Ob5/WEK8LyJ0OpY0=";
            }
            .${stdenv.hostPlatform.system};
        };

        dontConfigure = true;
        dontBuild = true;

        installPhase = ''
          cp -R . $out
        '';
      }
    else
      cef-binary.override {
        inherit version gitRevision chromiumVersion;

        srcHashes = {
          aarch64-linux = "sha256-+5U2KskaH3GuaoyLpNBkHK0IN1kExOfdHMPO67Gi2HU=";
          x86_64-linux = "sha256-bB1Ike84huPM9l0JKI2DBOP343JKR8kyk+K9Y+dlKOQ=";
        };
      };

  # Sources the libcef_dll_wrapper build needs
  sources = linkFarm "cef-sources-${cefVersion}" (
    lib.genAttrs [ "CMakeLists.txt" "cmake" "include" "libcef_dll" "CREDITS.html" ] (
      name: "${cef}/${name}"
    )
  );

  archiveJson = writeTextFile {
    name = "cef-archive.json";
    destination = "/archive.json";
    text = builtins.toJSON {
      type = "minimal";
      name = "cef_binary_${cefVersion}+g${gitRevision}+chromium-${chromiumVersion}_${platform}_minimal.tar.bz2";
      inherit sha1;
    };
  };
in
# Mirrors the managed installation layout produced by download-cef
symlinkJoin {
  name = "cef-with-archive-${cefVersion}";

  paths = [
    "${cef}/Release"
  ]
  ++ lib.optional (!stdenv.hostPlatform.isDarwin) "${cef}/Resources"
  ++ [
    sources
    archiveJson
  ];

  # Verifies the pinned archive against its declared SHA-1
  passthru.tests.cef-archive-sha1 = fetchurl {
    name = "cef-archive-sha1";
    url = "file://${cef.src}";
    inherit sha1;
  };
}
