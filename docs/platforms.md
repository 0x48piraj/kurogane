# Install notes

Kurogane manages Chromium setup and runtime configuration automatically.

Most platform-specific environment configuration is handled by the CLI.

Only minimal system dependencies are required.

## Linux

No manual setup or environment variables are usually required.

The Kurogane CLI handles Chromium runtime configuration internally.

### Optional (sandbox fallback)

In some restricted Linux environments, Chromium may require the SUID sandbox for renderer and GPU processes.

If you encounter startup or GPU issues, you may need to run:

```bash
sudo chown root:root ~/.local/share/kurogane/cef/{INSTALLED_CEF_VERSION}/chrome-sandbox
sudo chmod 4755 ~/.local/share/kurogane/cef/{INSTALLED_CEF_VERSION}/chrome-sandbox
```

> [!NOTE]
>  On Linux, GPU diagnostics typically require `mesa-utils` (for `glxinfo`) or equivalent OpenGL utilities:
>
> ```bash
> sudo apt install mesa-utils
> ```
>
> This is only needed if you want detailed GPU introspection via the `doctor` command.

## Windows

You must build the project inside a **Visual Studio developer environment** so `CMake` can find required build tools (`Ninja` / `MSVC`).

Open:

```
x64 Native Tools Command Prompt for VS
```

Then run:

```bat
kurogane new react
npm --prefix frontend install
npm --prefix frontend run dev
kurogane dev
```

## macOS

Requires CMake and Ninja. `hdiutil` ships with macOS; `--sign` additionally needs `codesign` from the Xcode Command Line Tools.

`kurogane dev` runs. The runtime resolves the managed Chromium framework, starts the browser, renderer and GPU processes and opens a window.

Distribution is supported via `kurogane bundle --format app`, which produces a macOS `.app` bundle with the CEF framework intact plus a `.dmg` disk image. Optionally sign with `--sign` and a `certificate-identity` (see [Code signing](bundling.md#code-signing)).

> [!NOTE]
> `--format dir` is not a macOS output and is rejected; `--format app` is the default on macOS.

### Keychain prompts

Chromium encrypts cookies and saved passwords with a key held by the Keychain. Keychain access is granted to a specific code identity and an unsigned binary has none that survives a rebuild, so every run raises a fresh authorization prompt.

Denying it is harmless. Chromium logs `Encryption is not available` and stores the data unencrypted.

Signing the application resolves it permanently. Until then, `CredentialStorage::Basic` bypasses the Keychain entirely; see [credential storage](recipes.md#credential-storage).

## Nix

Kurogane provides a Nix flake for both development and installation. Nix is not required for normal Kurogane development or use, but it provides a reproducible way to obtain the CLI together with its managed Chromium runtime and native dependencies.

### Development

If you are **working on Kurogane itself**, use `nix develop`:

```bash
nix develop github:0x48piraj/kurogane
```

This enters a development shell containing the tools and dependencies needed to build Kurogane from source, including Rust, CEF and the required native libraries.

Inside the shell, development remains a normal Cargo workflow:

```bash
cargo build
cargo test
cargo run -p kurogane-cli
```

`nix develop` is **not an installation command**. It does not put the packaged `kurogane` CLI on your `PATH`; it provides the environment in which you develop and build the source tree.

> [!NOTE]
> **Known Nix limitation:** `nix develop` currently fails if the project is located in a directory whose path contains spaces (for example `/home/user/My Projects/kurogane`). This is a known upstream Nix issue:
> https://github.com/NixOS/nix/issues/12413.
>
> If you encounter linker errors such as:
>
> ```text
> ld: cannot find .../outputs/out/lib: No such file or directory
> ```
>
> Move the project to a path without spaces. If renaming the original directory isn't practical, a space-free symlink may also work depending on how the shell is entered.

### Running without installing

To try the packaged Kurogane CLI without installing it into your user environment:

```bash
nix run github:0x48piraj/kurogane
```

Nix builds the package if necessary and runs it directly. The packaged CLI is wrapped with the Chromium runtime configuration it needs.

### Installing the CLI

If you want to **use Kurogane normally**, install the packaged CLI into your Nix user profile:

```bash
nix profile add github:0x48piraj/kurogane
```

After installation, `kurogane` is available on your `PATH`:

```bash
kurogane --version
kurogane init
kurogane dev
```

This is the Nix equivalent of installing the Kurogane CLI. The CLI and its required Chromium runtime are provided by the Nix package rather than requiring a separate imperative CEF installation.

To remove it later:

```bash
nix profile list
nix profile remove <index>
```

### The mental model

The three commands serve different purposes:

| Command               | Purpose                                                 |
| --------------------- | ------------------------------------------------------- |
| `nix develop`         | Develop **Kurogane itself** from source                 |
| `nix run`             | Run the packaged Kurogane CLI **without installing it** |
| `nix profile install` | **Install** the packaged Kurogane CLI for normal use    |

Nix therefore handles the reproducible packaging and runtime dependencies, while Kurogane itself remains a normal Rust/Cargo project. You do not need to make your application or development workflow Nix-native just because you use Nix to install or develop Kurogane.
