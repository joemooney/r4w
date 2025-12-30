//! Integration tests for WASM sandbox with a real waveform module.
//!
//! Tests load the r4w_wasm_test_waveform.wasm module and exercise its functions.

use r4w_sandbox::{WasmSandbox, WasmConfig, WasiCapabilities};

const WASM_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/r4w_wasm_test_waveform.wasm");

#[test]
fn test_load_waveform_module() {
    let sandbox = WasmSandbox::new(WasmConfig::default()).expect("failed to create sandbox");
    let module = sandbox.load_module(WASM_PATH).expect("failed to load module");

    assert_eq!(module.name(), "r4w_wasm_test_waveform");

    // Check exported functions
    let exports: Vec<&str> = module.exports().collect();
    assert!(exports.contains(&"add"), "should export 'add' function");
    assert!(exports.contains(&"multiply"), "should export 'multiply' function");
    assert!(exports.contains(&"version"), "should export 'version' function");
    assert!(exports.contains(&"bits_per_symbol"), "should export 'bits_per_symbol' function");
}

#[test]
fn test_instantiate_and_call_simple() {
    let sandbox = WasmSandbox::new(WasmConfig::default()).expect("failed to create sandbox");
    let module = sandbox.load_module(WASM_PATH).expect("failed to load module");
    let mut instance = sandbox.instantiate(&module).expect("failed to instantiate");

    // Test version()
    let result = instance.call_i32("version").expect("version call failed");
    assert_eq!(result.value, 1);
    println!("version() = {} ({}us)", result.value, result.execution_time_us);

    // Test bits_per_symbol()
    let result = instance.call_i32("bits_per_symbol").expect("bits_per_symbol call failed");
    assert_eq!(result.value, 1); // BPSK has 1 bit per symbol
    println!("bits_per_symbol() = {} ({}us)", result.value, result.execution_time_us);

    // Test sample_rate()
    let result = instance.call_i32("sample_rate").expect("sample_rate call failed");
    assert_eq!(result.value, 48000);
    println!("sample_rate() = {} ({}us)", result.value, result.execution_time_us);

    // Test samples_per_symbol()
    let result = instance.call_i32("samples_per_symbol").expect("samples_per_symbol call failed");
    assert_eq!(result.value, 48);
    println!("samples_per_symbol() = {} ({}us)", result.value, result.execution_time_us);
}

#[test]
fn test_add_and_multiply() {
    let sandbox = WasmSandbox::new(WasmConfig::default()).expect("failed to create sandbox");
    let module = sandbox.load_module(WASM_PATH).expect("failed to load module");
    let mut instance = sandbox.instantiate(&module).expect("failed to instantiate");

    // Test add(3, 5) = 8
    let result = instance.call_i32_i32_i32("add", 3, 5).expect("add call failed");
    assert_eq!(result.value, 8);
    println!("add(3, 5) = {} ({}us)", result.value, result.execution_time_us);

    // Test add(100, -50) = 50
    let result = instance.call_i32_i32_i32("add", 100, -50).expect("add call failed");
    assert_eq!(result.value, 50);

    // Test multiply(7, 6) = 42
    let result = instance.call_i32_i32_i32("multiply", 7, 6).expect("multiply call failed");
    assert_eq!(result.value, 42);
    println!("multiply(7, 6) = {} ({}us)", result.value, result.execution_time_us);

    // Test multiply(-3, 4) = -12
    let result = instance.call_i32_i32_i32("multiply", -3, 4).expect("multiply call failed");
    assert_eq!(result.value, -12);
}

#[test]
fn test_waveform_name() {
    let sandbox = WasmSandbox::new(WasmConfig::default()).expect("failed to create sandbox");
    let module = sandbox.load_module(WASM_PATH).expect("failed to load module");
    let mut instance = sandbox.instantiate(&module).expect("failed to instantiate");

    // Get name length
    let len_result = instance.call_i32("waveform_name_len").expect("waveform_name_len call failed");
    assert_eq!(len_result.value, 8); // "TestBPSK" = 8 chars

    // Allocate buffer for name
    let ptr = instance.alloc(len_result.value).expect("alloc failed");

    // Get name into buffer (returns void)
    instance.call_void_i32("waveform_name", ptr).expect("waveform_name call failed");

    // Read name from memory
    let name_bytes = instance.read_memory(ptr as usize, len_result.value as usize)
        .expect("read_memory failed");
    let name = String::from_utf8(name_bytes).expect("invalid UTF-8");

    assert_eq!(name, "TestBPSK");
    println!("waveform_name = \"{}\"", name);
}

#[test]
fn test_modulate_bit() {
    let sandbox = WasmSandbox::new(WasmConfig::default()).expect("failed to create sandbox");
    let module = sandbox.load_module(WASM_PATH).expect("failed to load module");
    let mut instance = sandbox.instantiate(&module).expect("failed to instantiate");

    // Modulate bit 0
    let ptr_result = instance.call_i32_i32("modulate_bit", 0).expect("modulate_bit call failed");
    let ptr = ptr_result.value;
    println!("modulate_bit(0) returned ptr={} ({}us)", ptr, ptr_result.execution_time_us);

    // Read first few I/Q samples
    let sps = 48;
    let size = sps * 2 * 4; // 48 samples * (I + Q) * sizeof(f32)
    let bytes = instance.read_memory(ptr as usize, size).expect("read_memory failed");

    // Parse first sample (I, Q)
    let i0 = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let q0 = f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);

    println!("First sample: I={:.4}, Q={:.4}", i0, q0);

    // For bit 0 with phase 0, first sample should be near (1, 0)
    assert!((i0 - 1.0).abs() < 0.01, "I should be ~1.0, got {}", i0);
    assert!(q0.abs() < 0.01, "Q should be ~0.0, got {}", q0);

    // Modulate bit 1
    let ptr_result = instance.call_i32_i32("modulate_bit", 1).expect("modulate_bit call failed");
    let ptr = ptr_result.value;

    let bytes = instance.read_memory(ptr as usize, size).expect("read_memory failed");
    let i0 = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let q0 = f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);

    println!("Bit 1 - First sample: I={:.4}, Q={:.4}", i0, q0);

    // For bit 1 with phase π, first sample should be near (-1, 0)
    assert!((i0 - (-1.0)).abs() < 0.01, "I should be ~-1.0, got {}", i0);
    assert!(q0.abs() < 0.01, "Q should be ~0.0, got {}", q0);
}

#[test]
fn test_memory_operations() {
    let sandbox = WasmSandbox::new(WasmConfig::default()).expect("failed to create sandbox");
    let module = sandbox.load_module(WASM_PATH).expect("failed to load module");
    let mut instance = sandbox.instantiate(&module).expect("failed to instantiate");

    // Allocate 16 bytes
    let ptr = instance.alloc(16).expect("alloc failed");
    assert!(ptr > 0, "pointer should be valid");

    // Write some data
    let data: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    instance.write_memory(ptr as usize, &data).expect("write_memory failed");

    // Read it back
    let read_data = instance.read_memory(ptr as usize, 16).expect("read_memory failed");
    assert_eq!(read_data, data);

    println!("Memory read/write test passed");
}

#[test]
fn test_fuel_metering() {
    // Create sandbox with fuel metering
    let config = WasmConfig::default().fuel_limit(1_000_000);
    let sandbox = WasmSandbox::new(config).expect("failed to create sandbox");
    let module = sandbox.load_module(WASM_PATH).expect("failed to load module");
    let mut instance = sandbox.instantiate(&module).expect("failed to instantiate");

    // Check initial fuel
    let initial_fuel = instance.remaining_fuel().expect("fuel should be available");
    assert_eq!(initial_fuel, 1_000_000);

    // Call a function
    let result = instance.call_i32_i32_i32("add", 1, 2).expect("add call failed");
    assert_eq!(result.value, 3);

    // Fuel should have been consumed
    let remaining_fuel = instance.remaining_fuel().expect("fuel should be available");
    assert!(remaining_fuel < initial_fuel, "fuel should have been consumed");

    let consumed = result.fuel_consumed.expect("fuel_consumed should be Some");
    assert!(consumed > 0, "should have consumed some fuel");

    println!("Fuel: initial={}, remaining={}, consumed={}",
             initial_fuel, remaining_fuel, consumed);
}

#[test]
fn test_dsp_config_preset() {
    let config = WasmConfig::dsp();

    // DSP config should have larger memory and SIMD enabled
    assert_eq!(config.max_memory, 512 * 1024 * 1024);
    assert!(config.enable_simd);
    assert_eq!(config.optimization_level, 3);

    let sandbox = WasmSandbox::new(config).expect("failed to create sandbox");
    let module = sandbox.load_module(WASM_PATH).expect("failed to load module");
    let mut instance = sandbox.instantiate(&module).expect("failed to instantiate");

    // Should work the same
    let result = instance.call_i32("version").expect("version call failed");
    assert_eq!(result.value, 1);
}

#[test]
fn test_minimal_capabilities() {
    // Create with no capabilities - most restrictive
    let config = WasmConfig::minimal();
    let sandbox = WasmSandbox::new(config).expect("failed to create sandbox");
    let module = sandbox.load_module(WASM_PATH).expect("failed to load module");
    let mut instance = sandbox.instantiate(&module).expect("failed to instantiate");

    // Should still work - our module doesn't need filesystem/network
    let result = instance.call_i32_i32_i32("multiply", 11, 11).expect("multiply call failed");
    assert_eq!(result.value, 121);
}

#[test]
fn test_benchmark_multiple_calls() {
    use r4w_sandbox::WasmBenchmark;

    let sandbox = WasmSandbox::new(WasmConfig::dsp()).expect("failed to create sandbox");
    let module = sandbox.load_module(WASM_PATH).expect("failed to load module");
    let mut instance = sandbox.instantiate(&module).expect("failed to instantiate");

    let mut bench = WasmBenchmark::new();

    // Warm up
    for _ in 0..10 {
        let _ = instance.call_i32_i32_i32("add", 1, 1);
    }

    // Benchmark
    for i in 0..100 {
        let result = instance.call_i32_i32_i32("add", i, i).expect("add call failed");
        bench.record(result.execution_time_us);
    }

    println!("Benchmark: {}", bench.summary());

    assert_eq!(bench.count(), 100);
    assert!(bench.mean_us() < 1000.0, "mean should be under 1ms"); // Should be very fast
}
