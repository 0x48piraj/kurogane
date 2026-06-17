# Kurogane: A composable Chromium runtime for Rust

Build desktop applications on Chromium, or embed it directly into existing applications.

Kurogane is a Rust-native runtime built on the [Chromium Embedded Framework (CEF)](https://en.wikipedia.org/wiki/Chromium_Embedded_Framework) for building high-performance, GPU-accelerated desktop applications.

It provides direct access to Chromium's rendering engine while allowing the host application to retain control over its event loop, windowing and lifecycle.

<p align="center">
  <img alt="Kurogane demo" src="docs/media/output.gif" width="400"><br>
  <b>Native Rust. No WebView. No Electron.</b>
</p>

## Getting started

### 1. Install Kurogane CLI (one-time)

```bash
cargo install --git https://github.com/0x48piraj/kurogane kurogane-cli
```

> Note: For platform-specific setup and troubleshooting, see [install notes](docs/platforms.md).

### 2. Try it

Run the built-in showcase and watch the runtime come alive:

```bash
kurogane showcase
```

Launches a native window running a Canvas-based animation. This is the **primary demo** for evaluating rendering behavior and performance.

## Initialize a project

```sh
kurogane init                  # Minimal starter, basic HTML frontend
kurogane init --template spa   # SPA template (frontend app entrypoint for dev-server use)
kurogane init --template ipc   # Structured IPC between Rust runtime and frontend (commands/events)
```

### Run your app

```sh
cd my-app
kurogane dev
```

The CLI will resolve and load the appropriate Chromium runtime automatically.

### Tooling

```sh
kurogane info      # Environment and runtime inspection
kurogane doctor    # Validate setup and project integrity
kurogane list      # Installed versions, profiles, runtime state
kurogane clean     # Remove runtime state and cache data
```

## Production packaging

Kurogane does not impose a packaging format.

In production, the embedding application is responsible for bundling frontend assets and selecting the startup URL.

For convenience, we include a straightforward way to do this:

```bash
kurogane bundle
```

Outputs a distributable app in the `dist/` directory.

## Motivation

This started as a GPU-accelerated visualization tool built on **Tauri** that performed extremely well on **Windows (WebView2)** out-of-the-box but encountered hard limitations on **Linux**:

System WebViews vary across platforms: WebKitGTK on Linux, WebView2 on Windows and WKWebView on macOS. This variation affects rendering behavior, GPU paths and performance characteristics that are not directly controllable from the application layer.

Those constraints are inherent to _system WebViews_.

Switching to [Chromium Embedded Framework (CEF)](https://github.com/chromiumembedded/cef) removes platform-level rendering variability but introduces a new set of tradeoffs around integration, lifecycle management and process coordination.

The alternatives weren't satisfying either. **Electron** bundles a Node.js runtime and a fixed application model, introducing constraints around process structure, IPC design and baseline runtime size. Building directly on Chromium or CEF provides maximum control, but is complex, fragile and expensive to maintain without a solid abstraction layer.

Kurogane exists as that layer, built for Rust.

## What Kurogane is built for

**Applications with existing architecture:** Supports embedding into host-managed environments with an existing event loop, window hierarchy, or GUI framework. Kurogane integrates Chromium as a component within the application, while the host retains control over execution flow and window ownership.
* **High-frequency rendering workloads:** WebGL, Canvas, WASM-heavy visualization, anything where rendering behavior across platforms matters and where you cannot accept the variance that system WebViews introduce
* **Developers who want Chromium-based rendering without Electron:** No embedded Node.js runtime. No imposed process model. Direct access to CEF's lifecycle hooks.
* **Building custom desktop shells, engines or non-standard desktop applications:** Applications that need direct control over browser process lifecycle, renderer-side extension points, or fine-grained IPC between Rust and JavaScript.

> Anyone who likes Tauri's philosophy but prefers Chromium instead of WebViews.

When you should *not* use this project:

* You want the smallest binary: use [Tauri](https://tauri.app)
* You want Node.js APIs: use [Electron](https://www.electronjs.org)
* You're building a standard CRUD UI: _use either Tauri or Electron_

This project is not intended as a replacement for Tauri or Electron. It optimizes for control over convenience and breadth.

## Architecture overview

Kurogane's runtime model is organized around a small set of clear ownership boundaries.

### Runtime and event loop

The runtime can be initialized without entering Chromium's internal blocking message loop. Applications provide their own event loop and drive Chromium's message pump explicitly. This is the foundation for embedding Kurogane into existing GUI frameworks: `winit`, raw OS window handles, or anything else that manages its own run loop.

### Runtime configuration

The runtime provides a minimal set of application-level controls for configuring Chromium behavior, GPU mode selection and startup flags. These are intentionally exposed at the application boundary so embedding applications can adjust runtime behavior without coupling to internal implementation details.

### Browser and window ownership

Browsers and windows are independently tracked entities with separate lifetimes. The runtime maintains a browser/window ownership graph with O(1) lookup, explicit popup ownership derived from opener browsers and DevTools browsers classified separately from application windows. Runtime shutdown is tied to browser lifetime, not window destruction. DevTools windows and auxiliary popups do not inadvertently tear down the application.

### Browser lifecycle

Browser creation returns a browser handle. Close routing follows CEF's expected browser shutdown protocol, including closing-state tracking, reentrancy protection and deterministic destruction sequencing. `on_before_close` fires reliably and shutdown signals propagate in a controlled and predictable order.

### Request/response IPC (RPC-style)

Kurogane provides IPC as a direct Rust-to-Chromium communication bridge designed for high-throughput interaction between runtime and renderer processes. Messages are structured and low-overhead, designed for high-frequency interaction between runtime and frontend. Large payloads are routed through a zero-copy transfer path instead of serialized IPC, allowing efficient exchange of binary data without additional runtime layers.

### Runtime extensibility

Applications can participate in browser process initialization and renderer process lifecycle without replacing Kurogane's default infrastructure. This includes custom command-line processing, V8 context lifecycle hooks, JavaScript exception handling, process message routing and more.

## 🚧 Current status

Early days! Architecture and APIs may change as the project evolves.

#### Roadmap

- [x] Cross-platform Rust-native CEF runtime integration (process model, browser lifecycle, shutdown correctness)
- [x] Modular runtime architecture with clear ownership boundaries
- [x] External event-loop integration
- [x] Native window creation and lifecycle management (CEF Views + embedded mode)
- [x] GPU-backed rendering pipeline via Chromium (CEF integration layer)
- [x] File-based and dev-server frontend loading
- [x] Linux and Windows support
- [x] Example suite covering core runtime capabilities
  - Rendering: Canvas, WebGL/2, WASM, DOM workloads
  - IPC: structured Rust <-> JS communication examples
  - Windowing: multi-window orchestration, popup flows, delegate handling
  - Stress testing: popup cascades, lifecycle edge cases
  - Integrations: winit-based embedding and external event-loop scenarios
- [x] Custom application protocol subsystem
  - Scheme handler implementation
  - Resource loading pipeline (file / dev-server / custom protocols)
  - URL routing and request interception inside CEF
- [x] Structured IPC system between Rust and renderer processes
- [x] Higher-level application runtime API
- [x] Packaging and distribution tooling
- [x] Project scaffolding / template system (CLI-driven generation)

#### In progress / planned

- [ ] End-to-end packaging pipeline (cross-platform artifacts)
- [ ] CI pipeline for runtime validation

##### Platform support

| Platform | Status |
|----------|--------|
| Linux    | Supported |
| Windows  | Supported |
| macOS    | Planned |

## Philosophy

Most desktop runtimes optimize for convenience and integration. Kurogane prioritizes control and predictable behavior.

System WebViews abstract rendering behind platform APIs. This simplifies application integration and provides platform-native behavior, but it also introduces variability across platforms and reduces visibility into performance-critical paths.

Kurogane tries to expose the underlying rendering stack instead of hiding it behind high-level abstractions.

The longer-term ambition is to make Chromium a genuinely composable component in the Rust ecosystem: something you can embed into complex applications with the same confidence you would have embedding any other library with clear ownership, predictable lifecycle and no hidden states.
