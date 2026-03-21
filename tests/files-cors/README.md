## Vite-based frontend test

This repository includes a **Vite-built frontend test** used to validate real-world asset loading, module resolution, and import behavior under the custom `app://` scheme.

### Purpose

The Vite test is intentionally minimal exercising features that often break in embedded Chromium runtimes:

* ES module loading
* CSS imports
* Static assets (SVG, images, text, etc.)
* Cross-file imports (`?raw`, nested assets)
* Same-origin behavior under a custom scheme

This makes it a good **integration test** for the runtime rather than a visual demo.

### Location

* Source: `tests/files-cors`
* Build output: `files-cors/dist`

### Building the frontend

From the `tests` project workspace root:

```bash
cd files-cors
bun install
bun run build
```

This produces a production-ready build in:

```text
./files-cors/dist
```

### Running the example

Once built, run it via:

```bash
cargo run --bin files-cors
```

The runtime will load the built `index.html` using the `app://app/` scheme and serve all assets through the custom CEF resource handler.

> **Note**: This example uses a production Vite build (`vite build`), not the Vite dev server.
