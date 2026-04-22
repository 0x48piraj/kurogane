
# Install notes

CEF still has a few operating-system security/runtime requirements that are not automated as of now.

## Linux (Sandbox permission)

Chromium requires the SUID sandbox for renderer and GPU processes.

Run **once** after installing:

```bash
sudo chown root:root ~/.local/share/cef/chrome-sandbox
sudo chmod 4755 ~/.local/share/cef/chrome-sandbox
```

Without this, Chromium may fail to start or run without GPU acceleration.

> The runtime intentionally does not modify system permissions automatically.

## Windows (MSVC build environment)

You must build the project inside a Visual Studio developer environment so CMake can find required build tools (Ninja/MSVC).

Open:

```
x64 Native Tools Command Prompt for VS
```

Then run:

```bat
cargo run
```

## macOS (experimental)

macOS support currently works in development but proper `.app` bundling and signing are not finalized.

No additional setup is required beyond installing CEF but distribution outside development environments may fail until bundling support is completed.

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
