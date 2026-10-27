{
  lib,
  stdenv,
  rustPlatform,
  rustc,
  cargo,
  openssl,
  dbus,
  at-spi2-core,
  glib,
  libGL,
  vulkan-loader,
  libxkbcommon,
  wayland,
  libX11,
  libXcomposite,
  libXcursor,
  libXdamage,
  libXext,
  libXfixes,
  libXi,
  libXrandr,
  libXrender,
  libXScrnSaver,
  libXtst,
  libxcb,
  gtk3,
  nss,
  nspr,
  pango,
  cairo,
  alsa-lib,
  at-spi2-atk,
  atk,
  cups,
  expat,
  fontconfig,
  gdk-pixbuf,
  libva,
  libgbm,
  libvdpau,
  libiconv,
  systemd,
  pkg-config,
  cctools,
  fetchFromGitHub,
  versionCheckHook,
  makeWrapper,
  runCommand,
  callPackage,
  mesa-demos,
  kurogane-nixpkgs,
}:
let
  version = "0.0.6-beta.1";

  # We have cef at home, will upstream soon™
  cef-binary = callPackage ./cef.nix { inherit kurogane-nixpkgs; };

  # nixpkgs packages glxinfo inside mesa-demos beside its demo programs.
  # A copy of the one binary keeps mesa-demos out of the closure.
  glxinfo = runCommand "glxinfo" { } ''
    install -Dm755 ${mesa-demos}/bin/glxinfo $out/bin/glxinfo
  '';

  buildInputs = [
    openssl
    nss
    nspr
    pango
    cairo
    glib
    expat
    fontconfig
  ]
  ++ lib.optionals stdenv.hostPlatform.isLinux [
    dbus
    at-spi2-core
    libGL
    vulkan-loader
    libxkbcommon
    wayland
    libX11
    libXcomposite
    libXcursor
    libXdamage
    libXext
    libXfixes
    libXi
    libXrandr
    libXrender
    libXScrnSaver
    libXtst
    libxcb
    gtk3
    alsa-lib
    at-spi2-atk
    atk
    cups
    gdk-pixbuf
    libva
    libgbm
    libvdpau
    systemd
  ]
  # macOS Rust binaries link libiconv which apple-sdk omits
  ++ lib.optionals stdenv.hostPlatform.isDarwin [
    libiconv
  ];

  nativeBuildInputs = [
    rustc
    cargo
    pkg-config
    stdenv.cc
    makeWrapper
  ]
  # kurogane doctor reads the GPU through glxinfo
  ++ lib.optionals stdenv.hostPlatform.isLinux [
    glxinfo
  ]
  # macOS bundles link CEF with otool and install_name_tool
  ++ lib.optionals stdenv.hostPlatform.isDarwin [
    cctools
  ];
in
rustPlatform.buildRustPackage {
  pname = "kurogane";
  inherit buildInputs nativeBuildInputs version;
  cargoHash = "sha256-SWYc9XZB8wenINuwb3YNOYk65wfn5n7SlNb3J848nn8=";

  src = fetchFromGitHub {
    owner = "0x48piraj";
    repo = "kurogane";
    tag = "v${version}";
    hash = "sha256-WyFYHRBZzJbMZ0jKwa12k4Xtaa3wrN6ogZCzDNdVw7w=";
  };

  strictDeps = true;

  postInstall = ''
    wrapProgram "$out/bin/kurogane" \
      --prefix LIBRARY_PATH : ${lib.makeLibraryPath buildInputs} \
      --set CEF_PATH ${cef-binary} \
      --prefix PATH : ${lib.makeBinPath nativeBuildInputs} \
      --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath (buildInputs ++ [ stdenv.cc.cc ])} \
      --prefix PKG_CONFIG_PATH : ${lib.makeSearchPath "lib/pkgconfig" buildInputs} \
      ${lib.optionalString stdenv.hostPlatform.isDarwin "--set-default MACOSX_DEPLOYMENT_TARGET ${stdenv.hostPlatform.darwinMinVersion}"}
  '';

  USER = "Kurogane Tests"; # Fallback when git.user and git.email aren't set
  CEF_PATH = cef-binary; # Don't download cef

  doInstallCheck = true;
  nativeInstallCheckInputs = [ versionCheckHook ];

  passthru = { inherit cef-binary nativeBuildInputs; };

  meta = {
    description = "Composable Chromium runtime for Rust";
    longDescription = ''
      Kurogane is a Rust-native runtime built on [Chromium Embedded Framework (CEF)](https://en.wikipedia.org/wiki/Chromium_Embedded_Framework), bringing Chromium to desktop applications while giving you control over windowing, event loops and lifecycle when you need it.
    '';
    homepage = "https://github.com/0x48piraj/kurogane";
    changelog = "https://github.com/0x48piraj/kurogane/releases";
    # Mirrors the allow-list in cargo-deny
    license = with lib.licenses; [
      asl20
      llvm-exception
      bsd2
      bsd3
      isc
      mit
      zlib
      mpl20
      cc0
      ofl
      unicode-30
      bzip2
      ufl
      {
        spdxId = "CDLA-Permissive-2.0";
        fullName = "Community Data License Agreement Permissive 2.0";
        url = "https://cdla.dev/permissive-2-0/";
        free = true;
        redistributable = true;
      }
    ];
    sourceProvenance = with lib.sourceTypes; [ fromSource ] ++ cef-binary.meta.sourceProvenance; # cef is binaryNativeCode
    maintainers = with lib.maintainers; [
      _0x48piraj
      R0M-A
    ]; # TODO: get on maintainers list
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
      "aarch64-darwin"
    ];
    mainProgram = "kurogane";
  };
}
