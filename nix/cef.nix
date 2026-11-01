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
  gitRevision = "cf671c0";
  chromiumVersion = "156.0.8078.80";

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
      "x86_64-linux" = "09235427222e026a4f34399742df27c5d98a4f8a";
      "aarch64-linux" = "85602db8e813ad824b45ae54218476aa3d91d561";
      "aarch64-darwin" = "bcdd1877edb300b0fb91d8f71c0b14aae256cd0f";
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
              "aarch64-darwin" = "sha256-on8dhW6EBHZX7SoUyvNV+wRIDE/hlj8EJJqKMYSvHDA=";
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
      (cef-binary.override {
        inherit version gitRevision chromiumVersion;

        srcHashes = {
          aarch64-linux = "sha256-gGZjqeyiqPe+GFSAzn53nFhktc3VK9d0iQU72xkz/rg=";
          x86_64-linux = "sha256-aHP5dbsEcxZoDqGDmYDtW9TDPIzUzkV2HJQhGFZgU4g=";
        };
      }).overrideAttrs
        (old: {
          # CEF bundles ANGLE in libcef.so; patch only libraries present in the archive
          # Goes away when https://github.com/NixOS/nixpkgs/pull/571896 lands
          installPhase = lib.concatMapStringsSep "\n" (
            line:
            let
              name = lib.findFirst (name: lib.hasInfix "/${name}" line) null [
                "libEGL.so"
                "libGLESv2.so"
              ];
            in
            if name != null && lib.hasPrefix "patchelf" (lib.trim line) then
              "[ ! -e ${old.passthru.buildType}/${name} ] || ${line}"
            else
              line
          ) (lib.splitString "\n" old.installPhase);
        });

  # CEF's license and credits which every bundle carries
  notices = linkFarm "cef-notices-${cefVersion}" (
    lib.genAttrs [ "LICENSE.txt" "CREDITS.html" ] (name: "${cef}/${name}")
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
# Bundle the runtime, notices and build metadata
symlinkJoin {
  name = "cef-with-archive-${cefVersion}";

  paths = [
    "${cef}/Release"
  ]
  ++ lib.optional (!stdenv.hostPlatform.isDarwin) "${cef}/Resources"
  ++ [
    notices
    archiveJson
  ];

  # Verifies the pinned archive against its declared SHA-1
  passthru.tests.cef-archive-sha1 = fetchurl {
    name = "cef-archive-sha1";
    url = "file://${cef.src}";
    inherit sha1;
  };
}
