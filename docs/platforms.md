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

Kurogane provides a Nix flake for a reproducible development environment:

```bash
nix develop github:0x48piraj/kurogane
```

This shell includes all required build tools, native dependencies and runtime libraries needed to work on the project.

### Why not pure Nix?

This project uses a hybrid approach where the development workflow remains based on standard Rust tooling and scripts.

The reason for this is intentional simplicity:

* Rust tooling (`cargo`) already provides fast, incremental builds
* Development is simpler without wrestling with Nix derivations
* Contributors don't need Nix knowledge to get started
* The workflow stays consistent outside of Nix environments

As the project matures, more parts may be optionally expressed in Nix (such as packaging or CI builds) but the core development workflow will hopefully remain tool-native for simplicity and speed.

## Notes

On Linux, GPU diagnostics typically require `mesa-utils` (for `glxinfo`) or equivalent OpenGL utilities:

```bash
sudo apt install mesa-utils
```

This is only needed if you want detailed GPU introspection via the `doctor` command.
