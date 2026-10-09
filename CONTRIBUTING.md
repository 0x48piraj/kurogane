# Contributing to Kurogane

Thank you for looking into contributing! Kurogane aims to make Chromium a predictable and composable part of the Rust ecosystem. It reaches Chromium through CEF's C API. A few strict architectural rules keep the runtime reliable and fast across that boundary.

## Getting started

1. Fork the repository on [GitHub](https://github.com/0x48piraj/kurogane)

2. Clone your fork:

```bash
git clone https://github.com/<your-username>/kurogane.git
cd kurogane
```

## Development workflow

Builds read no CEF. `kurogane run` and `kurogane dev` install the CEF version your project uses when it is missing. Applications load it when they start. You don't need to download CEF yourself.

### On Nix / NixOS

The flake opens a development shell with the native libraries, paths and build tools Kurogane needs:

```bash
nix develop github:0x48piraj/kurogane
```

### On Linux

Install a C compiler and Chromium's libraries. [Installing Kurogane](docs/install.md#what-else-you-need) lists them for each distribution. `kurogane doctor` reads GPU details through `mesa-utils`:

```bash
sudo apt install build-essential mesa-utils
```

### On macOS

Install the Xcode Command Line Tools:

```bash
xcode-select --install
```

### On Windows

Install the Visual Studio C++ Build Tools with the *Desktop development with C++* workload. It includes the Windows SDK. Cargo finds the linker itself. Any shell works.

## Ground rules

Kurogane is young and still evolving. We clean up older patterns as we go and steer all new work toward a few core design goals.

We don't expect perfection. Look through the existing modules to see these patterns at work:

### 1. Moving away from global state

* **North star:** State belongs to the handler or ownership graph that uses it. Avoid global registries and static singletons.
* **Why it matters:** Kurogane runs inside other applications' event loops. State with one clear owner keeps that integration clean.

### 2. High-throughput boundaries

* **North star:** High-frequency calls and large payloads between Rust and the renderer take zero-copy paths, shared memory or streams.
* **Why it matters:** Serializing large binary data over standard IPC blocks threads and costs performance. Start from the existing streaming paths when you write a performance-critical boundary.

### 3. Process isolation

* **North star:** Keep browser-process work (windowing, host coordination and lifecycle) apart from renderer-process work (V8 contexts and DOM interaction).

### Best way to start is to explore

The best documentation is the code itself!

Explore the examples and the core runtime setup before a heavy change. A place where the code doesn't match these goals yet makes a good first cleanup PR. Open an issue or a draft PR early when in doubt. We'll discuss the approach together.

## Areas for contribution

We welcome contributions across the whole stack. These areas have priority:

### 1. Cross-platform runtime

* **macOS notarization:** `kurogane bundle` signs the `.app` and its `.dmg` but does not notarize them yet.
* **Wayland embedding:** Embedded browsers on Linux run Chromium on X11. A Wayland session runs them through XWayland.
* **Windowing edge cases:** Multi-window orchestration, popup cascades and window delegates on every platform.

### 2. Tooling and developer experience

* **CLI:** Diagnostics in `kurogane doctor` and `kurogane info` that pin down GPU and sandbox problems on the host.
* **Production packaging:** Bring the experimental `kurogane bundle` closer to a stable pipeline for every platform.

## Pull request process

Reviews follow a predictable flow that keeps quality high without slowing development.

### 1. Pre-flight checks

Run these before you push your branch:

```bash
cargo fmt --all                                        # format every workspace crate
cargo clippy --workspace --all-targets -- -D warnings  # lints as CI runs them
cargo test                                             # unit and doc tests
```

Rendering, process lifecycles and GPU behavior also need a run of the suite's examples. See the testing workflow below.

### 2. Testing workflow

[`kurogane-suite`](kurogane-suite/README.md) holds the test scenarios for rendering, IPC, windows and lifecycle. Each scenario is an example of that crate.

#### Running a scenario

Run one through the repository's own CLI from `kurogane-suite/`. The GPU status scenario for example:

```bash
cd kurogane-suite
cargo run -p kurogane-cli -- run --example gpu
```

> **Tip:** `cargo run -p kurogane-cli` runs your CLI changes without a `cargo install` after each one.

### Optimizing your workflow

Install the CLI once when your changes stay in the runtime:

```bash
# from the repository
cargo install --git https://github.com/0x48piraj/kurogane kurogane-cli

# or from your checkout
cargo install --path kurogane-cli --force

# then run a scenario from kurogane-suite/
cd kurogane-suite
kurogane run --example gpu
```

### Creating custom test applications

Add a scenario to `kurogane-suite` to prototype or isolate a behavior:

1. **Create the source file:** Add its entry point at `kurogane-suite/scenarios/test-feature-1/main.rs`.
2. **Register the example:** Add it to `kurogane-suite/Cargo.toml`:

```toml
[[example]]
name = "test-feature-1"
path = "scenarios/test-feature-1/main.rs"
```

3. **Run it** from `kurogane-suite/`:

```bash
kurogane run --example test-feature-1
```

> **Working directory:** A scenario that loads a local frontend with `App::new()` and a relative path finds it only from `kurogane-suite/`.

### Submitting the PR

* **Keep it focused:** One feature or bug fix per PR. A PR that refactors several subsystems at once is hard to review and slow to land.
* **State the impact:** Say in the description how your change affects process ownership, memory use or cross-platform behavior.
