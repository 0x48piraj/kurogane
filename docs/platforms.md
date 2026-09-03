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

Requires CMake and Ninja.

`kurogane dev` runs. The runtime resolves the managed Chromium framework, starts the browser, renderer and GPU processes and opens a window.

Distribution does not. App bundling, code signing and `.app` packaging are not implemented, so `kurogane bundle` does not yet produce an artifact you can ship.

Treat macOS as usable for development and not yet for release.

### GPU libraries in development

Chromium resolves specific libraries against the running executable's own directory whenever the process is not inside an application bundle. Chromium ships them inside the framework, so `kurogane dev` copies them next to the binaries cargo produces.

Without that step the GPU process exits during initialization and the application falls back to software rendering everywhere.

### Keychain prompts

Chromium encrypts cookies and saved passwords with a key held by the Keychain. Keychain access is granted to a specific code identity and an unsigned binary has none that survives a rebuild, so every run raises a fresh authorization prompt.

Denying it is harmless. Chromium logs `Encryption is not available` and stores the data unencrypted.

Signing the application resolves it permanently. Until then, `CredentialStorage::Basic` bypasses the Keychain entirely; see [credential storage](recipes.md#credential-storage).

## NixOS

Kurogane provides a Nix flake so contributors can obtain a reproducible environment while the project itself remains tool-native and does not require Nix for normal development.

### Development

Enter the development environment with:

```bash
nix develop github:0x48piraj/kurogane
```

The shell includes the Rust toolchain, Chromium, native build dependencies and runtime libraries required to build and work on the project.

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

The packaged application automatically configures the required Chromium runtime environment.

### Why Cargo?

While the project ships a Nix flake, day-to-day development intentionally remains centered around standard Rust tooling.

This keeps the workflow simple while still allowing Nix to provide a reproducible environment:

* Rust tooling (`cargo`) continues to handle fast incremental builds
* Contributors don't need Nix knowledge to get started
* The same development workflow works both inside and outside Nix
* Nix handles toolchain provisioning, native dependencies and runtime setup

The flake also serves as the basis for reproducible packaging and distribution without requiring the project itself to become Nix-native.
