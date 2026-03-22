
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

Enter the dev-shell before building:

```bash
nix develop
```

Run this **once** to set up the shared linked CEF installation:

```bash
kurogane-setup
```
