# Kurogane tests

This workspace contains internal test applications used to validate runtime behavior and performance of the Kurogane engine.

These are **not user-facing examples**, they are development tools for verifying correctness, stability and performance.

## List of tests

### 1. IPC benchmark

A lightweight benchmarking suite for measuring:

* IPC latency
* Throughput
* Binary vs JSON payload performance

Used to evaluate and optimize communication between renderer and browser processes.

### 2. Frontend integration test

A minimal frontend (Vite-style) test used to validate:

* Asset loading via `app://`
* Module resolution (ESM imports)
* CORS behavior inside embedded Chromium
* Static file serving correctness

This ensures real-world frontend builds work correctly inside the runtime.

## Running

From the workspace root:

```bash
cargo run --bin benchmark
cargo run --bin files-cors
```

> Note: They are intended for development and debugging, NOT production use.
