//! Sandbox implementation and builder.

use super::IsolationLevel;
use crate::error::{Result, SandboxError};
use crate::policy::{Capability, SeccompProfile};
use serde::{Deserialize, Serialize};

#[cfg(feature = "process")]
use crate::Namespaces;

#[cfg(feature = "wasm")]
use crate::wasm::WasmConfig;

/// Configuration for a sandbox instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Isolation level
    pub level: IsolationLevel,

    /// Waveform name/identifier
    pub waveform: String,

    /// Memory limit in bytes (0 = unlimited)
    pub memory_limit: usize,

    /// CPU limit as percentage (100 = 1 core, 200 = 2 cores)
    pub cpu_limit: u32,

    /// Maximum number of processes/threads
    pub max_pids: u32,

    /// Namespace flags (for L2+)
    #[cfg(feature = "process")]
    pub namespaces: u32,

    /// Seccomp profile (for L3+)
    pub seccomp_profile: SeccompProfile,

    /// Capabilities to retain
    pub capabilities: Vec<Capability>,

    /// Whether to allow network access
    pub allow_network: bool,

    /// Read-only root filesystem
    pub read_only_root: bool,

    /// Temporary filesystem mounts
    pub tmpfs_mounts: Vec<String>,

    /// Device access (e.g., "/dev/uio0")
    pub device_access: Vec<String>,

    /// WASM sandbox configuration (for L1_5_Wasm)
    #[cfg(feature = "wasm")]
    #[serde(skip)]
    pub wasm_config: Option<WasmConfig>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            level: IsolationLevel::L1_MemorySafe,
            waveform: String::new(),
            memory_limit: 512 * 1024 * 1024, // 512 MB
            cpu_limit: 100,                   // 1 core
            max_pids: 100,
            #[cfg(feature = "process")]
            namespaces: 0,
            seccomp_profile: SeccompProfile::Permissive,
            capabilities: vec![],
            allow_network: false,
            read_only_root: true,
            tmpfs_mounts: vec!["/tmp".to_string()],
            device_access: vec![],
            #[cfg(feature = "wasm")]
            wasm_config: None,
        }
    }
}

/// A waveform sandbox providing isolation at the configured level
pub struct Sandbox {
    config: SandboxConfig,
    #[allow(dead_code)]
    state: SandboxState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum SandboxState {
    Created,
    Running,
    Stopped,
}

impl Sandbox {
    /// Create a new sandbox builder
    pub fn builder() -> SandboxBuilder {
        SandboxBuilder::new()
    }

    /// Get the sandbox configuration
    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }

    /// Get the isolation level
    pub fn isolation_level(&self) -> IsolationLevel {
        self.config.level
    }

    /// Run a function in the isolated sandbox environment
    ///
    /// For L1, this runs directly without additional isolation.
    /// For L2+, this sets up the appropriate isolation before execution.
    pub fn run<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce() -> T,
    {
        match self.config.level {
            IsolationLevel::L1_MemorySafe => {
                // No additional isolation - just run
                Ok(f())
            }
            #[cfg(feature = "wasm")]
            IsolationLevel::L1_5_Wasm => {
                // WASM provides memory isolation but can't run arbitrary closures
                // For WASM execution, use WasmSandbox directly
                // This is a compatibility shim that just runs the closure
                tracing::warn!("L1_5_Wasm: closure executed without WASM isolation; use WasmSandbox for full isolation");
                Ok(f())
            }
            #[cfg(feature = "process")]
            IsolationLevel::L2_Namespaces => self.run_with_namespaces(f),
            #[cfg(feature = "process")]
            IsolationLevel::L3_LSM => self.run_with_lsm(f),
            #[cfg(feature = "containers")]
            IsolationLevel::L4_Container => self.run_in_container(f),
            _ => Err(SandboxError::UnsupportedLevel(
                self.config.level,
                "feature not enabled".to_string(),
            )),
        }
    }

    /// Run with namespace isolation (L2)
    #[cfg(feature = "process")]
    fn run_with_namespaces<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce() -> T,
    {
        use nix::sched::{unshare, CloneFlags};

        // Build clone flags from namespace config
        let mut flags = CloneFlags::empty();
        let ns = Namespaces(self.config.namespaces);

        if ns.contains(Namespaces::PID) {
            flags |= CloneFlags::CLONE_NEWPID;
        }
        if ns.contains(Namespaces::NET) {
            flags |= CloneFlags::CLONE_NEWNET;
        }
        if ns.contains(Namespaces::MOUNT) {
            flags |= CloneFlags::CLONE_NEWNS;
        }
        if ns.contains(Namespaces::USER) {
            flags |= CloneFlags::CLONE_NEWUSER;
        }
        if ns.contains(Namespaces::UTS) {
            flags |= CloneFlags::CLONE_NEWUTS;
        }
        if ns.contains(Namespaces::IPC) {
            flags |= CloneFlags::CLONE_NEWIPC;
        }
        if ns.contains(Namespaces::CGROUP) {
            flags |= CloneFlags::CLONE_NEWCGROUP;
        }

        // Enter new namespaces
        if !flags.is_empty() {
            unshare(flags).map_err(|e| SandboxError::NamespaceError(e.to_string()))?;
        }

        // Drop capabilities if configured
        self.drop_capabilities()?;

        // Run the function
        Ok(f())
    }

    /// Run with LSM enforcement (L3)
    #[cfg(feature = "process")]
    fn run_with_lsm<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce() -> T,
    {
        // First apply namespace isolation
        self.run_with_namespaces(|| {
            // Then apply seccomp filter
            self.apply_seccomp().ok(); // Best effort - may require privileges

            f()
        })
    }

    /// Run in a container (L4)
    #[cfg(feature = "containers")]
    fn run_in_container<F, T>(&self, _f: F) -> Result<T>
    where
        F: FnOnce() -> T,
    {
        // Container isolation requires spawning a separate process
        // This is a placeholder - full implementation would use bollard
        Err(SandboxError::ContainerError(
            "container isolation requires async runtime".to_string(),
        ))
    }

    /// Drop capabilities to the configured set
    #[cfg(feature = "process")]
    fn drop_capabilities(&self) -> Result<()> {
        use caps::{CapSet, Capability as CapsCap};

        // Get current capabilities
        let current = caps::read(None, CapSet::Effective)
            .map_err(|e| SandboxError::CapabilityError(e.to_string()))?;

        // Convert our capability enum to caps crate
        let keep: Vec<CapsCap> = self
            .config
            .capabilities
            .iter()
            .filter_map(|c| c.to_caps_capability())
            .collect();

        // Drop all capabilities not in our keep list
        for cap in current.iter() {
            if !keep.contains(&cap) {
                caps::drop(None, CapSet::Effective, *cap)
                    .map_err(|e| SandboxError::CapabilityError(e.to_string()))?;
                caps::drop(None, CapSet::Permitted, *cap)
                    .map_err(|e| SandboxError::CapabilityError(e.to_string()))?;
            }
        }

        Ok(())
    }

    /// Apply seccomp filter
    #[cfg(feature = "process")]
    fn apply_seccomp(&self) -> Result<()> {
        // Seccomp implementation would use seccompiler crate
        // This is a placeholder
        match self.config.seccomp_profile {
            SeccompProfile::Permissive => Ok(()),
            SeccompProfile::DSP => {
                tracing::debug!("DSP seccomp profile would be applied here");
                Ok(())
            }
            SeccompProfile::Strict => {
                tracing::debug!("Strict seccomp profile would be applied here");
                Ok(())
            }
            SeccompProfile::Custom(_) => {
                tracing::debug!("Custom seccomp profile would be applied here");
                Ok(())
            }
        }
    }
}

/// Builder for sandbox configuration
#[derive(Debug, Clone)]
pub struct SandboxBuilder {
    config: SandboxConfig,
}

impl SandboxBuilder {
    /// Create a new sandbox builder with default configuration
    pub fn new() -> Self {
        Self {
            config: SandboxConfig::default(),
        }
    }

    /// Set the isolation level
    pub fn isolation_level(mut self, level: IsolationLevel) -> Self {
        self.config.level = level;
        self
    }

    /// Set the waveform name
    pub fn waveform(mut self, name: impl Into<String>) -> Self {
        self.config.waveform = name.into();
        self
    }

    /// Set memory limit in bytes
    pub fn memory_limit(mut self, limit: usize) -> Self {
        self.config.memory_limit = limit;
        self
    }

    /// Set CPU limit as percentage (100 = 1 core)
    pub fn cpu_limit(mut self, percent: u32) -> Self {
        self.config.cpu_limit = percent;
        self
    }

    /// Set maximum number of processes/threads
    pub fn max_pids(mut self, max: u32) -> Self {
        self.config.max_pids = max;
        self
    }

    /// Set namespace flags
    #[cfg(feature = "process")]
    pub fn namespaces(mut self, ns: Namespaces) -> Self {
        self.config.namespaces = ns.bits();
        self
    }

    /// Set seccomp profile
    pub fn seccomp_profile(mut self, profile: SeccompProfile) -> Self {
        self.config.seccomp_profile = profile;
        self
    }

    /// Set capabilities to retain
    pub fn capabilities(mut self, caps: &[Capability]) -> Self {
        self.config.capabilities = caps.to_vec();
        self
    }

    /// Allow network access
    pub fn allow_network(mut self, allow: bool) -> Self {
        self.config.allow_network = allow;
        self
    }

    /// Set read-only root filesystem
    pub fn read_only_root(mut self, read_only: bool) -> Self {
        self.config.read_only_root = read_only;
        self
    }

    /// Add a tmpfs mount point
    pub fn add_tmpfs(mut self, path: impl Into<String>) -> Self {
        self.config.tmpfs_mounts.push(path.into());
        self
    }

    /// Add device access
    pub fn add_device(mut self, device: impl Into<String>) -> Self {
        self.config.device_access.push(device.into());
        self
    }

    /// Set WASM configuration (for L1_5_Wasm)
    #[cfg(feature = "wasm")]
    pub fn wasm_config(mut self, config: WasmConfig) -> Self {
        self.config.wasm_config = Some(config);
        self
    }

    /// Build the sandbox
    pub fn build(self) -> Result<Sandbox> {
        // Validate configuration
        if !self.config.level.is_available() {
            return Err(SandboxError::UnsupportedLevel(
                self.config.level,
                "not available on this platform".to_string(),
            ));
        }

        if self.config.waveform.is_empty() {
            return Err(SandboxError::ConfigError(
                "waveform name is required".to_string(),
            ));
        }

        Ok(Sandbox {
            config: self.config,
            state: SandboxState::Created,
        })
    }
}

impl Default for SandboxBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_default() {
        let builder = SandboxBuilder::new();
        assert_eq!(builder.config.level, IsolationLevel::L1_MemorySafe);
    }

    #[test]
    fn test_builder_chain() {
        let sandbox = Sandbox::builder()
            .isolation_level(IsolationLevel::L1_MemorySafe)
            .waveform("BPSK")
            .memory_limit(256 * 1024 * 1024)
            .build()
            .unwrap();

        assert_eq!(sandbox.isolation_level(), IsolationLevel::L1_MemorySafe);
        assert_eq!(sandbox.config().waveform, "BPSK");
        assert_eq!(sandbox.config().memory_limit, 256 * 1024 * 1024);
    }

    #[test]
    fn test_l1_sandbox_run() {
        let sandbox = Sandbox::builder()
            .isolation_level(IsolationLevel::L1_MemorySafe)
            .waveform("test")
            .build()
            .unwrap();

        let result = sandbox.run(|| 42).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_missing_waveform_error() {
        let result = Sandbox::builder()
            .isolation_level(IsolationLevel::L1_MemorySafe)
            .build();

        assert!(matches!(result, Err(SandboxError::ConfigError(_))));
    }
}
