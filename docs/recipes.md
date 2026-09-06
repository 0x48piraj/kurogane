# Recipes

This document covers common workflows and advanced usage patterns when building applications with Kurogane.

## Choosing a frontend source

### Development server

Use a local development server during development.

```rust
use kurogane::App;

fn main() {
    App::url("http://localhost:5173").run_or_exit();
}
```

Works with Vite, React, Vue, Svelte and any HTTP server.

Generate a starter project (see [templates](templates.md) for git-hosted template references):

```bash
kurogane new my-app
```

To wrap an existing frontend project instead, run `kurogane init` inside it.

### Production assets

Load a bundled frontend directly from disk.

```rust
use kurogane::App;

fn main() {
    App::new("dist").run_or_exit();
}
```

Assets are served through the `app://app/` protocol.

### Switching between development and production

```rust
use kurogane::App;

fn main() {
    let app = if cfg!(debug_assertions) { // or anything
        App::url("http://localhost:5173")
    } else {
        App::new("dist")
    };

    app.run_or_exit();
}
```

## Loading WebAssembly modules

Kurogane can serve raw WebAssembly modules through the application protocol.

This allows you to move performance-critical logic into WebAssembly without requiring additional tooling.

### Key capabilities

* Load `.wasm` via the `app://app/` scheme
* Direct JS <-> WASM interop
* No dependency on `wasm-bindgen` or any Rust tooling baked into the runtime

### Build a module

```bash
rustc \
  --target wasm32-unknown-unknown \
  -O \
  --crate-type=cdylib \
  demo.rs \
  -o demo.wasm
```

### Required target

```bash
rustup target add wasm32-unknown-unknown
```

Place the compiled `.wasm` alongside your frontend:

```text
dist/
├── index.html
└── demo.wasm
```

Then load it using `fetch()` or `WebAssembly.instantiate`.

### Notes

* Only the compiled `.wasm` is required at runtime
* Source files are not needed in production
* You are free to use higher-level tooling if desired

## Creating additional windows

Additional browser windows can be created after startup.

```rust
runtime
    .create_window(kurogane::WindowOptions {
        url: "https://github.com".into(),
        bounds: kurogane::BrowserBounds {
            x: 100,
            y: 100,
            width: 800,
            height: 600,
        },
        show_state: kurogane::WindowState::Normal,
    })
    .expect("failed to create window");
```

### Multiple windows

```rust
let runtime = kurogane::App::url("https://xkcd.com")
    .start()
    .expect("Kurogane failed to initialize");

runtime.create_window(/* ... */)?;
runtime.create_window(/* ... */)?;
```

Each browser runs as a native top-level window.

See:

* [examples/multi_window.rs](../tests/multi-window.rs)

## Exposing Rust commands to JavaScript

Register commands using `App::command`.

```rust
use serde_json::json;

let runtime = App::url("https://example.com")
    .command("ping", |payload| {
        Ok(json!({"ok": true, "echo": payload}))
    })
    .start()?;
```

Invoke them from JavaScript:

```javascript
const result = await window.core.invoke("ping", { message: "hello" });
```

Commands exchange JSON values between JavaScript and Rust.

See:

* [examples/ipc.rs](../tests/ipc.rs)

## Adding Chromium flags

Pass Chromium command-line flags during startup.

```rust
use kurogane::App;

fn main() {
    App::new("frontend")
        .chromium_flag("disable-popup-blocking")
        .run_or_exit();
}
```

Flags with values:

```rust
use kurogane::App;

fn main() {
    App::new("frontend")
        .chromium_flag_with_value("enable-blink-features", "CanvasDrawElement")
        .run_or_exit();
}
```

Useful for enabling Chromium features, diagnostics and experimental functionality.

Examples:

* [examples/popups.rs](../tests/popups.rs)
* [examples/css-to-shader.rs](../tests/css-to-shader.rs)

## GPU mode selection

Control how Chromium performs rendering.

### Automatic (default)

```rust
use kurogane::{App, GpuMode};

fn main() {
    App::new("frontend")
        .gpu_mode(GpuMode::Auto)
        .run_or_exit();
}
```

Kurogane automatically selects an appropriate backend for the current environment.

### Hardware acceleration

```rust
use kurogane::{App, GpuMode};

fn main() {
    App::new("frontend")
        .gpu_mode(GpuMode::Hardware)
        .run_or_exit();
}
```

Forces GPU acceleration.

### Software rendering

```rust
use kurogane::{App, GpuMode};

fn main() {
    App::new("frontend")
        .gpu_mode(GpuMode::Software)
        .run_or_exit();
}
```

Useful for:

* Virtual machines
* CI environments
* Remote desktop sessions

### Disable GPU acceleration

```rust
use kurogane::{App, GpuMode};

fn main() {
    App::new("frontend")
        .gpu_mode(GpuMode::Disabled)
        .run_or_exit();
}
```

Disables GPU compositing and hardware acceleration.

## Credential storage

Control how Chromium protects cookies and saved passwords at rest.

### Platform credential store (default)

```rust
use kurogane::{App, CredentialStorage};

fn main() {
    App::new("frontend")
        .credential_storage(CredentialStorage::System)
        .run_or_exit();
}
```

Encryption keys are held by the Keychain on macOS, kwallet or gnome-keyring on
Linux and DPAPI on Windows.

Reaching those stores is not always possible. Access is granted to a specific code identity, so an unsigned macOS binary is re-authorized on every rebuild and raises a Keychain prompt each run.

Hosts with no keyring daemon have nothing to reach at all.

### Built-in store

```rust
use kurogane::{App, CredentialStorage};

fn main() {
    App::new("frontend")
        .credential_storage(CredentialStorage::Basic)
        .run_or_exit();
}
```

Chromium falls back to a fixed built-in key, which is obfuscation rather than encryption. Anything the process can read is readable by anyone with access to the profile directory.

Useful for:

* Unsigned development builds
* Containers and CI environments
* Headless hosts with no keyring daemon

Not suited to profiles holding data worth protecting.

## Custom runtime integration

Use `start()` when integrating Kurogane into an existing event loop or application runtime.

```rust
use std::time::Duration;

use kurogane::App;

fn main() {
    let runtime = App::url("https://example.com")
        .start()
        .expect("Kurogane failed to initialize");

    while !runtime.should_shutdown() {
        runtime.pump();
        std::thread::sleep(Duration::from_millis(16));
    }

    runtime.shutdown();
}
```

Useful for:

* Custom event loops
* Game engines
* Framework integrations

See:

* [examples/pump.rs](../tests/pump.rs)

## Advanced: Integrating with winit

Kurogane supports multiple integration strategies for `winit`, including:

* Polling
* Fixed-interval pumping
* Scheduler-driven pumping
* Native embedding

For detailed examples and guidance, see:

* [docs/winit.md](winit.md)

## Advanced: Browser delegates

Browser delegates expose browser-process lifecycle hooks.

```rust
use cef::*;
use kurogane::App;

struct BrowserDelegate;

impl kurogane::ClientAppBrowserDelegate for BrowserDelegate {
    fn on_context_initialized(&self) {
        println!("browser context initialized");
    }
}

fn main() {
    App::url("https://example.com")
        .delegate(BrowserDelegate)
        .run_or_exit();
}
```

A delegate can also supply any handler of the CEF client that Kurogane does
not implement itself, everything but load and life span: keyboard, command,
dialog, context menu, drag, permission, display, download, focus, find, JS
dialog, print, request, frame, audio and render. Each has a getter on the
delegate returning `None` by default; the first delegate returning one wins.
CEF asks for the handler each time it needs one, so a delegate holds the
instance and returns a clone.

```rust
use cef::*;

wrap_command_handler! {
    pub struct VetoNewTab {}

    impl CommandHandler {
        fn on_chrome_command(
            &self,
            _browser: Option<&mut Browser>,
            command_id: ::std::os::raw::c_int,
            _disposition: WindowOpenDisposition,
        ) -> ::std::os::raw::c_int {
            // 1 swallows the accelerator, 0 lets Chromium run it. The ids are
            // chrome/app/chrome_command_ids.h: 34014 is IDC_NEW_TAB (Ctrl+T).
            (command_id == 34014).into()
        }
    }
}

struct BrowserDelegate {
    veto: CommandHandler,
}

impl kurogane::ClientAppBrowserDelegate for BrowserDelegate {
    fn command_handler(&self) -> Option<CommandHandler> {
        Some(self.veto.clone())
    }
}

fn main() {
    App::url("https://example.com")
        .delegate(BrowserDelegate { veto: VetoNewTab::new() })
        .run_or_exit();
}
```

Useful for:

* browser process initialization
* Chromium integration
* application-wide shortcuts, native pickers, drag and drop
* diagnostics and logging

See:

* [examples/delegates.rs](../tests/delegates.rs)

## Advanced: Renderer delegates

Renderer delegates expose renderer-process lifecycle hooks.

```rust
use cef::*;
use kurogane::App;

struct RendererDelegate;

impl kurogane::ClientAppRendererDelegate for RendererDelegate {
    fn on_context_created(
        &self,
        _browser: Option<&Browser>,
        _frame: Option<&Frame>,
        _context: Option<&V8Context>,
    ) {
        println!("context created");
    }
}

fn main() {
    App::url("https://example.com")
        .renderer_delegate(RendererDelegate)
        .run_or_exit();
}
```

Useful for:

* JavaScript injection
* V8 integration
* renderer diagnostics
* custom renderer behavior

See:

* [examples/delegates.rs](../tests/delegates.rs)
