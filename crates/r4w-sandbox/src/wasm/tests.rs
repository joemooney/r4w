//! Tests for WASM sandbox isolation.

use super::*;

#[test]
fn test_wasm_config_default() {
    let config = WasmConfig::default();
    assert_eq!(config.max_memory, 256 * 1024 * 1024);
    assert!(config.enable_simd);
    assert!(!config.enable_threads);
}

#[test]
fn test_wasm_config_dsp() {
    let config = WasmConfig::dsp();
    assert_eq!(config.max_memory, 512 * 1024 * 1024);
    assert!(config.enable_simd);
    assert_eq!(config.optimization_level, 3);
}

#[test]
fn test_wasm_config_minimal() {
    let config = WasmConfig::minimal();
    assert_eq!(config.max_memory, 64 * 1024 * 1024);
    assert!(config.fuel_limit.is_some());
}

#[test]
fn test_wasi_capabilities_none() {
    let caps = WasiCapabilities::none();
    assert!(!caps.stdin);
    assert!(!caps.stdout);
    assert!(!caps.stderr);
    assert!(!caps.network);
    assert!(caps.preopened_dirs_ro.is_empty());
    assert!(caps.preopened_dirs_rw.is_empty());
}

#[test]
fn test_wasi_capabilities_dsp() {
    let caps = WasiCapabilities::dsp();
    assert!(!caps.stdin);
    assert!(caps.stdout);
    assert!(caps.stderr);
    assert!(caps.clocks);
    assert!(caps.random);
    assert!(!caps.network);
}

#[test]
fn test_wasi_capabilities_builder() {
    let caps = WasiCapabilities::none()
        .stdin(true)
        .stdout(true)
        .stderr(true)
        .env("DEBUG", "1")
        .arg("--verbose");

    assert!(caps.stdin);
    assert!(caps.stdout);
    assert!(caps.stderr);
    assert_eq!(caps.env_vars.len(), 1);
    assert_eq!(caps.args.len(), 1);
}

#[test]
fn test_sandbox_creation() {
    let config = WasmConfig::default();
    let sandbox = WasmSandbox::new(config);
    assert!(sandbox.is_ok());
}

#[test]
fn test_benchmark_empty() {
    let bench = WasmBenchmark::new();
    assert_eq!(bench.count(), 0);
    assert_eq!(bench.mean_us(), 0.0);
    assert_eq!(bench.p50_us(), 0);
    assert_eq!(bench.p99_us(), 0);
}

#[test]
fn test_benchmark_samples() {
    let mut bench = WasmBenchmark::new();
    bench.record(100);
    bench.record(200);
    bench.record(300);

    assert_eq!(bench.count(), 3);
    assert_eq!(bench.min_us(), 100);
    assert_eq!(bench.max_us(), 300);
    assert!((bench.mean_us() - 200.0).abs() < 0.001);
}

#[test]
fn test_benchmark_percentiles() {
    let mut bench = WasmBenchmark::new();
    for i in 1..=100 {
        bench.record(i);
    }

    assert_eq!(bench.count(), 100);
    // p50 of 1-100 is at index 50 which is value 51 (0-indexed)
    assert_eq!(bench.p50_us(), 51);
    assert_eq!(bench.p99_us(), 100);
}

#[test]
fn test_benchmark_summary() {
    let mut bench = WasmBenchmark::new();
    bench.record(10);
    bench.record(20);

    let summary = bench.summary();
    assert!(summary.contains("n=2"));
    assert!(summary.contains("min=10us"));
    assert!(summary.contains("max=20us"));
}

// Integration tests requiring actual WASM modules would go here
// These would need a test .wasm file to be compiled separately

#[test]
fn test_wasm_config_builder_chain() {
    let config = WasmConfig::default()
        .max_memory(128 * 1024 * 1024)
        .fuel_limit(500_000_000)
        .simd(true)
        .optimize(3);

    assert_eq!(config.max_memory, 128 * 1024 * 1024);
    assert_eq!(config.fuel_limit, Some(500_000_000));
    assert!(config.enable_simd);
    assert_eq!(config.optimization_level, 3);
}
