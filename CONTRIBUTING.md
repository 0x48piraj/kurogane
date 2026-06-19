# Contributing to Kurogane

Thank you for looking into contributing! Kurogane aims to make Chromium a predictable, composable component in the Rust ecosystem. Because we bridge the C++/Rust boundary via CEF, we maintain strict architectural constraints to keep the runtime reliable and fast.

## Getting started

1. Fork the repository on [GitHub](https://github.com/0x48piraj/kurogane)

2. Clone your fork:

```bash
git clone https://github.com/<your-username>/kurogane.git
cd kurogane
```

## Development workflow

Kurogane manages the CEF runtime download and linking automatically via `cargo` and our CLI. You do **not** need to manually download CEF binaries.

### On Nix / NixOS

We provide a flake that drops you into a fully configured development shell with all required native system libraries, `glibc` paths and build tools:

```bash
nix develop github:0x48piraj/kurogane
```

### On Linux

Ensure you have standard build essentials installed. For GPU diagnostics via `kurogane doctor`, install OpenGL utilities:

```bash
sudo apt install build-essential mesa-utils
```

### On Windows

You must compile within a Visual Studio developer environment so `CMake` can locate `MSVC` and `Ninja`.

1. Open the **`x64 Native Tools Command Prompt for VS`**.
2. Run your cargo/kurogane commands directly inside that shell.

## Ground rules

Kurogane is a young, evolving project. While our codebase is still maturing and we are actively cleaning up older patterns, we try to guide all new development toward a few core design goals.

We don't expect absolute perfection but we highly encourage looking through the existing modules to see how these patterns are being established:

### 1. Moving away from global state

* **North star:** We aim to keep state explicitly owned by individual handlers or tracking graphs (often keyed by CEF identifiers) rather than relying on global registries or static singletons.
* **Why it matters:** Because Kurogane is designed to be embedded safely inside *other* host applications' event loops, isolated and explicit state ownership makes integration significantly cleaner.

### 2. High-throughput & Efficient boundaries

* **North star:** For high-frequency interactions or large data payloads passing between Rust and the Chromium renderer, we lean toward zero-copy paths, shared memory or streaming hooks.
* **Why it matters:** Serializing massive blocks of binary data over standard IPC blocks threads and kills performance. When writing performance-critical boundaries, look at our existing streaming data pathways.

### 3. Process isolation

* **North star:** Keep browser-process responsibilities (windowing, host coordination, overall lifecycle) structurally distinct from renderer-process responsibilities (V8 contexts, DOM interaction).

### Best way to start is to explore

The best documentation is the code itself!

Before diving into a heavy change, spend some time exploring our built-in examples and core runtime setup. If you see a place where our current implementation doesn't perfectly match these goals, that might actually be a great spot for your first cleanup PR. When in doubt, open an issue or draft PR early so we can discuss the approach together.

## Areas for contribution

We welcome contributions across the entire stack, but we are actively prioritizing the following areas:

### 1. Cross-platform runtime stabilization

* **macOS porting:** Establishing basic runtime execution, windowing loops and addressing app bundling/code-signing quirks specific to macOS `.app` layouts.
* **Windows & Linux windowing edge cases:** Ironing out multi-window orchestration, popup cascades and window delegate edge cases.

### 2. Tooling & DX

* **CLI:** Telemetry and heuristics inside `kurogane doctor` and `kurogane info` for debugging tricky host GPU/sandbox mismatches.
* **Production packaging:** Moving the experimental `kurogane bundle` command closer to a stable, production-ready pipeline for cross-platform binaries.

## Pull request process

To ensure high code quality without slowing down development, we use a predictable review flow:

### 1. Pre-flight checks

Before pushing your branch, make sure your code aligns with our tooling standards:

```bash
cargo fmt --all           # Enforce consistent styling across all workspace crates
cargo clippy --all        # Validate memory safety, idiomatic patterns and performance lints
cargo test                # Verify core unit and integration test suites pass successfully
```

To validate visual rendering, complex process lifecycles or GPU behavior, complement these checks by running the interactive targets via the Kurogane CLI as detailed in the testing workflow below.

### 2. Testing workflow

Kurogane includes a dedicated `tests` workspace crate featuring automated and manual test scenarios covering rendering, IPC and lifecycle behaviors.

#### Running existing test scenarios

You can execute a specific test case (such as the hardware acceleration / GPU status test) directly through the repository's local CLI package:

```bash
cargo run -p kurogane-cli -- dev --example gpu
```

> **Tip:** Relying on `cargo run -p kurogane-cli` is the ideal workflow if you are actively modifying the CLI codebase itself, as it eliminates the need to repeatedly run a global `cargo install` to test your changes.

### Optimizing your workflow

If your changes are focused entirely on the runtime or application layers rather than the CLI codebase itself, you can install the CLI globally. This simplifies your execution syntax across the workspace:

```bash
# Install the Kurogane CLI globally from the repository source
cargo install --git https://github.com/0x48piraj/kurogane kurogane-cli

# Or locally
cargo install --path . --force

# Execute the target test binary directly
kurogane dev --example gpu
```

### Creating custom test applications

To rapidly prototype or isolate a specific behavior, you can add a custom test binary to the `tests/` directory:

1. **Create the source file:** Add your test entry point at `tests/test-feature-1.rs`.
2. **Register the binary:** Update `tests/Cargo.toml` to include the new target configuration:

```toml
[[bin]]
name = "test-feature-1"
path = "test-feature-1.rs"
```

3. **Execute the target:**

```bash
kurogane dev --bin test-feature-1
```

> **Important working directory Note:** If your test application initializes a local frontend via `App::new()` using relative file paths, ensure your working directory is set to the `tests/` directory prior to execution. Running the binary from the workspace root may result in path resolution errors for frontend assets.

### Submitting the PR

* **Keep it focused:** Try to keep PRs constrained to a single feature or bug fix. Large, monolithic PRs that refactor multiple sub-systems simultaneously are difficult to review and slow down integration.
* **State the impact:** In your PR description, explain how your change affects process ownership, memory overhead or cross-platform compatibility.
