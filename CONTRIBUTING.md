# Contributing

## Running locally

```
cargo run --example demo
```

## Guidelines

* Keep the runtime small and focused.
* Do not obscure Chromium's behavior unless there is a strong reason.
* Favor straightforward, readable code.
* Stability and predictability come before new features.

## Pull requests

Focus areas:

* Reliability
* Consistent Cross-platform behavior
* Performance

Avoid adding framework-level abstractions to the core runtime.

> *Features are added incrementally. Stability takes priority over convenience abstractions.*
