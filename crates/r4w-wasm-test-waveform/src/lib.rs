//! Simple test waveform for WASM sandbox validation.
//!
//! This module exports basic DSP functions that can be called from the sandbox.
//! Also demonstrates calling native DSP host functions for accelerated operations.

use std::alloc::{alloc as std_alloc, dealloc as std_dealloc, Layout};
use std::f32::consts::PI;

// =============================================================================
// Native DSP Host Function Imports
// =============================================================================
// These are provided by the r4w_dsp host module for accelerated DSP operations.

#[link(wasm_import_module = "r4w_dsp")]
extern "C" {
    /// Forward FFT: complex f32 input/output (interleaved re, im)
    fn fft(in_ptr: *const f32, out_ptr: *mut f32, len: i32);

    /// Inverse FFT: complex f32 input/output (interleaved re, im)
    fn ifft(in_ptr: *const f32, out_ptr: *mut f32, len: i32);

    /// Element-wise complex multiply: out = a * b
    fn complex_multiply(a_ptr: *const f32, b_ptr: *const f32, out_ptr: *mut f32, len: i32);

    /// Compute magnitudes of complex samples: out[i] = |in[i]|
    fn compute_magnitudes(in_ptr: *const f32, out_ptr: *mut f32, len: i32);

    /// Find index of peak magnitude in complex buffer
    fn find_peak(in_ptr: *const f32, len: i32) -> i32;

    /// Compute total power of complex samples
    fn total_power(in_ptr: *const f32, len: i32) -> f32;

    /// Generate Hann window of given length
    fn hann_window(out_ptr: *mut f32, len: i32);
}

/// Allocate memory for host to write into.
#[no_mangle]
pub extern "C" fn alloc(size: i32) -> i32 {
    let layout = Layout::from_size_align(size as usize, 8).unwrap();
    unsafe { std_alloc(layout) as i32 }
}

/// Free previously allocated memory.
#[no_mangle]
pub extern "C" fn dealloc(ptr: i32, size: i32) {
    let layout = Layout::from_size_align(size as usize, 8).unwrap();
    unsafe { std_dealloc(ptr as *mut u8, layout) };
}

/// Get the waveform name length.
#[no_mangle]
pub extern "C" fn waveform_name_len() -> i32 {
    b"TestBPSK".len() as i32
}

/// Write waveform name to the given pointer.
#[no_mangle]
pub extern "C" fn waveform_name(ptr: i32) {
    let name = b"TestBPSK";
    let dest = ptr as *mut u8;
    for (i, &byte) in name.iter().enumerate() {
        unsafe { *dest.add(i) = byte };
    }
}

/// Get bits per symbol for BPSK (always 1).
#[no_mangle]
pub extern "C" fn bits_per_symbol() -> i32 {
    1
}

/// Get sample rate.
#[no_mangle]
pub extern "C" fn sample_rate() -> i32 {
    48000
}

/// Get samples per symbol.
#[no_mangle]
pub extern "C" fn samples_per_symbol() -> i32 {
    48 // 1000 symbols/sec at 48kHz
}

/// Modulate a single bit to I/Q samples.
/// Returns pointer to 2 * samples_per_symbol f32 values (I, Q interleaved).
/// Caller must free with dealloc.
#[no_mangle]
pub extern "C" fn modulate_bit(bit: i32) -> i32 {
    let sps: usize = 48;
    let size = (sps * 2 * 4) as i32; // samples * (I + Q) * sizeof(f32)
    let ptr = alloc(size);

    // BPSK: bit 0 = phase 0, bit 1 = phase pi
    let phase = if bit != 0 { PI } else { 0.0 };

    let samples = ptr as *mut f32;
    for i in 0..sps {
        let t = i as f32 / 48000.0;
        let carrier_phase = 2.0 * PI * 1000.0 * t + phase;
        unsafe {
            // I component
            *samples.add(i * 2) = carrier_phase.cos();
            // Q component
            *samples.add(i * 2 + 1) = carrier_phase.sin();
        }
    }

    ptr
}

/// Compute magnitude of I/Q sample.
#[no_mangle]
pub extern "C" fn magnitude(i: f32, q: f32) -> f32 {
    (i * i + q * q).sqrt()
}

/// Compute phase of I/Q sample in radians.
#[no_mangle]
pub extern "C" fn phase(i: f32, q: f32) -> f32 {
    q.atan2(i)
}

/// Add two numbers (simple test function).
#[no_mangle]
pub extern "C" fn add(a: i32, b: i32) -> i32 {
    a + b
}

/// Multiply two numbers (simple test function).
#[no_mangle]
pub extern "C" fn multiply(a: i32, b: i32) -> i32 {
    a * b
}

/// Process a buffer of samples - apply gain.
/// input_ptr: pointer to f32 array
/// output_ptr: pointer to f32 array (preallocated)
/// len: number of f32 values
/// gain: multiplier
#[no_mangle]
pub extern "C" fn apply_gain(input_ptr: i32, output_ptr: i32, len: i32, gain: f32) {
    let input = input_ptr as *const f32;
    let output = output_ptr as *mut f32;

    for i in 0..len as usize {
        unsafe {
            *output.add(i) = *input.add(i) * gain;
        }
    }
}

/// Compute sum of f32 array.
#[no_mangle]
pub extern "C" fn sum_f32(ptr: i32, len: i32) -> f32 {
    let data = ptr as *const f32;
    let mut total = 0.0f32;
    for i in 0..len as usize {
        unsafe {
            total += *data.add(i);
        }
    }
    total
}

/// Compute energy (sum of squares) of f32 array.
#[no_mangle]
pub extern "C" fn energy_f32(ptr: i32, len: i32) -> f32 {
    let data = ptr as *const f32;
    let mut total = 0.0f32;
    for i in 0..len as usize {
        unsafe {
            let v = *data.add(i);
            total += v * v;
        }
    }
    total
}

/// Version number for ABI compatibility.
#[no_mangle]
pub extern "C" fn version() -> i32 {
    1
}

// =============================================================================
// DSP Host Function Wrappers (for testing)
// =============================================================================

/// Test FFT: perform forward FFT and return pointer to output buffer.
/// len: number of complex samples (buffer has 2*len floats)
/// Returns pointer to output buffer (caller must free with dealloc)
#[no_mangle]
pub extern "C" fn test_fft(input_ptr: i32, len: i32) -> i32 {
    let size = (len * 2 * 4) as i32; // complex samples * sizeof(f32)
    let output_ptr = alloc(size);

    unsafe {
        fft(input_ptr as *const f32, output_ptr as *mut f32, len);
    }

    output_ptr
}

/// Test IFFT: perform inverse FFT and return pointer to output buffer.
#[no_mangle]
pub extern "C" fn test_ifft(input_ptr: i32, len: i32) -> i32 {
    let size = (len * 2 * 4) as i32;
    let output_ptr = alloc(size);

    unsafe {
        ifft(input_ptr as *const f32, output_ptr as *mut f32, len);
    }

    output_ptr
}

/// Test complex multiply: multiply two complex buffers element-wise.
#[no_mangle]
pub extern "C" fn test_complex_multiply(a_ptr: i32, b_ptr: i32, len: i32) -> i32 {
    let size = (len * 2 * 4) as i32;
    let output_ptr = alloc(size);

    unsafe {
        complex_multiply(
            a_ptr as *const f32,
            b_ptr as *const f32,
            output_ptr as *mut f32,
            len,
        );
    }

    output_ptr
}

/// Test compute magnitudes: compute magnitude of each complex sample.
/// Returns pointer to f32 array with len elements.
#[no_mangle]
pub extern "C" fn test_compute_magnitudes(input_ptr: i32, len: i32) -> i32 {
    let size = (len * 4) as i32; // len f32 values
    let output_ptr = alloc(size);

    unsafe {
        compute_magnitudes(input_ptr as *const f32, output_ptr as *mut f32, len);
    }

    output_ptr
}

/// Test find_peak: find index of maximum magnitude in complex buffer.
#[no_mangle]
pub extern "C" fn test_find_peak(input_ptr: i32, len: i32) -> i32 {
    unsafe { find_peak(input_ptr as *const f32, len) }
}

/// Test total_power: compute total power of complex samples.
#[no_mangle]
pub extern "C" fn test_total_power(input_ptr: i32, len: i32) -> f32 {
    unsafe { total_power(input_ptr as *const f32, len) }
}

/// Test hann_window: generate Hann window.
/// Returns pointer to f32 array with len elements.
#[no_mangle]
pub extern "C" fn test_hann_window(len: i32) -> i32 {
    let size = (len * 4) as i32;
    let output_ptr = alloc(size);

    unsafe {
        hann_window(output_ptr as *mut f32, len);
    }

    output_ptr
}

/// FFT-based demodulation example: multiply by reference, FFT, find peak.
/// This demonstrates the hybrid architecture: WASM logic calling native DSP.
/// input_ptr: complex IQ samples (interleaved f32)
/// reference_ptr: reference signal to correlate with (interleaved f32)
/// len: number of complex samples
/// Returns: peak bin index
#[no_mangle]
pub extern "C" fn demodulate_fft(input_ptr: i32, reference_ptr: i32, len: i32) -> i32 {
    // Allocate temporary buffers
    let size = (len * 2 * 4) as i32;
    let mixed_ptr = alloc(size);
    let spectrum_ptr = alloc(size);

    unsafe {
        // Multiply input by reference (e.g., downchirp for LoRa)
        complex_multiply(
            input_ptr as *const f32,
            reference_ptr as *const f32,
            mixed_ptr as *mut f32,
            len,
        );

        // FFT to find correlation peak
        fft(mixed_ptr as *const f32, spectrum_ptr as *mut f32, len);

        // Find peak bin
        let peak = find_peak(spectrum_ptr as *const f32, len);

        // Free temporary buffers
        dealloc(mixed_ptr, size);
        dealloc(spectrum_ptr, size);

        peak
    }
}
