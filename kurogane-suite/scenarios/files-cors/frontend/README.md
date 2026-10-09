## Vite-based frontend test

A **Vite-built frontend test** for real-world asset loading, module resolution and import behavior under the custom `app://` scheme.

### Purpose

The test is small on purpose. It exercises features that often break in embedded Chromium runtimes:

* ES module loading
* CSS imports
* Static assets (SVG, images, text and others)
* Cross-file imports (`?raw` and nested assets)
* Same-origin behavior under a custom scheme

That makes it an **integration test** for the runtime and not a visual demo.

### Location

* Source: `kurogane-suite/scenarios/files-cors/frontend`
* Build output: `kurogane-suite/scenarios/files-cors/frontend/dist`

### Building the frontend

From `kurogane-suite/`:

```bash
cd scenarios/files-cors/frontend
bun install
bun run build
```

This produces a production build in:

```text
./scenarios/files-cors/frontend/dist
```

### Running the example

Run it from `kurogane-suite/` once built:

```bash
kurogane run --example files-cors
```

The runtime loads the built `index.html` through the `app://app/` scheme and serves every asset through its resource handler.

> [!NOTE]
> This example uses a production Vite build (`vite build`) and not the Vite dev server.
