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
use kurogane::{App, BrowserBounds, WindowOptions, WindowState};

let runtime = App::url("https://example.com")
    .start()
    .expect("Kurogane failed to initialize");

runtime
    .create_window(WindowOptions {
        url: "https://github.com".into(),
        bounds: BrowserBounds {
            x: 100,
            y: 100,
            width: 800,
            height: 600,
        },
        show_state: WindowState::Normal,
    })
    .expect("failed to create window");

runtime.run().expect("Kurogane failed");
```

### Multiple windows

```rust
use kurogane::{App, BrowserBounds, RuntimeError, WindowOptions, WindowState};

fn main() -> Result<(), RuntimeError> {
    let runtime = App::url("https://xkcd.com").start()?;

    for (x, url) in [(100, "https://example.com"), (960, "https://example.org")] {
        runtime.create_window(WindowOptions {
            url: url.into(),
            bounds: BrowserBounds { x, y: 100, width: 800, height: 600 },
            show_state: WindowState::Normal,
        })?;
    }

    runtime.run()
}
```

Each browser runs as a native top-level window. `run()` returns once the last browser has closed; a hidden window's browser counts too.

Only the application creates windows. Chrome's own window and tab commands (Ctrl+N, Ctrl+T, "Open link in new tab") do nothing in a Kurogane window.

See:

* [kurogane-suite/scenarios/multi-window.rs](../kurogane-suite/scenarios/multi-window.rs)
* [kurogane-suite/scenarios/window-management.rs](../kurogane-suite/scenarios/window-management.rs): windows that start minimized, maximized or hidden

## Links and new windows

When a page asks for a window of its own (`window.open`, a `target="_blank"` link, a form that targets a new window, a link clicked with Ctrl (Cmd on macOS), the middle button or Shift), Kurogane decides before the window exists. Chromium's own tabbed browser window never opens:

* A page of the application's own origin opens in an application window: `app://app` for `App::new`, the start URL's origin for `App::url`. So does a window the page fills in itself (`about:blank`).
* An `http` or `https` link the user clicked opens in the system's default browser.
* Anything else is refused: a script opening a website on its own, `mailto:`, `file:` and custom schemes.

`App::on_new_window` changes that per request:

```rust
use kurogane::{App, NewWindowDecision, Origin};

let sign_in = Origin::parse("https://accounts.example.com").unwrap();

App::new("dist")
    .on_new_window(move |request, _app| {
        if request.origin() == &sign_in {
            // The sign-in page reports back to the page that opened it,
            // so it has to run inside the app
            NewWindowDecision::Allow
        } else {
            NewWindowDecision::Default
        }
    })
    .run_or_exit();
```

* `Allow` opens the page in an application window whatever its origin. That page reaches only the commands and events `App::permit` grants its origin.
* `Deny` refuses the window.
* `OpenExternal` sends the URL to the system browser, but only an `http` or `https` link the user clicked; anything else is refused, so a page cannot make the app start another program on its own.
* `Default` leaves the request to Kurogane.

Compare origins (`request.origin()`), not URL strings: `https://accounts.example.com.evil.net` starts with `https://accounts.example.com`.

The hook runs on the UI thread before the window exists, so it must not block. A hook that panics refuses the window.

## One instance per profile

Your app keeps its settings and browsing data between launches. Starting the app again while it is already open brings the existing app window to the front instead of opening another one.

This is useful for things like opening a file or link in an app that is already running. For example, a user might double-click a file, choose your app from **Open With**, click a `myapp://` link, or run:

```text
myapp notes.txt
```

In these cases, Kurogane sends the new launch to the copy that is already running. `App::on_second_instance` lets your app decide what to do with it.

A common use is to open the file or link in the existing window:

```rust
use kurogane::App;

App::new("dist")
    .on_second_instance(|launch, app| {
        for arg in launch.args() {
            // Relative to where the new launch started.
            let path = match launch.working_dir() {
                Some(dir) => dir.join(arg),
                None => arg.into(),
            };

            app.broadcast_json("open-file", &path);
        }
    })
    .run_or_exit();
```

```javascript
kurogane.on("open-file", (json) => openFile(JSON.parse(json)));
```

The hook is only about **another launch of the app**. Opening another window from your own code is separate; your app can create as many windows as it needs without going through `on_second_instance`.

You do not have to handle a second launch. By default, starting the app again simply brings the existing windows to the front.

`launch.switch("new-window")` can be used to check for a switch such as `--new-window`.

On macOS, opening the app bundle while it is already running activates the existing app directly, without starting another process. The hook therefore runs for launches that actually start a new process, such as launching the executable from a terminal.

Sometimes two copies really do need to run at the same time. Give each one a different profile with `App::profile_id`.

## Exposing Rust commands to JavaScript

Register commands using `App::command`.

```rust
use kurogane::{App, AppHandle};
use serde_json::{Value, json};

App::url("https://example.com")
    .command("ping", |payload: Value, _: &AppHandle| {
        Ok(json!({"ok": true, "echo": payload}))
    })
    .run_or_exit();
```

The closure takes the request and the `AppHandle`. The request can be any type serde can deserialize.

Invoke them from JavaScript:

```javascript
const result = await window.kurogane.invoke("ping", { message: "hello" });
```

Commands exchange JSON values between JavaScript and Rust.

By default only the application's own pages can call a command: those of `app://app` for `App::new`, of the start URL's origin for `App::url` (here `https://example.com`). A page of any other origin, in a popup, an iframe or a window that followed a link, is refused with code `-4` unless `App::permit` names its origin for that command. `App::permit_all` opens a command to every origin, and `App::deny_unlisted` closes every command without a rule, to the application's own pages too. Event subscriptions follow the same rule through `App::permit_event`.

See:

* [kurogane-suite/scenarios/ipc/main.rs](../kurogane-suite/scenarios/ipc/main.rs)

## Streaming data

A stream carries chunks both ways between a page and a Rust handler. Register a factory with `App::stream`; it makes a handler for each stream a page opens.

```rust
use kurogane::{App, IpcError, StreamHandler, StreamResponder};

struct Upload {
    received: usize,
}

impl StreamHandler for Upload {
    // Accept or refuse; an Err rejects the page's openStream with its
    // message and code
    fn on_open(&mut self, metadata: &str) -> Result<(), IpcError> {
        if metadata.is_empty() {
            return Err("name the upload".into());
        }
        Ok(())
    }

    // The page holds the stream now: send, end, fail, or hand a clone of
    // the responder to a thread of your own
    fn on_opened(&mut self, responder: &StreamResponder) -> Result<(), IpcError> {
        responder.send_data(b"ready")
    }

    fn on_chunk(&mut self, data: &[u8], _: &StreamResponder) -> Result<(), IpcError> {
        self.received += data.len();
        Ok(())
    }

    fn on_end(&mut self, _: &str, responder: StreamResponder) -> Result<(), IpcError> {
        responder.end(&self.received.to_string())
    }
}

fn main() {
    App::new("frontend")
        .stream("upload", || Upload { received: 0 })
        .run_or_exit();
}
```

In the page:

```javascript
const stream = await window.kurogane.openStream("upload", "notes.txt");
stream.onData((chunk) => console.log(new TextDecoder().decode(chunk)));
stream.onEnd((result) => console.log(`${result} bytes received`));
stream.write(new TextEncoder().encode("hello"));
stream.end();
```

Handlers and sends fail with an `IpcError`, as commands do; a string converts into one. The first `end` or `error` a handler sends closes the stream; later sends fail with `stream closed`. When the page calls `end()` and `on_end` sends neither, the runtime ends the stream with `""`, so the page always hears back. Handlers run on the UI thread; a `StreamResponder` can be cloned and used from any thread.

## Logging

Kurogane reports what it does through [`tracing`](https://docs.rs/tracing) events: its lifecycle and IPC detail at `debug`, problems at `warn` and `error`. It never writes to stdout or stderr itself, so nothing appears until the application installs a subscriber. With `tracing-subscriber`:

```toml
[dependencies]
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

```rust
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::WARN.into())
                .from_env_lossy(),
        )
        .log_internal_errors(false)
        .init();

    kurogane::App::new("dist").run_or_exit();
}
```

Warnings and errors then print by default, and `RUST_LOG=kurogane=debug kurogane run` adds the detail.

Keep `log_internal_errors(false)`. With the default, a line that cannot be written, because the application's output goes into a program that has exited (`| tee` ended by Ctrl+C), is reported on stderr instead; when stderr is the same closed pipe, that report panics, and a panic inside a CEF callback aborts the application. Any other `tracing` subscriber works the same way.

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

A flag's name may be written with or without its leading `--`. A flag you pass overrides Kurogane's own value for the same switch.

Useful for enabling Chromium features, diagnostics and experimental functionality.

Examples:

* [kurogane-suite/scenarios/popups/main.rs](../kurogane-suite/scenarios/popups/main.rs)
* [kurogane-suite/scenarios/css-to-shader/main.rs](../kurogane-suite/scenarios/css-to-shader/main.rs)

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

Use `start()` when integrating Kurogane into an existing event loop or application runtime, and give it a scheduler: CEF then says when it next needs `pump()`, as CEF recommends for an application that pumps it from its own loop.

```rust
use std::sync::mpsc;
use std::time::{Duration, Instant};

use kurogane::{App, PumpRequest};

// The longest the loop waits between pumps, as CEF's cefclient does
const MAX_PUMP_DELAY: Duration = Duration::from_millis(1000 / 30);

fn main() {
    // CEF may call the scheduler from any thread: it hands the loop a deadline
    let (wake, deadlines) = mpsc::channel::<Instant>();

    let runtime = App::url("https://example.com")
        .scheduler(move |request: PumpRequest| {
            let _ = wake.send(request.deadline(Instant::now()));
        })
        .start()
        .expect("Kurogane failed to initialize");

    let mut next = Instant::now();
    while !runtime.should_shutdown() {
        let now = Instant::now();
        if now >= next {
            // Until CEF asks for an earlier pump
            next = now + MAX_PUMP_DELAY;
            runtime.pump();
        }
        // Sleep until the deadline or a new request; the earliest wins
        let wait = next.saturating_duration_since(Instant::now());
        if let Ok(deadline) = deadlines.recv_timeout(wait) {
            next = next.min(deadline);
        }
    }

    runtime.shutdown();
}
```

The loop runs on the thread that started Kurogane. It pumps once the earliest deadline it has not pumped yet has passed, and never waits more than 33 ms between pumps, as CEF's sample application, cefclient, does.

`should_shutdown()` becomes true when the application has finished closing its browsers; after the last window closes or after `AppHandle::shutdown()` closes them all. Keep calling `pump()` until then, then call `AppInstance::shutdown()` to shut down CEF. An application without a loop of its own calls `run()` instead.

Useful for:

* Custom event loops
* Game engines
* Framework integrations

See:

* [docs/winit.md](winit.md): the same loop with winit
* [kurogane-suite/winit/views_scheduler.rs](../kurogane-suite/winit/views_scheduler.rs)
* [kurogane-suite/scenarios/pump.rs](../kurogane-suite/scenarios/pump.rs): pumping every 16 ms without a scheduler, the mode CEF discourages

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

Useful for:

* browser process initialization
* Chromium integration
* diagnostics and logging

See:

* [kurogane-suite/scenarios/delegates.rs](../kurogane-suite/scenarios/delegates.rs)

## Advanced: Renderer delegates

Renderer delegates expose renderer-process lifecycle hooks.

```rust
use kurogane::App;
use kurogane::cef::{Browser, Frame, V8Context};

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

* [kurogane-suite/scenarios/delegates.rs](../kurogane-suite/scenarios/delegates.rs)
