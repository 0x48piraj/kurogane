# Recipes

This document covers common workflows and advanced usage patterns when building applications with Kurogane.

## Using a development server

Kurogane can load a remote development server instead of bundled frontend files.

This is useful when working with frameworks like Vite, React, or Vue that provide hot reload.

### Example

```rust
use kurogane::App;

fn main() {
    App::url("http://localhost:5173")
        .run_or_exit();
}
```

The runtime will open the specified URL instead of loading local assets.

You can also use the following template for this:

```bash
kurogane init --template server
```

* Works with any HTTP server
* Ideal for development workflows

> **Note:** In production, you should bundle your frontend instead

## Loading frontend from disk

You can load the frontend directly from a local directory.

### Example

```rust
use kurogane::App;

fn main() {
    App::new("/absolute/path/to/frontend")
        .run_or_exit();
}
```

The runtime will load `index.html` from the specified directory.

## Optional: runtime switching

If you want to switch without rebuilding yopur project, you can handle it in your application entrypoint:

```rust
use kurogane::App;

fn main() {
    let app = if std::env::var("DEV").is_ok() {
        App::url("http://localhost:5173")
    } else {
        App::new("/absolute/path/to/frontend")
    };

    app.run_or_exit();
}
```

### Use cases

* Testing static builds
* Integrating external frontend pipelines
* Debugging asset resolution issues

## Switching between development and production

Typically, you'll switch between:

* a **dev server** during development
* a **built frontend directory** in production

### Development

```rust
App::url("http://localhost:5173")
```

### Production

```rust
App::new("dist")
```

## WebAssembly (WASM) integration

Kurogane supports loading raw `.wasm` modules directly in the renderer.

This allows you to move performance-critical logic into WebAssembly without requiring additional tooling.

### Key capabilities

* Load `.wasm` via the `app://app/` scheme
* Direct JS <-> WASM interop
* No dependency on `wasm-bindgen` or any Rust tooling baked into the runtime
* Works with Canvas/WebGL pipelines

### Building a WASM module

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

### Usage

Place the compiled `.wasm` alongside your frontend:

```text
index.html
demo.wasm
```

Then load it using `fetch()` or `WebAssembly.instantiate`.

### Notes

* Only the compiled `.wasm` is required at runtime
* Source files are not needed in production
* You are free to use higher-level tooling if desired

## Custom protocol (`app://app/`)

Kurogane serves application assets through a custom scheme:

```text
app://app/
```

This replaces traditional `file://` loading and provides better control over resource handling.

### Why this matters

The custom protocol enables:

* Consistent same-origin behavior
* Controlled asset loading
* Compatibility with modern frontend tooling
* Avoidance of `file://` security restrictions

### Example

```text
app://app/index.html
app://app/assets/logo.svg
app://app/script.js
```

All assets are resolved relative to the application root.

### Behavior

* Treated as a secure origin by Chromium
* Supports ES modules, CSS imports and static assets
* Works with bundlers like Vite and Webpack

## Summary

* Use **dev servers** for fast iteration
* Use **local assets** for production builds
* Use **WASM** for performance-critical logic
* Use **custom protocol** for reliable asset handling
