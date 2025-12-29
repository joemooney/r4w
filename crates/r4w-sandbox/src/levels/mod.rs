//! Isolation level definitions and sandbox builder.
//!
//! This module provides the core `Sandbox` type and `IsolationLevel` enum
//! for configuring waveform isolation.

mod sandbox;

pub use sandbox::{Sandbox, SandboxBuilder, SandboxConfig};

use serde::{Deserialize, Serialize};

/// Isolation levels from least to most isolated.
///
/// Higher levels provide stronger isolation but may have higher overhead
/// or require more system privileges to configure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
#[allow(non_camel_case_types)]
pub enum IsolationLevel {
    /// Level 1: Rust memory safety only (no additional isolation)
    ///
    /// Provides: Memory safety through Rust's type system
    /// Requires: Nothing special
    /// Use case: Development, testing
    L1_MemorySafe = 1,

    /// Level 1.5: WebAssembly sandbox isolation
    ///
    /// Provides: Memory isolation, capability-based security, syscall filtering via WASI
    /// Requires: wasmtime runtime, wasm32-wasip2 compilation target
    /// Use case: Plugin isolation, untrusted waveforms, portable deployment
    /// Trade-offs: Higher latency than L1 (not suitable for hard real-time DSP)
    #[cfg(feature = "wasm")]
    L1_5_Wasm = 15, // Using 15 to represent 1.5 while maintaining ordering

    /// Level 2: Linux namespaces (PID, NET, MOUNT, USER)
    ///
    /// Provides: Process isolation, separate network stack, isolated filesystem view
    /// Requires: Linux kernel with namespace support
    /// Use case: Multi-tenant deployments, privilege separation
    L2_Namespaces = 2,

    /// Level 3: Namespaces + Seccomp + LSM (SELinux/AppArmor)
    ///
    /// Provides: Syscall filtering, mandatory access control
    /// Requires: LSM-enabled kernel, appropriate profiles
    /// Use case: Defense contractors, government deployments
    L3_LSM = 3,

    /// Level 4: Container isolation (Docker/Podman)
    ///
    /// Provides: Full container isolation with resource limits
    /// Requires: Container runtime (Docker, Podman)
    /// Use case: Cloud deployments, easy management
    L4_Container = 4,

    /// Level 5: MicroVM isolation (Firecracker, gVisor)
    ///
    /// Provides: VM-level isolation with minimal overhead
    /// Requires: KVM, Firecracker runtime
    /// Use case: High assurance, multi-tenant cloud
    L5_MicroVM = 5,

    /// Level 6: Full VM isolation (KVM/QEMU)
    ///
    /// Provides: Complete virtual machine isolation
    /// Requires: KVM, QEMU
    /// Use case: Certification requirements, highest software isolation
    L6_FullVM = 6,

    /// Level 7: Hardware isolation (FPGA partitioning, CPU pinning)
    ///
    /// Provides: Physical resource separation
    /// Requires: Hardware support (dedicated CPUs, IOMMU, FPGA)
    /// Use case: Critical infrastructure, real-time requirements
    L7_Hardware = 7,

    /// Level 8: Air gap (physical separation)
    ///
    /// Provides: Complete physical isolation
    /// Requires: Separate hardware systems
    /// Use case: Classified operations, highest security
    L8_AirGap = 8,
}

impl IsolationLevel {
    /// Get a human-readable description of the isolation level
    pub fn description(&self) -> &'static str {
        match self {
            Self::L1_MemorySafe => "Rust memory safety (development)",
            #[cfg(feature = "wasm")]
            Self::L1_5_Wasm => "WebAssembly sandbox (plugins, portability)",
            Self::L2_Namespaces => "Linux namespaces (multi-tenant)",
            Self::L3_LSM => "Seccomp + SELinux/AppArmor (government)",
            Self::L4_Container => "Container isolation (cloud)",
            Self::L5_MicroVM => "MicroVM isolation (high assurance)",
            Self::L6_FullVM => "Full VM isolation (certification)",
            Self::L7_Hardware => "Hardware isolation (FPGA, CPU pinning)",
            Self::L8_AirGap => "Air gap (classified)",
        }
    }

    /// Check if this level requires root/admin privileges
    pub fn requires_root(&self) -> bool {
        match self {
            #[cfg(feature = "wasm")]
            Self::L1_5_Wasm => false, // WASM runs unprivileged
            _ => matches!(
                self,
                Self::L2_Namespaces
                    | Self::L3_LSM
                    | Self::L5_MicroVM
                    | Self::L6_FullVM
                    | Self::L7_Hardware
            ),
        }
    }

    /// Check if this level is available on the current platform
    pub fn is_available(&self) -> bool {
        match self {
            Self::L1_MemorySafe => true,
            #[cfg(feature = "wasm")]
            Self::L1_5_Wasm => true, // WASM runtime is always available when feature enabled
            #[cfg(target_os = "linux")]
            Self::L2_Namespaces | Self::L3_LSM => true,
            #[cfg(not(target_os = "linux"))]
            Self::L2_Namespaces | Self::L3_LSM => false,
            Self::L4_Container => check_container_runtime(),
            Self::L5_MicroVM => check_kvm_available(),
            Self::L6_FullVM => check_kvm_available(),
            Self::L7_Hardware => check_hardware_isolation(),
            Self::L8_AirGap => true, // Always "available" - it's a deployment choice
        }
    }
}

impl Default for IsolationLevel {
    fn default() -> Self {
        Self::L1_MemorySafe
    }
}

impl std::fmt::Display for IsolationLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.description())
    }
}

/// Check if a container runtime is available
fn check_container_runtime() -> bool {
    std::process::Command::new("docker")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
        || std::process::Command::new("podman")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
}

/// Check if KVM is available
fn check_kvm_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new("/dev/kvm").exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Check if hardware isolation features are available
fn check_hardware_isolation() -> bool {
    #[cfg(target_os = "linux")]
    {
        // Check for IOMMU support
        let iommu = std::path::Path::new("/sys/kernel/iommu_groups").exists();
        // Check for Intel CAT (Resource Director Technology)
        let intel_cat = std::path::Path::new("/sys/fs/resctrl").exists();
        iommu || intel_cat
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_level_ordering() {
        assert!(IsolationLevel::L1_MemorySafe < IsolationLevel::L2_Namespaces);
        assert!(IsolationLevel::L7_Hardware < IsolationLevel::L8_AirGap);
    }

    #[test]
    fn test_level_description() {
        assert!(!IsolationLevel::L1_MemorySafe.description().is_empty());
    }

    #[test]
    fn test_l1_always_available() {
        assert!(IsolationLevel::L1_MemorySafe.is_available());
    }
}
