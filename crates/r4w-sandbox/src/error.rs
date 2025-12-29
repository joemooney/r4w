//! Error types for the sandbox crate.

use thiserror::Error;

/// Sandbox error type
#[derive(Error, Debug)]
pub enum SandboxError {
    /// Failed to create namespace
    #[error("namespace creation failed: {0}")]
    NamespaceError(String),

    /// Failed to drop capabilities
    #[error("capability error: {0}")]
    CapabilityError(String),

    /// Seccomp filter error
    #[error("seccomp error: {0}")]
    SeccompError(String),

    /// Memory protection error
    #[error("memory error: {0}")]
    MemoryError(String),

    /// IPC error
    #[error("IPC error: {0}")]
    IpcError(String),

    /// Container error
    #[error("container error: {0}")]
    ContainerError(String),

    /// VM error
    #[error("VM error: {0}")]
    VmError(String),

    /// FPGA isolation error
    #[error("FPGA isolation error: {0}")]
    FpgaError(String),

    /// Hardware isolation error
    #[error("hardware isolation error: {0}")]
    HardwareError(String),

    /// WebAssembly sandbox error
    #[error("WASM sandbox error: {0}")]
    WasmError(String),

    /// Configuration error
    #[error("configuration error: {0}")]
    ConfigError(String),

    /// Policy violation
    #[error("policy violation: {0}")]
    PolicyViolation(String),

    /// Permission denied
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// Resource exhausted
    #[error("resource exhausted: {0}")]
    ResourceExhausted(String),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Isolation level not supported on this platform
    #[error("isolation level {0:?} not supported: {1}")]
    UnsupportedLevel(super::IsolationLevel, String),
}

/// Result type alias for sandbox operations
pub type Result<T> = std::result::Result<T, SandboxError>;
