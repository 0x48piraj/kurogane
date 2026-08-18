//! Request/response IPC subsystem.
//!
//! Supports synchronous and asynchronous command handlers, pending
//! request tracking, cancellation and promise resolution.
//! Handles both JSON string and arbitrary binary payloads.

pub mod renderer;
