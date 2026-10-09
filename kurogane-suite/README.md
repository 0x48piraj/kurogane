# Kurogane suite

Test applications for developing Kurogane. Each one exercises a part of the runtime so you can see it work, measure it or check it by hand.

These are not starting points for your app. See [templates](../docs/templates.md) for those.

## Running

Run every example from `kurogane-suite/` so it finds its frontend:

```bash
cd kurogane-suite
```

```bash
kurogane run --example <name>
```

Each example prints Kurogane's warnings and errors. Set `RUST_LOG` to see its lifecycle and IPC detail too:

```bash
RUST_LOG=kurogane=debug kurogane run --example <name>
```

## Examples

### IPC

| Example | What it is |
|---------|------------|
| `ipc` | Commands between a page and Rust covering JSON and binary payloads, errors and argument types |
| `benchmark` | Command latency and throughput for JSON against binary payloads |
| `stream-benchmark` | Throughput of a stream that echoes what the page sends |

### Rendering

| Example | What it is |
|---------|------------|
| `gpu` | Chromium's GPU report at `chrome://gpu` |
| `dom` | Many animated DOM elements that show where DOM animation stops scaling |
| `wasm` | A page that runs a WebAssembly module |
| `css-to-shader` | HTML drawn into a canvas and post-processed by a shader |
| `files-cors` | A Vite production build served over `app://` using module imports and CORS |

### Windows and pages

| Example | What it is |
|---------|------------|
| `multi-window` | Three application windows placed and titled by their options or their page |
| `window-management` | Windows that start shown, maximized or hidden |
| `popups` | Many popups at once with Chromium's popup blocker off |
| `new-window` | A manual check of every way a page opens a window or navigates its own |
| `context-menu` | A manual check of Kurogane's right-click menu and the items an application adds |
| `delegates` | Browser and renderer delegates that print each call CEF makes |

### Event loops and embedding

| Example | What it is |
|---------|------------|
| `pump` | Chromium pumped from the application's own loop every 16 ms |
| `winit_views_poll` | A winit loop that pumps Chromium on every iteration |
| `winit_views_timer` | A winit loop that pumps Chromium at a fixed interval |
| `winit_views_scheduler` | A winit loop that pumps Chromium when it asks |
| `winit_native_embedding` | Chromium as a child browser inside a window winit owns |

In the three `winit_views` examples Chromium owns its window. See [winit](../docs/winit.md) for each mode.

### Sandbox

| Example | What it is |
|---------|------------|
| `sandbox-smoke` | A check that a page's renderer runs under the sandbox policy it asked for |

### Capabilities

| Example | What it is |
|---------|------------|
| `workspace` | A filesystem workspace a page reaches only through the grant it holds |

## Notes

### files-cors

It loads a production build. Build its frontend first from `kurogane-suite/`:

```bash
cd scenarios/files-cors/frontend
bun install
bun run build
```

Then run it from `kurogane-suite/`:

```bash
kurogane run --example files-cors
```

### dom

It shows the limits of DOM animation. It is not a benchmark. It covers:

* Why DOM animation does not scale
* How main-thread and compositor work affect rendering
* What DOM-heavy animation costs the CPU
* Why WebGL and Canvas 2D suit high-frequency rendering

### winit_views_scheduler

`KUROGANE_PUMP_STATS=1` prints its pump rate every second.

### sandbox-smoke

It starts under `SandboxMode::Chromium`. `KUROGANE_SMOKE_SANDBOX=disabled` starts it under `SandboxMode::Disabled` instead. It exits 0 when the renderer's sandbox matches the policy, 1 when it does not and 2 when the runtime refuses to start.

### workspace

The origin `app://app` holds six capabilities over one directory. It can list, read, create, write, rename and delete. The `vault/` subtree inside it is denied. The workspace lives in your temporary directory under `kurogane-workspace`.

The page shows the directory tree, an editor, the grant and every `fs.*` call with its round-trip time.

Try these:

* **Open in system** shows the granted directory in your file manager. Edit a file, quit and run again; the edit is still there.
* The grant panel's `vault/` row reads the denied subtree and gets `-6` with the operation, path and reason. The tree never shows `vault/` because the listing leaves it out.
* The grant has no `METADATA`. The page cannot ask for a file's size or whether it exists.
* Remove `RENAME` from the grant in `main.rs` and the Rename button fails with `-5`.

`workspace.reveal` is the app's only command. It takes no argument. The page cannot name a path for it. The page holds capabilities; the app keeps full authority and decides what to grant.
