# Kurogane tests

This workspace contains internal test applications used to validate runtime behavior and performance of the Kurogane engine.

These are **not user-facing examples**, they are development tools for verifying correctness, stability and performance.

## Running

From the workspace root:

```bash
cd tests

kurogane dev --bin <test-name>
```

## List of tests

### 1. IPC benchmark

A lightweight benchmarking suite for measuring:

* IPC latency
* Throughput
* Binary vs JSON payload performance

Run:

```bash
kurogane dev --bin benchmark
```

Used to evaluate and optimize communication between renderer and browser processes.

### 2. Frontend integration test

A minimal frontend (Vite-style) test used to validate:

* Asset loading via `app://`
* Module resolution (ESM imports)
* CORS behavior inside embedded Chromium
* Static file serving correctness

Run:

```bash
kurogane dev --bin files-cors
```

This ensures real-world frontend builds work correctly inside the runtime.

### 3. DOM-based educational demo

This demo intentionally animates many DOM elements illustrating DOM animation limits and CPU-bound rendering behavior.

Run:

```bash
kurogane dev --bin dom
```

This is not a performance benchmark.

Learn from them:

* Why DOM animation does not scale
* How main-thread vs compositor behavior affects rendering
* CPU costs of DOM-heavy animations
* Why WebGL / Canvas2D are preferred for high-frequency rendering

> Note: They are intended for development and debugging, NOT production use.
