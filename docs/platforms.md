# Install notes

Kurogane manages CEF setup and runtime configuration automatically.

Most platform-specific environment configuration is handled by the CLI.

Only minimal system dependencies are required.

## Linux

No manual setup or environment variables are usually required.

The Kurogane CLI handles CEF runtime configuration internally.

### Optional (sandbox fallback)

In some restricted Linux environments, Chromium may require the SUID sandbox for renderer and GPU processes.

If you encounter startup or GPU issues, you may need to run:

```bash
sudo chown root:root ~/.local/share/cef/{INSTALLED_CEF_VERSION}/chrome-sandbox
sudo chmod 4755 ~/.local/share/cef/{INSTALLED_CEF_VERSION}/chrome-sandbox
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
kurogane init
kurogane dev
```

## macOS

While early development work exists, the runtime is not functional on macOS in its current state.

App bundling, code signing and proper `.app` distribution are not implemented yet.

Do not expect successful execution on macOS at this time.

## NixOS

Kurogane provides a Nix flake so contributors can obtain a reproducible environment while the project itself remains tool-native and does not require Nix for normal development.

### Development

Enter the development environment with:

```bash
nix develop github:0x48piraj/kurogane
```

The shell includes the Rust toolchain, CEF, native build dependencies and runtime libraries required to build and work on the project.

> [!NOTE]
> **Known Nix limitation:** `nix develop` currently fails if the project is
> located in a directory whose path contains spaces (for example
> `/home/user/My Projects/kurogane`). This is a known upstream Nix issue:
> https://github.com/NixOS/nix/issues/12413.
>
> If you encounter linker errors such as:
>
> ```text
> ld: cannot find .../outputs/out/lib: No such file or directory
> ```
>
> Move the project to a path without spaces. If renaming the original directory isn't practical, a space-free symlink may also work depending on how the shell is entered.

### Running

You can also run the packaged application directly without installing it:

```bash
nix run github:0x48piraj/kurogane
```

The packaged application automatically configures the required CEF runtime environment.

### Why Cargo?

While the project ships a Nix flake, day-to-day development intentionally remains centered around standard Rust tooling.

This keeps the workflow simple while still allowing Nix to provide a reproducible environment:

* Rust tooling (`cargo`) continues to handle fast incremental builds
* Contributors don't need Nix knowledge to get started
* The same development workflow works both inside and outside Nix
* Nix handles toolchain provisioning, native dependencies and runtime setup

The flake also serves as the basis for reproducible packaging and distribution without requiring the project itself to become Nix-native.
