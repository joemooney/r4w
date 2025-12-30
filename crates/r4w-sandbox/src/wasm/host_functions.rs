//! DSP host functions for WASM waveform modules.
//!
//! This module provides native DSP functions that WASM modules can call
//! for performance-critical operations. This enables the hybrid architecture:
//! - WASM: Waveform logic, state machines, protocol handling (isolated)
//! - Native: FFT, filters, SIMD operations (fast)
//!
//! ## Memory Convention
//!
//! All complex sample buffers use interleaved f32 layout:
//! ```text
//! [re0, im0, re1, im1, re2, im2, ...]
//! ```
//! - Each complex sample = 8 bytes (2 × f32)
//! - Pointers are i32 (WASM32 address space)
//! - Lengths are in number of complex samples (not bytes)
//!
//! ## Performance Optimization
//!
//! This implementation uses f32 throughout to avoid conversion overhead:
//! - Direct f32 memory access (no f32↔f64 conversion)
//! - rustfft with Complex<f32> for FFT operations
//! - Native f32 math for other operations
//!
//! ## Import Module
//!
//! Host functions are imported under the `r4w_dsp` namespace:
//! ```wat
//! (import "r4w_dsp" "fft" (func $fft (param i32 i32 i32)))
//! ```

use num_complex::Complex;
use rustfft::FftPlanner;
use std::f32::consts::PI;
use wasmtime::{Caller, Linker, Memory};

use super::runtime::WasmHostState;
use crate::error::{Result, SandboxError};

/// Complex f32 type alias for clarity.
type Complex32 = Complex<f32>;

/// DSP host functions exposed to WASM modules.
///
/// This is a marker struct for organizing host function registration.
/// FFT processors are created on-demand (rustfft internally caches plans).
pub struct DspHostFunctions;

impl Default for DspHostFunctions {
    fn default() -> Self {
        Self::new()
    }
}

impl DspHostFunctions {
    /// Create a new DSP host functions instance.
    pub fn new() -> Self {
        Self
    }

    /// Register all DSP host functions with the wasmtime linker.
    pub fn register(linker: &mut Linker<WasmHostState>) -> Result<()> {
        // FFT functions
        Self::register_fft(linker)?;
        Self::register_ifft(linker)?;

        // Complex math functions
        Self::register_complex_multiply(linker)?;
        Self::register_complex_conjugate_multiply(linker)?;
        Self::register_compute_magnitudes(linker)?;
        Self::register_compute_power(linker)?;

        // Signal processing functions
        Self::register_frequency_shift(linker)?;
        Self::register_find_peak(linker)?;
        Self::register_scale(linker)?;
        Self::register_total_power(linker)?;

        // Window functions
        Self::register_hann_window(linker)?;
        Self::register_hamming_window(linker)?;

        Ok(())
    }

    // ========================================================================
    // FFT Functions
    // ========================================================================

    fn register_fft(linker: &mut Linker<WasmHostState>) -> Result<()> {
        linker
            .func_wrap(
                "r4w_dsp",
                "fft",
                |mut caller: Caller<'_, WasmHostState>,
                 in_ptr: i32,
                 out_ptr: i32,
                 len: i32| {
                    let memory = get_memory(&mut caller)?;
                    let len = len as usize;

                    // Read input directly as Complex<f32>
                    let mut buffer = read_complex_f32(&memory, &caller, in_ptr as usize, len)?;

                    // Perform FFT using rustfft with f32
                    let mut planner = FftPlanner::<f32>::new();
                    let fft = planner.plan_fft_forward(len);
                    fft.process(&mut buffer);

                    // Write output directly
                    write_complex_f32(&memory, &mut caller, out_ptr as usize, &buffer)?;

                    Ok(())
                },
            )
            .map_err(|e| SandboxError::WasmError(format!("failed to register fft: {}", e)))?;
        Ok(())
    }

    fn register_ifft(linker: &mut Linker<WasmHostState>) -> Result<()> {
        linker
            .func_wrap(
                "r4w_dsp",
                "ifft",
                |mut caller: Caller<'_, WasmHostState>,
                 in_ptr: i32,
                 out_ptr: i32,
                 len: i32| {
                    let memory = get_memory(&mut caller)?;
                    let len = len as usize;

                    let mut buffer = read_complex_f32(&memory, &caller, in_ptr as usize, len)?;

                    // Perform IFFT using rustfft with f32
                    let mut planner = FftPlanner::<f32>::new();
                    let ifft = planner.plan_fft_inverse(len);
                    ifft.process(&mut buffer);

                    // Normalize by 1/N
                    let scale = 1.0 / len as f32;
                    for c in &mut buffer {
                        *c *= scale;
                    }

                    write_complex_f32(&memory, &mut caller, out_ptr as usize, &buffer)?;

                    Ok(())
                },
            )
            .map_err(|e| SandboxError::WasmError(format!("failed to register ifft: {}", e)))?;
        Ok(())
    }

    // ========================================================================
    // Complex Math Functions
    // ========================================================================

    fn register_complex_multiply(linker: &mut Linker<WasmHostState>) -> Result<()> {
        linker
            .func_wrap(
                "r4w_dsp",
                "complex_multiply",
                |mut caller: Caller<'_, WasmHostState>,
                 a_ptr: i32,
                 b_ptr: i32,
                 out_ptr: i32,
                 len: i32| {
                    let memory = get_memory(&mut caller)?;
                    let len = len as usize;

                    let a = read_complex_f32(&memory, &caller, a_ptr as usize, len)?;
                    let b = read_complex_f32(&memory, &caller, b_ptr as usize, len)?;

                    // Element-wise complex multiply in f32
                    let output: Vec<Complex32> = a.iter().zip(b.iter()).map(|(x, y)| x * y).collect();

                    write_complex_f32(&memory, &mut caller, out_ptr as usize, &output)?;

                    Ok(())
                },
            )
            .map_err(|e| {
                SandboxError::WasmError(format!("failed to register complex_multiply: {}", e))
            })?;
        Ok(())
    }

    fn register_complex_conjugate_multiply(linker: &mut Linker<WasmHostState>) -> Result<()> {
        linker
            .func_wrap(
                "r4w_dsp",
                "complex_conjugate_multiply",
                |mut caller: Caller<'_, WasmHostState>,
                 a_ptr: i32,
                 b_ptr: i32,
                 out_ptr: i32,
                 len: i32| {
                    let memory = get_memory(&mut caller)?;
                    let len = len as usize;

                    let a = read_complex_f32(&memory, &caller, a_ptr as usize, len)?;
                    let b = read_complex_f32(&memory, &caller, b_ptr as usize, len)?;

                    // Element-wise: a * conj(b) in f32
                    let output: Vec<Complex32> =
                        a.iter().zip(b.iter()).map(|(x, y)| x * y.conj()).collect();

                    write_complex_f32(&memory, &mut caller, out_ptr as usize, &output)?;

                    Ok(())
                },
            )
            .map_err(|e| {
                SandboxError::WasmError(format!(
                    "failed to register complex_conjugate_multiply: {}",
                    e
                ))
            })?;
        Ok(())
    }

    fn register_compute_magnitudes(linker: &mut Linker<WasmHostState>) -> Result<()> {
        linker
            .func_wrap(
                "r4w_dsp",
                "compute_magnitudes",
                |mut caller: Caller<'_, WasmHostState>,
                 in_ptr: i32,
                 out_ptr: i32,
                 len: i32| {
                    let memory = get_memory(&mut caller)?;
                    let len = len as usize;

                    let input = read_complex_f32(&memory, &caller, in_ptr as usize, len)?;

                    // Compute magnitudes in f32
                    let output: Vec<f32> = input.iter().map(|c| c.norm()).collect();

                    write_f32(&memory, &mut caller, out_ptr as usize, &output)?;

                    Ok(())
                },
            )
            .map_err(|e| {
                SandboxError::WasmError(format!("failed to register compute_magnitudes: {}", e))
            })?;
        Ok(())
    }

    fn register_compute_power(linker: &mut Linker<WasmHostState>) -> Result<()> {
        linker
            .func_wrap(
                "r4w_dsp",
                "compute_power",
                |mut caller: Caller<'_, WasmHostState>,
                 in_ptr: i32,
                 out_ptr: i32,
                 len: i32| {
                    let memory = get_memory(&mut caller)?;
                    let len = len as usize;

                    let input = read_complex_f32(&memory, &caller, in_ptr as usize, len)?;

                    // Compute power (magnitude squared) in f32
                    let output: Vec<f32> = input.iter().map(|c| c.norm_sqr()).collect();

                    write_f32(&memory, &mut caller, out_ptr as usize, &output)?;

                    Ok(())
                },
            )
            .map_err(|e| {
                SandboxError::WasmError(format!("failed to register compute_power: {}", e))
            })?;
        Ok(())
    }

    // ========================================================================
    // Signal Processing Functions
    // ========================================================================

    fn register_frequency_shift(linker: &mut Linker<WasmHostState>) -> Result<()> {
        linker
            .func_wrap(
                "r4w_dsp",
                "frequency_shift",
                |mut caller: Caller<'_, WasmHostState>,
                 in_ptr: i32,
                 out_ptr: i32,
                 len: i32,
                 freq_hz: f32,
                 sample_rate: f32| {
                    let memory = get_memory(&mut caller)?;
                    let len = len as usize;

                    let input = read_complex_f32(&memory, &caller, in_ptr as usize, len)?;

                    // NCO-based frequency shift in f32
                    let phase_inc = 2.0 * PI * freq_hz / sample_rate;
                    let output: Vec<Complex32> = input
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            let phase = phase_inc * i as f32;
                            let nco = Complex32::new(phase.cos(), phase.sin());
                            c * nco
                        })
                        .collect();

                    write_complex_f32(&memory, &mut caller, out_ptr as usize, &output)?;

                    Ok(())
                },
            )
            .map_err(|e| {
                SandboxError::WasmError(format!("failed to register frequency_shift: {}", e))
            })?;
        Ok(())
    }

    fn register_find_peak(linker: &mut Linker<WasmHostState>) -> Result<()> {
        linker
            .func_wrap(
                "r4w_dsp",
                "find_peak",
                |mut caller: Caller<'_, WasmHostState>, in_ptr: i32, len: i32| -> i32 {
                    let memory = match get_memory(&mut caller) {
                        Ok(m) => m,
                        Err(_) => return -1,
                    };
                    let len = len as usize;

                    let input = match read_complex_f32(&memory, &caller, in_ptr as usize, len) {
                        Ok(i) => i,
                        Err(_) => return -1,
                    };

                    // Find index of maximum magnitude in f32
                    let mut max_mag_sq = 0.0f32;
                    let mut max_idx = 0i32;
                    for (i, c) in input.iter().enumerate() {
                        let mag_sq = c.norm_sqr();
                        if mag_sq > max_mag_sq {
                            max_mag_sq = mag_sq;
                            max_idx = i as i32;
                        }
                    }

                    max_idx
                },
            )
            .map_err(|e| {
                SandboxError::WasmError(format!("failed to register find_peak: {}", e))
            })?;
        Ok(())
    }

    fn register_scale(linker: &mut Linker<WasmHostState>) -> Result<()> {
        linker
            .func_wrap(
                "r4w_dsp",
                "scale",
                |mut caller: Caller<'_, WasmHostState>,
                 in_ptr: i32,
                 out_ptr: i32,
                 len: i32,
                 factor: f32| {
                    let memory = get_memory(&mut caller)?;
                    let len = len as usize;

                    let input = read_complex_f32(&memory, &caller, in_ptr as usize, len)?;

                    // Scale in f32
                    let output: Vec<Complex32> = input.iter().map(|c| c * factor).collect();

                    write_complex_f32(&memory, &mut caller, out_ptr as usize, &output)?;

                    Ok(())
                },
            )
            .map_err(|e| SandboxError::WasmError(format!("failed to register scale: {}", e)))?;
        Ok(())
    }

    fn register_total_power(linker: &mut Linker<WasmHostState>) -> Result<()> {
        linker
            .func_wrap(
                "r4w_dsp",
                "total_power",
                |mut caller: Caller<'_, WasmHostState>, in_ptr: i32, len: i32| -> f32 {
                    let memory = match get_memory(&mut caller) {
                        Ok(m) => m,
                        Err(_) => return 0.0,
                    };
                    let len = len as usize;

                    let input = match read_complex_f32(&memory, &caller, in_ptr as usize, len) {
                        Ok(i) => i,
                        Err(_) => return 0.0,
                    };

                    // Sum of magnitude squared in f32
                    input.iter().map(|c| c.norm_sqr()).sum()
                },
            )
            .map_err(|e| {
                SandboxError::WasmError(format!("failed to register total_power: {}", e))
            })?;
        Ok(())
    }

    // ========================================================================
    // Window Functions
    // ========================================================================

    fn register_hann_window(linker: &mut Linker<WasmHostState>) -> Result<()> {
        linker
            .func_wrap(
                "r4w_dsp",
                "hann_window",
                |mut caller: Caller<'_, WasmHostState>, out_ptr: i32, len: i32| {
                    let memory = get_memory(&mut caller)?;
                    let len = len as usize;

                    // Generate Hann window (periodic/DFT-even) in f32
                    let window: Vec<f32> = (0..len)
                        .map(|i| {
                            let x = 2.0 * PI * (i as f32) / (len as f32);
                            0.5 * (1.0 - x.cos())
                        })
                        .collect();

                    write_f32(&memory, &mut caller, out_ptr as usize, &window)?;

                    Ok(())
                },
            )
            .map_err(|e| {
                SandboxError::WasmError(format!("failed to register hann_window: {}", e))
            })?;
        Ok(())
    }

    fn register_hamming_window(linker: &mut Linker<WasmHostState>) -> Result<()> {
        linker
            .func_wrap(
                "r4w_dsp",
                "hamming_window",
                |mut caller: Caller<'_, WasmHostState>, out_ptr: i32, len: i32| {
                    let memory = get_memory(&mut caller)?;
                    let len = len as usize;

                    // Generate Hamming window in f32
                    let window: Vec<f32> = (0..len)
                        .map(|i| {
                            let x = 2.0 * PI * (i as f32) / (len as f32);
                            0.54 - 0.46 * x.cos()
                        })
                        .collect();

                    write_f32(&memory, &mut caller, out_ptr as usize, &window)?;

                    Ok(())
                },
            )
            .map_err(|e| {
                SandboxError::WasmError(format!("failed to register hamming_window: {}", e))
            })?;
        Ok(())
    }
}

// ============================================================================
// Memory Access Helpers (f32 native)
// ============================================================================

/// Get the memory export from the caller.
fn get_memory(caller: &mut Caller<'_, WasmHostState>) -> Result<Memory> {
    caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| SandboxError::WasmError("no memory export found".to_string()))
}

/// Read complex samples directly from WASM memory as Complex<f32>.
/// No f64 conversion - reads f32 pairs directly.
fn read_complex_f32(
    memory: &Memory,
    caller: &Caller<'_, WasmHostState>,
    offset: usize,
    len: usize,
) -> Result<Vec<Complex32>> {
    let byte_len = len * 8; // 2 f32s per complex = 8 bytes
    let data = memory.data(caller);

    if offset + byte_len > data.len() {
        return Err(SandboxError::WasmError(
            "memory read out of bounds".to_string(),
        ));
    }

    let mut result = Vec::with_capacity(len);
    for i in 0..len {
        let base = offset + i * 8;
        let re = f32::from_le_bytes([data[base], data[base + 1], data[base + 2], data[base + 3]]);
        let im = f32::from_le_bytes([
            data[base + 4],
            data[base + 5],
            data[base + 6],
            data[base + 7],
        ]);
        result.push(Complex32::new(re, im));
    }

    Ok(result)
}

/// Write Complex<f32> samples directly to WASM memory.
/// No f64 conversion - writes f32 pairs directly.
fn write_complex_f32(
    memory: &Memory,
    caller: &mut Caller<'_, WasmHostState>,
    offset: usize,
    data: &[Complex32],
) -> Result<()> {
    let byte_len = data.len() * 8;
    let mem_data = memory.data_mut(caller);

    if offset + byte_len > mem_data.len() {
        return Err(SandboxError::WasmError(
            "memory write out of bounds".to_string(),
        ));
    }

    for (i, c) in data.iter().enumerate() {
        let base = offset + i * 8;
        let re_bytes = c.re.to_le_bytes();
        let im_bytes = c.im.to_le_bytes();

        mem_data[base..base + 4].copy_from_slice(&re_bytes);
        mem_data[base + 4..base + 8].copy_from_slice(&im_bytes);
    }

    Ok(())
}

/// Write f32 values directly to WASM memory.
fn write_f32(
    memory: &Memory,
    caller: &mut Caller<'_, WasmHostState>,
    offset: usize,
    data: &[f32],
) -> Result<()> {
    let byte_len = data.len() * 4;
    let mem_data = memory.data_mut(caller);

    if offset + byte_len > mem_data.len() {
        return Err(SandboxError::WasmError(
            "memory write out of bounds".to_string(),
        ));
    }

    for (i, &v) in data.iter().enumerate() {
        let base = offset + i * 4;
        let bytes = v.to_le_bytes();
        mem_data[base..base + 4].copy_from_slice(&bytes);
    }

    Ok(())
}
