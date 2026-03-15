# Extended examples

### WASM demo

This project includes a **pure WebAssembly demo** that mirrors the canvas animation example, but moves the **simulation logic into a standalone WASM module**.

#### What this demo demonstrates

* Loading raw `.wasm` binaries via the custom `app://app/` scheme
* JS <-> WASM interop without `wasm-bindgen`
* Canvas rendering driven by WASM logic
* Zero Rust WASM tooling baked into the runtime

> The demo includes both the Rust source and the compiled `.wasm` for reference. In real applications, only the compiled `.wasm` would typically be shipped.

#### Building the WASM module

From the demo directory:

```bash
rustc \
  --target wasm32-unknown-unknown \
  -O \
  --crate-type=cdylib \
  demo.rs \
  -o demo.wasm
```

This produces a standalone `demo.wasm` suitable for direct loading via `fetch()`.

**Note:**

The `wasm32-unknown-unknown` target must be installed:

```bash
rustup target add wasm32-unknown-unknown
```

Once the `.wasm` file is present alongside `index.html`:

```bash
cargo run --example wasm
```


### Development server

Override the frontend with a live dev server (useful for hot reload):

```bash
CEF_START_URL=http://localhost:5173 cargo run --example demo
```

or

```bash
cargo run --example server
```

### Custom frontend directory

Load a custom frontend directly from disk:

```bash
CEF_APP_PATH=/abs/path/to/frontend cargo run --example demo
```

The runtime will load `index.html` from the specified directory.

## Vite-based frontend example

This repository includes a **Vite-built frontend example** used to validate real-world asset loading, module resolution and import behavior under the custom app scheme.

### Purpose

The Vite example is intentionally minimal but it exercises features that often break in embedded Chromium runtimes:

* ES module loading
* CSS imports
* Static assets (SVG, images, text, etc.)
* Cross-file imports (`?raw`, nested assets)
* Same-origin behavior under a custom scheme

This makes it a good **integration test** for the runtime rather than a visual demo.

### Building the frontend

* Source: `tests/files-cors`
* Build output: `examples/files-cors`

From the project root:

```bash
cd tests/files-cors
bun install
bun run build
```

This produces a production-ready build in:

```text
examples/files-cors/
```

### Running the example

Once built, run it via:

```bash
cargo run --example files-cors
```

The runtime will load the built `index.html` using the `app://app/` scheme and serve all assets through the custom CEF resource handler.

> **Note**: This example uses a production Vite build (`vite build`), not the Vite dev server. Dev server usage is supported separately via `CEF_START_URL`.

Advanced users can still:

```
Runtime::run(url, true);
register_command(...)
```
