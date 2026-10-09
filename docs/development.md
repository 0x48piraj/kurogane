# Development

This page covers how you run your app, choose a frontend source and configure the runtime day to day.

## Overview

Kurogane separates the *frontend* (your HTML, JS and CSS, usually a Vite app) from the *runtime* (the Rust binary and embedded Chromium).

During development the two run side by side:

* A dev server such as Vite serves your frontend over HTTP.
* `kurogane dev` runs your Rust binary in debug mode. It opens a Kurogane app on that server.

A `kurogane.toml` at your project root ties them together. `kurogane new` writes it. `kurogane init` writes it for an existing app.

## Create a project

```bash
kurogane new react
```

`kurogane new` generates a project and prints the commands to run next. It reads them from `kurogane.toml`. You can change them there:

```toml
[app]
frontend = "frontend"                       # source npm project
frontend-dist = "frontend/dist"             # build output, bundled at package time
frontend-install = "npm --prefix frontend install"
frontend-run = "npm --prefix frontend run dev"
```

* `frontend-install` installs your frontend's dependencies. You usually run it once.
* `frontend-run` starts the dev server. `kurogane dev` expects it to be running already.

See [templates](templates.md) for custom templates and the whole manifest.

### Add Kurogane to an existing app

Already have a frontend project? `kurogane init` adds Kurogane around it without touching your files:

```sh
cd my-vite-app
kurogane init
# or non-interactively:
kurogane init --assets dist --dev-url http://localhost:5173
```

The npm project already lives at your project root. `init` leaves `frontend` unset. It writes `frontend-dist` to `kurogane.toml` and the dev URL into `src/main.rs`. See [templates](templates.md#adding-kurogane-to-an-existing-app) for everything it generates.

## Run your app

From a new project:

```bash
npm --prefix frontend install     # once
npm --prefix frontend run dev     # start the dev server
kurogane dev                      # launch the app at the dev server
```

## Choosing a frontend source

`src/main.rs` decides where the window points with `App::url` or `App::new`.

The official starters use one per build profile:

```rust
fn main() {
    #[cfg(debug_assertions)]
    App::url("http://localhost:5173").run_or_exit();   // dev server

    #[cfg(not(debug_assertions))]
    App::new("content").run_or_exit();                 // bundled assets
}
```

* **Dev (`App::url`):** Points at your live dev server. Works with Vite and any HTTP server.
* **Release (`App::new`):** Loads a directory from disk. A bundled app serves its frontend from the `content/` directory inside the bundle. See [Bundling](bundling.md).

An application without an HTML frontend uses `App::url` with its own serving or no frontend at all.

## One binary, several processes

Chromium splits its work across processes. The browser process owns your windows and runs your commands. Helper processes render pages and do GPU and utility work. Chromium starts each helper by running your binary again with a `--type=` argument (`--type=renderer`, `--type=gpu-process` and others). A bundled macOS app does the same through its `Helper.app` copies (see [Bundling](bundling.md#subprocess-helpers)).

A helper runs `main` from the top until the call that starts the runtime (`App::run`, `run_or_exit`, `start`, `build` or `start_embedded`). That call turns the process into the helper and never returns. Everything before it runs once per process, not once per app:

* **Building the `App` is fine.** It is configuration. The helpers need the same configuration (custom schemes and renderer delegates).
* **Side effects are not.** Creating or deleting files, printing, opening sockets or spawning processes happens again in every helper at the same time as the browser process. A helper that recreates a directory pulls it out from under a handle the browser process holds.

Run one-time work in the browser process only. Guard it with `kurogane::is_browser_process()` or move it into `ClientAppBrowserDelegate::on_context_initialized`. Only the browser process calls it.

```rust
fn main() {
    if kurogane::is_browser_process() {
        prepare_data_dir(); // once, not once per helper
    }
    App::new("content").run_or_exit();
}
```

## Runtime configuration

Chromium comes as a runtime installed apart from your crate.

* `kurogane install` downloads and verifies the Chromium runtime your application loads into tetsu's shared installation.
* `kurogane dev`, `run` and `bundle` find that runtime. They install it when it is missing, incomplete or unverified.
* `kurogane doctor` checks your setup. It reports the expected Chromium version, the installed versions, the runtime `bundle` packages, the toolchain and the frontend.
* `kurogane list` shows application profiles, the CLI's version and the CEF version it was built with. `kurogane info` shows the CLI, environment and project configuration.

`kurogane dev` and `run` start your application with plain `cargo run` and pass it nothing. With `CEF_PATH` set they check that runtime instead of installing one. A bundled application uses only the runtime inside its bundle and never `CEF_PATH`. See [Bundling](bundling.md#chromium-resolution) for how bundles pick and verify their runtime.

A plain `cargo run` works too. Outside a bundle the application loads the first of these it finds:

1. A runtime beside its executable
2. The runtime `CEF_PATH` names
3. The installed runtime of the CEF version it was built against

A `CEF_PATH` that names no directory stops the application. It never falls back to another runtime. Loading refuses a libcef that is not the CEF build the application was built against.

## CLI commands

| Command | What it does |
|---------|--------------|
| `kurogane new` | Creates a project from a starter or template |
| `kurogane init` | Adds Kurogane to an existing frontend project |
| `kurogane dev` | Runs the application and installs its Chromium runtime when missing |
| `kurogane run` | Runs the application with Cargo and passes your arguments to Cargo |
| `kurogane install` | Installs the Chromium runtime the project uses |
| `kurogane bundle` | Builds the application and packages it for distribution |
| `kurogane doctor` | Checks the Chromium runtime, the toolchain and the project (`--json` prints the full report) |
| `kurogane list` | Lists application profiles and versions |
| `kurogane info` | Shows the CLI, environment and project configuration |
| `kurogane clean` | Removes the project's `dist/` and Kurogane's caches (`clean all` also removes tetsu's shared Chromium installation, build tools and every application profile) |
| `kurogane showcase` | Runs Kurogane's showcase application |
| `kurogane self uninstall` | Removes an installer-managed Kurogane installation |

`--ci` runs any command without prompting. A true `CI` environment variable does the same.

## Advanced workflows

* **`kurogane run`** passes its arguments to Cargo. Arguments after `--` reach your application (`kurogane run --release -- --flag`).
* **`sandbox = true`** under `[app]` in `kurogane.toml` makes `run`, `dev` and `bundle` start the application through CEF's sandbox bootstrap on Windows. It does nothing elsewhere. See [Install notes](platforms.md#chromium-sandbox-on-windows).
* **`cargo build --release`** compiles a release binary without bundling. No build reads CEF.
* **`kurogane bundle`** packages your app for distribution. See [Bundling](bundling.md).
* **Custom protocols, IPC and windowing** are covered in [Recipes](recipes.md).
* **Embedding into an existing event loop or window host** such as winit is covered in [winit integration](winit.md).

## Platform notes

[Install notes](platforms.md) covers platform setup, the Chromium sandbox on each platform and Nix.
