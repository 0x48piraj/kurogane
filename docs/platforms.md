# Install notes

Kurogane installs and finds the Chromium runtime itself. Each platform needs a few system tools. This page lists them with what is specific to each platform. [Installing Kurogane](install.md) covers the CLI itself.

## Linux

Install a C compiler and Chromium's libraries. [Installing Kurogane](install.md#what-else-you-need) lists them for each distribution. Nothing else needs setting up. `kurogane dev`, `run` and `bundle` install the Chromium runtime when it is missing.

What is specific to Linux:

* **Window class:** Windows take the executable's name as their class unless `App::window_class` sets one. An AppImage's desktop entry declares the same class.
* **Wayland:** Chromium windows run natively in a Wayland session. Embedded browsers run Chromium on X11 through XWayland.
* **Formats:** `kurogane bundle` builds a directory or a single-file AppImage (`--format appimage`).

### Chromium sandbox on Linux

Apps run unsandboxed by default. An app that opts in with `SandboxMode::Chromium` needs one of these:

* Unprivileged user namespaces. Most distributions have them. Ubuntu 24.04 restricts them through AppArmor. Lift the restriction with `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`.
* The setuid helper shipped with CEF:

```bash
sudo chown root:root ~/.local/share/tetsu/cef/{INSTALLED_CEF_VERSION}/cef_linux_x86_64/chrome-sandbox
sudo chmod 4755 ~/.local/share/tetsu/cef/{INSTALLED_CEF_VERSION}/cef_linux_x86_64/chrome-sandbox
```

The app checks both at startup. When neither is usable it refuses to start and prints these instructions. AppImages and Nix-store installations cannot use the setuid helper and need user namespaces.

> [!NOTE]
> `kurogane doctor` reads GPU details through `glxinfo` from `mesa-utils`. You only need it for that report:
>
> ```bash
> sudo apt install mesa-utils
> ```

## Windows

Install the Visual Studio C++ Build Tools with the *Desktop development with C++* workload. It includes the Windows SDK.

`kurogane dev`, `run` and `bundle` work from any terminal. Nothing is compiled from C++ and nothing links Chromium. The Build Tools only provide the linker Rust uses.

```bat
kurogane new react
npm --prefix frontend install
npm --prefix frontend run dev
kurogane dev
```

What is specific to Windows:

* **Application manifest:** An application's `build.rs` calls `kurogane_build::build()` to embed the manifest CEF's own executables carry. Without it Windows tells Chromium it runs on Windows 8. See [the build script](templates.md#the-build-script).
* **Formats:** `kurogane bundle` builds a directory or an NSIS installer (`--format nsis`).
* **Signing:** `--sign` signs the bundle's `.exe` and `.dll` files. See [code signing](bundling.md#code-signing).

### Chromium sandbox on Windows

CEF's Windows sandbox starts the application from CEF's bootstrap executable. The bootstrap loads the application as a DLL. `SandboxMode::Chromium` needs two changes to the crate and one line in `kurogane.toml`.

Give the crate a library target. Keep the binary so unsandboxed runs and `cargo run` still work:

```toml
[lib]
name = "myapp_lib"
crate-type = ["cdylib", "rlib"]

[[bin]]
name = "myapp"
```

The library takes a name of its own. Cargo warns when a library and a binary of the same name write the same `.pdb` on Windows. The bootstrap and the bundle still take the binary's name.

Move the application into `src/lib.rs` and declare the entry points the bootstrap calls:

```rust
// src/lib.rs
pub fn run() {
    kurogane::App::new("content")
        .sandbox_mode(kurogane::SandboxMode::Chromium)
        .run_or_exit();
}

kurogane::sandbox_entry!(run);
```

```rust
// src/main.rs
fn main() {
    myapp_lib::run()
}
```

Off Windows `sandbox_entry!` only checks the signature. The same source builds on every platform.

Then tell the CLI the application is sandboxed:

```toml
# kurogane.toml
[app]
sandbox = true
```

`kurogane run` and `kurogane dev` then build the library and stage it with the Chromium runtime and CEF's console bootstrap in `target/<profile>/sandbox/`. They start the application from there. The bootstrap only accepts the application's library and `chrome_elf.dll` from its own directory. Staged files are hard links where the filesystem allows them. Repeat runs stay cheap.

`kurogane bundle` does the same for a packaged application with the windowed bootstrap. No console appears. See [Windows directory](bundling.md#windows-directory---format-dir).

The `sandbox` key does nothing on Linux and macOS. One `kurogane.toml` serves every platform.

Under the sandbox the GPU runs in a process of its own. Chromium sandboxes it at low integrity. Without the sandbox Kurogane runs GPU work in the browser process (`--in-process-gpu`). There it waits for a lost D3D device to recover. An out-of-process GPU restarts instead and falls back to software rendering after three losses. In-process GPU work runs with the user's full rights. The sandbox never uses it.

The process model, `is_browser_process` and everything before `App::run` behave the same under the sandbox. Chromium relaunches the bootstrap for its helper processes.

An application that asks for `SandboxMode::Chromium` without the bootstrap refuses to start and says why. It also refuses to start when the bootstrap's sandbox ABI differs from the one `libcef.dll` was built with.

## macOS

Install the Xcode Command Line Tools:

```bash
xcode-select --install
```

`kurogane dev`, `run` and `bundle` work as on the other platforms. `hdiutil` ships with macOS. `--sign` also needs `codesign` from the Command Line Tools.

`kurogane bundle --format app` produces a `.app` with the CEF framework inside. It also produces a `.dmg` holding the app beside an `Applications` link for drag-to-install. The app runs on macOS 12 and later. That is the oldest version CEF supports. `[macos] minimum-system-version` can raise it. See [What the app declares](bundling.md#what-the-app-declares).

`--sign` with an `identity` in `[signing.macos]` signs the app and its disk image. Notarization is not performed. See [Code signing](bundling.md#code-signing).

What is specific to macOS:

* **Menus:** Every app gets the standard App, Edit and Window menus.
* **Privacy declarations:** The app declares camera and microphone use with a reason macOS shows when a page asks. `[macos.privacy]` sets your own reasons.
* **Formats:** `--format app` is the default on macOS. `--format dir` is not a macOS output and is refused.

### Chromium sandbox on macOS

`SandboxMode::Chromium` needs the app to run from a `.app` bundle (`kurogane bundle --format app`). Unbundled `kurogane dev` runs refuse to start in that mode. No entitlement changes are needed.

### Keychain prompts

Chromium encrypts cookies and saved passwords with a key held by the Keychain. An unsigned binary has no code identity that survives a rebuild. Every run raises a fresh Keychain prompt.

Denying it is harmless. Chromium logs `Encryption is not available` and stores the data unencrypted.

Signing the application ends the prompts. Until then `CredentialStorage::Basic` keeps the Keychain out of it. See [credential storage](recipes.md#credential-storage).

## Nix

Kurogane has a Nix flake for development and installation. Nix is optional. It gives you the CLI with its Chromium runtime and native dependencies in a reproducible way.

### Development

Use `nix develop` when you are **working on Kurogane itself**:

```bash
nix develop github:0x48piraj/kurogane
```

It opens a shell with what building Kurogane from source needs. That is Rust, CEF and the native libraries.

Inside the shell development stays a normal Cargo workflow:

```bash
cargo build
cargo test
cargo run -p kurogane-cli
```

`nix develop` is **not an installation command**. It does not put the packaged `kurogane` CLI on your `PATH`. It gives you the environment to develop and build the source tree. Its `CEF_PATH` names the Nix store's CEF. An application started from the shell loads it.

`nix flake check` builds the package and runs its tests. It checks the pinned CEF archive against CEF's published SHA-1. It also bundles `nix/fixture` with the packaged CLI inside the sandbox and offline. `nix/fixture` is a small application on the checkout's Kurogane.

> [!NOTE]
> **Known Nix limitation:** `nix develop` fails when the project's path contains spaces (for example `/home/user/My Projects/kurogane`). This is a known upstream Nix issue:
> https://github.com/NixOS/nix/issues/12413.
>
> It shows up as linker errors such as:
>
> ```text
> ld: cannot find .../outputs/out/lib: No such file or directory
> ```
>
> Move the project to a path without spaces. A symlink without spaces may work too. It depends on how the shell is entered.

### Running without installing

To try the packaged Kurogane CLI without installing it into your user environment:

```bash
nix run github:0x48piraj/kurogane
```

Nix builds the package when needed and runs it. The packaged CLI's `CEF_PATH` names the Nix store's CEF. That CEF carries its license, credits and an `archive.json` naming its build. `kurogane run` and `kurogane bundle` use it.

### Installing the CLI

Install the packaged CLI into your Nix user profile to **use Kurogane normally**:

```bash
nix profile add github:0x48piraj/kurogane
```

`kurogane` is then on your `PATH`:

```bash
kurogane --version
kurogane init
kurogane dev
```

This is the Nix way to install the Kurogane CLI. The Nix package provides the CLI and its Chromium runtime. No separate CEF installation is needed.

To remove it later:

```bash
nix profile list
nix profile remove kurogane
```

### The mental model

The three commands serve different purposes:

| Command               | Purpose                                                 |
| --------------------- | ------------------------------------------------------- |
| `nix develop`         | Develop **Kurogane itself** from source                 |
| `nix run`             | Run the packaged Kurogane CLI **without installing it** |
| `nix profile add`     | **Install** the packaged Kurogane CLI for normal use    |

Nix provides the packaging and runtime dependencies. Kurogane stays a normal Rust and Cargo project. Your application and workflow need not be Nix-native because you use Nix to install or develop Kurogane.
