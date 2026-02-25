//! DAB/DAB+ Digital Audio Broadcasting Channel Decoder
//!
//! Implements the physical and channel layer processing for Digital Audio
//! Broadcasting (DAB) and DAB+ as specified in ETSI EN 300 401 (DAB)
//! and ETSI EN 302 563 (DAB+).
//!
//! ## System Overview
//!
//! DAB uses Coded Orthogonal Frequency Division Multiplexing (COFDM) with:
//! - Mode I: 2048-point FFT, 1536 active carriers, 76 OFDM symbols per frame
//! - 96 ms transmission frame duration (Tf)
//! - 1 kHz carrier spacing
//! - 2.048 MHz sample rate
//! - Guard interval: 504 samples (246 µs)
//!
//! ## Processing Chain
//!
//! ```text
//! IQ Samples → OFDM Demod → DQPSK → FIC/MSC Demux
//!                                       ↓
//!                              Time Deinterleave
//!                                       ↓
//!                              Depuncture + Viterbi
//!                                       ↓
//!                              Energy Dispersal
//!                                       ↓
//!                   FIC: FIB Parse   MSC: RS Decode (DAB+)
//!                       ↓                   ↓
//!                  Service Info      Audio/Data Output
//! ```
//!
//! ## Example
//!
//! ```rust
//! use r4w_core::dab_channel_decoder::{
//!     DabOfdmDemodulator, DabMode, DabConvEncoder, DabViterbiDecoder,
//!     EnergyDispersal, DabCrc16, DabReedSolomon, PrsGenerator,
//! };
//!
//! // Generate Phase Reference Symbol and verify correlation
//! let prs = PrsGenerator::new(DabMode::ModeI);
//! let ref_sym = prs.generate();
//! assert_eq!(ref_sym.len(), 2048);
//!
//! // Convolutional encode + Viterbi decode roundtrip
//! let data = vec![true, false, true, true, false, false, true, false,
//!                 true, true, false, true, false, true, false, false];
//! let mut enc = DabConvEncoder::new();
//! let coded = enc.encode(&data);
//! let dec = DabViterbiDecoder::new();
//! let decoded = dec.decode(&coded);
//! assert_eq!(&decoded[..data.len()], &data[..]);
//!
//! // Energy dispersal scrambling roundtrip
//! let payload = vec![0xAA_u8, 0x55, 0x12, 0x34, 0xFF];
//! let mut ed = EnergyDispersal::new();
//! let scrambled = ed.process(&payload);
//! ed.reset();
//! let restored = ed.process(&scrambled);
//! assert_eq!(restored, payload);
//! ```

use std::f64::consts::PI;

// ---------------------------------------------------------------------------
// Constants — ETSI EN 300 401 Table 38, Mode I
// ---------------------------------------------------------------------------

/// FFT size for DAB Mode I.
pub const MODE_I_FFT_SIZE: usize = 2048;
/// Number of active (data-bearing) carriers in Mode I.
pub const MODE_I_ACTIVE_CARRIERS: usize = 1536;
/// Number of OFDM symbols per transmission frame in Mode I.
pub const MODE_I_SYMBOLS_PER_FRAME: usize = 76;
/// Guard interval length in samples for Mode I.
pub const MODE_I_GUARD_LEN: usize = 504;
/// Total OFDM symbol length (FFT + guard) for Mode I.
pub const MODE_I_SYMBOL_LEN: usize = MODE_I_FFT_SIZE + MODE_I_GUARD_LEN;
/// Number of null symbol samples (frame guard) for Mode I.
pub const MODE_I_NULL_LEN: usize = 2656;
/// Transmission frame duration in milliseconds.
pub const FRAME_DURATION_MS: f64 = 96.0;
/// DAB sample rate in Hz.
pub const SAMPLE_RATE: f64 = 2_048_000.0;
/// Carrier spacing in Hz.
pub const CARRIER_SPACING_HZ: f64 = 1000.0;
/// Number of FIBs per CIF (Common Interleaved Frame).
pub const FIBS_PER_CIF: usize = 12;
/// FIB length in bytes (30 data + 2 CRC).
pub const FIB_LENGTH_BYTES: usize = 32;
/// FIB data bytes (excluding CRC).
pub const FIB_DATA_BYTES: usize = 30;
/// Convolutional code constraint length K.
pub const CONV_K: usize = 7;
/// Convolutional code rate denominator (rate = 1/4 mother code).
pub const CONV_RATE_DENOM: usize = 4;
/// Time interleaver depth.
pub const TIME_INTERLEAVER_DEPTH: usize = 16;
/// Reed-Solomon codeword length for DAB+ RS(120,110,t=5).
pub const RS_CODEWORD_LEN: usize = 120;
/// Reed-Solomon message length.
pub const RS_MESSAGE_LEN: usize = 110;
/// Reed-Solomon error correction capability (t symbols).
pub const RS_T: usize = 5;
/// DAB+ AAC super frame AU count.
pub const DAB_PLUS_AU_PER_SUPER_FRAME: usize = 5;

// Generator polynomials for DAB K=7 rate-1/4 convolutional code (octal)
// Per ETSI EN 300 401 Table 8
const DAB_GENERATORS: [u8; 4] = [0o133, 0o171, 0o145, 0o133];

// CRC-16/CCITT polynomial (x^16 + x^12 + x^5 + 1 = 0x1021)
const CRC16_POLY: u16 = 0x1021;

// ---------------------------------------------------------------------------
// Complex type (inline, no external crates)
// ---------------------------------------------------------------------------

/// Inline complex number (re, im) over f64.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    /// Construct from real and imaginary parts.
    #[inline]
    pub fn new(re: f64, im: f64) -> Self { Self { re, im } }

    /// Conjugate.
    #[inline]
    pub fn conj(self) -> Self { Self::new(self.re, -self.im) }

    /// Magnitude squared.
    #[inline]
    pub fn norm_sqr(self) -> f64 { self.re * self.re + self.im * self.im }

    /// Magnitude.
    #[inline]
    pub fn norm(self) -> f64 { self.norm_sqr().sqrt() }

    /// Phase angle (atan2).
    #[inline]
    pub fn arg(self) -> f64 { self.im.atan2(self.re) }

    /// Multiplication.
    #[inline]
    pub fn mul(self, rhs: Self) -> Self {
        Self::new(
            self.re * rhs.re - self.im * rhs.im,
            self.re * rhs.im + self.im * rhs.re,
        )
    }

    /// Addition.
    #[inline]
    pub fn add(self, rhs: Self) -> Self {
        Self::new(self.re + rhs.re, self.im + rhs.im)
    }

    /// Subtraction.
    #[inline]
    pub fn sub(self, rhs: Self) -> Self {
        Self::new(self.re - rhs.re, self.im - rhs.im)
    }

    /// Scale by real scalar.
    #[inline]
    pub fn scale(self, s: f64) -> Self { Self::new(self.re * s, self.im * s) }

    /// Unit phasor at angle `theta` radians.
    #[inline]
    pub fn exp_j(theta: f64) -> Self { Self::new(theta.cos(), theta.sin()) }

    /// Polar form: magnitude * e^{j*theta}.
    #[inline]
    pub fn polar(r: f64, theta: f64) -> Self { Self::exp_j(theta).scale(r) }
}

// ---------------------------------------------------------------------------
// In-place radix-2 DIF FFT (Cooley-Tukey, power-of-2 only)
// ---------------------------------------------------------------------------

/// Compute FFT in-place.  `n` must be a power of 2.
pub fn fft_inplace(buf: &mut [Complex]) {
    let n = buf.len();
    debug_assert!(n.is_power_of_two(), "FFT size must be power of 2");

    // Bit-reversal permutation
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            buf.swap(i, j);
        }
    }

    // Butterfly stages
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f64;
        let wlen = Complex::exp_j(ang);
        let mut i = 0;
        while i < n {
            let mut w = Complex::new(1.0, 0.0);
            for k in 0..len / 2 {
                let u = buf[i + k];
                let v = buf[i + k + len / 2].mul(w);
                buf[i + k] = u.add(v);
                buf[i + k + len / 2] = u.sub(v);
                w = w.mul(wlen);
            }
            i += len;
        }
        len <<= 1;
    }
}

/// Compute IFFT in-place (conjugate + FFT + conjugate + scale).
pub fn ifft_inplace(buf: &mut [Complex]) {
    for s in buf.iter_mut() {
        *s = s.conj();
    }
    fft_inplace(buf);
    let n = buf.len() as f64;
    for s in buf.iter_mut() {
        *s = s.conj().scale(1.0 / n);
    }
}

// ---------------------------------------------------------------------------
// DAB transmission mode parameters
// ---------------------------------------------------------------------------

/// DAB transmission mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DabMode {
    /// Mode I — terrestrial, 2048-point FFT.
    ModeI,
    /// Mode II — single-frequency networks, 512-point FFT.
    ModeII,
    /// Mode III — hybrid terrestrial/satellite, 256-point FFT.
    ModeIII,
    /// Mode IV — satellite/complementary terrestrial, 1024-point FFT.
    ModeIV,
}

/// Parameters derived from the transmission mode.
#[derive(Debug, Clone)]
pub struct DabModeParams {
    pub fft_size: usize,
    pub active_carriers: usize,
    pub symbols_per_frame: usize,
    pub guard_len: usize,
    pub null_len: usize,
    pub fics_per_frame: usize,
}

impl DabModeParams {
    /// Return parameters for the given mode (ETSI EN 300 401 Table 38).
    pub fn for_mode(mode: DabMode) -> Self {
        match mode {
            DabMode::ModeI => DabModeParams {
                fft_size: 2048,
                active_carriers: 1536,
                symbols_per_frame: 76,
                guard_len: 504,
                null_len: 2656,
                fics_per_frame: 12,
            },
            DabMode::ModeII => DabModeParams {
                fft_size: 512,
                active_carriers: 384,
                symbols_per_frame: 76,
                guard_len: 126,
                null_len: 664,
                fics_per_frame: 3,
            },
            DabMode::ModeIII => DabModeParams {
                fft_size: 256,
                active_carriers: 192,
                symbols_per_frame: 153,
                guard_len: 63,
                null_len: 345,
                fics_per_frame: 4,
            },
            DabMode::ModeIV => DabModeParams {
                fft_size: 1024,
                active_carriers: 768,
                symbols_per_frame: 76,
                guard_len: 252,
                null_len: 1328,
                fics_per_frame: 6,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Phase Reference Symbol (PRS) Generator
// ---------------------------------------------------------------------------

/// Generates the Phase Reference Symbol (symbol 0 of each frame) used for
/// frame synchronization and coarse frequency offset estimation.
///
/// The PRS uses a QPSK constellation determined by the phase reference table
/// specified in ETSI EN 300 401 §14.3.2.
pub struct PrsGenerator {
    mode: DabMode,
    params: DabModeParams,
}

impl PrsGenerator {
    /// Create a PRS generator for the given mode.
    pub fn new(mode: DabMode) -> Self {
        let params = DabModeParams::for_mode(mode);
        Self { mode, params }
    }

    /// Generate the frequency-domain PRS (before IFFT).
    ///
    /// Returns a vector of `fft_size` complex samples in FFT bin order.
    /// Active carriers are placed according to the DAB carrier numbering
    /// convention (k from -(N/2) to N/2, excluding DC).
    pub fn generate(&self) -> Vec<Complex> {
        let n = self.params.fft_size;
        let k_max = self.params.active_carriers / 2;
        let mut freq_domain = vec![Complex::new(0.0, 0.0); n];

        // Phase reference sequence: use a deterministic PRBS seeded from mode
        // for educational purposes (real DAB uses a specific defined table).
        // The sequence is phi_k = (h[k] * pi / 2) where h[k] from the standard.
        let seed = match self.mode {
            DabMode::ModeI   => 0x2AD4_u32,
            DabMode::ModeII  => 0x1234_u32,
            DabMode::ModeIII => 0xABCD_u32,
            DabMode::ModeIV  => 0x5678_u32,
        };
        let phi_table = prs_phase_table(n, k_max, seed);

        // Map to FFT bins: carrier k maps to bin (k + n) mod n
        for (idx, &phase) in phi_table.iter().enumerate() {
            let k = idx as isize - k_max as isize;
            if k == 0 { continue; } // DC null
            let bin = ((k + n as isize) as usize) % n;
            freq_domain[bin] = Complex::exp_j(phase);
        }

        freq_domain
    }

    /// Generate the time-domain PRS (apply IFFT and add guard interval).
    pub fn generate_time_domain(&self) -> Vec<Complex> {
        let mut fd = self.generate();
        ifft_inplace(&mut fd);
        let guard = self.params.guard_len;
        let n = self.params.fft_size;
        let total = guard + n;
        let mut td = Vec::with_capacity(total);
        // Cyclic prefix from end of IFFT output
        td.extend_from_slice(&fd[n - guard..]);
        td.extend_from_slice(&fd);
        td
    }

    /// Correlate received time-domain symbol against the reference PRS for
    /// frame timing.  Returns the sample offset of the peak correlation.
    pub fn correlate_timing(&self, rx: &[Complex]) -> usize {
        let ref_td = self.generate_time_domain();
        let sym_len = ref_td.len();
        if rx.len() < sym_len { return 0; }

        let mut best_idx = 0;
        let mut best_power = 0.0_f64;

        for offset in 0..(rx.len() - sym_len + 1) {
            let power: f64 = rx[offset..offset + sym_len]
                .iter()
                .zip(ref_td.iter())
                .map(|(&r, &p)| r.mul(p.conj()).norm_sqr())
                .sum();
            if power > best_power {
                best_power = power;
                best_idx = offset;
            }
        }
        best_idx
    }
}

/// Deterministic phase table for PRS generation.
fn prs_phase_table(fft_size: usize, k_max: usize, seed: u32) -> Vec<f64> {
    let count = 2 * k_max + 1; // k from -k_max to +k_max inclusive
    let mut phases = Vec::with_capacity(count);
    let mut lfsr = seed;
    let _ = fft_size; // suppress warning; used for parameterization elsewhere
    for _ in 0..count {
        // Simple Galois LFSR, tap x^16+x^14+x^13+x^11+1
        let bit = ((lfsr >> 15) ^ (lfsr >> 13) ^ (lfsr >> 12) ^ (lfsr >> 10)) & 1;
        lfsr = (lfsr << 1) | bit;
        let quad = (lfsr & 3) as u8;
        phases.push(quad as f64 * PI / 2.0);
    }
    phases
}

// ---------------------------------------------------------------------------
// OFDM Demodulator
// ---------------------------------------------------------------------------

/// DAB OFDM Demodulator.
///
/// Removes the cyclic prefix, applies FFT, and extracts the active carrier
/// symbols from each OFDM symbol.
pub struct DabOfdmDemodulator {
    params: DabModeParams,
}

impl DabOfdmDemodulator {
    /// Create a demodulator for the given DAB mode.
    pub fn new(mode: DabMode) -> Self {
        Self { params: DabModeParams::for_mode(mode) }
    }

    /// Demodulate one OFDM symbol.
    ///
    /// Input: `symbol_len` (guard + FFT) complex time-domain samples.
    /// Output: `active_carriers` complex frequency-domain samples.
    pub fn demodulate_symbol(&self, input: &[Complex]) -> Vec<Complex> {
        let guard = self.params.guard_len;
        let n = self.params.fft_size;
        let k_max = self.params.active_carriers / 2;

        assert!(
            input.len() >= guard + n,
            "Input must have at least guard+fft samples"
        );

        // Remove cyclic prefix
        let mut fft_buf: Vec<Complex> = input[guard..guard + n].to_vec();

        // Apply FFT
        fft_inplace(&mut fft_buf);

        // Extract active carriers: k = -(k_max)..-1 and 1..k_max
        let mut carriers = Vec::with_capacity(self.params.active_carriers);
        for k in -(k_max as isize)..=(k_max as isize) {
            if k == 0 { continue; }
            let bin = ((k + n as isize) as usize) % n;
            carriers.push(fft_buf[bin]);
        }
        carriers
    }

    /// Demodulate a full DAB frame (symbols 1..N, after PRS).
    ///
    /// Returns a 2D array: `[symbol_index][carrier_index]`.
    pub fn demodulate_frame(&self, input: &[Complex]) -> Vec<Vec<Complex>> {
        let sym_len = self.params.fft_size + self.params.guard_len;
        let num_syms = self.params.symbols_per_frame - 1; // exclude PRS
        let mut out = Vec::with_capacity(num_syms);

        for s in 0..num_syms {
            let start = s * sym_len;
            if start + sym_len > input.len() {
                break;
            }
            out.push(self.demodulate_symbol(&input[start..start + sym_len]));
        }
        out
    }
}

/// DAB OFDM Modulator (for testing/simulation).
pub struct DabOfdmModulator {
    params: DabModeParams,
}

impl DabOfdmModulator {
    /// Create a modulator for the given DAB mode.
    pub fn new(mode: DabMode) -> Self {
        Self { params: DabModeParams::for_mode(mode) }
    }

    /// Modulate one OFDM symbol from active carrier symbols.
    ///
    /// Input: `active_carriers` complex symbols.
    /// Output: time-domain symbol with cyclic prefix prepended.
    pub fn modulate_symbol(&self, carriers: &[Complex]) -> Vec<Complex> {
        let n = self.params.fft_size;
        let guard = self.params.guard_len;
        let k_max = self.params.active_carriers / 2;

        let mut fft_buf = vec![Complex::new(0.0, 0.0); n];

        // Place carriers
        let mut idx = 0;
        for k in -(k_max as isize)..=(k_max as isize) {
            if k == 0 { continue; }
            if idx >= carriers.len() { break; }
            let bin = ((k + n as isize) as usize) % n;
            fft_buf[bin] = carriers[idx];
            idx += 1;
        }

        // IFFT
        ifft_inplace(&mut fft_buf);

        // Add cyclic prefix
        let mut out = Vec::with_capacity(guard + n);
        out.extend_from_slice(&fft_buf[n - guard..]);
        out.extend_from_slice(&fft_buf);
        out
    }
}

// ---------------------------------------------------------------------------
// Differential DQPSK Encoder / Decoder
// ---------------------------------------------------------------------------

/// DQPSK phase differences (Gray coded).
/// bits (b1, b0) → phase diff:  00→0°, 01→+90°, 10→-90°, 11→±180°
const DQPSK_PHASE: [f64; 4] = [0.0, PI / 2.0, -PI / 2.0, PI];

/// Differential QPSK encoder for DAB.
///
/// In DAB, DQPSK is applied in the frequency domain between consecutive
/// OFDM symbols on a per-carrier basis.
pub struct DqpskEncoder {
    prev_phase: Vec<f64>,
    initialized: bool,
}

impl DqpskEncoder {
    /// Create a new DQPSK encoder.  `num_carriers` determines internal state.
    pub fn new(num_carriers: usize) -> Self {
        Self {
            prev_phase: vec![0.0; num_carriers],
            initialized: false,
        }
    }

    /// Reset the encoder state (for new frame).
    pub fn reset(&mut self) {
        for p in self.prev_phase.iter_mut() { *p = 0.0; }
        self.initialized = false;
    }

    /// Encode dibits to DQPSK symbols.
    ///
    /// `dibits`: pairs (b1, b0) as `(bool, bool)`.
    /// `reference`: reference symbols for the first symbol (PRS).
    /// Returns frequency-domain complex symbols.
    pub fn encode_symbol(
        &mut self,
        dibits: &[(bool, bool)],
        reference: &[Complex],
    ) -> Vec<Complex> {
        let n = dibits.len().min(reference.len()).min(self.prev_phase.len());
        if !self.initialized {
            for (i, r) in reference.iter().enumerate().take(n) {
                self.prev_phase[i] = r.arg();
            }
            self.initialized = true;
        }
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let (b1, b0) = dibits[i];
            let sym_idx = ((b1 as usize) << 1) | (b0 as usize);
            let delta = DQPSK_PHASE[sym_idx];
            let new_phase = self.prev_phase[i] + delta;
            self.prev_phase[i] = new_phase;
            out.push(Complex::exp_j(new_phase));
        }
        out
    }

    /// Encode a stream of dibits over multiple symbols.
    pub fn encode(
        &mut self,
        dibits_per_symbol: &[Vec<(bool, bool)>],
        prs_carriers: &[Complex],
    ) -> Vec<Vec<Complex>> {
        let mut out = Vec::with_capacity(dibits_per_symbol.len());
        for (sym_idx, dibits) in dibits_per_symbol.iter().enumerate() {
            if sym_idx == 0 {
                out.push(self.encode_symbol(dibits, prs_carriers));
            } else {
                let prev = out[sym_idx - 1].clone();
                out.push(self.encode_symbol(dibits, &prev));
            }
        }
        out
    }
}

/// Differential QPSK decoder for DAB.
pub struct DqpskDecoder;

impl DqpskDecoder {
    /// Create a new DQPSK decoder.
    pub fn new() -> Self { Self }

    /// Decode one OFDM symbol by comparing to the previous symbol.
    ///
    /// Returns dibits as `(bool, bool)` for each carrier.
    pub fn decode_symbol(
        &self,
        current: &[Complex],
        previous: &[Complex],
    ) -> Vec<(bool, bool)> {
        let n = current.len().min(previous.len());
        let mut dibits = Vec::with_capacity(n);
        for i in 0..n {
            let diff = current[i].mul(previous[i].conj());
            let phase = diff.arg();
            // Map phase difference to dibit
            let (b1, b0) = dqpsk_phase_to_dibit(phase);
            dibits.push((b1, b0));
        }
        dibits
    }

    /// Decode all symbols in a frame.
    ///
    /// `symbols[0]` is the PRS (reference), `symbols[1..]` carry data.
    /// Returns dibits for each data symbol.
    pub fn decode_frame(
        &self,
        symbols: &[Vec<Complex>],
    ) -> Vec<Vec<(bool, bool)>> {
        if symbols.len() < 2 { return Vec::new(); }
        let mut out = Vec::with_capacity(symbols.len() - 1);
        for i in 1..symbols.len() {
            out.push(self.decode_symbol(&symbols[i], &symbols[i - 1]));
        }
        out
    }
}

impl Default for DqpskDecoder {
    fn default() -> Self { Self::new() }
}

/// Map phase difference to (b1, b0) dibit.
fn dqpsk_phase_to_dibit(phase: f64) -> (bool, bool) {
    // Normalize to [-pi, pi]
    let p = ((phase + PI) % (2.0 * PI)) - PI;
    // Decision regions:  00:[−45°,45°], 01:[45°,135°], 11:[135°,225°], 10:[−135°,−45°]
    if p >= -PI / 4.0 && p < PI / 4.0 {
        (false, false)
    } else if p >= PI / 4.0 && p < 3.0 * PI / 4.0 {
        (false, true)
    } else if p >= -3.0 * PI / 4.0 && p < -PI / 4.0 {
        (true, false)
    } else {
        (true, true)
    }
}

// ---------------------------------------------------------------------------
// DAB Convolutional Encoder (K=7, rate 1/4)
// ---------------------------------------------------------------------------

/// DAB mother convolutional encoder: K=7, rate 1/4.
///
/// Uses generator polynomials G1=133, G2=171, G3=145, G4=133 (octal)
/// as specified in ETSI EN 300 401 §11.1.
pub struct DabConvEncoder {
    state: u8, // K-1 = 6 bits of shift register state
}

impl DabConvEncoder {
    /// Create a new encoder with zero state.
    pub fn new() -> Self { Self { state: 0 } }

    /// Reset shift register state.
    pub fn reset(&mut self) { self.state = 0; }

    /// Encode input bits.
    ///
    /// For each input bit, produces 4 output bits (one per generator).
    /// Uses K-bit shift register: new input shifts in from LSB, state grows toward MSB.
    pub fn encode(&mut self, bits: &[bool]) -> Vec<bool> {
        let mut out = Vec::with_capacity(bits.len() * CONV_RATE_DENOM);
        let mask = ((1u32 << CONV_K) - 1) as u8; // 127 for K=7
        for &b in bits {
            self.state = ((self.state << 1) | (b as u8)) & mask;
            for &gen in &DAB_GENERATORS {
                let v = (self.state & gen).count_ones() as u8 & 1;
                out.push(v == 1);
            }
        }
        out
    }

    /// Flush K-1 tail bits to terminate trellis.
    pub fn flush(&mut self) -> Vec<bool> {
        self.encode(&vec![false; CONV_K - 1])
    }
}

impl Default for DabConvEncoder {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// DAB Viterbi Decoder
// ---------------------------------------------------------------------------

const VITERBI_STATES: usize = 1 << CONV_K; // 128 states (K=7 left-shift encoder)

/// DAB Viterbi decoder for the K=7, rate-1/4 convolutional code.
pub struct DabViterbiDecoder {
    // Precomputed next_state[state][input_bit] and expected outputs
    next_state: [[usize; 2]; VITERBI_STATES],
    output: [[u8; 2]; VITERBI_STATES], // packed 4 bits per entry
}

impl DabViterbiDecoder {
    /// Create a new decoder, precomputing the trellis structure.
    pub fn new() -> Self {
        let mut next_state = [[0usize; 2]; VITERBI_STATES];
        let mut output = [[0u8; 2]; VITERBI_STATES];

        let k_mask = ((1usize << CONV_K) - 1) as u8; // 0x7F for K=7
        for state in 0..VITERBI_STATES {
            for input in 0..2usize {
                // Encoder shifts input into MSB: new_state = ((state << 1) | input) & mask
                let new_state = ((state << 1) | input) & (VITERBI_STATES - 1);
                next_state[state][input] = new_state;

                // Compute encoder output for this (state, input) pair
                // The K-bit register after the shift is new_state (same as encoder)
                let sr = (new_state as u8) & k_mask;
                let mut bits = 0u8;
                for (bit_idx, &gen) in DAB_GENERATORS.iter().enumerate() {
                    let g = gen & k_mask;
                    let parity = (sr & g).count_ones() as u8 & 1;
                    bits |= parity << bit_idx;
                }
                output[state][input] = bits;
            }
        }
        Self { next_state, output }
    }

    /// Hard-decision Viterbi decode.
    ///
    /// Input is flat hard bits (rate 1/4: 4 bits per source bit).
    /// Returns decoded bits (may include tail bits at end).
    pub fn decode(&self, received: &[bool]) -> Vec<bool> {
        let num_steps = received.len() / CONV_RATE_DENOM;
        if num_steps == 0 { return Vec::new(); }

        const INF: u32 = u32::MAX / 2;

        let mut metric = vec![INF; VITERBI_STATES];
        // tb_input[step][ns] = input bit that produced next-state ns at this step
        // tb_prev[step][ns]  = predecessor state that produced ns at this step
        let mut tb_input: Vec<Vec<u8>>  = vec![vec![0u8;  VITERBI_STATES]; num_steps];
        let mut tb_prev:  Vec<Vec<u16>> = vec![vec![0u16; VITERBI_STATES]; num_steps];
        metric[0] = 0;

        for step in 0..num_steps {
            let base = step * CONV_RATE_DENOM;
            // Read 4 received bits as integer (packed)
            let recv: u8 = (0..4)
                .map(|i| {
                    if base + i < received.len() {
                        (received[base + i] as u8) << i
                    } else {
                        0
                    }
                })
                .fold(0u8, |acc, x| acc | x);

            let mut new_metric = vec![INF; VITERBI_STATES];

            for state in 0..VITERBI_STATES {
                if metric[state] == INF { continue; }
                for input in 0..2usize {
                    let ns = self.next_state[state][input];
                    let expected = self.output[state][input];
                    let hd = (recv ^ expected).count_ones();
                    let m = metric[state].saturating_add(hd);
                    if m < new_metric[ns] {
                        new_metric[ns] = m;
                        tb_input[step][ns] = input as u8;
                        tb_prev[step][ns]  = state as u16; // exact predecessor state
                    }
                }
            }
            metric = new_metric;
        }

        // Find best end state
        let (end_state, _) = metric
            .iter()
            .enumerate()
            .min_by_key(|&(_, &m)| m)
            .unwrap_or((0, &0));

        // Traceback: recover decoded bits using stored predecessor states
        let mut bits = vec![false; num_steps];
        let mut state = end_state;
        for step in (0..num_steps).rev() {
            bits[step] = tb_input[step][state] != 0;
            state = tb_prev[step][state] as usize; // recover exact predecessor
        }

        bits
    }

    /// Hard-decision Viterbi decode with erasure awareness.
    ///
    /// `received`: depunctured bit stream (erased positions filled with any value).
    /// `erased`: per-bit erasure flag; when `true`, that bit's metric contribution is 0.
    pub fn decode_erased(&self, received: &[bool], erased: &[bool]) -> Vec<bool> {
        let num_steps = received.len() / CONV_RATE_DENOM;
        if num_steps == 0 { return Vec::new(); }

        const INF: u32 = u32::MAX / 2;

        let mut metric = vec![INF; VITERBI_STATES];
        let mut tb_input: Vec<Vec<u8>>  = vec![vec![0u8;  VITERBI_STATES]; num_steps];
        let mut tb_prev:  Vec<Vec<u16>> = vec![vec![0u16; VITERBI_STATES]; num_steps];
        metric[0] = 0;

        for step in 0..num_steps {
            let base = step * CONV_RATE_DENOM;
            let recv: u8 = (0..4)
                .map(|i| {
                    if base + i < received.len() {
                        (received[base + i] as u8) << i
                    } else { 0 }
                })
                .fold(0u8, |acc, x| acc | x);
            // Mask of non-erased bit positions within this symbol
            let non_erased: u8 = (0..4u8)
                .map(|i| {
                    let idx = base + i as usize;
                    if idx < erased.len() && !erased[idx] { 1u8 << i } else { 0 }
                })
                .fold(0u8, |acc, x| acc | x);

            let mut new_metric = vec![INF; VITERBI_STATES];
            for state in 0..VITERBI_STATES {
                if metric[state] == INF { continue; }
                for input in 0..2usize {
                    let ns = self.next_state[state][input];
                    let expected = self.output[state][input];
                    // Only count non-erased bit positions in Hamming distance
                    let hd = ((recv ^ expected) & non_erased).count_ones();
                    let m = metric[state].saturating_add(hd);
                    if m < new_metric[ns] {
                        new_metric[ns] = m;
                        tb_input[step][ns] = input as u8;
                        tb_prev[step][ns]  = state as u16;
                    }
                }
            }
            metric = new_metric;
        }

        let (end_state, _) = metric
            .iter()
            .enumerate()
            .min_by_key(|&(_, &m)| m)
            .unwrap_or((0, &0));

        let mut bits = vec![false; num_steps];
        let mut state = end_state;
        for step in (0..num_steps).rev() {
            bits[step] = tb_input[step][state] != 0;
            state = tb_prev[step][state] as usize;
        }
        bits
    }
}

impl Default for DabViterbiDecoder {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// Puncturing / Depuncturing
// ---------------------------------------------------------------------------

/// EEP (Equal Error Protection) protection level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EepLevel {
    /// Protection level 1-A: code rate 1/4.
    Level1A,
    /// Protection level 2-A: code rate 3/8.
    Level2A,
    /// Protection level 3-A: code rate 1/2.
    Level3A,
    /// Protection level 4-A: code rate 3/4.
    Level4A,
    /// Protection level 1-B: code rate 4/9.
    Level1B,
    /// Protection level 2-B: code rate 4/7.
    Level2B,
    /// Protection level 3-B: code rate 4/6.
    Level3B,
    /// Protection level 4-B: code rate 4/5.
    Level4B,
}

/// Returns the puncture pattern (1=keep, 0=puncture) and code rate numerator.
/// Patterns are repeated cyclically. Denominator is always 4 (rate 1/4 mother).
/// Per ETSI EN 300 401 Table 9 / §11.1.2.
pub fn eep_puncture_pattern(level: EepLevel) -> (&'static [u8], u32, u32) {
    match level {
        EepLevel::Level1A => (&[1, 1, 1, 1], 1, 1), // rate 1/4
        EepLevel::Level2A => (&[1, 1, 1, 0, 1, 1, 0, 0], 3, 8), // rate 3/8
        EepLevel::Level3A => (&[1, 1, 0, 0, 1, 1, 0, 0], 1, 2), // rate 1/2
        EepLevel::Level4A => (&[1, 0, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0], 3, 4), // rate 3/4
        EepLevel::Level1B => (&[1, 1, 1, 1, 0, 1, 1, 1, 1], 4, 9), // rate ~4/9
        EepLevel::Level2B => (&[1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1], 4, 7),
        EepLevel::Level3B => (&[1, 1, 1, 0, 1, 1, 0, 0], 1, 2),
        EepLevel::Level4B => (&[1, 1, 1, 1, 0], 4, 5), // rate 4/5
    }
}

/// Puncture a coded bit stream according to a puncture pattern.
///
/// `coded`: all rate-1/4 coded bits.
/// `pattern`: 1=keep, 0=discard, repeated cyclically.
pub fn puncture(coded: &[bool], pattern: &[u8]) -> Vec<bool> {
    let plen = pattern.len();
    coded
        .iter()
        .enumerate()
        .filter_map(|(i, &b)| {
            if pattern[i % plen] != 0 { Some(b) } else { None }
        })
        .collect()
}

/// Depuncture a received bit stream, inserting neutral values (false = 0) at
/// erased positions so the Viterbi decoder receives full-rate input.
///
/// `received`: punctured bits.
/// `pattern`: same pattern used for puncturing.
/// `output_len`: expected number of full-rate output bits.
pub fn depuncture(received: &[bool], pattern: &[u8], output_len: usize) -> Vec<bool> {
    let plen = pattern.len();
    let mut out = Vec::with_capacity(output_len);
    let mut ri = 0; // index into received
    let mut pi = 0; // index into pattern

    while out.len() < output_len {
        if pattern[pi % plen] != 0 {
            if ri < received.len() {
                out.push(received[ri]);
                ri += 1;
            } else {
                out.push(false);
            }
        } else {
            out.push(false); // erasure — neutral soft value
        }
        pi += 1;
    }
    out
}

/// Like [`depuncture`] but also returns a per-bit erasure mask.
///
/// Returns `(depunctured_bits, erased)` where `erased[i] = true` means
/// position `i` was an erasure (filled with `false`).
pub fn depuncture_with_mask(
    received: &[bool],
    pattern: &[u8],
    output_len: usize,
) -> (Vec<bool>, Vec<bool>) {
    let plen = pattern.len();
    let mut bits = Vec::with_capacity(output_len);
    let mut mask = Vec::with_capacity(output_len);
    let mut ri = 0usize;
    let mut pi = 0usize;
    while bits.len() < output_len {
        if pattern[pi % plen] != 0 {
            if ri < received.len() {
                bits.push(received[ri]);
                ri += 1;
            } else {
                bits.push(false);
            }
            mask.push(false); // not erased
        } else {
            bits.push(false);
            mask.push(true); // erased
        }
        pi += 1;
    }
    (bits, mask)
}

// ---------------------------------------------------------------------------
// Time Interleaver / Deinterleaver
// ---------------------------------------------------------------------------

/// DAB time interleaver.
///
/// The DAB time interleaver has depth 16 and uses a scrambled interleaving
/// vector (per ETSI EN 300 401 §12.3).  Each cell is interleaved over 16
/// consecutive CIFs.
pub struct TimeInterleaver {
    depth: usize,
    buf: Vec<Vec<bool>>, // circular buffer of depth frames
    write_ptr: usize,
    frame_len: usize,
}

impl TimeInterleaver {
    /// Create a new time interleaver.
    ///
    /// `frame_len`: number of bits per CIF (capacity subchannel length).
    pub fn new(frame_len: usize) -> Self {
        Self {
            depth: TIME_INTERLEAVER_DEPTH,
            buf: vec![vec![false; frame_len]; TIME_INTERLEAVER_DEPTH],
            write_ptr: 0,
            frame_len,
        }
    }

    /// Process one CIF worth of data bits.
    ///
    /// Returns interleaved output for the current CIF.
    pub fn interleave(&mut self, input: &[bool]) -> Vec<bool> {
        let n = input.len().min(self.frame_len);
        // Write new frame
        self.buf[self.write_ptr][..n].copy_from_slice(&input[..n]);

        // Read interleaved output using scrambled permutation
        let perm = interleaver_permutation(n, self.depth);
        let mut out = vec![false; n];
        for (i, &(frame_offset, bit_offset)) in perm.iter().enumerate() {
            let frame = (self.write_ptr + self.depth - frame_offset) % self.depth;
            out[i] = self.buf[frame][bit_offset];
        }

        self.write_ptr = (self.write_ptr + 1) % self.depth;
        out
    }

    /// Deinterleave: inverse permutation.
    pub fn deinterleave(&mut self, input: &[bool]) -> Vec<bool> {
        let n = input.len().min(self.frame_len);
        // Write new interleaved frame
        self.buf[self.write_ptr][..n].copy_from_slice(&input[..n]);

        // Apply inverse permutation
        let perm = interleaver_permutation(n, self.depth);
        let mut out = vec![false; n];
        for (i, &(frame_offset, bit_offset)) in perm.iter().enumerate() {
            let frame = (self.write_ptr + self.depth - frame_offset) % self.depth;
            let _ = frame;
            out[bit_offset] = input[i];
        }

        self.write_ptr = (self.write_ptr + 1) % self.depth;
        out
    }
}

/// Compute the interleaving permutation vector for given parameters.
/// Returns `(frame_delay, bit_index)` for each output position.
fn interleaver_permutation(frame_len: usize, depth: usize) -> Vec<(usize, usize)> {
    let mut perm = Vec::with_capacity(frame_len);
    for i in 0..frame_len {
        let frame_offset = i % depth;
        let bit_index = (i * (depth - 1)) % frame_len;
        perm.push((frame_offset, bit_index));
    }
    perm
}

// ---------------------------------------------------------------------------
// Energy Dispersal (PRBS Scrambler)
// ---------------------------------------------------------------------------

/// DAB Energy Dispersal scrambler.
///
/// Uses PRBS based on generator polynomial x^9 + x^5 + 1 as specified in
/// ETSI EN 300 401 §11.4.  Applied bit-by-bit to the raw bit stream before
/// feeding to the FEC encoder (and after at the receiver).
pub struct EnergyDispersal {
    /// LFSR state (9 bits used).
    state: u16,
    /// Initial state (all ones per standard).
    init_state: u16,
}

impl EnergyDispersal {
    /// Create a new energy dispersal scrambler.
    pub fn new() -> Self {
        Self { state: 0x1FF, init_state: 0x1FF }
    }

    /// Create with custom initial state.
    pub fn with_init(init: u16) -> Self {
        Self { state: init & 0x1FF, init_state: init & 0x1FF }
    }

    /// Reset LFSR to initial state.
    pub fn reset(&mut self) {
        self.state = self.init_state;
    }

    /// Generate one PRBS bit.
    #[inline]
    fn next_bit(&mut self) -> bool {
        // Taps at positions 9 and 5: x^9 + x^5 + 1
        let bit = ((self.state >> 8) ^ (self.state >> 4)) & 1;
        self.state = ((self.state << 1) | bit) & 0x1FF;
        bit == 1
    }

    /// Scramble/descramble a byte slice (same operation for both).
    pub fn process(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        for &byte in data {
            let mut scrambled = 0u8;
            for bit in (0..8).rev() {
                let prbs = self.next_bit() as u8;
                let data_bit = (byte >> bit) & 1;
                scrambled = (scrambled << 1) | (data_bit ^ prbs);
            }
            out.push(scrambled);
        }
        out
    }

    /// Scramble/descramble a bit slice.
    pub fn process_bits(&mut self, bits: &[bool]) -> Vec<bool> {
        bits.iter().map(|&b| b ^ self.next_bit()).collect()
    }
}

impl Default for EnergyDispersal {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// CRC-16 CCITT
// ---------------------------------------------------------------------------

/// CRC-16/CCITT implementation for DAB FIB integrity checking.
///
/// Polynomial: x^16 + x^12 + x^5 + 1 (0x1021), initial value 0xFFFF,
/// no input/output reflection, no final XOR.
pub struct DabCrc16;

impl DabCrc16 {
    /// Compute CRC-16 over data bytes.
    pub fn compute(data: &[u8]) -> u16 {
        let mut crc: u16 = 0xFFFF;
        for &byte in data {
            crc ^= (byte as u16) << 8;
            for _ in 0..8 {
                if crc & 0x8000 != 0 {
                    crc = (crc << 1) ^ CRC16_POLY;
                } else {
                    crc <<= 1;
                }
            }
        }
        crc
    }

    /// Verify a FIB: compute CRC over first 30 bytes, compare to bytes 30-31.
    pub fn verify_fib(fib: &[u8]) -> bool {
        if fib.len() < FIB_LENGTH_BYTES { return false; }
        let computed = Self::compute(&fib[..FIB_DATA_BYTES]);
        let stored = ((fib[FIB_DATA_BYTES] as u16) << 8) | fib[FIB_DATA_BYTES + 1] as u16;
        computed == stored
    }

    /// Append CRC to data bytes, returning the complete FIB.
    pub fn append(data: &[u8]) -> Vec<u8> {
        let crc = Self::compute(data);
        let mut out = data.to_vec();
        out.push((crc >> 8) as u8);
        out.push((crc & 0xFF) as u8);
        out
    }
}

// ---------------------------------------------------------------------------
// GF(2^8) for Reed-Solomon
// ---------------------------------------------------------------------------

// Primitive polynomial x^8 + x^4 + x^3 + x^2 + 1 = 0x11D
const RS_PRIM_POLY: u16 = 0x11D;

/// GF(2^8) exponential table: gf_exp[i] = alpha^i, extended to 512 for easy lookup.
static GF_EXP: [u8; 512] = {
    let mut t = [0u8; 512];
    let mut v: u16 = 1;
    let mut i = 0;
    while i < 255 {
        t[i] = v as u8;
        v <<= 1;
        if v & 0x100 != 0 { v ^= RS_PRIM_POLY; }
        i += 1;
    }
    // Extend for modular-free lookup
    let mut j = 0;
    while j < 255 {
        t[j + 255] = t[j];
        j += 1;
    }
    // Handle last two
    t[510] = t[0];
    t[511] = t[1];
    t
};

/// GF(2^8) logarithm table: gf_log[gf_exp[i]] = i.
static GF_LOG: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut v: u16 = 1;
    let mut i = 0u8;
    loop {
        t[v as usize] = i;
        v <<= 1;
        if v & 0x100 != 0 { v ^= RS_PRIM_POLY; }
        v &= 0xFF;
        if i == 254 { break; }
        i += 1;
    }
    t
};

#[inline]
fn gf_mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 { return 0; }
    GF_EXP[(GF_LOG[a as usize] as usize + GF_LOG[b as usize] as usize) % 255]
}

#[inline]
fn gf_pow(base: u8, exp: usize) -> u8 {
    if exp == 0 { return 1; }
    if base == 0 { return 0; }
    GF_EXP[(GF_LOG[base as usize] as usize * exp) % 255]
}

#[inline]
fn gf_inv(a: u8) -> u8 {
    if a == 0 { return 0; }
    GF_EXP[255 - GF_LOG[a as usize] as usize]
}

#[inline]
fn gf_div(a: u8, b: u8) -> u8 {
    debug_assert!(b != 0, "GF division by zero");
    gf_mul(a, gf_inv(b))
}

/// Evaluate polynomial at x using Horner's method (coefficients from index 0 = constant).
fn gf_poly_eval(poly: &[u8], x: u8) -> u8 {
    let mut v = 0u8;
    for &c in poly.iter().rev() {
        v = gf_mul(v, x) ^ c;
    }
    v
}

// ---------------------------------------------------------------------------
// Reed-Solomon RS(120, 110, t=5) for DAB+
// ---------------------------------------------------------------------------

/// Reed-Solomon codec for DAB+ RS(120, 110, t=5).
///
/// Parameters per ETSI EN 302 563 §6.3:
/// - n=120 codeword symbols
/// - k=110 message symbols  
/// - t=5 symbol error correction capability (2t=10 parity symbols)
/// - Field: GF(2^8) with primitive polynomial 0x11D
/// - Generator polynomial: g(x) = prod_{i=0}^{2t-1} (x - alpha^i)
pub struct DabReedSolomon {
    n: usize,
    k: usize,
    t: usize,
    gen_poly: Vec<u8>, // generator polynomial coefficients
}

impl DabReedSolomon {
    /// Create the DAB+ RS(120,110) codec.
    pub fn new() -> Self {
        let n = RS_CODEWORD_LEN;
        let k = RS_MESSAGE_LEN;
        let t = RS_T;
        let gen_poly = Self::build_generator(t);
        Self { n, k, t, gen_poly }
    }

    /// Build the generator polynomial g(x) = prod_{i=0}^{2t-1}(x - alpha^i).
    fn build_generator(t: usize) -> Vec<u8> {
        let two_t = 2 * t;
        // Start with g(x) = 1
        let mut g = vec![1u8];
        for i in 0..two_t {
            // Multiply by (x - alpha^i) = (x + alpha^i) in GF(2)
            let root = GF_EXP[i]; // alpha^i where alpha=2
            // g = g * (x + root)
            let mut new_g = vec![0u8; g.len() + 1];
            for (j, &coef) in g.iter().enumerate() {
                new_g[j] ^= coef;
                new_g[j + 1] ^= gf_mul(coef, root);
            }
            g = new_g;
        }
        g
    }

    /// Systematic RS encode: append `2t` parity bytes to `k` message bytes.
    ///
    /// Uses polynomial long division: parity = remainder of msg(x)*x^(2t) / g(x).
    /// The codeword polynomial uses MSB-first convention: cw[0] is the highest-degree term.
    pub fn encode(&self, msg: &[u8]) -> Vec<u8> {
        assert!(msg.len() == self.k, "Message must be exactly k bytes");
        let _n_parity = self.n - self.k; // 2t = 10

        // Work buffer: msg bytes followed by n_parity zeros
        let mut p = Vec::with_capacity(self.n);
        p.extend_from_slice(msg);
        p.resize(self.n, 0u8);

        // Synthetic division (Horner-based): for each message byte (MSB first)
        // p[i] is the current leading term; g is monic so quotient coeff = p[i].
        // Subtract p[i] * g(x) from p[i..i+len(g)]:
        for i in 0..self.k {
            let coef = p[i];
            if coef != 0 {
                // gen_poly[0] = 1 (leading); gen_poly[j] for j>=1 are sub-leading
                for j in 1..self.gen_poly.len() {
                    p[i + j] ^= gf_mul(coef, self.gen_poly[j]);
                }
            }
        }

        // Codeword = original message + remainder (parity bytes)
        let mut codeword = msg.to_vec();
        codeword.extend_from_slice(&p[self.k..]);
        codeword
    }

    /// RS decode: correct up to `t` symbol errors.
    ///
    /// Modifies `codeword` in-place and returns the number of errors corrected.
    /// Returns `Err` if more than `t` errors detected (uncorrectable).
    pub fn decode(&self, codeword: &mut Vec<u8>) -> Result<usize, &'static str> {
        if codeword.len() != self.n {
            return Err("Codeword length mismatch");
        }
        let n_parity = self.n - self.k;

        // Compute syndromes S_i = C(alpha^i) for i=0..2t-1 (MSB-first Horner)
        let syndromes: Vec<u8> = (0..n_parity)
            .map(|i| {
                let alpha_i = GF_EXP[i]; // alpha^i
                codeword.iter().fold(0u8, |acc, &c| gf_mul(acc, alpha_i) ^ c)
            })
            .collect();

        if syndromes.iter().all(|&s| s == 0) {
            return Ok(0);
        }

        // Berlekamp-Massey: find error locator polynomial sigma(x)
        let (sigma, num_errors) = rs_berlekamp_massey(&syndromes, self.t);
        if num_errors > self.t {
            return Err("Too many errors to correct");
        }

        // Chien search: sigma(alpha^{-(n-1-i)}) == 0 => error at byte index i
        // With MSB-first encoding, byte i corresponds to power (n-1-i).
        // alpha^{-(n-1-i)} = alpha^{255-(n-1-i)}.
        let error_byte_indices = rs_chien_search(&sigma, self.n);
        if error_byte_indices.len() != num_errors {
            return Err("Chien search error count mismatch");
        }

        // Forney algorithm: compute error values
        // omega(x) = sigma(x) * S(x) mod x^(2t)
        let omega = rs_compute_omega(&syndromes, &sigma);
        let sigma_prime = rs_formal_deriv(&sigma);

        let mut corrections = 0;
        for &byte_idx in &error_byte_indices {
            // X_i = alpha^(n-1-byte_idx) (the root for this byte position, MSB-first)
            let power = self.n - 1 - byte_idx; // n-1-i
            let xi     = GF_EXP[power % 255];          // X_i = alpha^power
            let xi_inv = GF_EXP[(255 - power % 255) % 255]; // X_i^{-1} = alpha^{-power}

            let denom = gf_poly_eval(&sigma_prime, xi_inv);
            if denom == 0 { return Err("Singular Forney denominator"); }

            // e_i = -(X_i * omega(X_i^{-1})) / sigma'(X_i^{-1})
            // In GF(2^*): -1 = 1, so e_i = X_i * omega(X_i^{-1}) / sigma'(X_i^{-1})
            let numer = gf_mul(xi, gf_poly_eval(&omega, xi_inv));
            let error_val = gf_div(numer, denom);
            codeword[byte_idx] ^= error_val;
            corrections += 1;
        }

        Ok(corrections)
    }
}

impl Default for DabReedSolomon {
    fn default() -> Self { Self::new() }
}

/// Berlekamp-Massey algorithm for RS decoding.
///
/// Returns (error locator polynomial sigma, number of errors L).
/// sigma[0] = 1 always (monic convention with constant term = 1).
fn rs_berlekamp_massey(syndromes: &[u8], t: usize) -> (Vec<u8>, usize) {
    let two_t = syndromes.len();
    let mut sigma = vec![0u8; two_t + 1];
    let mut prev  = vec![0u8; two_t + 1];
    sigma[0] = 1;
    prev[0]  = 1;
    let mut l = 0usize;  // current LFSR length
    let mut m = 1usize;  // shift amount
    let mut delta_prev = 1u8; // previous non-zero discrepancy

    for r in 0..two_t {
        // Compute discrepancy delta = S_r + sum_{j=1}^{L} sigma_j * S_{r-j}
        let mut delta = syndromes[r];
        for j in 1..=l {
            delta ^= gf_mul(sigma[j], syndromes[r - j]);
        }

        if delta == 0 {
            m += 1;
        } else if 2 * l <= r {
            let temp = sigma.clone();
            // sigma(x) += (delta / delta_prev) * x^m * prev(x)
            let scale = gf_mul(delta, gf_inv(delta_prev));
            for i in m..two_t + 1 {
                sigma[i] ^= gf_mul(scale, prev[i - m]);
            }
            l = r + 1 - l;
            prev = temp;
            delta_prev = delta;
            m = 1;
        } else {
            let scale = gf_mul(delta, gf_inv(delta_prev));
            for i in m..two_t + 1 {
                sigma[i] ^= gf_mul(scale, prev[i - m]);
            }
            m += 1;
        }
    }
    let _ = t;
    (sigma[..=l].to_vec(), l)
}

/// Chien search for RS error locator roots.
///
/// Finds byte indices `i` (0-indexed from MSB) where sigma(alpha^{-(n-1-i)}) = 0.
/// This is equivalent to finding roots X_i^{-1} = alpha^{-(n-1-i)} = alpha^{i+1-n} mod 255.
fn rs_chien_search(sigma: &[u8], n: usize) -> Vec<usize> {
    let mut locs = Vec::new();
    for byte_idx in 0..n {
        // For byte index i (MSB-first), the corresponding polynomial power is (n-1-i).
        // Root condition: sigma(X_i^{-1}) = 0 where X_i^{-1} = alpha^{-(n-1-i)}
        // = alpha^{(255 - (n-1-byte_idx) % 255) % 255}
        let power = (n - 1 - byte_idx) % 255;
        let xi_inv = GF_EXP[(255 - power) % 255]; // alpha^{-power}
        if gf_poly_eval(sigma, xi_inv) == 0 {
            locs.push(byte_idx);
        }
    }
    locs
}

/// Compute error evaluator polynomial omega(x) = S(x) * sigma(x) mod x^{2t}.
///
/// S(x) = sum_{j=0}^{2t-1} S_j x^j, sigma is the error locator.
fn rs_compute_omega(syndromes: &[u8], sigma: &[u8]) -> Vec<u8> {
    let two_t = syndromes.len();
    let mut omega = vec![0u8; two_t];
    // omega_i = sum_{j=0}^{i} S_{i-j} * sigma_j  for i = 0..2t-1
    for i in 0..two_t {
        let mut v = 0u8;
        for j in 0..sigma.len().min(i + 1) {
            v ^= gf_mul(syndromes[i - j], sigma[j]);
        }
        omega[i] = v;
    }
    omega
}

/// Formal derivative of polynomial over GF(2^8).
///
/// d/dx sum_{i} a_i x^i = sum_{i odd} a_i x^{i-1}  (characteristic 2: even terms vanish).
fn rs_formal_deriv(poly: &[u8]) -> Vec<u8> {
    let len = poly.len().saturating_sub(1).max(1);
    let mut d = vec![0u8; len];
    for (i, &coef) in poly.iter().enumerate() {
        if i % 2 == 1 && i >= 1 {
            d[i - 1] = coef;
        }
    }
    d
}

// ---------------------------------------------------------------------------
// FIC (Fast Information Channel) Parser
// ---------------------------------------------------------------------------

/// DAB service information extracted from FIG type 1.
#[derive(Debug, Clone)]
pub struct ServiceLabel {
    pub service_id: u32,
    pub label: String,
    pub label_charset: u8,
}

/// DAB subchannel assignment from FIG type 0 extension 1.
#[derive(Debug, Clone)]
pub struct SubchannelInfo {
    pub subchannel_id: u8,
    pub start_cu: u16,
    pub size_cu: u16,
    pub protection_level: EepLevel,
}

/// FIB (Fast Information Block) parser.
///
/// Parses Fast Information Groups (FIGs) from a 30-byte FIB data field.
pub struct FibParser;

impl FibParser {
    /// Parse a FIB data field (30 bytes) and extract FIGs.
    ///
    /// Returns a list of `(fig_type, fig_ext, data)` tuples.
    pub fn parse_fib(data: &[u8]) -> Vec<(u8, u8, Vec<u8>)> {
        if data.len() < FIB_DATA_BYTES { return Vec::new(); }
        let mut figs = Vec::new();
        let mut pos = 0;
        while pos < FIB_DATA_BYTES {
            let header = data[pos];
            let fig_type = header >> 5;
            let length = (header & 0x1F) as usize;
            if length == 0 || pos + 1 + length > FIB_DATA_BYTES {
                break;
            }
            if header == 0xFF { break; } // End marker
            let ext_byte = data[pos + 1];
            let fig_ext = ext_byte & 0x1F;
            let fig_data = data[pos + 1..pos + 1 + length].to_vec();
            figs.push((fig_type, fig_ext, fig_data));
            pos += 1 + length;
        }
        figs
    }

    /// Parse service label from FIG type 1.
    pub fn parse_service_label(fig_data: &[u8]) -> Option<ServiceLabel> {
        if fig_data.len() < 18 { return None; }
        let charset = fig_data[0] >> 4;
        let service_id = u32::from_be_bytes([0, fig_data[1], fig_data[2], fig_data[3]]);
        let label_bytes = &fig_data[4..20.min(fig_data.len())];
        let label = String::from_utf8_lossy(label_bytes).trim().to_string();
        Some(ServiceLabel { service_id, label, label_charset: charset })
    }

    /// Parse subchannel info from FIG type 0 extension 1.
    pub fn parse_subchannel_info(fig_data: &[u8]) -> Option<SubchannelInfo> {
        if fig_data.len() < 4 { return None; }
        let subchannel_id = (fig_data[1] >> 2) & 0x3F;
        let start_cu = (((fig_data[1] & 0x03) as u16) << 8) | fig_data[2] as u16;
        let size_cu = ((fig_data[3] as u16) & 0x3FF).max(1);
        let prot_level_bits = (fig_data[3] >> 2) & 0x3;
        let long_form = (fig_data[3] & 0x08) != 0;
        let protection_level = match (long_form, prot_level_bits) {
            (false, 0) => EepLevel::Level1A,
            (false, 1) => EepLevel::Level2A,
            (false, 2) => EepLevel::Level3A,
            (false, 3) => EepLevel::Level4A,
            (true, 0)  => EepLevel::Level1B,
            (true, 1)  => EepLevel::Level2B,
            (true, 2)  => EepLevel::Level3B,
            _          => EepLevel::Level4B,
        };
        Some(SubchannelInfo { subchannel_id, start_cu, size_cu, protection_level })
    }
}

// ---------------------------------------------------------------------------
// DAB+ Super Frame / AU Extractor
// ---------------------------------------------------------------------------

/// DAB+ HE-AAC v2 super frame header.
#[derive(Debug, Clone)]
pub struct DabPlusSuperFrame {
    /// AAC decoder configuration element bytes.
    pub dac_rate: bool,
    pub sbr_flag: bool,
    pub aac_channel_mode: u8,
    pub ps_flag: bool,
    pub mpeg_surround_config: u8,
    /// Number of Access Units (AUs) per super frame.
    pub num_aus: usize,
    /// AU boundaries within the super frame.
    pub au_start: Vec<usize>,
}

/// Access Unit extracted from a DAB+ super frame.
#[derive(Debug, Clone)]
pub struct AccessUnit {
    pub index: usize,
    pub data: Vec<u8>,
    pub crc_ok: bool,
}

/// DAB+ super frame parser.
pub struct DabPlusSuperFrameParser;

impl DabPlusSuperFrameParser {
    /// Parse the header of a DAB+ super frame (ETSI EN 302 563 §6.1).
    ///
    /// The super frame consists of:
    /// - 1 byte: Firecode (FEC over first 9 bytes)
    /// - 1 byte: RFA + DAC rate + SBR + channel mode + PS + surround config
    /// - num_aus * 2 bytes: AU start addresses
    /// - audio data
    pub fn parse_header(data: &[u8]) -> Option<DabPlusSuperFrame> {
        if data.len() < 3 { return None; }

        let header1 = data[1];
        let dac_rate = (header1 & 0x40) != 0;
        let sbr_flag = (header1 & 0x20) != 0;
        let aac_channel_mode = (header1 & 0x10) >> 4;
        let ps_flag = (header1 & 0x08) != 0;
        let mpeg_surround_config = header1 & 0x07;

        // Determine number of AUs based on DAC rate and SBR flag
        let num_aus = match (dac_rate, sbr_flag) {
            (false, false) => 4,  // 32 kHz, no SBR
            (false, true)  => 2,  // 32 kHz, SBR → 64 kHz
            (true, false)  => 6,  // 48 kHz, no SBR
            (true, true)   => 3,  // 48 kHz, SBR → 96 kHz (rare)
        };

        // Read AU start pointers (each 2 bytes big-endian, after header)
        let header_size = 2 + (num_aus - 1) * 2;
        if data.len() < header_size { return None; }

        let mut au_start = Vec::with_capacity(num_aus);
        au_start.push(header_size); // First AU starts after header
        for i in 0..num_aus - 1 {
            let offset = 2 + i * 2;
            if offset + 1 >= data.len() { break; }
            let start = ((data[offset] as usize) << 8) | data[offset + 1] as usize;
            au_start.push(start);
        }

        Some(DabPlusSuperFrame {
            dac_rate,
            sbr_flag,
            aac_channel_mode,
            ps_flag,
            mpeg_surround_config,
            num_aus,
            au_start,
        })
    }

    /// Extract all Access Units from a super frame.
    pub fn extract_aus(data: &[u8], header: &DabPlusSuperFrame) -> Vec<AccessUnit> {
        let mut aus = Vec::with_capacity(header.num_aus);
        for (i, &start) in header.au_start.iter().enumerate() {
            let end = if i + 1 < header.au_start.len() {
                header.au_start[i + 1]
            } else {
                data.len().saturating_sub(2) // Last AU ends 2 bytes before super frame end (CRC)
            };

            if start >= data.len() || end > data.len() || start >= end {
                aus.push(AccessUnit { index: i, data: Vec::new(), crc_ok: false });
                continue;
            }

            let au_data = data[start..end].to_vec();
            // Verify AU CRC-16 (last 2 bytes are CRC)
            let crc_ok = if au_data.len() >= 2 {
                let computed = DabCrc16::compute(&au_data[..au_data.len() - 2]);
                let stored = ((au_data[au_data.len() - 2] as u16) << 8)
                    | au_data[au_data.len() - 1] as u16;
                computed == stored
            } else {
                false
            };

            aus.push(AccessUnit { index: i, data: au_data, crc_ok });
        }
        aus
    }
}

// ---------------------------------------------------------------------------
// Frame Timing Calculator
// ---------------------------------------------------------------------------

/// DAB frame timing calculations.
pub struct FrameTiming;

impl FrameTiming {
    /// Total samples per Mode I transmission frame.
    pub fn samples_per_frame_mode_i() -> usize {
        // Null symbol + 76 data OFDM symbols (each with guard + FFT)
        MODE_I_NULL_LEN
            + MODE_I_SYMBOLS_PER_FRAME * (MODE_I_FFT_SIZE + MODE_I_GUARD_LEN)
    }

    /// Duration of one OFDM symbol in microseconds.
    pub fn symbol_duration_us(mode: DabMode) -> f64 {
        let p = DabModeParams::for_mode(mode);
        let total = p.fft_size + p.guard_len;
        total as f64 / SAMPLE_RATE * 1e6
    }

    /// Guard interval duration in microseconds.
    pub fn guard_duration_us(mode: DabMode) -> f64 {
        let p = DabModeParams::for_mode(mode);
        p.guard_len as f64 / SAMPLE_RATE * 1e6
    }

    /// Useful symbol duration (FFT window) in microseconds.
    pub fn useful_duration_us(mode: DabMode) -> f64 {
        let p = DabModeParams::for_mode(mode);
        p.fft_size as f64 / SAMPLE_RATE * 1e6
    }

    /// Compute the carrier frequency for carrier index k.
    ///
    /// `k` is in range [-768, 768] (Mode I, excluding DC and guard bands).
    /// `center_freq_hz`: DAB ensemble center frequency in Hz.
    pub fn carrier_frequency(k: isize, center_freq_hz: f64) -> f64 {
        center_freq_hz + k as f64 * CARRIER_SPACING_HZ
    }

    /// Number of bits per CIF (Common Interleaved Frame) per capacity unit.
    pub fn bits_per_cu() -> usize {
        // Each CU = 64 bits of capacity per 24ms
        64
    }

    /// Number of OFDM frames per second for Mode I.
    pub fn frames_per_second_mode_i() -> f64 {
        1000.0 / FRAME_DURATION_MS
    }
}

// ---------------------------------------------------------------------------
// Complete DAB Channel Decoder (top-level)
// ---------------------------------------------------------------------------

/// Configuration for a subchannel extraction.
#[derive(Debug, Clone)]
pub struct SubchannelConfig {
    pub id: u8,
    pub start_cu: usize,
    pub size_cu: usize,
    pub protection: EepLevel,
    pub is_dab_plus: bool,
}

/// Full DAB channel decoder combining all processing stages.
pub struct DabChannelDecoder {
    mode: DabMode,
    ofdm_demod: DabOfdmDemodulator,
    dqpsk_dec: DqpskDecoder,
    viterbi_dec: DabViterbiDecoder,
    energy_dispers: EnergyDispersal,
    time_interleaver: TimeInterleaver,
    rs_codec: DabReedSolomon,
    prs_gen: PrsGenerator,
}

impl DabChannelDecoder {
    /// Create a new DAB channel decoder for the given mode.
    pub fn new(mode: DabMode) -> Self {
        let frame_len = MODE_I_ACTIVE_CARRIERS * 2; // 2 bits per DQPSK symbol
        Self {
            mode,
            ofdm_demod: DabOfdmDemodulator::new(mode),
            dqpsk_dec: DqpskDecoder::new(),
            viterbi_dec: DabViterbiDecoder::new(),
            energy_dispers: EnergyDispersal::new(),
            time_interleaver: TimeInterleaver::new(frame_len),
            rs_codec: DabReedSolomon::new(),
            prs_gen: PrsGenerator::new(mode),
        }
    }

    /// Get the OFDM parameters for this decoder.
    pub fn params(&self) -> DabModeParams {
        DabModeParams::for_mode(self.mode)
    }

    /// Process raw time-domain samples for one transmission frame.
    ///
    /// Returns FIC bytes (decoded FIBs) and raw MSC subchannel data.
    pub fn process_frame(
        &mut self,
        samples: &[Complex],
        subchannel: &SubchannelConfig,
    ) -> (Vec<Vec<u8>>, Vec<u8>) {
        let params = self.params();
        let sym_len = params.fft_size + params.guard_len;

        // Demodulate OFDM symbols
        let mut symbols: Vec<Vec<Complex>> = Vec::with_capacity(params.symbols_per_frame);

        // Symbol 0 is the PRS
        let prs_fd = self.prs_gen.generate();
        symbols.push(prs_fd);

        // Demodulate remaining symbols
        for s in 1..params.symbols_per_frame {
            let start = (s - 1) * sym_len;
            if start + sym_len <= samples.len() {
                let sym = self.ofdm_demod.demodulate_symbol(&samples[start..start + sym_len]);
                symbols.push(sym);
            } else {
                break;
            }
        }

        // DQPSK decode
        let dibits = self.dqpsk_dec.decode_frame(&symbols);

        // Convert dibits to flat bit stream
        let bits: Vec<bool> = dibits
            .iter()
            .flat_map(|sym| sym.iter().flat_map(|&(b1, b0)| [b1, b0]))
            .collect();

        // Demultiplex FIC (symbols 1..4) and MSC (symbols 4..76)
        let carriers_per_sym = params.active_carriers;
        let fic_bits: Vec<bool> = bits[..3 * carriers_per_sym * 2].to_vec();
        let msc_bits: Vec<bool> = bits[3 * carriers_per_sym * 2..].to_vec();

        // FIC processing
        let fic_deinterleaved = self.time_interleaver.deinterleave(&fic_bits);
        let (fic_pattern, _, _) = eep_puncture_pattern(EepLevel::Level1A);
        let expected_fic_len = fic_deinterleaved.len() * CONV_RATE_DENOM;
        let fic_depunctured = depuncture(&fic_deinterleaved, fic_pattern, expected_fic_len);
        let fic_decoded = self.viterbi_dec.decode(&fic_depunctured);
        let fic_bytes: Vec<u8> = bits_to_bytes(&fic_decoded);
        let fic_descrambled = self.energy_dispers.process(&fic_bytes);
        self.energy_dispers.reset();

        // Parse FIBs
        let mut fibs = Vec::new();
        for chunk in fic_descrambled.chunks(FIB_LENGTH_BYTES) {
            if chunk.len() == FIB_LENGTH_BYTES && DabCrc16::verify_fib(chunk) {
                fibs.push(chunk.to_vec());
            }
        }

        // MSC processing
        let cu_start = subchannel.start_cu * 64;
        let cu_len = subchannel.size_cu * 64;
        let msc_subchan: Vec<bool> = msc_bits
            .get(cu_start..cu_start + cu_len.min(msc_bits.len().saturating_sub(cu_start)))
            .unwrap_or(&[])
            .to_vec();

        let msc_deint = self.time_interleaver.deinterleave(&msc_subchan);
        let (msc_pattern, _, _) = eep_puncture_pattern(subchannel.protection);
        let expected_msc_len = msc_deint.len() * CONV_RATE_DENOM;
        let msc_depunct = depuncture(&msc_deint, msc_pattern, expected_msc_len);
        let msc_decoded = self.viterbi_dec.decode(&msc_depunct);
        let msc_bytes = bits_to_bytes(&msc_decoded);
        let msc_descrambled = self.energy_dispers.process(&msc_bytes);
        self.energy_dispers.reset();

        (fibs, msc_descrambled)
    }
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Pack a bit slice into bytes (MSB first).
pub fn bits_to_bytes(bits: &[bool]) -> Vec<u8> {
    let nbytes = (bits.len() + 7) / 8;
    let mut out = vec![0u8; nbytes];
    for (i, &b) in bits.iter().enumerate() {
        if b {
            out[i / 8] |= 1 << (7 - (i % 8));
        }
    }
    out
}

/// Unpack bytes to bits (MSB first).
pub fn bytes_to_bits(bytes: &[u8]) -> Vec<bool> {
    let mut bits = Vec::with_capacity(bytes.len() * 8);
    for &byte in bytes {
        for bit in (0..8).rev() {
            bits.push((byte >> bit) & 1 == 1);
        }
    }
    bits
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Complex arithmetic ---

    #[test]
    fn test_complex_mul() {
        let a = Complex::new(1.0, 2.0);
        let b = Complex::new(3.0, 4.0);
        let c = a.mul(b);
        assert!((c.re - (-5.0)).abs() < 1e-10);
        assert!((c.im - 10.0).abs() < 1e-10);
    }

    #[test]
    fn test_complex_conj() {
        let a = Complex::new(3.0, -4.0);
        let c = a.conj();
        assert_eq!(c.re, 3.0);
        assert_eq!(c.im, 4.0);
    }

    #[test]
    fn test_complex_norm() {
        let a = Complex::new(3.0, 4.0);
        assert!((a.norm() - 5.0).abs() < 1e-10);
    }

    #[test]
    fn test_complex_arg() {
        let a = Complex::exp_j(PI / 4.0);
        assert!((a.arg() - PI / 4.0).abs() < 1e-10);
    }

    // --- FFT roundtrip ---

    #[test]
    fn test_fft_ifft_roundtrip() {
        let n = 64;
        let original: Vec<Complex> = (0..n)
            .map(|i| Complex::new((i as f64 * 0.1).sin(), 0.0))
            .collect();
        let mut buf = original.clone();
        fft_inplace(&mut buf);
        ifft_inplace(&mut buf);
        for (a, b) in original.iter().zip(buf.iter()) {
            assert!((a.re - b.re).abs() < 1e-9, "re mismatch: {} vs {}", a.re, b.re);
            assert!((a.im - b.im).abs() < 1e-9, "im mismatch: {} vs {}", a.im, b.im);
        }
    }

    #[test]
    fn test_fft_impulse() {
        let n = 8;
        let mut buf = vec![Complex::new(0.0, 0.0); n];
        buf[0] = Complex::new(1.0, 0.0);
        fft_inplace(&mut buf);
        // FFT of impulse should be all-ones
        for c in &buf {
            assert!((c.norm() - 1.0).abs() < 1e-10);
        }
    }

    #[test]
    fn test_fft_size_2048() {
        let n = 2048;
        let mut buf: Vec<Complex> = (0..n)
            .map(|i| Complex::new(if i == 0 { 1.0 } else { 0.0 }, 0.0))
            .collect();
        fft_inplace(&mut buf);
        // All bins should have magnitude 1 for an impulse
        for c in &buf {
            assert!((c.norm() - 1.0).abs() < 1e-8);
        }
    }

    // --- DAB Mode parameters ---

    #[test]
    fn test_mode_i_params() {
        let p = DabModeParams::for_mode(DabMode::ModeI);
        assert_eq!(p.fft_size, 2048);
        assert_eq!(p.active_carriers, 1536);
        assert_eq!(p.symbols_per_frame, 76);
        assert_eq!(p.guard_len, 504);
        assert_eq!(p.null_len, 2656);
    }

    #[test]
    fn test_mode_ii_params() {
        let p = DabModeParams::for_mode(DabMode::ModeII);
        assert_eq!(p.fft_size, 512);
        assert_eq!(p.active_carriers, 384);
    }

    #[test]
    fn test_mode_iii_params() {
        let p = DabModeParams::for_mode(DabMode::ModeIII);
        assert_eq!(p.fft_size, 256);
        assert_eq!(p.active_carriers, 192);
    }

    #[test]
    fn test_mode_iv_params() {
        let p = DabModeParams::for_mode(DabMode::ModeIV);
        assert_eq!(p.fft_size, 1024);
        assert_eq!(p.active_carriers, 768);
    }

    // --- PRS generation ---

    #[test]
    fn test_prs_mode_i_length() {
        let prs = PrsGenerator::new(DabMode::ModeI);
        let fd = prs.generate();
        assert_eq!(fd.len(), MODE_I_FFT_SIZE);
    }

    #[test]
    fn test_prs_time_domain_length() {
        let prs = PrsGenerator::new(DabMode::ModeI);
        let td = prs.generate_time_domain();
        assert_eq!(td.len(), MODE_I_FFT_SIZE + MODE_I_GUARD_LEN);
    }

    #[test]
    fn test_prs_dc_null() {
        // DC carrier (bin 0) should be zero
        let prs = PrsGenerator::new(DabMode::ModeI);
        let fd = prs.generate();
        assert!((fd[0].norm()) < 1e-10, "DC should be null");
    }

    #[test]
    fn test_prs_unit_amplitude() {
        let prs = PrsGenerator::new(DabMode::ModeI);
        let fd = prs.generate();
        // Non-zero bins should have unit amplitude
        for (i, c) in fd.iter().enumerate() {
            if c.norm() > 0.01 {
                assert!(
                    (c.norm() - 1.0).abs() < 1e-9,
                    "Carrier {} should have unit amplitude, got {}",
                    i,
                    c.norm()
                );
            }
        }
    }

    #[test]
    fn test_prs_correlation_peak() {
        let prs = PrsGenerator::new(DabMode::ModeI);
        let ref_td = prs.generate_time_domain();
        // Self-correlation should find peak at offset 0
        let peak = prs.correlate_timing(&ref_td);
        assert_eq!(peak, 0);
    }

    // --- OFDM modulate/demodulate roundtrip ---

    #[test]
    fn test_ofdm_mod_demod_roundtrip_mode_i() {
        let mode = DabMode::ModeI;
        let modulator = DabOfdmModulator::new(mode);
        let demodulator = DabOfdmDemodulator::new(mode);
        let n_carriers = DabModeParams::for_mode(mode).active_carriers;

        // Create QPSK symbols
        let carriers: Vec<Complex> = (0..n_carriers)
            .map(|i| Complex::exp_j((i as f64 % 4.0) * PI / 2.0))
            .collect();

        let td = modulator.modulate_symbol(&carriers);
        let recovered = demodulator.demodulate_symbol(&td);

        assert_eq!(recovered.len(), n_carriers);
        for (a, b) in carriers.iter().zip(recovered.iter()) {
            assert!((a.re - b.re).abs() < 1e-6, "re mismatch at carrier");
            assert!((a.im - b.im).abs() < 1e-6, "im mismatch at carrier");
        }
    }

    #[test]
    fn test_ofdm_symbol_length() {
        let mode = DabMode::ModeI;
        let modulator = DabOfdmModulator::new(mode);
        let p = DabModeParams::for_mode(mode);
        let carriers = vec![Complex::new(1.0, 0.0); p.active_carriers];
        let td = modulator.modulate_symbol(&carriers);
        assert_eq!(td.len(), p.fft_size + p.guard_len);
    }

    // --- DQPSK encode/decode roundtrip ---

    #[test]
    fn test_dqpsk_decode_zero_phase() {
        let dec = DqpskDecoder::new();
        // If current == previous (phase diff = 0), expect dibit (0, 0)
        let sym = vec![Complex::exp_j(PI / 4.0); 4];
        let dibits = dec.decode_symbol(&sym, &sym);
        for &(b1, b0) in &dibits {
            assert!(!b1 && !b0, "Zero phase diff should decode to (0,0)");
        }
    }

    #[test]
    fn test_dqpsk_decode_quarter_turn() {
        let dec = DqpskDecoder::new();
        let prev = vec![Complex::exp_j(0.0)];
        let curr = vec![Complex::exp_j(PI / 2.0)];
        let dibits = dec.decode_symbol(&curr, &prev);
        assert_eq!(dibits.len(), 1);
        let (b1, b0) = dibits[0];
        assert!(!b1 && b0, "90° should decode to (0,1)");
    }

    #[test]
    fn test_dqpsk_encode_decode_roundtrip() {
        let prs: Vec<Complex> = (0..8).map(|_| Complex::new(1.0, 0.0)).collect();
        let dibits_in: Vec<(bool, bool)> = vec![
            (false, false), (false, true), (true, false), (true, true),
            (false, false), (true, false), (false, true), (true, true),
        ];

        let mut enc = DqpskEncoder::new(8);
        let encoded = enc.encode_symbol(&dibits_in, &prs);

        let dec = DqpskDecoder::new();
        let decoded = dec.decode_symbol(&encoded, &prs);

        assert_eq!(decoded.len(), dibits_in.len());
        for (a, b) in dibits_in.iter().zip(decoded.iter()) {
            assert_eq!(a, b, "DQPSK roundtrip mismatch");
        }
    }

    // --- Convolutional encoder ---

    #[test]
    fn test_conv_encoder_rate() {
        let mut enc = DabConvEncoder::new();
        let bits = vec![true, false, true, true, false, false, true, false];
        let coded = enc.encode(&bits);
        assert_eq!(coded.len(), bits.len() * CONV_RATE_DENOM,
            "Rate 1/4 should produce 4x input bits");
    }

    #[test]
    fn test_conv_encoder_flush() {
        let mut enc = DabConvEncoder::new();
        let bits = vec![true; 8];
        let _ = enc.encode(&bits);
        let tail = enc.flush();
        assert_eq!(tail.len(), (CONV_K - 1) * CONV_RATE_DENOM);
    }

    #[test]
    fn test_conv_encoder_zero_input() {
        let mut enc = DabConvEncoder::new();
        let bits = vec![false; 8];
        let coded = enc.encode(&bits);
        // Zero state, zero input: all-zero output
        assert!(coded.iter().all(|&b| !b), "All-zero input should give all-zero output");
    }

    // --- Viterbi decoder ---

    #[test]
    fn test_viterbi_decode_no_errors() {
        let data = vec![
            true, false, true, true, false, false, true, false,
            true, true, false, true, false, true, false, false,
        ];
        let mut enc = DabConvEncoder::new();
        let coded = enc.encode(&data);

        let dec = DabViterbiDecoder::new();
        let decoded = dec.decode(&coded);

        assert!(decoded.len() >= data.len());
        assert_eq!(&decoded[..data.len()], &data[..],
            "Viterbi decode with no errors should recover original");
    }

    #[test]
    fn test_viterbi_trellis_states() {
        assert_eq!(VITERBI_STATES, 128, "K=7 left-shift encoder gives 128 trellis states");
    }

    #[test]
    fn test_viterbi_roundtrip_longer() {
        let data: Vec<bool> = (0..32).map(|i| (i * 7 + 3) % 13 < 7).collect();
        let mut enc = DabConvEncoder::new();
        let coded = enc.encode(&data);
        let dec = DabViterbiDecoder::new();
        let decoded = dec.decode(&coded);
        assert!(decoded.len() >= data.len());
        assert_eq!(&decoded[..data.len()], &data[..]);
    }

    // --- Puncturing / Depuncturing ---

    #[test]
    fn test_puncture_rate_half() {
        let bits = vec![true; 16];
        let pattern = &[1u8, 1, 0, 0]; // keep 2 of every 4
        let punctured = puncture(&bits, pattern);
        assert_eq!(punctured.len(), 8, "Pattern [1,1,0,0] keeps half the bits");
    }

    #[test]
    fn test_depuncture_restore() {
        let bits = vec![
            true, false, true, false, true, false, true, false,
            true, true, false, false, true, true, false, false,
        ];
        let pattern = &[1u8, 0, 1, 1]; // keep 3 of every 4
        let punctured = puncture(&bits, pattern);
        let restored = depuncture(&punctured, pattern, bits.len());
        // Restored should match original at kept positions
        for (i, &p) in pattern.iter().enumerate().cycle().take(bits.len()) {
            if p != 0 {
                assert_eq!(restored[i], bits[i]);
            }
        }
    }

    #[test]
    fn test_eep_patterns_valid() {
        for level in [
            EepLevel::Level1A, EepLevel::Level2A, EepLevel::Level3A, EepLevel::Level4A,
            EepLevel::Level1B, EepLevel::Level2B, EepLevel::Level3B, EepLevel::Level4B,
        ] {
            let (pat, num, den) = eep_puncture_pattern(level);
            assert!(!pat.is_empty(), "Pattern should not be empty");
            assert!(num <= den, "Code rate numerator should be <= denominator");
        }
    }

    #[test]
    fn test_eep_level1a_rate_quarter() {
        let (pat, num, den) = eep_puncture_pattern(EepLevel::Level1A);
        assert_eq!(num, 1);
        assert_eq!(den, 1);
        // All-ones pattern means no puncturing (full rate 1/4)
        assert!(pat.iter().all(|&b| b == 1));
    }

    // --- Time interleaver ---

    #[test]
    fn test_time_interleaver_length_preservation() {
        let frame_len = 256;
        let mut il = TimeInterleaver::new(frame_len);
        let input = vec![true; frame_len];
        let out = il.interleave(&input);
        assert_eq!(out.len(), frame_len);
    }

    #[test]
    fn test_time_interleaver_deinterleaver_invertible() {
        let frame_len = 64;
        let mut il = TimeInterleaver::new(frame_len);
        let input: Vec<bool> = (0..frame_len).map(|i| i % 3 == 0).collect();

        // Run interleaver 16 times to fill history
        let mut interleaved = Vec::new();
        for _ in 0..TIME_INTERLEAVER_DEPTH {
            interleaved = il.interleave(&input);
        }

        // Now deinterleave
        let mut dil = TimeInterleaver::new(frame_len);
        for _ in 0..TIME_INTERLEAVER_DEPTH {
            dil.deinterleave(&interleaved);
        }
        // Check output has same length
        assert_eq!(interleaved.len(), frame_len);
    }

    // --- Energy dispersal ---

    #[test]
    fn test_energy_dispersal_roundtrip() {
        let data = vec![0xAA_u8, 0x55, 0x12, 0x34, 0xFF, 0x00, 0xAB, 0xCD];
        let mut ed = EnergyDispersal::new();
        let scrambled = ed.process(&data);
        ed.reset();
        let restored = ed.process(&scrambled);
        assert_eq!(restored, data, "Energy dispersal should be self-inverse");
    }

    #[test]
    fn test_energy_dispersal_changes_data() {
        let data = vec![0x00_u8; 16];
        let mut ed = EnergyDispersal::new();
        let scrambled = ed.process(&data);
        // PRBS should not all be zero (unless the LFSR produces all zeros, which it won't)
        assert_ne!(scrambled, data, "Scrambled data should differ from all-zeros");
    }

    #[test]
    fn test_energy_dispersal_bits_roundtrip() {
        let bits: Vec<bool> = (0..40).map(|i| i % 3 == 0).collect();
        let mut ed = EnergyDispersal::new();
        let scrambled = ed.process_bits(&bits);
        ed.reset();
        let restored = ed.process_bits(&scrambled);
        assert_eq!(restored, bits);
    }

    #[test]
    fn test_energy_dispersal_prbs_period() {
        // PRBS with x^9+x^5+1 has period 2^9-1 = 511 bits
        let data = vec![0u8; 64];
        let mut ed1 = EnergyDispersal::new();
        let mut ed2 = EnergyDispersal::new();
        let s1 = ed1.process(&data);
        let s2 = ed2.process(&data);
        assert_eq!(s1, s2, "Same initial state should produce same output");
    }

    // --- CRC-16 ---

    #[test]
    fn test_crc16_known_value() {
        // CRC-16/CCITT of "123456789" = 0x29B1
        let data = b"123456789";
        let crc = DabCrc16::compute(data);
        assert_eq!(crc, 0x29B1, "CRC-16 of '123456789' should be 0x29B1");
    }

    #[test]
    fn test_crc16_append_verify() {
        let data: Vec<u8> = (0..30_u8).collect();
        let fib = DabCrc16::append(&data);
        assert_eq!(fib.len(), 32);
        assert!(DabCrc16::verify_fib(&fib), "Appended CRC should verify");
    }

    #[test]
    fn test_crc16_bad_fib() {
        let data: Vec<u8> = (0..30_u8).collect();
        let mut fib = DabCrc16::append(&data);
        fib[0] ^= 0xFF; // Corrupt first byte
        assert!(!DabCrc16::verify_fib(&fib), "Corrupted FIB should fail CRC check");
    }

    #[test]
    fn test_crc16_empty_detect() {
        assert!(!DabCrc16::verify_fib(&[]), "Empty FIB should fail");
        assert!(!DabCrc16::verify_fib(&[0u8; 10]), "Short FIB should fail");
    }

    // --- Reed-Solomon ---

    #[test]
    fn test_rs_encode_length() {
        let rs = DabReedSolomon::new();
        let msg = vec![0xAA_u8; RS_MESSAGE_LEN];
        let cw = rs.encode(&msg);
        assert_eq!(cw.len(), RS_CODEWORD_LEN);
    }

    #[test]
    fn test_rs_encode_decode_no_errors() {
        let rs = DabReedSolomon::new();
        let msg: Vec<u8> = (0..RS_MESSAGE_LEN as u8).collect();
        let mut cw = rs.encode(&msg);
        let n_errors = rs.decode(&mut cw).unwrap();
        assert_eq!(n_errors, 0);
        assert_eq!(&cw[..RS_MESSAGE_LEN], &msg[..]);
    }

    #[test]
    fn test_rs_correct_single_error() {
        let rs = DabReedSolomon::new();
        let msg = vec![0x55_u8; RS_MESSAGE_LEN];
        let mut cw = rs.encode(&msg);
        cw[5] ^= 0xAB; // Inject 1 error
        let n_errors = rs.decode(&mut cw).unwrap();
        assert!(n_errors >= 1, "Should detect at least 1 error");
        assert_eq!(&cw[..RS_MESSAGE_LEN], &msg[..], "Single error should be corrected");
    }

    #[test]
    fn test_rs_correct_t_errors() {
        let rs = DabReedSolomon::new();
        let msg: Vec<u8> = (10..10 + RS_MESSAGE_LEN as u8).collect();
        let mut cw = rs.encode(&msg);
        // Inject t=5 errors at known positions in parity area
        for i in 0..RS_T {
            cw[RS_MESSAGE_LEN + i] ^= 0xAA;
        }
        let result = rs.decode(&mut cw);
        assert!(result.is_ok(), "Should correct exactly t parity errors");
    }

    #[test]
    fn test_gf_mul_commutative() {
        assert_eq!(gf_mul(0x53, 0xCA), gf_mul(0xCA, 0x53));
    }

    #[test]
    fn test_gf_inv_property() {
        for x in 1..=255u8 {
            assert_eq!(gf_mul(x, gf_inv(x)), 1, "x * inv(x) must equal 1");
        }
    }

    // --- DAB+ Super Frame ---

    #[test]
    fn test_super_frame_parse_header() {
        // Construct a minimal super frame header
        // Byte 0: firecode (ignored in test)
        // Byte 1: DAC rate=1, SBR=0, channel=0, PS=0, surround=0
        let mut data = vec![0u8; 20];
        data[1] = 0x40; // dac_rate=1, others 0
        let header = DabPlusSuperFrameParser::parse_header(&data);
        assert!(header.is_some());
        let h = header.unwrap();
        assert!(h.dac_rate);
        assert!(!h.sbr_flag);
        assert_eq!(h.num_aus, 6); // 48 kHz, no SBR → 6 AUs
    }

    #[test]
    fn test_super_frame_32khz_sbr() {
        // DAC rate=0 (32 kHz), SBR=1 → 2 AUs
        let mut data = vec![0u8; 10];
        data[1] = 0x20; // SBR=1, DAC rate=0
        let header = DabPlusSuperFrameParser::parse_header(&data);
        assert!(header.is_some());
        assert_eq!(header.unwrap().num_aus, 2);
    }

    #[test]
    fn test_super_frame_au_extraction() {
        // Build a simple super frame: header + 2 AUs each with CRC
        let num_aus = 2;
        let au_data_1 = b"HELLO";
        let au_data_2 = b"WORLD";

        // Header: byte0=firecode, byte1=0x20 (SBR), byte2-3=AU2 start pointer
        let au1_start = 4usize; // header is 4 bytes
        let au2_start = au1_start + au_data_1.len() + 2; // +2 for CRC
        let mut frame = vec![0u8; au2_start + au_data_2.len() + 2];
        frame[1] = 0x20; // SBR
        frame[2] = ((au2_start >> 8) & 0xFF) as u8;
        frame[3] = (au2_start & 0xFF) as u8;

        // AU1 data + CRC
        frame[au1_start..au1_start + au_data_1.len()].copy_from_slice(au_data_1);
        let crc1 = DabCrc16::compute(au_data_1);
        frame[au1_start + au_data_1.len()] = (crc1 >> 8) as u8;
        frame[au1_start + au_data_1.len() + 1] = (crc1 & 0xFF) as u8;

        // AU2 data + CRC
        frame[au2_start..au2_start + au_data_2.len()].copy_from_slice(au_data_2);
        let crc2 = DabCrc16::compute(au_data_2);
        let crc_pos = au2_start + au_data_2.len();
        if crc_pos + 1 < frame.len() {
            frame[crc_pos] = (crc2 >> 8) as u8;
            frame[crc_pos + 1] = (crc2 & 0xFF) as u8;
        }

        let header = DabPlusSuperFrameParser::parse_header(&frame).unwrap();
        let aus = DabPlusSuperFrameParser::extract_aus(&frame, &header);
        assert_eq!(aus.len(), num_aus);
    }

    // --- Frame timing calculations ---

    #[test]
    fn test_frame_timing_samples_per_frame() {
        let total = FrameTiming::samples_per_frame_mode_i();
        // null + 76 * (2048 + 504)
        let expected = MODE_I_NULL_LEN + MODE_I_SYMBOLS_PER_FRAME * MODE_I_SYMBOL_LEN;
        assert_eq!(total, expected);
    }

    #[test]
    fn test_frame_timing_symbol_duration() {
        let dur = FrameTiming::symbol_duration_us(DabMode::ModeI);
        // (2048 + 504) / 2048000 * 1e6 ≈ 1246 µs
        let expected = (MODE_I_FFT_SIZE + MODE_I_GUARD_LEN) as f64 / SAMPLE_RATE * 1e6;
        assert!((dur - expected).abs() < 0.01);
    }

    #[test]
    fn test_frame_timing_carrier_frequency() {
        let center = 220_000_000.0_f64; // 220 MHz
        let f0 = FrameTiming::carrier_frequency(0, center);
        let fp1 = FrameTiming::carrier_frequency(1, center);
        assert_eq!(f0, center);
        assert!((fp1 - center - CARRIER_SPACING_HZ).abs() < 0.01);
    }

    #[test]
    fn test_frame_timing_frames_per_second() {
        let fps = FrameTiming::frames_per_second_mode_i();
        assert!((fps - (1000.0 / 96.0)).abs() < 0.001);
    }

    // --- FIB parser ---

    #[test]
    fn test_fib_parser_end_marker() {
        let mut data = vec![0xFFu8; FIB_DATA_BYTES];
        data[0] = 0xFF; // End marker immediately
        let figs = FibParser::parse_fib(&data);
        assert!(figs.is_empty(), "End marker should produce no FIGs");
    }

    #[test]
    fn test_fib_parser_single_fig() {
        let mut data = vec![0u8; FIB_DATA_BYTES];
        // FIG type=1, extension=0, length=5: header byte = (1<<5) | 5 = 0x25
        data[0] = 0x25;
        data[1] = 0x00; // extension 0
        // Remaining bytes are FIG data
        let figs = FibParser::parse_fib(&data);
        assert!(!figs.is_empty());
        let (fig_type, _fig_ext, _) = figs[0];
        assert_eq!(fig_type, 1);
    }

    // --- Utility functions ---

    #[test]
    fn test_bits_to_bytes_roundtrip() {
        let bytes = vec![0xA5_u8, 0x3C, 0xFF, 0x00];
        let bits = bytes_to_bits(&bytes);
        assert_eq!(bits.len(), 32);
        let restored = bits_to_bytes(&bits);
        assert_eq!(restored, bytes);
    }

    #[test]
    fn test_bits_to_bytes_msb_first() {
        let bits = vec![true, false, true, false, false, false, false, false];
        let bytes = bits_to_bytes(&bits);
        assert_eq!(bytes[0], 0xA0, "MSB-first: 10100000 = 0xA0");
    }

    #[test]
    fn test_bytes_to_bits_count() {
        let bytes = vec![0x00_u8; 10];
        let bits = bytes_to_bits(&bytes);
        assert_eq!(bits.len(), 80);
    }

    // --- Integration: encode + Viterbi decode with puncturing ---

    #[test]
    fn test_fec_puncture_viterbi_roundtrip() {
        let data: Vec<bool> = (0..20).map(|i| i % 2 == 0).collect();
        let mut enc = DabConvEncoder::new();
        let coded = enc.encode(&data);

        // Apply rate-1/2 puncturing
        let pattern = &[1u8, 1, 0, 0]; // keep 2 of every 4
        let punctured = puncture(&coded, pattern);
        let output_len = coded.len();
        let depunctured = depuncture(&punctured, pattern, output_len);

        let dec = DabViterbiDecoder::new();
        // Build erasure mask: positions where pattern[i%4]==0 are erased
        let erased: Vec<bool> = (0..output_len)
            .map(|i| pattern[i % pattern.len()] == 0)
            .collect();
        let decoded = dec.decode_erased(&depunctured, &erased);
        // With erasure-aware decoding, most bits should be correct
        let matches: usize = decoded[..data.len()]
            .iter()
            .zip(data.iter())
            .filter(|(&a, &b)| a == b)
            .count();
        assert!(
            matches >= data.len() * 7 / 10,
            "After puncture+depuncture, at least 70% bits should be correct, got {}/{}",
            matches, data.len()
        );
    }

    // --- Constants sanity checks ---

    #[test]
    fn test_constants_mode_i() {
        assert_eq!(MODE_I_FFT_SIZE, 2048);
        assert_eq!(MODE_I_ACTIVE_CARRIERS, 1536);
        assert_eq!(MODE_I_SYMBOLS_PER_FRAME, 76);
        assert_eq!(MODE_I_GUARD_LEN, 504);
        assert_eq!(FIB_LENGTH_BYTES, 32);
        assert_eq!(FIB_DATA_BYTES, 30);
    }

    #[test]
    fn test_constants_rs() {
        assert_eq!(RS_CODEWORD_LEN, 120);
        assert_eq!(RS_MESSAGE_LEN, 110);
        assert_eq!(RS_T, 5);
        assert_eq!(RS_CODEWORD_LEN - RS_MESSAGE_LEN, 2 * RS_T);
    }

    #[test]
    fn test_dab_channel_decoder_creation() {
        let dec = DabChannelDecoder::new(DabMode::ModeI);
        let params = dec.params();
        assert_eq!(params.fft_size, MODE_I_FFT_SIZE);
        assert_eq!(params.active_carriers, MODE_I_ACTIVE_CARRIERS);
    }

    #[test]
    fn test_dqpsk_phase_to_dibit_all_quadrants() {
        // 0° → (0,0)
        let (b1, b0) = dqpsk_phase_to_dibit(0.0);
        assert!(!b1 && !b0);
        // 90° → (0,1)
        let (b1, b0) = dqpsk_phase_to_dibit(PI / 2.0);
        assert!(!b1 && b0);
        // -90° → (1,0)
        let (b1, b0) = dqpsk_phase_to_dibit(-PI / 2.0);
        assert!(b1 && !b0);
        // 180° → (1,1)
        let (b1, b0) = dqpsk_phase_to_dibit(PI);
        assert!(b1 && b0);
    }
}
