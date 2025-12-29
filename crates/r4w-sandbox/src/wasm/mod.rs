//! WebAssembly sandbox isolation for waveforms.
//!
//! This module provides WASM-based isolation for running untrusted or portable
//! waveform code. It uses wasmtime as the runtime with WASI for system access.
//!
//! ## Security Model
//!
//! - **Memory isolation**: Each WASM module runs in its own linear memory space
//! - **Capability-based**: No permissions unless explicitly granted via WASI
//! - **No direct syscalls**: All system interactions proxied through WASI
//! - **Deny-by-default**: Filesystem, network, env vars all require explicit opt-in
//!
//! ## Trade-offs
//!
//! - **Latency**: ~10-50% overhead vs native; not suitable for hard real-time DSP
//! - **Portability**: Same WASM binary runs on any platform with wasmtime
//! - **Cold start**: Much faster than containers (~10-15x faster than Docker)
//!
//! ## Usage
//!
//! ```rust,ignore
//! use r4w_sandbox::wasm::{WasmSandbox, WasmConfig};
//!
//! // Create sandbox with minimal capabilities
//! let sandbox = WasmSandbox::new(WasmConfig::default())?;
//!
//! // Load a compiled waveform module
//! let module = sandbox.load_module("waveform.wasm")?;
//!
//! // Call waveform functions
//! let result = sandbox.call_modulate(&module, &bits)?;
//! ```

mod runtime;
mod config;

pub use runtime::{WasmSandbox, WasmModule, WasmInstance, WasmCallResult, WasmBenchmark};
pub use config::{WasmConfig, WasiCapabilities};

#[cfg(test)]
mod tests;
