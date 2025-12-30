//! WASM runtime implementation using wasmtime.
//!
//! Uses wasmtime-wasi preview1 for compatibility with standard WASM modules.

use super::config::{WasmConfig, WasiCapabilities};
use super::host_functions::DspHostFunctions;
use crate::error::{Result, SandboxError};

use std::path::Path;
use std::time::Instant;

use wasmtime::*;

/// Host state for the WASM store, containing WASI preview1 context.
pub struct WasmHostState {
    preview1: wasmtime_wasi::preview1::WasiP1Ctx,
    limits: StoreLimits,
}

impl WasmHostState {
    /// Get the preview1 context.
    fn preview1(&mut self) -> &mut wasmtime_wasi::preview1::WasiP1Ctx {
        &mut self.preview1
    }
}

/// A WebAssembly sandbox for running isolated waveform code.
pub struct WasmSandbox {
    engine: Engine,
    config: WasmConfig,
}

/// A compiled WebAssembly module.
pub struct WasmModule {
    module: Module,
    #[allow(dead_code)]
    name: String,
}

/// An instantiated WebAssembly module ready for execution.
pub struct WasmInstance {
    store: Store<WasmHostState>,
    instance: Instance,
}

/// Result of a WASM function call with timing information.
#[derive(Debug, Clone)]
pub struct WasmCallResult<T> {
    /// The return value
    pub value: T,
    /// Execution time in microseconds
    pub execution_time_us: u64,
    /// Fuel consumed (if fuel metering enabled)
    pub fuel_consumed: Option<u64>,
}

impl WasmSandbox {
    /// Create a new WASM sandbox with the given configuration.
    pub fn new(config: WasmConfig) -> Result<Self> {
        let mut engine_config = Config::new();

        // Configure optimization
        engine_config.cranelift_opt_level(match config.optimization_level {
            0 => OptLevel::None,
            1 => OptLevel::Speed,
            _ => OptLevel::Speed, // wasmtime doesn't have SpeedAndSize as separate
        });

        // Enable SIMD if requested
        engine_config.wasm_simd(config.enable_simd);

        // Enable threads if requested
        engine_config.wasm_threads(config.enable_threads);

        // Enable memory64 if requested
        engine_config.wasm_memory64(config.enable_memory64);

        // Enable fuel metering if configured
        if config.fuel_limit.is_some() {
            engine_config.consume_fuel(true);
        }

        // Enable epoch interruption if configured
        if config.epoch_interruption {
            engine_config.epoch_interruption(true);
        }

        // Configure caching if path provided
        if let Some(ref cache_path) = config.cache_path {
            if let Err(e) = engine_config.cache_config_load(cache_path) {
                tracing::warn!("Failed to load cache config: {}", e);
            }
        }

        let engine = Engine::new(&engine_config)
            .map_err(|e| SandboxError::WasmError(format!("engine creation failed: {}", e)))?;

        Ok(Self { engine, config })
    }

    /// Load a WASM module from a file.
    pub fn load_module(&self, path: impl AsRef<Path>) -> Result<WasmModule> {
        let path = path.as_ref();
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let module = Module::from_file(&self.engine, path)
            .map_err(|e| SandboxError::WasmError(format!("module load failed: {}", e)))?;

        Ok(WasmModule { module, name })
    }

    /// Load a WASM module from bytes.
    pub fn load_module_bytes(&self, name: &str, bytes: &[u8]) -> Result<WasmModule> {
        let module = Module::new(&self.engine, bytes)
            .map_err(|e| SandboxError::WasmError(format!("module creation failed: {}", e)))?;

        Ok(WasmModule {
            module,
            name: name.to_string(),
        })
    }

    /// Instantiate a module with WASI context.
    pub fn instantiate(&self, module: &WasmModule) -> Result<WasmInstance> {
        let host_state = self.build_host_state(&self.config.capabilities)?;
        let mut store = Store::new(&self.engine, host_state);

        // Configure resource limits via the stored limiter
        store.limiter(|state| &mut state.limits);

        // Add fuel if configured
        if let Some(fuel) = self.config.fuel_limit {
            store
                .set_fuel(fuel)
                .map_err(|e| SandboxError::WasmError(format!("fuel setup failed: {}", e)))?;
        }

        // Create linker and add WASI preview1 functions
        let mut linker: Linker<WasmHostState> = Linker::new(&self.engine);
        wasmtime_wasi::preview1::add_to_linker_sync(&mut linker, |state| state.preview1())
            .map_err(|e| SandboxError::WasmError(format!("WASI link failed: {}", e)))?;

        // Register DSP host functions (r4w_dsp namespace)
        DspHostFunctions::register(&mut linker)?;

        // Instantiate the module
        let instance = linker
            .instantiate(&mut store, &module.module)
            .map_err(|e| SandboxError::WasmError(format!("instantiation failed: {}", e)))?;

        Ok(WasmInstance { store, instance })
    }

    /// Build host state from capabilities.
    fn build_host_state(&self, caps: &WasiCapabilities) -> Result<WasmHostState> {
        let mut builder = wasmtime_wasi::WasiCtxBuilder::new();

        // Configure stdio
        if caps.stdin {
            builder.inherit_stdin();
        }
        if caps.stdout {
            builder.inherit_stdout();
        }
        if caps.stderr {
            builder.inherit_stderr();
        }

        // Add environment variables
        for (key, value) in &caps.env_vars {
            builder.env(key, value);
        }

        // Add arguments
        builder.args(&caps.args);

        // Add preopened directories (read-only)
        for dir in &caps.preopened_dirs_ro {
            builder
                .preopened_dir(
                    dir,
                    dir.to_string_lossy(),
                    wasmtime_wasi::DirPerms::READ,
                    wasmtime_wasi::FilePerms::READ,
                )
                .map_err(|e| {
                    SandboxError::WasmError(format!("failed to open dir {:?}: {}", dir, e))
                })?;
        }

        // Add preopened directories (read-write)
        for dir in &caps.preopened_dirs_rw {
            builder
                .preopened_dir(
                    dir,
                    dir.to_string_lossy(),
                    wasmtime_wasi::DirPerms::all(),
                    wasmtime_wasi::FilePerms::all(),
                )
                .map_err(|e| {
                    SandboxError::WasmError(format!("failed to open dir {:?}: {}", dir, e))
                })?;
        }

        // Build the preview2 context and wrap it for preview1 compatibility
        let preview1 = builder.build_p1();

        // Create resource limits
        let limits = StoreLimitsBuilder::new()
            .memory_size(self.config.max_memory)
            .build();

        Ok(WasmHostState { preview1, limits })
    }

    /// Get the configuration.
    pub fn config(&self) -> &WasmConfig {
        &self.config
    }
}

impl WasmModule {
    /// Get the module name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get exported function names.
    pub fn exports(&self) -> impl Iterator<Item = &str> {
        self.module.exports().filter_map(|e| {
            if matches!(e.ty(), ExternType::Func(_)) {
                Some(e.name())
            } else {
                None
            }
        })
    }
}

impl WasmInstance {
    /// Call a function that takes no arguments and returns an i32.
    pub fn call_i32(&mut self, name: &str) -> Result<WasmCallResult<i32>> {
        let func = self.get_typed_func::<(), i32>(name)?;
        let start = Instant::now();
        let fuel_before = self.store.get_fuel().ok();

        let value = func
            .call(&mut self.store, ())
            .map_err(|e| SandboxError::WasmError(format!("call failed: {}", e)))?;

        let fuel_after = self.store.get_fuel().ok();
        let fuel_consumed = fuel_before.zip(fuel_after).map(|(b, a)| b - a);

        Ok(WasmCallResult {
            value,
            execution_time_us: start.elapsed().as_micros() as u64,
            fuel_consumed,
        })
    }

    /// Call a function that takes an i32 and returns an i32.
    pub fn call_i32_i32(&mut self, name: &str, arg: i32) -> Result<WasmCallResult<i32>> {
        let func = self.get_typed_func::<i32, i32>(name)?;
        let start = Instant::now();
        let fuel_before = self.store.get_fuel().ok();

        let value = func
            .call(&mut self.store, arg)
            .map_err(|e| SandboxError::WasmError(format!("call failed: {}", e)))?;

        let fuel_after = self.store.get_fuel().ok();
        let fuel_consumed = fuel_before.zip(fuel_after).map(|(b, a)| b - a);

        Ok(WasmCallResult {
            value,
            execution_time_us: start.elapsed().as_micros() as u64,
            fuel_consumed,
        })
    }

    /// Call a function that takes an i32 and returns nothing.
    pub fn call_void_i32(&mut self, name: &str, arg: i32) -> Result<WasmCallResult<()>> {
        let func = self.get_typed_func::<i32, ()>(name)?;
        let start = Instant::now();
        let fuel_before = self.store.get_fuel().ok();

        func.call(&mut self.store, arg)
            .map_err(|e| SandboxError::WasmError(format!("call failed: {}", e)))?;

        let fuel_after = self.store.get_fuel().ok();
        let fuel_consumed = fuel_before.zip(fuel_after).map(|(b, a)| b - a);

        Ok(WasmCallResult {
            value: (),
            execution_time_us: start.elapsed().as_micros() as u64,
            fuel_consumed,
        })
    }

    /// Call a function with two i32 args and returns i32.
    pub fn call_i32_i32_i32(&mut self, name: &str, a: i32, b: i32) -> Result<WasmCallResult<i32>> {
        let func = self.get_typed_func::<(i32, i32), i32>(name)?;
        let start = Instant::now();
        let fuel_before = self.store.get_fuel().ok();

        let value = func
            .call(&mut self.store, (a, b))
            .map_err(|e| SandboxError::WasmError(format!("call failed: {}", e)))?;

        let fuel_after = self.store.get_fuel().ok();
        let fuel_consumed = fuel_before.zip(fuel_after).map(|(b, a)| b - a);

        Ok(WasmCallResult {
            value,
            execution_time_us: start.elapsed().as_micros() as u64,
            fuel_consumed,
        })
    }

    /// Call a function with two i32 args (pointer, length) and returns i32 (result pointer).
    /// Useful for processing arrays like sample buffers.
    pub fn call_buffer(&mut self, name: &str, ptr: i32, len: i32) -> Result<WasmCallResult<i32>> {
        let func = self.get_typed_func::<(i32, i32), i32>(name)?;
        let start = Instant::now();
        let fuel_before = self.store.get_fuel().ok();

        let value = func
            .call(&mut self.store, (ptr, len))
            .map_err(|e| SandboxError::WasmError(format!("call failed: {}", e)))?;

        let fuel_after = self.store.get_fuel().ok();
        let fuel_consumed = fuel_before.zip(fuel_after).map(|(b, a)| b - a);

        Ok(WasmCallResult {
            value,
            execution_time_us: start.elapsed().as_micros() as u64,
            fuel_consumed,
        })
    }

    /// Get a typed function from the instance.
    fn get_typed_func<P, R>(&mut self, name: &str) -> Result<TypedFunc<P, R>>
    where
        P: WasmParams,
        R: WasmResults,
    {
        self.instance
            .get_typed_func::<P, R>(&mut self.store, name)
            .map_err(|e| SandboxError::WasmError(format!("function '{}' not found: {}", name, e)))
    }

    /// Write bytes to WASM memory at the given offset.
    pub fn write_memory(&mut self, offset: usize, data: &[u8]) -> Result<()> {
        let memory = self.get_memory()?;
        let mem_data = memory.data_mut(&mut self.store);

        if offset + data.len() > mem_data.len() {
            return Err(SandboxError::WasmError(
                "memory write out of bounds".to_string(),
            ));
        }

        mem_data[offset..offset + data.len()].copy_from_slice(data);
        Ok(())
    }

    /// Read bytes from WASM memory at the given offset.
    pub fn read_memory(&mut self, offset: usize, len: usize) -> Result<Vec<u8>> {
        let memory = self.get_memory()?;
        let mem_data = memory.data(&self.store);

        if offset + len > mem_data.len() {
            return Err(SandboxError::WasmError(
                "memory read out of bounds".to_string(),
            ));
        }

        Ok(mem_data[offset..offset + len].to_vec())
    }

    /// Get the memory export.
    fn get_memory(&mut self) -> Result<Memory> {
        self.instance
            .get_memory(&mut self.store, "memory")
            .ok_or_else(|| SandboxError::WasmError("no memory export found".to_string()))
    }

    /// Allocate memory in the WASM module (requires module to export `alloc`).
    pub fn alloc(&mut self, size: i32) -> Result<i32> {
        let result = self.call_i32_i32("alloc", size)?;
        Ok(result.value)
    }

    /// Free memory in the WASM module (requires module to export `dealloc`).
    pub fn dealloc(&mut self, ptr: i32, size: i32) -> Result<()> {
        let func = self.get_typed_func::<(i32, i32), ()>("dealloc")?;
        func.call(&mut self.store, (ptr, size))
            .map_err(|e| SandboxError::WasmError(format!("dealloc failed: {}", e)))?;
        Ok(())
    }

    /// Get remaining fuel (if fuel metering enabled).
    pub fn remaining_fuel(&self) -> Option<u64> {
        self.store.get_fuel().ok()
    }

    /// Call a function with three i32 args that returns i32.
    pub fn call_i32_i32_i32_i32(
        &mut self,
        name: &str,
        a: i32,
        b: i32,
        c: i32,
    ) -> Result<WasmCallResult<i32>> {
        let func = self.get_typed_func::<(i32, i32, i32), i32>(name)?;
        let start = Instant::now();
        let fuel_before = self.store.get_fuel().ok();

        let value = func
            .call(&mut self.store, (a, b, c))
            .map_err(|e| SandboxError::WasmError(format!("call failed: {}", e)))?;

        let fuel_after = self.store.get_fuel().ok();
        let fuel_consumed = fuel_before.zip(fuel_after).map(|(b, a)| b - a);

        Ok(WasmCallResult {
            value,
            execution_time_us: start.elapsed().as_micros() as u64,
            fuel_consumed,
        })
    }

    /// Get list of exported function names.
    pub fn exported_functions(&mut self) -> Vec<String> {
        // Collect names first to avoid borrow issues
        let names: Vec<_> = self.instance.exports(&mut self.store).map(|e| e.name().to_string()).collect();
        // Now check types
        names
            .into_iter()
            .filter(|name| {
                self.instance
                    .get_func(&mut self.store, name)
                    .is_some()
            })
            .collect()
    }
}

/// Benchmark helper for measuring WASM execution overhead.
pub struct WasmBenchmark {
    samples: Vec<u64>,
}

impl WasmBenchmark {
    /// Create a new benchmark collector.
    pub fn new() -> Self {
        Self { samples: Vec::new() }
    }

    /// Record a timing sample in microseconds.
    pub fn record(&mut self, us: u64) {
        self.samples.push(us);
    }

    /// Get the number of samples.
    pub fn count(&self) -> usize {
        self.samples.len()
    }

    /// Get the mean execution time in microseconds.
    pub fn mean_us(&self) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        self.samples.iter().sum::<u64>() as f64 / self.samples.len() as f64
    }

    /// Get the p50 (median) execution time in microseconds.
    pub fn p50_us(&self) -> u64 {
        self.percentile(50)
    }

    /// Get the p99 execution time in microseconds.
    pub fn p99_us(&self) -> u64 {
        self.percentile(99)
    }

    /// Get a percentile value.
    pub fn percentile(&self, p: usize) -> u64 {
        if self.samples.is_empty() {
            return 0;
        }
        let mut sorted = self.samples.clone();
        sorted.sort();
        let idx = (p * sorted.len() / 100).min(sorted.len() - 1);
        sorted[idx]
    }

    /// Get min execution time.
    pub fn min_us(&self) -> u64 {
        self.samples.iter().copied().min().unwrap_or(0)
    }

    /// Get max execution time.
    pub fn max_us(&self) -> u64 {
        self.samples.iter().copied().max().unwrap_or(0)
    }

    /// Print a summary of the benchmark results.
    pub fn summary(&self) -> String {
        format!(
            "n={} min={}us mean={:.1}us p50={}us p99={}us max={}us",
            self.count(),
            self.min_us(),
            self.mean_us(),
            self.p50_us(),
            self.p99_us(),
            self.max_us()
        )
    }
}

impl Default for WasmBenchmark {
    fn default() -> Self {
        Self::new()
    }
}
