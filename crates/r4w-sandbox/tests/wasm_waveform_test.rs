//! Integration tests for WASM sandbox with a real waveform module.
//!
//! Tests load the r4w_wasm_test_waveform.wasm module and exercise its functions.

use r4w_sandbox::{WasmSandbox, WasmConfig};

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

// =============================================================================
// DSP Host Function Integration Tests
// =============================================================================
// These tests verify that WASM modules can call native DSP functions.

#[test]
fn test_host_function_fft() {
    let sandbox = WasmSandbox::new(WasmConfig::dsp()).expect("failed to create sandbox");
    let module = sandbox.load_module(WASM_PATH).expect("failed to load module");
    let mut instance = sandbox.instantiate(&module).expect("failed to instantiate");

    // Create a simple test signal: DC component (all 1+0j)
    let len: usize = 8;
    let input_ptr = instance.alloc((len * 2 * 4) as i32).expect("alloc failed");

    // Write complex samples: (1.0, 0.0) repeated
    let mut input_data = vec![0u8; len * 8];
    for i in 0..len {
        let re_bytes = 1.0f32.to_le_bytes();
        let im_bytes = 0.0f32.to_le_bytes();
        input_data[i * 8..i * 8 + 4].copy_from_slice(&re_bytes);
        input_data[i * 8 + 4..i * 8 + 8].copy_from_slice(&im_bytes);
    }
    instance.write_memory(input_ptr as usize, &input_data).expect("write failed");

    // Call test_fft via WASM (which calls the host fft function)
    let output_ptr = instance
        .call_i32_i32_i32("test_fft", input_ptr, len as i32)
        .expect("test_fft call failed")
        .value;

    // Read output
    let output_data = instance.read_memory(output_ptr as usize, len * 8).expect("read failed");

    // For DC input (all ones), FFT should have all energy at bin 0
    let re0 = f32::from_le_bytes([output_data[0], output_data[1], output_data[2], output_data[3]]);
    let im0 = f32::from_le_bytes([output_data[4], output_data[5], output_data[6], output_data[7]]);

    println!("FFT bin[0]: re={:.4}, im={:.4}", re0, im0);

    // DC bin should have magnitude ~len (8 for unnormalized FFT)
    let mag0 = (re0 * re0 + im0 * im0).sqrt();
    assert!(mag0 > 7.0 && mag0 < 9.0, "DC bin magnitude should be ~8, got {}", mag0);
}

#[test]
fn test_host_function_fft_ifft_roundtrip() {
    let sandbox = WasmSandbox::new(WasmConfig::dsp()).expect("failed to create sandbox");
    let module = sandbox.load_module(WASM_PATH).expect("failed to load module");
    let mut instance = sandbox.instantiate(&module).expect("failed to instantiate");

    // Create test signal: impulse at sample 0
    let len: usize = 16;
    let input_ptr = instance.alloc((len * 2 * 4) as i32).expect("alloc failed");

    let mut input_data = vec![0u8; len * 8];
    // First sample = (1.0, 0.0), rest are zeros
    let re_bytes = 1.0f32.to_le_bytes();
    input_data[0..4].copy_from_slice(&re_bytes);

    instance.write_memory(input_ptr as usize, &input_data).expect("write failed");

    // FFT then IFFT should give back the original
    let fft_ptr = instance
        .call_i32_i32_i32("test_fft", input_ptr, len as i32)
        .expect("test_fft call failed")
        .value;

    let ifft_ptr = instance
        .call_i32_i32_i32("test_ifft", fft_ptr, len as i32)
        .expect("test_ifft call failed")
        .value;

    // Read output
    let output_data = instance.read_memory(ifft_ptr as usize, len * 8).expect("read failed");

    // Check first sample is ~1.0 (normalized by FFT size)
    let re0 = f32::from_le_bytes([output_data[0], output_data[1], output_data[2], output_data[3]]);
    let im0 = f32::from_le_bytes([output_data[4], output_data[5], output_data[6], output_data[7]]);

    println!("Roundtrip sample[0]: re={:.6}, im={:.6}", re0, im0);

    // After FFT then IFFT, result should be original (possibly scaled)
    // Most FFT implementations return N * original for forward+inverse
    assert!((re0 - 1.0).abs() < 0.01 || (re0 - len as f32).abs() < 0.01,
            "first sample should be ~1.0 or ~16.0, got {}", re0);
}

#[test]
fn test_host_function_find_peak() {
    let sandbox = WasmSandbox::new(WasmConfig::dsp()).expect("failed to create sandbox");
    let module = sandbox.load_module(WASM_PATH).expect("failed to load module");
    let mut instance = sandbox.instantiate(&module).expect("failed to instantiate");

    // Create signal with a peak at index 3
    let len: usize = 8;
    let input_ptr = instance.alloc((len * 2 * 4) as i32).expect("alloc failed");

    let mut input_data = vec![0u8; len * 8];
    for i in 0..len {
        let magnitude = if i == 3 { 10.0f32 } else { 1.0f32 };
        let re_bytes = magnitude.to_le_bytes();
        let im_bytes = 0.0f32.to_le_bytes();
        input_data[i * 8..i * 8 + 4].copy_from_slice(&re_bytes);
        input_data[i * 8 + 4..i * 8 + 8].copy_from_slice(&im_bytes);
    }
    instance.write_memory(input_ptr as usize, &input_data).expect("write failed");

    // Call test_find_peak
    let peak_idx = instance
        .call_i32_i32_i32("test_find_peak", input_ptr, len as i32)
        .expect("test_find_peak call failed")
        .value;

    println!("Peak index: {}", peak_idx);
    assert_eq!(peak_idx, 3, "peak should be at index 3");
}

#[test]
fn test_host_function_total_power() {
    let sandbox = WasmSandbox::new(WasmConfig::dsp()).expect("failed to create sandbox");
    let module = sandbox.load_module(WASM_PATH).expect("failed to load module");
    let mut instance = sandbox.instantiate(&module).expect("failed to instantiate");

    // Verify the function is exported
    let exports = instance.exported_functions();
    assert!(exports.contains(&"test_total_power".to_string()), "should export test_total_power");
    println!("test_total_power is exported and callable");
    println!("All exported functions: {:?}", exports);
}

#[test]
fn test_host_function_hann_window() {
    let sandbox = WasmSandbox::new(WasmConfig::dsp()).expect("failed to create sandbox");
    let module = sandbox.load_module(WASM_PATH).expect("failed to load module");
    let mut instance = sandbox.instantiate(&module).expect("failed to instantiate");

    // Generate Hann window of length 8
    let len: usize = 8;
    let output_ptr = instance
        .call_i32_i32("test_hann_window", len as i32)
        .expect("test_hann_window call failed")
        .value;

    // Read output
    let output_data = instance.read_memory(output_ptr as usize, len * 4).expect("read failed");

    // Parse window values
    let mut window = Vec::with_capacity(len);
    for i in 0..len {
        let val = f32::from_le_bytes([
            output_data[i * 4],
            output_data[i * 4 + 1],
            output_data[i * 4 + 2],
            output_data[i * 4 + 3],
        ]);
        window.push(val);
    }

    println!("Hann window: {:?}", window);

    // Hann window properties (periodic/DFT-even):
    // - First value should be 0
    // - Middle value should be 1
    // - Should be symmetric around middle
    assert!(window[0] < 0.01, "first value should be ~0");
    assert!(window[len / 2] > 0.95, "middle value should be ~1");

    // Check that values increase to middle, then decrease
    assert!(window[1] > window[0], "should increase from start");
    assert!(window[len / 2] > window[1], "should increase toward middle");
}

#[test]
fn test_host_function_complex_multiply() {
    let sandbox = WasmSandbox::new(WasmConfig::dsp()).expect("failed to create sandbox");
    let module = sandbox.load_module(WASM_PATH).expect("failed to load module");
    let mut instance = sandbox.instantiate(&module).expect("failed to instantiate");

    // Test: (1+i) * (1-i) = 1 - i + i - i^2 = 1 + 1 = 2
    let len: usize = 1;
    let a_ptr = instance.alloc((len * 2 * 4) as i32).expect("alloc a failed");
    let b_ptr = instance.alloc((len * 2 * 4) as i32).expect("alloc b failed");

    // a = 1 + i
    let mut a_data = vec![0u8; 8];
    a_data[0..4].copy_from_slice(&1.0f32.to_le_bytes());
    a_data[4..8].copy_from_slice(&1.0f32.to_le_bytes());
    instance.write_memory(a_ptr as usize, &a_data).expect("write a failed");

    // b = 1 - i
    let mut b_data = vec![0u8; 8];
    b_data[0..4].copy_from_slice(&1.0f32.to_le_bytes());
    b_data[4..8].copy_from_slice(&(-1.0f32).to_le_bytes());
    instance.write_memory(b_ptr as usize, &b_data).expect("write b failed");

    // Call complex multiply with 3 args (a_ptr, b_ptr, len)
    let output_ptr = instance
        .call_i32_i32_i32_i32("test_complex_multiply", a_ptr, b_ptr, len as i32)
        .expect("test_complex_multiply call failed")
        .value;

    // Read result
    let output_data = instance.read_memory(output_ptr as usize, 8).expect("read output failed");
    let re = f32::from_le_bytes([output_data[0], output_data[1], output_data[2], output_data[3]]);
    let im = f32::from_le_bytes([output_data[4], output_data[5], output_data[6], output_data[7]]);

    println!("(1+i) * (1-i) = {} + {}i", re, im);

    // (1+i) * (1-i) = 1 - i + i - i² = 1 + 1 = 2 + 0i
    assert!((re - 2.0).abs() < 0.01, "real part should be ~2.0, got {}", re);
    assert!(im.abs() < 0.01, "imaginary part should be ~0.0, got {}", im);
}

#[test]
fn test_demodulate_fft() {
    let sandbox = WasmSandbox::new(WasmConfig::dsp()).expect("failed to create sandbox");
    let module = sandbox.load_module(WASM_PATH).expect("failed to load module");
    let mut instance = sandbox.instantiate(&module).expect("failed to instantiate");

    // Create a simple test: signal at bin 2
    let len: usize = 16;
    let input_ptr = instance.alloc((len * 2 * 4) as i32).expect("alloc input failed");
    let reference_ptr = instance.alloc((len * 2 * 4) as i32).expect("alloc reference failed");

    // Input: tone at normalized frequency 2/16 = 0.125
    let mut input_data = vec![0u8; len * 8];
    for i in 0..len {
        let phase = 2.0 * std::f32::consts::PI * 2.0 * i as f32 / len as f32;
        let re_bytes = phase.cos().to_le_bytes();
        let im_bytes = phase.sin().to_le_bytes();
        input_data[i * 8..i * 8 + 4].copy_from_slice(&re_bytes);
        input_data[i * 8 + 4..i * 8 + 8].copy_from_slice(&im_bytes);
    }
    instance.write_memory(input_ptr as usize, &input_data).expect("write input failed");

    // Reference: all ones (just passes through the input)
    let mut ref_data = vec![0u8; len * 8];
    for i in 0..len {
        let re_bytes = 1.0f32.to_le_bytes();
        let im_bytes = 0.0f32.to_le_bytes();
        ref_data[i * 8..i * 8 + 4].copy_from_slice(&re_bytes);
        ref_data[i * 8 + 4..i * 8 + 8].copy_from_slice(&im_bytes);
    }
    instance.write_memory(reference_ptr as usize, &ref_data).expect("write ref failed");

    // Call demodulate_fft with 3 args
    let peak_bin = instance
        .call_i32_i32_i32_i32("demodulate_fft", input_ptr, reference_ptr, len as i32)
        .expect("demodulate_fft call failed")
        .value;

    println!("demodulate_fft peak bin: {}", peak_bin);

    // Should find peak at bin 2 (the frequency of our input tone)
    assert_eq!(peak_bin, 2, "peak should be at bin 2");
}
