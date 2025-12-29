//! Configuration for WASM sandbox.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// WASI capability grants for the sandbox.
///
/// Following the deny-by-default security model, all capabilities
/// start disabled and must be explicitly enabled.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WasiCapabilities {
    /// Allow access to stdin
    pub stdin: bool,

    /// Allow access to stdout
    pub stdout: bool,

    /// Allow access to stderr
    pub stderr: bool,

    /// Directories to pre-open for filesystem access (read-only)
    pub preopened_dirs_ro: Vec<PathBuf>,

    /// Directories to pre-open for filesystem access (read-write)
    pub preopened_dirs_rw: Vec<PathBuf>,

    /// Environment variables to expose
    pub env_vars: Vec<(String, String)>,

    /// Command-line arguments to pass
    pub args: Vec<String>,

    /// Allow network access (requires WASI preview2)
    pub network: bool,

    /// Allow clock/time access
    pub clocks: bool,

    /// Allow random number generation
    pub random: bool,
}

impl WasiCapabilities {
    /// Create capabilities with nothing allowed (maximum isolation)
    pub fn none() -> Self {
        Self::default()
    }

    /// Create capabilities suitable for DSP waveforms
    ///
    /// Allows: stdout/stderr for logging, clocks for timing, random for crypto
    /// Denies: filesystem, network, env vars
    pub fn dsp() -> Self {
        Self {
            stdout: true,
            stderr: true,
            clocks: true,
            random: true,
            ..Default::default()
        }
    }

    /// Create capabilities with stdio allowed
    pub fn with_stdio() -> Self {
        Self {
            stdin: true,
            stdout: true,
            stderr: true,
            ..Default::default()
        }
    }

    /// Builder: enable stdin
    pub fn stdin(mut self, allow: bool) -> Self {
        self.stdin = allow;
        self
    }

    /// Builder: enable stdout
    pub fn stdout(mut self, allow: bool) -> Self {
        self.stdout = allow;
        self
    }

    /// Builder: enable stderr
    pub fn stderr(mut self, allow: bool) -> Self {
        self.stderr = allow;
        self
    }

    /// Builder: add read-only directory
    pub fn preopened_dir_ro(mut self, path: impl Into<PathBuf>) -> Self {
        self.preopened_dirs_ro.push(path.into());
        self
    }

    /// Builder: add read-write directory
    pub fn preopened_dir_rw(mut self, path: impl Into<PathBuf>) -> Self {
        self.preopened_dirs_rw.push(path.into());
        self
    }

    /// Builder: add environment variable
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_vars.push((key.into(), value.into()));
        self
    }

    /// Builder: add command-line argument
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Builder: enable clocks
    pub fn clocks(mut self, allow: bool) -> Self {
        self.clocks = allow;
        self
    }

    /// Builder: enable random
    pub fn random(mut self, allow: bool) -> Self {
        self.random = allow;
        self
    }
}

/// Configuration for the WASM sandbox runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmConfig {
    /// WASI capability configuration
    pub capabilities: WasiCapabilities,

    /// Maximum memory in bytes (0 = unlimited, default = 256MB)
    pub max_memory: usize,

    /// Maximum execution time in milliseconds (0 = unlimited)
    pub max_execution_time_ms: u64,

    /// Enable fuel-based execution limiting
    pub fuel_limit: Option<u64>,

    /// Enable epoch-based interruption
    pub epoch_interruption: bool,

    /// Cranelift optimization level (0-3)
    pub optimization_level: u8,

    /// Enable SIMD support
    pub enable_simd: bool,

    /// Enable multi-threading support
    pub enable_threads: bool,

    /// Enable memory64 (64-bit memory)
    pub enable_memory64: bool,

    /// Enable component model (WASI preview2)
    pub enable_component_model: bool,

    /// Cache compiled modules to disk
    pub cache_path: Option<PathBuf>,
}

impl Default for WasmConfig {
    fn default() -> Self {
        Self {
            capabilities: WasiCapabilities::dsp(),
            max_memory: 256 * 1024 * 1024, // 256 MB
            max_execution_time_ms: 0,       // No limit
            fuel_limit: None,
            epoch_interruption: false,
            optimization_level: 2,
            enable_simd: true,
            enable_threads: false,
            enable_memory64: false,
            enable_component_model: false,
            cache_path: None,
        }
    }
}

impl WasmConfig {
    /// Create a minimal config for maximum isolation
    pub fn minimal() -> Self {
        Self {
            capabilities: WasiCapabilities::none(),
            max_memory: 64 * 1024 * 1024, // 64 MB
            fuel_limit: Some(1_000_000_000), // 1B fuel units
            ..Default::default()
        }
    }

    /// Create config optimized for DSP workloads
    pub fn dsp() -> Self {
        Self {
            capabilities: WasiCapabilities::dsp(),
            max_memory: 512 * 1024 * 1024, // 512 MB for large sample buffers
            enable_simd: true,
            optimization_level: 3, // Maximum optimization
            ..Default::default()
        }
    }

    /// Create config for development/debugging
    pub fn development() -> Self {
        Self {
            capabilities: WasiCapabilities::with_stdio(),
            optimization_level: 0, // Faster compilation
            ..Default::default()
        }
    }

    /// Builder: set capabilities
    pub fn capabilities(mut self, caps: WasiCapabilities) -> Self {
        self.capabilities = caps;
        self
    }

    /// Builder: set max memory
    pub fn max_memory(mut self, bytes: usize) -> Self {
        self.max_memory = bytes;
        self
    }

    /// Builder: set fuel limit
    pub fn fuel_limit(mut self, fuel: u64) -> Self {
        self.fuel_limit = Some(fuel);
        self
    }

    /// Builder: enable SIMD
    pub fn simd(mut self, enable: bool) -> Self {
        self.enable_simd = enable;
        self
    }

    /// Builder: set optimization level
    pub fn optimize(mut self, level: u8) -> Self {
        self.optimization_level = level.min(3);
        self
    }

    /// Builder: set cache path
    pub fn cache(mut self, path: impl Into<PathBuf>) -> Self {
        self.cache_path = Some(path.into());
        self
    }
}
