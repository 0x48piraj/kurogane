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

Rust and the platform build tools are yours to install. `kurogane run`, `dev` and `bundle` install the Chromium runtime when it is missing; `kurogane doctor` checks the runtime, the build tools and the project.

## Supported platforms

| Platform | Architectures | Minimum OS | Release artifact |
|---|---|---|---|
| Linux (glibc) | x86_64, aarch64 | glibc 2.31 (Ubuntu 20.04, Debian 11) for apps; the CLI itself is static | `kurogane-cli-<arch>-unknown-linux-musl.tar.gz` |
| macOS | Apple Silicon, Intel | macOS 12 | `kurogane-cli-<arch>-apple-darwin.tar.gz` |
| Windows | x86_64, ARM64 | Windows 10 | `kurogane-cli-<arch>-pc-windows-msvc.zip` |
| NixOS, Nix | x86_64-linux, aarch64-linux, aarch64-darwin | - | flake, built from source |

The Linux CLI is a static binary and starts on any distribution. Apps need glibc because Chromium does. Alpine and other musl systems can install the CLI but cannot run apps.

An app also needs at least the glibc of the machine that built it. Build Linux bundles on the oldest distribution you support.

## Where things go

| | Unix | Windows |
|---|---|---|
| CLI | `~/.kurogane/bin/kurogane` | `%LOCALAPPDATA%\kurogane\bin\kurogane.exe` |
| PATH setup | `~/.kurogane/env` sourced from `~/.profile`, `~/.bashrc`, `~/.bash_profile`, `~/.bash_login` and `~/.zshenv` where they exist (`.profile` is created, `.zshenv` too when zsh is installed); fish `conf.d/kurogane.fish` | user `Path` in `HKCU\Environment` |
| Install receipt, read by `kurogane self uninstall` | `~/.kurogane/receipt.json` | `%LOCALAPPDATA%\kurogane\receipt.json` |
| Chromium runtime (`kurogane install`), shared with every project built with tetsu | `~/.local/share/tetsu/cef/<version>/cef_linux_<arch>` (Linux), `~/Library/Application Support/tetsu/cef/<version>/cef_macos_<arch>` (macOS) | `%LOCALAPPDATA%\tetsu\cef\<version>\cef_windows_<arch>` |

Running the installer again upgrades in place. The existing binary is replaced only after the new one has been downloaded, verified and run successfully. An interrupted or failed install leaves the previous version intact.

## Options

`install.sh` takes options after `sh -s --`:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://kurogane-rs.org/install.sh | sh -s -- --version 0.0.6-alpha.2
```

| `install.sh` | `install.ps1` | Environment | Effect |
|---|---|---|---|
| `--version <v>` | `-Version <v>` | `KUROGANE_VERSION` | install a specific release instead of the latest |
| `--install-dir <dir>` | `-InstallDir <dir>` | `KUROGANE_INSTALL_DIR` | install the binary somewhere else |
| | | `KUROGANE_HOME` | keep the installation in another home than `~/.kurogane` (Unix) |
| `--no-modify-path` | `-NoModifyPath` | `KUROGANE_NO_MODIFY_PATH=1` | leave shell startup files and the user PATH alone |
| `--force-generic` | | | install the generic Linux binary on NixOS anyway |
| `-q`, `--quiet` | | | print only errors and the summary |
| `-y`, `--yes` | | | accepted for compatibility; the installers never prompt |
| | `-Arch <arch>` | `KUROGANE_ARCH` | install `x86_64` or `aarch64` instead of the detected architecture (Windows) |
| | `-DownloadUrl <url>` | `KUROGANE_DOWNLOAD_URL` | release mirror base URL (https only) |

`irm | iex` cannot pass arguments. On Windows set the variables first or run the script as a script block:

```powershell
$env:KUROGANE_VERSION = '0.0.6-alpha.2'; irm https://kurogane-rs.org/install.ps1 | iex
& ([scriptblock]::Create((irm https://kurogane-rs.org/install.ps1))) -Version 0.0.6-alpha.2
```

The installers never prompt. They behave the same from a pipe, a terminal or CI. In GitHub Actions they also append the install directory to `$GITHUB_PATH`.

## Verifying a release

The installers check every download against the SHA-256 published in the same release. They refuse to install on a mismatch or when no SHA-256 tool is available.

Every release artifact also carries a [GitHub artifact attestation](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations). `install.sh` and `install.ps1` carry one too. It proves the release workflow in [kurogane-rs/kurogane-install](https://github.com/kurogane-rs/kurogane-install) built the artifact. Check one with the GitHub CLI:

```bash
gh release download v0.0.6-alpha.2 --repo 0x48piraj/kurogane --pattern 'kurogane-cli-x86_64-unknown-linux-musl.tar.gz'
gh attestation verify kurogane-cli-x86_64-unknown-linux-musl.tar.gz --repo kurogane-rs/kurogane-install
```

Review the installer before running it:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://kurogane-rs.org/install.sh -o install.sh
less install.sh
sh install.sh
```

## Nix and NixOS

On NixOS install through the flake. Prebuilt Linux binaries and the Chromium runtime they download expect the usual `/lib` layout. NixOS has none. `install.sh` stops there and prints the Nix command instead.

```bash
nix profile add github:0x48piraj/kurogane                 # latest
nix profile add github:0x48piraj/kurogane/v0.0.6-alpha.2  # a release
nix run github:0x48piraj/kurogane -- --help               # without installing
```

The flake builds the CLI from source and includes the Chromium runtime, its libraries and the required toolchain. `kurogane dev` works right away.

Both installation routes work on other Linux distributions with Nix.

See [Install notes](platforms.md#nix) for `nix develop` and contributor setup.

## Uninstalling

```bash
kurogane self uninstall
```

It removes an installer-managed Kurogane installation with its CLI, PATH setup, Chromium runtimes and caches.

The Chromium runtimes live in tetsu's shared installation at `tetsu/cef` in the local data directory. Other projects built on tetsu use it too. Uninstalling removes all of it.

The command shows what it will remove and asks for confirmation. Use `--yes` to skip the prompt. Use `--keep-data` to keep runtimes and caches.

An installation in another home (`KUROGANE_HOME`) is found from its binary in that home's `bin` folder. Set `KUROGANE_HOME` to that home again to uninstall an installation that used both another home and `--install-dir`.

Kurogane keeps:

* Application profiles in `kurogane/profiles` in the local data directory (`~/.local/share`, `~/Library/Application Support` or `%LOCALAPPDATA%`). They hold application data such as cookies and storage. Uninstalling keeps them and reports where they are.
* Your shell startup files. Only the installer's own entries are removed.
* An `--install-dir` and anything else installed there.
* Rust, Node.js, build tools and other system dependencies.

### Installations without a receipt

The receipt defines what Kurogane may remove. A binary the receipt does not name stays untouched. The command points you to the manual steps instead.

Remove such a binary the way you installed it:

```bash
cargo uninstall kurogane-cli # installed with cargo
```

```bash
nix profile remove kurogane  # installed with Nix
```

Kurogane can still remove its installed runtimes and caches in this case.

### Manual uninstall

Remove the installation by hand when `kurogane self uninstall` cannot run.

On Unix:

```bash
rm -r ~/.kurogane
rm -r ~/.cache/kurogane ~/.local/share/tetsu/cef
```

On macOS the cache and runtime directories are:

```text
~/Library/Caches/kurogane
~/Library/Application Support/tetsu/cef
```

Remove the installer's line from your shell startup files. It names your home directory in full:

```bash
. "/home/you/.kurogane/env"
```

Fish users remove:

```text
~/.config/fish/conf.d/kurogane.fish
```

On Windows remove everything under `%LOCALAPPDATA%\kurogane` except `profiles`. Remove the Chromium runtimes in `%LOCALAPPDATA%\tetsu\cef` too. Then remove `%LOCALAPPDATA%\kurogane\bin` from your user `Path` in **Edit environment variables for your account**.

## What else you need

The CLI is all the installers put on your machine. Building and running apps also needs the following; `kurogane doctor` checks the platform's build tools.

| | Needed for | Install |
|---|---|---|
| Rust (stable) | all platforms | [rustup.rs](https://rustup.rs) |
| Visual Studio C++ Build Tools | Windows | Visual Studio Installer, workload *Desktop development with C++* (includes the Windows SDK) |
| Xcode Command Line Tools | macOS | `xcode-select --install` |
| C compiler and Chromium's libraries | Linux | Debian/Ubuntu: `sudo apt install build-essential libnss3 libgtk-3-0 libgbm1 libxkbcommon0 libasound2` (Ubuntu 24.04: `libgtk-3-0t64 libasound2t64`); Fedora: `sudo dnf install gcc nss gtk3 mesa-libgbm libxkbcommon alsa-lib` |
| Node.js | starters with a JavaScript frontend | [nodejs.org](https://nodejs.org) |
