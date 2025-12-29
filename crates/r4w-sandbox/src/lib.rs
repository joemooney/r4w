//! # R4W Sandbox
//!
//! Waveform isolation and sandboxing for R4W (Rust for Waveforms).
//!
//! This crate provides comprehensive isolation mechanisms for running waveforms
//! in secure, isolated environments. It supports multiple isolation levels from
//! basic process isolation to hardware-enforced separation.
//!
//! ## Isolation Levels
//!
//! | Level | Mechanism | Use Case |
//! |-------|-----------|----------|
//! | L1 | Rust memory safety | Basic safety, development |
//! | L1.5 | WebAssembly (WASM) | Plugin isolation, portability |
//! | L2 | Linux namespaces | Multi-tenant, privilege separation |
//! | L3 | Seccomp + SELinux/AppArmor | Defense contractors, government |
//! | L4 | Containers (Docker/Podman) | Cloud deployment, easy management |
//! | L5 | MicroVMs (Firecracker) | High assurance, rapid isolation |
//! | L6 | Full VMs (KVM/QEMU) | Certification requirements |
//! | L7 | Hardware isolation | FPGA partitioning, CPU pinning |
//! | L8 | Air gap | Classified operations |
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use r4w_sandbox::{Sandbox, IsolationLevel};
//!
//! // Create a sandbox with namespace isolation
//! let sandbox = Sandbox::builder()
//!     .isolation_level(IsolationLevel::L2_Namespaces)
//!     .waveform("BPSK")
//!     .memory_limit(512 * 1024 * 1024)
//!     .build()?;
//!
//! // Execute code in the isolated environment
//! let result = sandbox.run(|| {
//!     // Waveform processing runs isolated here
//!     process_samples(&samples)
//! })?;
//! ```
//!
//! ## Features
//!
//! - `process` - Process-level isolation (namespaces, capabilities, seccomp)
//! - `containers` - Container isolation (Docker/Podman)
//! - `microvm` - MicroVM isolation (Firecracker)
//! - `vm` - Full VM isolation (KVM/QEMU)
//! - `fpga` - FPGA partition isolation
//! - `hardware` - Hardware isolation (CPU pinning, NUMA, Intel CAT)
//! - `memory` - Memory protection (encrypted buffers, guard pages)
//! - `wasm` - WebAssembly sandbox isolation (wasmtime)
//! - `full` - All features enabled
//!
//! See the [ISOLATION_GUIDE.md](../../../docs/ISOLATION_GUIDE.md) for detailed documentation.

#![warn(missing_docs)]
#![warn(rust_2018_idioms)]

pub mod error;
pub mod levels;

#[cfg(feature = "memory")]
pub mod memory;

#[cfg(feature = "process")]
pub mod ipc;

#[cfg(feature = "fpga")]
pub mod fpga;

#[cfg(feature = "wasm")]
pub mod wasm;

pub mod policy;

// Re-export main types
pub use error::{SandboxError, Result};
pub use levels::{IsolationLevel, Sandbox, SandboxBuilder};
pub use policy::{SeccompProfile, Capability};

#[cfg(feature = "memory")]
pub use memory::{SecureBuffer, EncryptedBuffer};

#[cfg(feature = "wasm")]
pub use wasm::{WasmSandbox, WasmConfig, WasiCapabilities, WasmModule, WasmInstance, WasmCallResult, WasmBenchmark};

/// Namespace flags for process isolation
#[cfg(feature = "process")]
pub mod namespaces {
    use std::ops::BitOr;

    /// Namespace configuration flags
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Namespaces(pub(crate) u32);

    impl Namespaces {
        /// No namespaces
        pub const NONE: Namespaces = Namespaces(0);
        /// PID namespace - isolated process IDs
        pub const PID: Namespaces = Namespaces(1 << 0);
        /// Network namespace - isolated network stack
        pub const NET: Namespaces = Namespaces(1 << 1);
        /// Mount namespace - isolated filesystem view
        pub const MOUNT: Namespaces = Namespaces(1 << 2);
        /// User namespace - isolated user/group IDs
        pub const USER: Namespaces = Namespaces(1 << 3);
        /// UTS namespace - isolated hostname
        pub const UTS: Namespaces = Namespaces(1 << 4);
        /// IPC namespace - isolated System V IPC
        pub const IPC: Namespaces = Namespaces(1 << 5);
        /// Cgroup namespace - isolated cgroup root
        pub const CGROUP: Namespaces = Namespaces(1 << 6);

        /// All namespaces enabled
        pub const ALL: Namespaces = Namespaces(0x7F);

        /// Check if a namespace flag is set
        pub fn contains(&self, other: Namespaces) -> bool {
            (self.0 & other.0) == other.0
        }

        /// Get raw flags
        pub fn bits(&self) -> u32 {
            self.0
        }
    }

    impl BitOr for Namespaces {
        type Output = Self;

        fn bitor(self, rhs: Self) -> Self::Output {
            Namespaces(self.0 | rhs.0)
        }
    }
}

#[cfg(feature = "process")]
pub use namespaces::Namespaces;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_isolation_levels() {
        assert!(IsolationLevel::L1_MemorySafe < IsolationLevel::L8_AirGap);
        assert!(IsolationLevel::L3_LSM > IsolationLevel::L2_Namespaces);
    }

    #[cfg(feature = "process")]
    #[test]
    fn test_namespace_flags() {
        let ns = Namespaces::PID | Namespaces::NET;
        assert!(ns.contains(Namespaces::PID));
        assert!(ns.contains(Namespaces::NET));
        assert!(!ns.contains(Namespaces::MOUNT));
    }
}
