# "Pure-GPU" HTML Renderer, minus the bullsh*t

A Rust-native Chromium runtime for building high-performance, GPU-accelerated desktop applications **without Electron and without system WebViews**.

Kurogane is a low-level Rust runtime built directly on the **Chromium Embedded Framework (CEF)** for developers who need control, performance and consistency beyond OS-managed WebViews.

<p align="center">
  <img alt="Kurogane demo" src="docs/media/output.gif" width="400"><br>
  <b>Native Rust. No WebView. No Electron.</b>
</p>

## Motivation

This project started as a "_GPU-accelerated FPS toy demo built with Tauri for the boys_" that performed extremely well on **Windows (WebView2)** out-of-the-box but encountered hard limitations on **Linux**:

* Compositor vsync limits i.e. VSync-locked rendering on WebKitGTK / WKWebView (~60 FPS)
* Inconsistent GPU paths across OSes
* Limited control over rendering lifecycle

Those constraints are inherent to _system WebViews_. So we pivoted to **CEF**. Chromium gives you the native GPU pipeline but most integrations come with baggage:

* **Electron** bundles Node.js, adds runtime overhead and forces a Node.js runtime into every application.
* **Building directly on Chromium/CEF** gives full control but is complex, fragile and expensive to maintain.

Kurogane sits between these extremes:

* Native, reliable Chromium GPU pipeline especially on Linux
* Direct control over application lifecycle and process model
* Fine-grained control over IPC
* No embedded Node.js runtime

## What this project optimizes for

This runtime is well-suited for:

* High-frequency rendering (WebGL/Canvas/WASM-heavy visualization workloads)
* Developers who want **Chromium without Electron**
* Cases where rendering behavior across platforms matters more than convenience
* Building custom shells, engines or non-standard desktop applications

> Anyone who likes Tauri's philosophy but prefers Chromium instead of WebViews.

When you should *not* use this project:

* You want the smallest binary: **use Tauri**
* You want Node.js APIs: **use Electron**
* You're building a standard CRUD UI: use **Tauri or Electron**

This project is not a replacement for Tauri or Electron.

## Getting started

### 1. Install Kurogane CLI (one-time)

```bash
cargo install --git https://github.com/0x48piraj/kurogane kurogane-cli
```

> Note: For platform-specific setup and troubleshooting (if needed), see [install notes](docs/platforms.md) for details.

### 2. Verify installation (optional)

Run the built-in showcase to confirm the runtime is working:

```bash
kurogane showcase
```

Launches a native window rendering a **canvas-based animation** designed to reflect GPU-backed rendering performance.

This is the **primary demo** for evaluating rendering behavior and performance.

> **Rendering note**
>
> Unlike Chrome or Electron, as of now, this runtime does not ship with a browser helper process model. Some GPU features may behave differently depending on platform and driver configuration. These differences are architectural and not regressions in rendering performance.

## Creating an application

### 3. Create a new project

```bash
kurogane init
```

A minimal starter template with a vanilla HTML frontend.

### 4. Run your app

```bash
cd my-app
kurogane dev
```

The CLI will resolve and load the appropriate Chromium runtime automatically.

## Templates

Kurogane includes built-in templates to help you get started.

#### SPA

The SPA template provides a basic HTML frontend and a dev-server-ready structure.

```bash
kurogane init --template spa
```

Use this for:

* frontend apps
* Vite / React / vanilla JS
* dev-server workflows

#### IPC

Rust <-> frontend communication via structured commands.

```bash
kurogane init --template ipc
```

Use this for:

* Desktop-style applications
* Structured data exchange between UI and runtime

## Production packaging

Kurogane does not impose a packaging format.

In production, the embedding application is responsible for bundling frontend assets and selecting the startup URL.

For convenience, we include a straightforward way to do this:

```bash
kurogane bundle
```

Outputs a distributable app in the `dist/` directory.

## 🚧 Current status

Early days! Architecture and APIs may change as the project evolves.

#### Implemented

- [x] Cross-platform Rust-native CEF runtime
- [x] Modular runtime architecture
- [x] Native window creation and lifecycle management
- [x] GPU-accelerated rendering via Chromium
- [x] File-based and dev-server frontend loading
- [x] Linux and Windows support
- [x] Examples gallery (Canvas, WebGL/2, WASM, DOM, IPC)
- [x] Custom app protocol
- [x] Structured IPC
- [x] Higher-level application API
- [x] Packaging & distribution helpers

#### In progress / planned

- [ ] macOS support
- [ ] End-to-end packaging helpers
- [ ] CI builds and example verification
- [ ] Nominal project scaffolding / starter layout

## Philosophy

Most desktop runtimes optimize for convenience and integration.

Kurogane prioritizes control and predictable behavior.

System WebViews abstract rendering behind platform APIs. This simplifies application integration and provides platform-native behavior, but it also introduces variability across platforms and reduces visibility into performance-critical paths.

Kurogane tries to expose the underlying rendering stack instead of hiding it behind high-level abstractions.

The goal is straightforward:

> Avoid unnecessary layers between the application and the rendering engine.
