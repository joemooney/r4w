//! WASM Sandbox Benchmarks
//!
//! Run with: cargo run -p r4w-sandbox --example wasm_benchmark --features wasm --release

use r4w_sandbox::{WasmBenchmark, WasmConfig, WasmSandbox};
use std::time::Instant;

const WASM_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/r4w_wasm_test_waveform.wasm"
);

fn main() {
    println!("=== WASM Sandbox Benchmarks ===\n");

    bench_module_loading();
    bench_instantiation();
    bench_function_calls();
    bench_memory_operations();
    bench_modulation();
    bench_with_fuel_metering();
    bench_native_comparison();

    println!("\n=== Benchmark Complete ===");
}

fn bench_module_loading() {
    println!("## Module Loading");

    let mut times = Vec::new();

    // Warm up
    let sandbox = WasmSandbox::new(WasmConfig::default()).unwrap();
    let _ = sandbox.load_module(WASM_PATH).unwrap();

    // Benchmark
    for _ in 0..100 {
        let start = Instant::now();
        let _ = sandbox.load_module(WASM_PATH).unwrap();
        times.push(start.elapsed().as_micros() as u64);
    }

    print_stats("load_module", &times);
    println!();
}

fn bench_instantiation() {
    println!("## Instantiation");

    let sandbox = WasmSandbox::new(WasmConfig::default()).unwrap();
    let module = sandbox.load_module(WASM_PATH).unwrap();

    let mut times = Vec::new();

    // Warm up
    for _ in 0..5 {
        let _ = sandbox.instantiate(&module).unwrap();
    }

    // Benchmark
    for _ in 0..100 {
        let start = Instant::now();
        let _ = sandbox.instantiate(&module).unwrap();
        times.push(start.elapsed().as_micros() as u64);
    }

    print_stats("instantiate", &times);
    println!();
}

fn bench_function_calls() {
    println!("## Function Call Overhead");

    let sandbox = WasmSandbox::new(WasmConfig::dsp()).unwrap();
    let module = sandbox.load_module(WASM_PATH).unwrap();
    let mut instance = sandbox.instantiate(&module).unwrap();

    // Warm up
    for _ in 0..100 {
        let _ = instance.call_i32("version");
    }

    // Benchmark different function signatures
    let iterations = 10_000;

    // () -> i32
    let mut bench = WasmBenchmark::new();
    for _ in 0..iterations {
        let result = instance.call_i32("version").unwrap();
        bench.record(result.execution_time_us);
    }
    println!("  call_i32 (version):       {}", bench.summary());

    // (i32) -> i32
    let mut bench = WasmBenchmark::new();
    for i in 0..iterations {
        let result = instance.call_i32_i32("modulate_bit", (i % 2) as i32).unwrap();
        bench.record(result.execution_time_us);
    }
    println!("  call_i32_i32 (modulate):  {}", bench.summary());

    // (i32, i32) -> i32
    let mut bench = WasmBenchmark::new();
    for i in 0..iterations {
        let result = instance.call_i32_i32_i32("add", i as i32, 1).unwrap();
        bench.record(result.execution_time_us);
    }
    println!("  call_i32_i32_i32 (add):   {}", bench.summary());

    // multiply
    let mut bench = WasmBenchmark::new();
    for i in 0..iterations {
        let result = instance.call_i32_i32_i32("multiply", i as i32, 2).unwrap();
        bench.record(result.execution_time_us);
    }
    println!("  call_i32_i32_i32 (mult):  {}", bench.summary());

    println!();
}

fn bench_memory_operations() {
    println!("## Memory Operations");

    let sandbox = WasmSandbox::new(WasmConfig::dsp()).unwrap();
    let module = sandbox.load_module(WASM_PATH).unwrap();
    let mut instance = sandbox.instantiate(&module).unwrap();

    let iterations = 1000;

    // Allocate various sizes
    for size in [64, 256, 1024, 4096, 16384] {
        let mut times = Vec::new();

        for _ in 0..iterations {
            let start = Instant::now();
            let ptr = instance.alloc(size).unwrap();
            times.push(start.elapsed().as_nanos() as u64);

            // Don't leak - dealloc
            let _ = instance.call_i32_i32_i32("dealloc", ptr, size);
        }

        let mean = times.iter().sum::<u64>() as f64 / times.len() as f64;
        let min = *times.iter().min().unwrap();
        let max = *times.iter().max().unwrap();
        println!("  alloc({:>5} bytes): mean={:.0}ns min={}ns max={}ns", size, mean, min, max);
    }

    // Write/read benchmark
    let ptr = instance.alloc(4096).unwrap();
    let data: Vec<u8> = (0..4096).map(|i| (i % 256) as u8).collect();

    let mut write_times = Vec::new();
    let mut read_times = Vec::new();

    for _ in 0..iterations {
        let start = Instant::now();
        instance.write_memory(ptr as usize, &data).unwrap();
        write_times.push(start.elapsed().as_nanos() as u64);

        let start = Instant::now();
        let _ = instance.read_memory(ptr as usize, 4096).unwrap();
        read_times.push(start.elapsed().as_nanos() as u64);
    }

    let write_mean = write_times.iter().sum::<u64>() as f64 / write_times.len() as f64;
    let read_mean = read_times.iter().sum::<u64>() as f64 / read_times.len() as f64;

    println!("  write_memory(4096):       mean={:.0}ns ({:.1} MB/s)",
             write_mean, 4096.0 / write_mean * 1000.0);
    println!("  read_memory(4096):        mean={:.0}ns ({:.1} MB/s)",
             read_mean, 4096.0 / read_mean * 1000.0);

    println!();
}

fn bench_modulation() {
    println!("## BPSK Modulation (48 samples/symbol)");

    let sandbox = WasmSandbox::new(WasmConfig::dsp()).unwrap();
    let module = sandbox.load_module(WASM_PATH).unwrap();
    let mut instance = sandbox.instantiate(&module).unwrap();

    // Warm up
    for i in 0..100 {
        let ptr = instance.call_i32_i32("modulate_bit", i % 2).unwrap().value;
        let _ = instance.call_i32_i32_i32("dealloc", ptr, 48 * 2 * 4);
    }

    let iterations = 10_000;
    let mut bench = WasmBenchmark::new();

    for i in 0..iterations {
        let result = instance.call_i32_i32("modulate_bit", (i % 2) as i32).unwrap();
        bench.record(result.execution_time_us);

        // Dealloc to prevent memory exhaustion
        let _ = instance.call_i32_i32_i32("dealloc", result.value, 48 * 2 * 4);
    }

    println!("  modulate_bit:             {}", bench.summary());

    // Calculate throughput
    let samples_per_call = 48 * 2; // I and Q
    let mean_us = bench.mean_us();
    if mean_us > 0.0 {
        let samples_per_sec = samples_per_call as f64 / (mean_us / 1_000_000.0);
        println!("  throughput:               {:.2} Msamples/sec", samples_per_sec / 1_000_000.0);
    }

    // Batch modulation - simulate 1000 bits
    let start = Instant::now();
    for i in 0..1000 {
        let ptr = instance.call_i32_i32("modulate_bit", (i % 2) as i32).unwrap().value;
        let _ = instance.call_i32_i32_i32("dealloc", ptr, 48 * 2 * 4);
    }
    let batch_time = start.elapsed();
    println!("  1000 bits modulated in:   {:?}", batch_time);
    println!("  symbol rate:              {:.0} symbols/sec", 1000.0 / batch_time.as_secs_f64());

    println!();
}

fn bench_with_fuel_metering() {
    println!("## Fuel Metering Overhead");

    // Without fuel
    let sandbox_no_fuel = WasmSandbox::new(WasmConfig::dsp()).unwrap();
    let module = sandbox_no_fuel.load_module(WASM_PATH).unwrap();
    let mut instance_no_fuel = sandbox_no_fuel.instantiate(&module).unwrap();

    // With fuel
    let config_with_fuel = WasmConfig::dsp().fuel_limit(u64::MAX);
    let sandbox_fuel = WasmSandbox::new(config_with_fuel).unwrap();
    let module_fuel = sandbox_fuel.load_module(WASM_PATH).unwrap();
    let mut instance_fuel = sandbox_fuel.instantiate(&module_fuel).unwrap();

    let iterations = 10_000;

    // Warm up
    for _ in 0..100 {
        let _ = instance_no_fuel.call_i32_i32_i32("add", 1, 1);
        let _ = instance_fuel.call_i32_i32_i32("add", 1, 1);
    }

    // Without fuel
    let mut bench_no_fuel = WasmBenchmark::new();
    for i in 0..iterations {
        let result = instance_no_fuel.call_i32_i32_i32("add", i as i32, 1).unwrap();
        bench_no_fuel.record(result.execution_time_us);
    }

    // With fuel
    let mut bench_fuel = WasmBenchmark::new();
    let mut total_fuel_consumed = 0u64;
    for i in 0..iterations {
        let result = instance_fuel.call_i32_i32_i32("add", i as i32, 1).unwrap();
        bench_fuel.record(result.execution_time_us);
        total_fuel_consumed += result.fuel_consumed.unwrap_or(0);
    }

    println!("  without fuel metering:    {}", bench_no_fuel.summary());
    println!("  with fuel metering:       {}", bench_fuel.summary());
    println!("  avg fuel per call:        {}", total_fuel_consumed / iterations as u64);

    let overhead = if bench_no_fuel.mean_us() > 0.0 {
        ((bench_fuel.mean_us() - bench_no_fuel.mean_us()) / bench_no_fuel.mean_us()) * 100.0
    } else {
        0.0
    };
    println!("  overhead:                 {:.1}%", overhead);

    println!();
}

fn print_stats(name: &str, times: &[u64]) {
    if times.is_empty() {
        println!("  {}: no samples", name);
        return;
    }

    let mut sorted = times.to_vec();
    sorted.sort();

    let min = sorted[0];
    let max = sorted[sorted.len() - 1];
    let mean = times.iter().sum::<u64>() as f64 / times.len() as f64;
    let p50 = sorted[sorted.len() / 2];
    let p99 = sorted[(sorted.len() * 99) / 100];

    println!("  {}: n={} min={}us mean={:.1}us p50={}us p99={}us max={}us",
             name, times.len(), min, mean, p50, p99, max);
}

fn bench_native_comparison() {
    use std::f32::consts::PI;

    println!("## Native vs WASM Comparison");

    // WASM setup
    let sandbox = WasmSandbox::new(WasmConfig::dsp()).unwrap();
    let module = sandbox.load_module(WASM_PATH).unwrap();
    let mut instance = sandbox.instantiate(&module).unwrap();

    let iterations = 100_000;

    // Native add
    let start = Instant::now();
    let mut sum = 0i32;
    for i in 0..iterations {
        sum = sum.wrapping_add(native_add(i as i32, 1));
    }
    let native_add_time = start.elapsed();
    std::hint::black_box(sum);

    // WASM add
    let start = Instant::now();
    let mut sum = 0i32;
    for i in 0..iterations {
        sum = sum.wrapping_add(instance.call_i32_i32_i32("add", i as i32, 1).unwrap().value);
    }
    let wasm_add_time = start.elapsed();
    std::hint::black_box(sum);

    println!("  add ({} calls):", iterations);
    println!("    native:   {:?} ({:.1} ns/call)",
             native_add_time, native_add_time.as_nanos() as f64 / iterations as f64);
    println!("    WASM:     {:?} ({:.1} ns/call)",
             wasm_add_time, wasm_add_time.as_nanos() as f64 / iterations as f64);
    println!("    overhead: {:.1}x",
             wasm_add_time.as_nanos() as f64 / native_add_time.as_nanos() as f64);

    // Native modulate
    let iterations = 10_000;
    let start = Instant::now();
    for i in 0..iterations {
        let samples = native_modulate_bit((i % 2) as i32);
        std::hint::black_box(samples);
    }
    let native_mod_time = start.elapsed();

    // WASM modulate
    let start = Instant::now();
    for i in 0..iterations {
        let ptr = instance.call_i32_i32("modulate_bit", (i % 2) as i32).unwrap().value;
        let _ = instance.call_i32_i32_i32("dealloc", ptr, 48 * 2 * 4);
    }
    let wasm_mod_time = start.elapsed();

    println!("\n  modulate_bit ({} calls):", iterations);
    println!("    native:   {:?} ({:.1} us/call)",
             native_mod_time, native_mod_time.as_micros() as f64 / iterations as f64);
    println!("    WASM:     {:?} ({:.1} us/call)",
             wasm_mod_time, wasm_mod_time.as_micros() as f64 / iterations as f64);
    println!("    overhead: {:.1}x",
             wasm_mod_time.as_nanos() as f64 / native_mod_time.as_nanos() as f64);

    // Summary
    println!("\n  Summary:");
    println!("    Simple arithmetic: WASM adds ~{:.0}ns overhead per call",
             (wasm_add_time.as_nanos() as f64 - native_add_time.as_nanos() as f64) / iterations as f64);
    println!("    DSP modulation:    WASM is {:.1}x slower than native",
             wasm_mod_time.as_nanos() as f64 / native_mod_time.as_nanos() as f64);

    println!();
}

#[inline(never)]
fn native_add(a: i32, b: i32) -> i32 {
    a + b
}

#[inline(never)]
fn native_modulate_bit(bit: i32) -> Vec<f32> {
    use std::f32::consts::PI;

    let sps = 48;
    let phase = if bit != 0 { PI } else { 0.0 };
    let mut samples = Vec::with_capacity(sps * 2);

    for i in 0..sps {
        let t = i as f32 / 48000.0;
        let carrier_phase = 2.0 * PI * 1000.0 * t + phase;
        samples.push(carrier_phase.cos());
        samples.push(carrier_phase.sin());
    }

    samples
}
