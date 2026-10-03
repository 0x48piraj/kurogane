# Installing Kurogane

```bash
# macOS / Linux
curl --proto '=https' --tlsv1.2 -LsSf https://kurogane-rs.org/install.sh | sh
```

```powershell
# Windows
powershell -c "irm https://kurogane-rs.org/install.ps1|iex"
```

```bash
# Nix / NixOS
nix profile add github:0x48piraj/kurogane
```

The installers download the prebuilt `kurogane` CLI for your platform, verify its SHA-256 checksum and put it on your `PATH`. They need no administrator rights, no Rust toolchain and no package manager.

Everything else (Rust, the Chromium runtime, platform build tools) is checked by `kurogane` itself when you create or run a project; `kurogane doctor` shows the full picture.

## Supported platforms

| Platform | Architectures | Minimum OS | Release artifact |
|---|---|---|---|
| Linux (glibc) | x86_64, aarch64 | glibc 2.31 (Ubuntu 20.04, Debian 11) for apps; the CLI itself is static | `kurogane-cli-<arch>-unknown-linux-musl.tar.gz` |
| macOS | Apple Silicon, Intel | macOS 12 | `kurogane-cli-<arch>-apple-darwin.tar.gz` |
| Windows | x86_64, ARM64 | Windows 10 | `kurogane-cli-<arch>-pc-windows-msvc.zip` |
| NixOS, Nix | x86_64-linux, aarch64-linux, aarch64-darwin | - | flake, built from source |

The Linux CLI is a static binary and starts on any distribution, but apps run on Chromium, which needs glibc: Alpine and other musl systems can install the CLI but cannot run apps.

## Where things go

| | Unix | Windows |
|---|---|---|
| CLI | `~/.kurogane/bin/kurogane` | `%LOCALAPPDATA%\kurogane\bin\kurogane.exe` |
| PATH setup | `~/.kurogane/env`, sourced from `~/.profile`, `~/.bashrc`, `~/.bash_profile`, `~/.zshenv` (those that exist; `.profile` and `.zshenv` are created), fish `conf.d/kurogane.fish` | user `Path` in `HKCU\Environment` |
| Chromium runtime (`kurogane install`) | `~/.local/share/kurogane/cef/<version>` (Linux), `~/Library/Application Support/kurogane/cef/<version>` (macOS) | `%LOCALAPPDATA%\kurogane\cef\<version>` |

Running the installer again upgrades in place. The existing binary is replaced only after the new one has been downloaded, verified and run successfully. Interrupted or failed installs leave the previous version intact.

## Options

`install.sh` takes options after `sh -s --`:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://kurogane-rs.org/install.sh | sh -s -- --version 0.0.6
```

| `install.sh` | `install.ps1` / environment | Effect |
|---|---|---|
| `--version <v>` | `KUROGANE_VERSION` | install a specific release instead of the latest |
| `--install-dir <dir>` | `KUROGANE_INSTALL_DIR` | install the binary somewhere else |
| `--no-modify-path` | `KUROGANE_NO_MODIFY_PATH=1` | leave shell startup files / the user PATH alone |
| `--force-generic` | | install the generic Linux binary on NixOS anyway |
| `-q`, `--quiet` | | only print errors and the summary |
| | `KUROGANE_ARCH` | `x86_64` or `aarch64`, overrides detection (Windows) |
| | `KUROGANE_DOWNLOAD_URL` | release mirror base URL (https only) |

`irm | iex` cannot pass arguments, so on Windows either set the variables first or run the script as a script block:

```powershell
$env:KUROGANE_VERSION = '0.0.6'; irm https://kurogane-rs.org/install.ps1 | iex
& ([scriptblock]::Create((irm https://kurogane-rs.org/install.ps1))) -Version 0.0.6
```

The installers never prompt, so they behave identically from a pipe, a terminal or CI. In GitHub Actions they also append the install directory to `$GITHUB_PATH`.

## Verifying a release

The installers check every download against the SHA-256 published in the same release and refuse to install on a mismatch, or when no SHA-256 tool is available.

Every release artifact, including `install.sh` and `install.ps1` themselves, also carries a [GitHub artifact attestation](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations) proving it was built by the release workflow in [kurogane-rs/kurogane-install](https://github.com/kurogane-rs/kurogane-install). To check one yourself with the GitHub CLI:

```bash
gh release download v0.0.6 --repo 0x48piraj/kurogane --pattern 'kurogane-cli-x86_64-unknown-linux-musl.tar.gz'
gh attestation verify kurogane-cli-x86_64-unknown-linux-musl.tar.gz --repo kurogane-rs/kurogane-install
```

To review the installer before running it:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://kurogane-rs.org/install.sh -o install.sh
less install.sh
sh install.sh
```

## Nix and NixOS

On NixOS, install through the flake. Prebuilt Linux binaries and the Chromium runtime they download expect the usual `/lib` layout, which NixOS does not have, so `install.sh` stops and prints the Nix command instead.

```bash
nix profile add github:0x48piraj/kurogane          # latest
nix profile add github:0x48piraj/kurogane/v0.0.6   # a release
nix run github:0x48piraj/kurogane -- --help        # without installing
```

The flake builds the CLI from source and includes the Chromium runtime, its libraries and the required toolchain. `kurogane dev` is ready to use out of the box.

On other Linux distributions with Nix installed, both installation routes work.

See [Install notes](platforms.md#nix) for `nix develop` and contributor setup.

## Uninstalling

```bash
kurogane clean all --yes   # removes Chromium runtimes and caches
rm -r ~/.kurogane          # removes the CLI and its PATH setup script
```

Then delete the line `. "$HOME/.kurogane/env"` from your shell startup files.

On Windows, run `kurogane clean all --yes`, delete `%LOCALAPPDATA%\kurogane` and remove `%LOCALAPPDATA%\kurogane\bin` from your user PATH (Settings, then "Edit environment variables for your account").

## What else you need

The CLI is all the installers put on your machine. Building and running apps also needs the following; `kurogane doctor` checks all of it.

| | Needed for | Install |
|---|---|---|
| Rust (stable) | all platforms | [rustup.rs](https://rustup.rs) |
| Visual Studio C++ Build Tools | Windows | Visual Studio Installer, workload *Desktop development with C++* (includes the Windows SDK) |
| Xcode Command Line Tools | macOS | `xcode-select --install` |
| CMake and Ninja | macOS | `brew install cmake ninja`, or the official installers |
| C compiler and Chromium's libraries | Linux | Debian/Ubuntu: `sudo apt install build-essential libnss3 libgtk-3-0 libgbm1 libxkbcommon0 libasound2` (Ubuntu 24.04: `libgtk-3-0t64 libasound2t64`); Fedora: `sudo dnf install gcc nss gtk3 mesa-libgbm libxkbcommon alsa-lib` |
| Node.js | starters with a JavaScript frontend | [nodejs.org](https://nodejs.org) |

Working on Kurogane itself or building an app with plain `cargo build` requires CMake 3.21+ and Ninja on Windows because Cargo compiles CEF's C++ wrapper library. See [CONTRIBUTING](../CONTRIBUTING.md).
