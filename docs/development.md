# Development

This document covers the day-to-day development workflow for running your app, choosing a frontend source and configuring the runtime.

## Overview

Kurogane separates the *frontend* (your HTML/JS/CSS, usually a Vite app) from the *runtime* (the Rust binary and embedded Chromium).

During development the two run side by side:

- A dev server (e.g. Vite) serves your frontend over HTTP.
- `kurogane dev` launches your Rust binary in debug mode, which opens a Kurogane window pointed at that server.

The pieces are wired together by a `kurogane.toml` at your project root, produced by `kurogane new` (or `kurogane init` for an existing app).

## Create a project

```bash
kurogane new react
```

`kurogane new` scaffolds a project and prints the commands you need to run. It reads them from `kurogane.toml`, so they are configurable:

```toml
[app]
frontend = "frontend"                       # source npm project
frontend-dist = "frontend/dist"             # build output, bundled at package time
frontend-install = "npm --prefix frontend install"
frontend-run = "npm --prefix frontend run dev"
```

- `frontend-install` installs your frontend's dependencies (typically once).
- `frontend-run` starts the dev server. `kurogane dev` expects this server to already be running.

See [templates](templates.md) for custom templates and the complete manifest schema.

### Add Kurogane to an existing app

Already have a frontend project? `kurogane init` wraps Kurogane around it without touching your existing files:

```sh
cd my-vite-app
kurogane init
# or non-interactively:
kurogane init --assets dist --dev-url http://localhost:5173
```

Because the source npm project already lives at your project root, `init` leaves `frontend` unset and only plants the build output (`frontend-dist`) and dev URL.

## Run your app

From a freshly created project:

```bash
npm --prefix frontend install     # once
npm --prefix frontend run dev     # start the dev server
kurogane dev                      # launch the app at the dev server
```

## Choosing a frontend source

`src/main.rs` decides where the window points at runtime via `App::url` or `App::new`.

The official starters use both, gated per build profile:

```rust
fn main() {
    #[cfg(debug_assertions)]
    App::url("http://localhost:5173").run_or_exit();   // dev server

    #[cfg(not(debug_assertions))]
    App::new("content").run_or_exit();                 // bundled assets
}
```

- **Dev (`App::url`):** points at your live dev server. Works with Vite and any HTTP server.
- **Release (`App::new`):** loads a directory from disk. The bundled app serves its frontend from the fixed `content/` directory inside the bundle. See [Bundling](bundling.md).

For applications with no HTML frontend, use `App::url` and your own serving strategy, or skip the frontend entirely.

## Runtime configuration

Chromium is supplied as a managed runtime, separate from your crate and gives you complete control over the browser process.

- `kurogane install` fetches and verifies the managed Chromium distribution for the version your build links against.
- `kurogane dev` and `kurogane bundle` resolve this runtime automatically (`kurogane dev` runs the install step if needed).
- `kurogane doctor` inspects your setup: expected Chromium version, installed versions, frontend source/distribution, and container/CI detection.
- `kurogane list` shows available profiles and versions; `kurogane info` prints your project's configured manifest.

Chromium resolution prefers a `CEF_PATH` override when set, falling back to the managed installation. See [Bundling](bundling.md#chromium-resolution) for the resolution and provenance rules.

## Advanced workflows

- **`kurogane run`** passes arguments straight to Cargo (unlike `dev`), useful when you need `cargo run` passthrough.
- **`kurogane build`** compiles a release binary without bundling.
- **`kurogane bundle`** packages your app into a distributable artifact. See [Bundling](bundling.md).
- **Custom protocols, IPC and windowing** are covered in [Recipes](recipes.md).
- **Embedding into an existing event loop / window host** (e.g. winit) is covered in [winit integration](winit.md).

## Platform notes

Platform-specific setup (Visual Studio environment on Windows, sandbox fallback, Nix) lives in [Install notes](platforms.md).
