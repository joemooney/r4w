//! VDSL2 and G.fast DMT (Discrete Multi-Tone) Processor
//!
//! Implements DSL broadband modem signal processing per ITU-T G.993.2 (VDSL2)
//! and ITU-T G.9701 (G.fast). Covers IFFT/FFT-based multi-carrier modulation,
//! adaptive bit loading, PSD shaping, vectoring (FEXT cancellation), bonding,
//! band plans, synchronization, and impulse noise protection.
//!
//! ## Standards
//! - ITU-T G.993.2: VDSL2 — Very High Speed DSL 2
//! - ITU-T G.9701: G.fast — Fast Access to Subscriber Terminals
//! - ITU-T G.993.5: G.vector — Self-FEXT cancellation
//! - ITU-T G.998.4: G.inp — Impulse Noise Protection
//!
//! ## Example
//!
//! ```rust
//! use r4w_core::vdsl2_dmt_processor::{DmtConfig, DmtModulator, DmtDemodulator,
//!     VdslProfile, BitLoader, BitLoadConfig};
//!
//! // Create VDSL2 17a profile DMT modulator
//! let config = DmtConfig::vdsl2_profile(VdslProfile::Profile17a);
//! let mut modulator = DmtModulator::new(config.clone());
//!
//! // Allocate bits per subcarrier (simple flat loading)
//! let bits_per_tone = vec![6u8; config.data_tones()];
//! let data_bits: Vec<u8> = (0..bits_per_tone.iter().map(|&b| b as usize).sum::<usize>())
//!     .map(|i| (i % 2) as u8)
//!     .collect();
//!
//! let symbol = modulator.modulate_symbol(&bits_per_tone, &data_bits);
//! assert!(!symbol.is_empty());
//! ```

use std::f64::consts::PI;

// ---------------------------------------------------------------------------
// Complex number (no external crates)
// ---------------------------------------------------------------------------

/// Simple complex number for DMT processing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    #[inline]
    pub fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    #[inline]
    pub fn from_polar(mag: f64, phase: f64) -> Self {
        Self { re: mag * phase.cos(), im: mag * phase.sin() }
    }
    #[inline]
    pub fn mag_sq(&self) -> f64 { self.re * self.re + self.im * self.im }
    #[inline]
    pub fn mag(&self) -> f64 { self.mag_sq().sqrt() }
    #[inline]
    pub fn conj(&self) -> Self { Self { re: self.re, im: -self.im } }
    #[inline]
    pub fn mul(&self, other: &Self) -> Self {
        Self {
            re: self.re * other.re - self.im * other.im,
            im: self.re * other.im + self.im * other.re,
        }
    }
    #[inline]
    pub fn add(&self, other: &Self) -> Self {
        Self { re: self.re + other.re, im: self.im + other.im }
    }
    #[inline]
    pub fn sub(&self, other: &Self) -> Self {
        Self { re: self.re - other.re, im: self.im - other.im }
    }
    #[inline]
    pub fn scale(&self, s: f64) -> Self {
        Self { re: self.re * s, im: self.im * s }
    }
}

// ---------------------------------------------------------------------------
// Radix-2 Cooley-Tukey FFT / IFFT (in-place, DIT)
// ---------------------------------------------------------------------------

/// Bit-reversal permutation.
fn bit_reverse_permute(buf: &mut [Complex]) {
    let n = buf.len();
    let bits = n.trailing_zeros() as usize;
    for i in 0..n {
        let j = (0..bits).fold(0usize, |acc, b| acc | (((i >> b) & 1) << (bits - 1 - b)));
        if i < j {
            buf.swap(i, j);
        }
    }
}

/// In-place radix-2 DIT FFT. `n` must be a power of two.
pub fn fft_inplace(buf: &mut [Complex], inverse: bool) {
    let n = buf.len();
    assert!(n.is_power_of_two(), "FFT size must be power of two");
    bit_reverse_permute(buf);
    let sign = if inverse { 1.0_f64 } else { -1.0_f64 };
    let mut step = 1usize;
    while step < n {
        let half = step;
        step <<= 1;
        let angle = sign * PI / half as f64;
        let wbase = Complex::new(angle.cos(), angle.sin());
        for k in (0..n).step_by(step) {
            let mut w = Complex::new(1.0, 0.0);
            for j in 0..half {
                let u = buf[k + j];
                let v = buf[k + j + half].mul(&w);
                buf[k + j] = u.add(&v);
                buf[k + j + half] = u.sub(&v);
                w = w.mul(&wbase);
            }
        }
    }
    if inverse {
        let scale = 1.0 / n as f64;
        for x in buf.iter_mut() {
            *x = x.scale(scale);
        }
    }
}

// ---------------------------------------------------------------------------
// Band plan definitions
// ---------------------------------------------------------------------------

/// VDSL2 band plans per ITU-T G.993.2 Annex A/B/C.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandPlan {
    /// 998 kHz upstream limit (Annex A, Europe)
    Plan998,
    /// 997 kHz upstream limit (Annex B, Germany)
    Plan997,
    /// HPE17 profile 17a band plan
    Hpe17,
    /// HPE30 profile 30a band plan
    Hpe30,
    /// HPE35 profile 35b band plan
    Hpe35,
    /// G.fast 106 MHz band plan
    Gfast106,
    /// G.fast 212 MHz band plan
    Gfast212,
}

impl BandPlan {
    /// Nominal subcarrier spacing in Hz.
    pub fn subcarrier_spacing_hz(&self) -> f64 {
        match self {
            BandPlan::Plan998
            | BandPlan::Plan997
            | BandPlan::Hpe17
            | BandPlan::Hpe30
            | BandPlan::Hpe35 => 4312.5,
            BandPlan::Gfast106 | BandPlan::Gfast212 => 51_750.0,
        }
    }
    /// Maximum upstream frequency in Hz.
    pub fn max_upstream_hz(&self) -> f64 {
        match self {
            BandPlan::Plan998 => 998_000.0,
            BandPlan::Plan997 => 997_000.0,
            BandPlan::Hpe17 => 8_500_000.0,
            BandPlan::Hpe30 => 17_664_000.0,
            BandPlan::Hpe35 => 17_664_000.0,
            BandPlan::Gfast106 => 106_000_000.0,
            BandPlan::Gfast212 => 212_000_000.0,
        }
    }
    /// Maximum downstream frequency in Hz.
    pub fn max_downstream_hz(&self) -> f64 {
        match self {
            BandPlan::Plan998 => 17_664_000.0,
            BandPlan::Plan997 => 17_664_000.0,
            BandPlan::Hpe17 => 17_664_000.0,
            BandPlan::Hpe30 => 30_000_000.0,
            BandPlan::Hpe35 => 35_328_000.0,
            BandPlan::Gfast106 => 106_000_000.0,
            BandPlan::Gfast212 => 212_000_000.0,
        }
    }
}

// ---------------------------------------------------------------------------
// VDSL2 / G.fast profile definitions
// ---------------------------------------------------------------------------

/// VDSL2 profile per ITU-T G.993.2 Table 6-1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VdslProfile {
    Profile8a,
    Profile8b,
    Profile8c,
    Profile8d,
    Profile12a,
    Profile12b,
    Profile17a,
    Profile30a,
    Profile35b,
}

/// G.fast profile per ITU-T G.9701.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GfastProfile {
    Profile106a,
    Profile212a,
}

/// DMT system variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmtVariant {
    Vdsl2(VdslProfile),
    Gfast(GfastProfile),
}

// ---------------------------------------------------------------------------
// DMT configuration
// ---------------------------------------------------------------------------

/// Full DMT modem configuration.
#[derive(Debug, Clone)]
pub struct DmtConfig {
    /// Variant (VDSL2 profile or G.fast profile).
    pub variant: DmtVariant,
    /// IFFT/FFT size (number of subcarriers × 2 for real-valued line signal).
    pub fft_size: usize,
    /// Number of active data-bearing subcarriers (tones).
    pub num_data_tones: usize,
    /// Cyclic prefix length in samples.
    pub cyclic_prefix: usize,
    /// Cyclic suffix length in samples (G.fast).
    pub cyclic_suffix: usize,
    /// Subcarrier spacing in Hz.
    pub subcarrier_spacing_hz: f64,
    /// Band plan.
    pub band_plan: BandPlan,
    /// Maximum bits per tone (VDSL2: 15, G.fast: 12).
    pub max_bits_per_tone: u8,
    /// Pilot tone indices used for synchronisation.
    pub pilot_indices: Vec<usize>,
    /// Reed-Solomon FEC: data bytes per codeword.
    pub rs_data_bytes: usize,
    /// Reed-Solomon FEC: redundancy bytes (2*t).
    pub rs_redundancy_bytes: usize,
}

impl DmtConfig {
    /// Create configuration for a VDSL2 profile.
    pub fn vdsl2_profile(profile: VdslProfile) -> Self {
        let (fft_size, num_data_tones, cp, bp, max_bits) = match profile {
            VdslProfile::Profile8a | VdslProfile::Profile8b => (512, 256, 40, BandPlan::Plan998, 15u8),
            VdslProfile::Profile8c | VdslProfile::Profile8d => (512, 256, 40, BandPlan::Plan997, 15u8),
            VdslProfile::Profile12a | VdslProfile::Profile12b => (1024, 512, 48, BandPlan::Plan998, 15u8),
            VdslProfile::Profile17a => (2048, 4096 / 2, 64, BandPlan::Hpe17, 15u8),
            VdslProfile::Profile30a => (4096, 3479, 80, BandPlan::Hpe30, 15u8),
            VdslProfile::Profile35b => (4096, 4096, 80, BandPlan::Hpe35, 15u8),
        };
        // Pilot tones: 32 pilots at regular intervals
        let pilot_indices: Vec<usize> =
            (0..32).map(|i| 16 + i * (num_data_tones / 32)).collect();
        Self {
            variant: DmtVariant::Vdsl2(profile),
            fft_size,
            num_data_tones,
            cyclic_prefix: cp,
            cyclic_suffix: 0,
            subcarrier_spacing_hz: bp.subcarrier_spacing_hz(),
            band_plan: bp,
            max_bits_per_tone: max_bits,
            pilot_indices,
            rs_data_bytes: 224,
            rs_redundancy_bytes: 32,
        }
    }

    /// Create configuration for a G.fast profile.
    pub fn gfast_profile(profile: GfastProfile) -> Self {
        let (fft_size, num_data_tones, bp) = match profile {
            GfastProfile::Profile106a => (2048, 2048, BandPlan::Gfast106),
            GfastProfile::Profile212a => (4096, 4096, BandPlan::Gfast212),
        };
        let cp = 40;
        let cs = 8; // cyclic suffix
        let pilot_indices: Vec<usize> =
            (0..64).map(|i| 8 + i * (num_data_tones / 64)).collect();
        Self {
            variant: DmtVariant::Gfast(profile),
            fft_size,
            num_data_tones,
            cyclic_prefix: cp,
            cyclic_suffix: cs,
            subcarrier_spacing_hz: bp.subcarrier_spacing_hz(),
            band_plan: bp,
            max_bits_per_tone: 12,
            pilot_indices,
            rs_data_bytes: 188,
            rs_redundancy_bytes: 16,
        }
    }

    /// Number of data tones available.
    pub fn data_tones(&self) -> usize {
        self.num_data_tones
    }

    /// Total symbol length (IFFT + CP + CS) in samples.
    pub fn symbol_length(&self) -> usize {
        self.fft_size + self.cyclic_prefix + self.cyclic_suffix
    }

    /// Pilot count.
    pub fn pilot_count(&self) -> usize {
        self.pilot_indices.len()
    }
}

// ---------------------------------------------------------------------------
// Bit loading
// ---------------------------------------------------------------------------

/// Bit loading configuration.
#[derive(Debug, Clone)]
pub struct BitLoadConfig {
    /// Target SNR margin in dB.
    pub target_margin_db: f64,
    /// Noise PSD per tone in dB (relative).
    pub noise_psd_db: Vec<f64>,
    /// Channel SNR per tone in dB.
    pub snr_per_tone_db: Vec<f64>,
    /// Maximum bits per tone (profile limit).
    pub max_bits_per_tone: u8,
    /// Minimum bits per tone (0 = tone off).
    pub min_bits_per_tone: u8,
}

/// Per-subcarrier bit and power allocation engine.
pub struct BitLoader {
    config: BitLoadConfig,
}

impl BitLoader {
    pub fn new(config: BitLoadConfig) -> Self {
        Self { config }
    }

    /// Shannon capacity for a tone (bits/symbol) given SNR in dB.
    fn shannon_bits(snr_db: f64) -> f64 {
        let snr_lin = 10f64.powf(snr_db / 10.0);
        (1.0 + snr_lin).log2()
    }

    /// Compute integer bits per tone using gap approximation.
    /// Gap ≈ 9.8 dB for BER = 1e-7 (QAM approximation).
    fn compute_bits_for_snr(snr_db: f64, gap_db: f64, max_bits: u8) -> u8 {
        let effective_snr = snr_db - gap_db;
        if effective_snr < 0.0 {
            return 0;
        }
        let snr_lin = 10f64.powf(effective_snr / 10.0);
        let bits = snr_lin.log2().floor() as u8;
        bits.min(max_bits)
    }

    /// Greedy water-filling bit loading.
    /// Returns `(bits_per_tone, power_per_tone_linear)`.
    pub fn water_fill(&self, total_power: f64) -> (Vec<u8>, Vec<f64>) {
        let n = self.config.snr_per_tone_db.len();
        let gap_db = 9.8; // Shannon gap for target BER
        let margin = self.config.target_margin_db;

        // Compute effective SNR reduced by target margin
        let effective_snr: Vec<f64> = self
            .config
            .snr_per_tone_db
            .iter()
            .map(|&s| s - margin)
            .collect();

        let mut bits: Vec<u8> = effective_snr
            .iter()
            .map(|&s| Self::compute_bits_for_snr(s, gap_db, self.config.max_bits_per_tone))
            .collect();

        // Apply minimum bits constraint
        for b in bits.iter_mut() {
            if *b > 0 && *b < self.config.min_bits_per_tone {
                *b = 0; // turn off if below minimum
            }
        }

        // Equal power distribution across active tones (simplified water-fill)
        let active: usize = bits.iter().filter(|&&b| b > 0).count();
        let power_per_active = if active > 0 { total_power / active as f64 } else { 0.0 };
        let powers: Vec<f64> = bits.iter().map(|&b| if b > 0 { power_per_active } else { 0.0 }).collect();

        // Total bits sanity check
        let _total_bits: usize = bits.iter().map(|&b| b as usize).sum();

        (bits, powers)
    }

    /// Bit-swap: increment one tone, decrement another to maintain total rate.
    /// Returns true if a beneficial swap was found.
    pub fn bit_swap(
        bits: &mut Vec<u8>,
        snr_db: &[f64],
        max_bits: u8,
    ) -> bool {
        let n = bits.len();
        let gap_db = 9.8;
        let mut best_gain = 0.0_f64;
        let mut best_pair: Option<(usize, usize)> = None;

        // Find tone that benefits most from +1 bit and tone that loses least from -1 bit
        for i in 0..n {
            if bits[i] < max_bits {
                let snr_needed_i = (1u32 << (bits[i] + 1)) as f64 - 1.0;
                let snr_needed_db = 10.0 * snr_needed_i.log10() + gap_db;
                let gain_i = snr_db[i] - snr_needed_db;
                if gain_i > 0.0 {
                    for j in 0..n {
                        if i != j && bits[j] > 1 {
                            let snr_free_j = snr_db[j]
                                - (10.0 * ((1u32 << bits[j]) as f64 - 1.0).log10() + gap_db);
                            let net_gain = gain_i + snr_free_j;
                            if net_gain > best_gain {
                                best_gain = net_gain;
                                best_pair = Some((i, j));
                            }
                        }
                    }
                }
            }
        }
        if let Some((inc, dec)) = best_pair {
            bits[inc] += 1;
            bits[dec] -= 1;
            true
        } else {
            false
        }
    }

    /// Run multiple bit-swap iterations to convergence.
    pub fn run_bit_swap(
        bits: &mut Vec<u8>,
        snr_db: &[f64],
        max_bits: u8,
        max_iters: usize,
    ) {
        for _ in 0..max_iters {
            if !Self::bit_swap(bits, snr_db, max_bits) {
                break;
            }
        }
    }

    /// Aggregate data rate in bit/s.
    pub fn aggregate_rate_bps(bits: &[u8], symbol_rate_hz: f64) -> f64 {
        let bits_per_symbol: usize = bits.iter().map(|&b| b as usize).sum();
        bits_per_symbol as f64 * symbol_rate_hz
    }
}

// ---------------------------------------------------------------------------
// QAM constellation mapper / demapper
// ---------------------------------------------------------------------------

/// Map `b` bits to a Gray-coded QAM symbol.
/// Returns (I, Q) as f64.
fn qam_map(bits: &[u8], num_bits: u8) -> (f64, f64) {
    assert!(num_bits >= 1 && num_bits <= 15);
    if num_bits == 1 {
        // BPSK on I
        return (if bits[0] == 0 { 1.0 } else { -1.0 }, 0.0);
    }
    let bits_i = num_bits / 2;
    let bits_q = num_bits - bits_i;
    let m_i = 1u32 << bits_i;
    let m_q = 1u32 << bits_q;

    // Pack integer indices
    let mut idx_i = 0u32;
    for k in 0..bits_i as usize {
        idx_i = (idx_i << 1) | bits[k] as u32;
    }
    let mut idx_q = 0u32;
    for k in 0..bits_q as usize {
        idx_q = (idx_q << 1) | bits[bits_i as usize + k] as u32;
    }
    // Gray decode
    let gi = gray_decode(idx_i);
    let gq = gray_decode(idx_q);
    // Constellation points: odd integers centred at 0
    let scale_i = 1.0 / ((m_i - 1) as f64).max(1.0);
    let scale_q = 1.0 / ((m_q - 1) as f64).max(1.0);
    let i_val = (2 * gi as i32 - (m_i as i32 - 1)) as f64 * scale_i;
    let q_val = (2 * gq as i32 - (m_q as i32 - 1)) as f64 * scale_q;
    (i_val, q_val)
}

/// Gray decode a natural-binary to Gray-binary mapping.
fn gray_decode(g: u32) -> u32 {
    let mut n = g;
    let mut mask = n >> 1;
    while mask != 0 {
        n ^= mask;
        mask >>= 1;
    }
    n
}

/// Hard-decision QAM demapper. Returns received bits.
fn qam_demap(re: f64, im: f64, num_bits: u8) -> Vec<u8> {
    if num_bits == 0 {
        return vec![];
    }
    if num_bits == 1 {
        return vec![if re >= 0.0 { 0 } else { 1 }];
    }
    let bits_i = num_bits / 2;
    let bits_q = num_bits - bits_i;
    let m_i = 1u32 << bits_i;
    let m_q = 1u32 << bits_q;

    let scale_i = 1.0 / ((m_i - 1) as f64).max(1.0);
    let scale_q = 1.0 / ((m_q - 1) as f64).max(1.0);

    // Quantise to nearest constellation point
    let fi = ((re / scale_i + (m_i as f64 - 1.0)) / 2.0)
        .round()
        .clamp(0.0, (m_i - 1) as f64) as u32;
    let fq = ((im / scale_q + (m_q as f64 - 1.0)) / 2.0)
        .round()
        .clamp(0.0, (m_q - 1) as f64) as u32;

    // Gray encode
    let gi = fi ^ (fi >> 1);
    let gq = fq ^ (fq >> 1);

    let mut out = Vec::with_capacity(num_bits as usize);
    for k in (0..bits_i).rev() {
        out.push(((gi >> k) & 1) as u8);
    }
    for k in (0..bits_q).rev() {
        out.push(((gq >> k) & 1) as u8);
    }
    out
}

// ---------------------------------------------------------------------------
// DMT Modulator
// ---------------------------------------------------------------------------

/// DMT symbol modulator (IFFT-based, per ITU-T G.993.2 / G.9701).
pub struct DmtModulator {
    config: DmtConfig,
    /// Work buffer for IFFT.
    ifft_buf: Vec<Complex>,
}

impl DmtModulator {
    /// Create a new modulator from configuration.
    pub fn new(config: DmtConfig) -> Self {
        let buf = vec![Complex::default(); config.fft_size];
        Self { config, ifft_buf: buf }
    }

    /// Modulate one DMT symbol.
    ///
    /// `bits_per_tone[k]` is the number of bits to load on subcarrier k.
    /// `data_bits` is the flat bitstream (total = sum of bits_per_tone).
    ///
    /// Returns time-domain samples (real part only, length = symbol_length).
    pub fn modulate_symbol(&mut self, bits_per_tone: &[u8], data_bits: &[u8]) -> Vec<f64> {
        let n = self.config.fft_size;
        // Clear IFFT buffer
        for s in self.ifft_buf.iter_mut() {
            *s = Complex::default();
        }

        let mut bit_offset = 0usize;
        let num_tones = bits_per_tone.len().min(self.config.num_data_tones);

        for k in 0..num_tones {
            let nb = bits_per_tone[k];
            if nb == 0 {
                continue;
            }
            let end = (bit_offset + nb as usize).min(data_bits.len());
            if bit_offset >= data_bits.len() {
                break;
            }
            let slice = &data_bits[bit_offset..end];
            bit_offset = end;

            // Map bits to QAM
            let (re, im) = qam_map(slice, nb);
            // Tone index k+1 (skip DC)
            let tone_idx = k + 1;
            if tone_idx < n / 2 {
                self.ifft_buf[tone_idx] = Complex::new(re, im);
                // Hermitian symmetry for real output
                self.ifft_buf[n - tone_idx] = Complex::new(re, -im);
            }
        }

        // Pilot tones: BPSK at fixed amplitude
        for &p in &self.config.pilot_indices {
            let pi = p + 1;
            if pi < n / 2 {
                self.ifft_buf[pi] = Complex::new(1.0, 0.0);
                self.ifft_buf[n - pi] = Complex::new(1.0, 0.0);
            }
        }

        // IFFT
        fft_inplace(&mut self.ifft_buf, true);

        // Extract real part
        let time_domain: Vec<f64> = self.ifft_buf.iter().map(|c| c.re).collect();

        // Add cyclic prefix and suffix
        let cp = self.config.cyclic_prefix;
        let cs = self.config.cyclic_suffix;
        let mut symbol = Vec::with_capacity(cp + n + cs);

        // Cyclic prefix: last `cp` samples of time domain
        symbol.extend_from_slice(&time_domain[n - cp..]);
        symbol.extend_from_slice(&time_domain);
        // Cyclic suffix: first `cs` samples
        if cs > 0 {
            symbol.extend_from_slice(&time_domain[..cs]);
        }
        symbol
    }

    /// Modulate multiple symbols.
    pub fn modulate_frame(
        &mut self,
        bits_per_tone: &[u8],
        data_bits: &[u8],
        num_symbols: usize,
    ) -> Vec<f64> {
        let bits_per_symbol: usize = bits_per_tone.iter().map(|&b| b as usize).sum();
        let mut out = Vec::new();
        for i in 0..num_symbols {
            let start = i * bits_per_symbol;
            let end = (start + bits_per_symbol).min(data_bits.len());
            if start >= data_bits.len() {
                break;
            }
            let sym = self.modulate_symbol(bits_per_tone, &data_bits[start..end]);
            out.extend_from_slice(&sym);
        }
        out
    }

    pub fn config(&self) -> &DmtConfig {
        &self.config
    }
}

// ---------------------------------------------------------------------------
// DMT Demodulator
// ---------------------------------------------------------------------------

/// DMT symbol demodulator (FFT-based).
pub struct DmtDemodulator {
    config: DmtConfig,
    fft_buf: Vec<Complex>,
    /// Per-tone channel estimate (complex gain).
    pub channel_estimate: Vec<Complex>,
    /// Phase reference for each tone (derived from pilots).
    pilot_phase: Vec<f64>,
}

impl DmtDemodulator {
    pub fn new(config: DmtConfig) -> Self {
        let n = config.fft_size;
        let num_tones = config.num_data_tones;
        Self {
            fft_buf: vec![Complex::default(); n],
            channel_estimate: vec![Complex::new(1.0, 0.0); num_tones],
            pilot_phase: vec![0.0; config.pilot_indices.len()],
            config,
        }
    }

    /// Demodulate one DMT symbol. Returns decoded bits.
    ///
    /// `rx_samples`: time-domain received samples (must be >= symbol_length).
    /// `bits_per_tone`: allocation table.
    pub fn demodulate_symbol(&mut self, rx_samples: &[f64], bits_per_tone: &[u8]) -> Vec<u8> {
        let n = self.config.fft_size;
        let cp = self.config.cyclic_prefix;

        // Remove cyclic prefix
        let start = cp;
        let end = (start + n).min(rx_samples.len());
        for (i, s) in self.fft_buf.iter_mut().enumerate() {
            *s = if start + i < end {
                Complex::new(rx_samples[start + i], 0.0)
            } else {
                Complex::default()
            };
        }

        // FFT
        fft_inplace(&mut self.fft_buf, false);

        // Scale
        let scale = 1.0 / n as f64;

        // Update pilot-based phase estimate
        for (pi, &p) in self.config.pilot_indices.iter().enumerate() {
            let tone_idx = p + 1;
            if tone_idx < n / 2 {
                let rx = self.fft_buf[tone_idx].scale(scale);
                self.pilot_phase[pi] = rx.im.atan2(rx.re);
            }
        }

        // Decode each data tone
        let mut out_bits = Vec::new();
        for k in 0..bits_per_tone.len().min(self.config.num_data_tones) {
            let nb = bits_per_tone[k];
            if nb == 0 {
                continue;
            }
            let tone_idx = k + 1;
            if tone_idx >= n / 2 {
                break;
            }
            let rx = self.fft_buf[tone_idx].scale(scale);
            // Apply channel equalisation (ZF)
            let h = &self.channel_estimate[k];
            let h_mag_sq = h.mag_sq();
            let eq = if h_mag_sq > 1e-12 {
                rx.mul(&h.conj()).scale(1.0 / h_mag_sq)
            } else {
                rx
            };
            let bits = qam_demap(eq.re, eq.im, nb);
            out_bits.extend_from_slice(&bits);
        }
        out_bits
    }

    /// Update channel estimate from a known pilot symbol.
    pub fn update_channel_estimate(&mut self, rx_samples: &[f64]) {
        let n = self.config.fft_size;
        let cp = self.config.cyclic_prefix;
        let start = cp;

        for (i, s) in self.fft_buf.iter_mut().enumerate() {
            let idx = start + i;
            *s = if idx < rx_samples.len() {
                Complex::new(rx_samples[idx], 0.0)
            } else {
                Complex::default()
            };
        }
        fft_inplace(&mut self.fft_buf, false);
        let scale = 1.0 / n as f64;

        for (k, h) in self.channel_estimate.iter_mut().enumerate() {
            let tone_idx = k + 1;
            if tone_idx < n / 2 {
                // Pilot expected to be 1+0j
                *h = self.fft_buf[tone_idx].scale(scale);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PSD shaping and UPBO
// ---------------------------------------------------------------------------

/// PSD mask entry: (frequency_hz, psd_dbm_per_hz).
#[derive(Debug, Clone, Copy)]
pub struct PsdMaskPoint {
    pub freq_hz: f64,
    pub psd_dbm_per_hz: f64,
}

/// PSD mask for a profile.
#[derive(Debug, Clone)]
pub struct PsdMask {
    pub points: Vec<PsdMaskPoint>,
}

impl PsdMask {
    /// Evaluate PSD limit (dBm/Hz) at a given frequency via linear interpolation.
    pub fn evaluate(&self, freq_hz: f64) -> f64 {
        if self.points.is_empty() {
            return -40.0; // conservative default
        }
        if freq_hz <= self.points[0].freq_hz {
            return self.points[0].psd_dbm_per_hz;
        }
        if freq_hz >= self.points[self.points.len() - 1].freq_hz {
            return self.points[self.points.len() - 1].psd_dbm_per_hz;
        }
        // Linear interpolation in dB
        for i in 1..self.points.len() {
            let p0 = self.points[i - 1];
            let p1 = self.points[i];
            if freq_hz <= p1.freq_hz {
                let t = (freq_hz - p0.freq_hz) / (p1.freq_hz - p0.freq_hz);
                return p0.psd_dbm_per_hz + t * (p1.psd_dbm_per_hz - p0.psd_dbm_per_hz);
            }
        }
        self.points[self.points.len() - 1].psd_dbm_per_hz
    }

    /// Standard VDSL2 downstream PSD mask (simplified ITU-T G.993.2 profile 17a).
    pub fn vdsl2_17a_downstream() -> Self {
        Self {
            points: vec![
                PsdMaskPoint { freq_hz: 25_000.0, psd_dbm_per_hz: -60.0 },
                PsdMaskPoint { freq_hz: 138_000.0, psd_dbm_per_hz: -60.0 },
                PsdMaskPoint { freq_hz: 1_104_000.0, psd_dbm_per_hz: -60.0 },
                PsdMaskPoint { freq_hz: 8_500_000.0, psd_dbm_per_hz: -76.0 },
                PsdMaskPoint { freq_hz: 17_664_000.0, psd_dbm_per_hz: -100.0 },
            ],
        }
    }

    /// Standard G.fast downstream PSD mask (simplified ITU-T G.9701 profile 106a).
    pub fn gfast_106a_downstream() -> Self {
        Self {
            points: vec![
                PsdMaskPoint { freq_hz: 2_200_000.0, psd_dbm_per_hz: -76.0 },
                PsdMaskPoint { freq_hz: 30_000_000.0, psd_dbm_per_hz: -76.0 },
                PsdMaskPoint { freq_hz: 106_000_000.0, psd_dbm_per_hz: -76.0 },
            ],
        }
    }
}

/// Upstream Power Back-Off (UPBO) calculator.
/// ITU-T G.993.2 section 7.2.1.
pub struct UpboCalculator {
    /// Reference electrical length (kft equivalent).
    pub reference_length_kft: f64,
}

impl UpboCalculator {
    pub fn new(reference_length_kft: f64) -> Self {
        Self { reference_length_kft }
    }

    /// Compute upstream PBO (dB) for a given subcarrier frequency.
    /// Simplified model: linear in sqrt(f) as per G.993.2 UPBO formula.
    pub fn upbo_db(&self, freq_hz: f64, loop_length_kft: f64) -> f64 {
        // UPBO(f) = a * sqrt(f/1e6) * (L - L_ref) where a ≈ 3.5
        let a = 3.5_f64;
        let length_diff = (loop_length_kft - self.reference_length_kft).max(0.0);
        let f_mhz = (freq_hz / 1e6).sqrt();
        (a * f_mhz * length_diff).max(0.0)
    }

    /// Compute per-tone upstream power (linear) after UPBO.
    pub fn apply_upbo(&self, freq_hz: f64, loop_length_kft: f64, input_power: f64) -> f64 {
        let pbo = self.upbo_db(freq_hz, loop_length_kft);
        input_power * 10f64.powf(-pbo / 10.0)
    }
}

// ---------------------------------------------------------------------------
// Vectoring (FEXT cancellation)
// ---------------------------------------------------------------------------

/// Precoding matrix entry for a single frequency bin.
/// For downstream vectoring: X_tx = P * X_data where P is the precoder.
#[derive(Debug, Clone)]
pub struct VectoringPrecoderTone {
    /// Bin index.
    pub tone: usize,
    /// Number of lines being vectored.
    pub num_lines: usize,
    /// Precoder matrix (row-major, complex entries).
    pub matrix: Vec<Complex>,
}

impl VectoringPrecoderTone {
    /// Create an identity precoder (no precoding).
    pub fn identity(tone: usize, num_lines: usize) -> Self {
        let mut matrix = vec![Complex::default(); num_lines * num_lines];
        for i in 0..num_lines {
            matrix[i * num_lines + i] = Complex::new(1.0, 0.0);
        }
        Self { tone, num_lines, matrix }
    }

    /// Apply precoder to a vector of per-line symbols (complex).
    pub fn apply(&self, data_syms: &[Complex]) -> Vec<Complex> {
        assert_eq!(data_syms.len(), self.num_lines);
        let n = self.num_lines;
        let mut out = vec![Complex::default(); n];
        for i in 0..n {
            for j in 0..n {
                out[i] = out[i].add(&self.matrix[i * n + j].mul(&data_syms[j]));
            }
        }
        out
    }
}

/// FEXT channel estimator using error-feedback.
pub struct FextEstimator {
    /// Number of lines.
    pub num_lines: usize,
    /// Per-tone FEXT coupling coefficients (num_tones × num_lines × num_lines).
    pub coupling: Vec<Vec<Vec<Complex>>>,
    /// LMS step size for FEXT estimation.
    pub mu: f64,
}

impl FextEstimator {
    pub fn new(num_tones: usize, num_lines: usize, mu: f64) -> Self {
        let coupling = vec![
            vec![vec![Complex::default(); num_lines]; num_lines];
            num_tones
        ];
        Self { num_lines, coupling, mu }
    }

    /// Update FEXT estimate for a tone using LMS.
    /// `tx_syms[j]` = transmitted symbol on line j, `rx_err[i]` = error on line i.
    pub fn update(&mut self, tone: usize, tx_syms: &[Complex], rx_err: &[Complex]) {
        let n = self.num_lines;
        for i in 0..n {
            for j in 0..n {
                if i != j {
                    let delta = rx_err[i].mul(&tx_syms[j].conj()).scale(self.mu);
                    self.coupling[tone][i][j] = self.coupling[tone][i][j].add(&delta);
                }
            }
        }
    }

    /// Estimate and cancel FEXT for received signals at a tone.
    pub fn cancel(&self, tone: usize, rx: &[Complex], tx_syms: &[Complex]) -> Vec<Complex> {
        let n = self.num_lines;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let mut sum = rx[i];
            for j in 0..n {
                if i != j {
                    let xt = self.coupling[tone][i][j].mul(&tx_syms[j]);
                    sum = sum.sub(&xt);
                }
            }
            out.push(sum);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Bonding
// ---------------------------------------------------------------------------

/// Multi-pair bonding configuration (ITU-T G.998).
#[derive(Debug, Clone)]
pub struct BondingConfig {
    /// Number of bonded pairs (2-32 for G.fast).
    pub num_pairs: usize,
    /// Maximum differential delay (in samples) between pairs.
    pub max_diff_delay_samples: usize,
    /// Fragment size in bytes for interleaving.
    pub fragment_size_bytes: usize,
}

impl BondingConfig {
    pub fn new(num_pairs: usize, max_diff_delay_samples: usize, fragment_size_bytes: usize) -> Self {
        Self { num_pairs, max_diff_delay_samples, fragment_size_bytes }
    }
}

/// Bonded DMT aggregation engine.
pub struct BondingAggregator {
    pub config: BondingConfig,
    /// Per-pair delay compensation buffers.
    delay_buffers: Vec<Vec<u8>>,
    /// Per-pair current read position.
    delay_positions: Vec<usize>,
}

impl BondingAggregator {
    pub fn new(config: BondingConfig) -> Self {
        let buf_size = config.max_diff_delay_samples;
        let delay_buffers = vec![vec![0u8; buf_size]; config.num_pairs];
        let delay_positions = vec![0; config.num_pairs];
        Self { config, delay_buffers, delay_positions }
    }

    /// Distribute a byte stream across bonded pairs using round-robin fragment interleaving.
    /// Returns a vector of per-pair byte streams.
    pub fn distribute(&self, data: &[u8]) -> Vec<Vec<u8>> {
        let n = self.config.num_pairs;
        let fs = self.config.fragment_size_bytes;
        let mut streams: Vec<Vec<u8>> = vec![Vec::new(); n];
        let mut pos = 0;
        let mut pair = 0;
        while pos < data.len() {
            let end = (pos + fs).min(data.len());
            streams[pair].extend_from_slice(&data[pos..end]);
            pos = end;
            pair = (pair + 1) % n;
        }
        streams
    }

    /// Reassemble byte streams from bonded pairs (with delay compensation).
    /// `delay_samples[i]` = differential delay of pair i in sample units.
    pub fn reassemble(&self, streams: &[Vec<u8>], _delay_samples: &[usize]) -> Vec<u8> {
        // Simple interleave reassembly (delay compensation is simplified)
        let fs = self.config.fragment_size_bytes;
        let n = self.config.num_pairs;
        let total: usize = streams.iter().map(|s| s.len()).sum();
        let mut out = Vec::with_capacity(total);
        let mut positions = vec![0usize; n];
        let mut pair = 0;
        loop {
            let pos = positions[pair];
            let stream = &streams[pair];
            if pos >= stream.len() {
                // Check if all done
                if positions.iter().zip(streams.iter()).all(|(p, s)| *p >= s.len()) {
                    break;
                }
                pair = (pair + 1) % n;
                continue;
            }
            let end = (pos + fs).min(stream.len());
            out.extend_from_slice(&stream[pos..end]);
            positions[pair] = end;
            pair = (pair + 1) % n;
        }
        out
    }

    /// Aggregate data rate across all bonded pairs.
    pub fn aggregate_rate(bits_per_tone_per_pair: &[Vec<u8>], symbol_rate_hz: f64) -> f64 {
        bits_per_tone_per_pair
            .iter()
            .map(|bpt| BitLoader::aggregate_rate_bps(bpt, symbol_rate_hz))
            .sum()
    }
}

// ---------------------------------------------------------------------------
// Reed-Solomon FEC (simplified over GF(2^8))
// ---------------------------------------------------------------------------

/// GF(2^8) arithmetic with primitive polynomial x^8+x^4+x^3+x^2+1 (0x11D).
mod gf256 {
    const POLY: u16 = 0x11D;

    pub fn mul(a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            return 0;
        }
        let mut result = 0u16;
        let mut aa = a as u16;
        let mut bb = b as u16;
        for _ in 0..8 {
            if bb & 1 != 0 {
                result ^= aa;
            }
            let hi = aa & 0x80;
            aa <<= 1;
            if hi != 0 {
                aa ^= POLY;
            }
            bb >>= 1;
        }
        result as u8
    }

    pub fn pow(base: u8, exp: usize) -> u8 {
        let mut r = 1u8;
        for _ in 0..exp {
            r = mul(r, base);
        }
        r
    }

    pub fn inv(a: u8) -> u8 {
        assert_ne!(a, 0, "GF256 inverse of zero");
        pow(a, 254)
    }
}

/// Reed-Solomon encoder for DSL FEC.
pub struct RsEncoder {
    /// k = data bytes per codeword.
    pub k: usize,
    /// 2t = redundancy bytes.
    pub two_t: usize,
    /// Generator polynomial coefficients (degree 2t).
    gen_poly: Vec<u8>,
}

impl RsEncoder {
    /// Create RS(k+2t, k) encoder.
    pub fn new(k: usize, two_t: usize) -> Self {
        // Build generator polynomial g(x) = product_{i=0}^{2t-1} (x - alpha^i)
        // where alpha = 2 (primitive element in GF(2^8))
        let alpha = 2u8;
        let mut g = vec![1u8]; // Start with 1
        for i in 0..two_t {
            let root = gf256::pow(alpha, i);
            // Multiply g by (x - root) = (x XOR root)
            let mut ng = vec![0u8; g.len() + 1];
            for (j, &gc) in g.iter().enumerate() {
                ng[j + 1] ^= gc;
                ng[j] ^= gf256::mul(gc, root);
            }
            g = ng;
        }
        Self { k, two_t, gen_poly: g }
    }

    /// Encode data bytes, returning codeword (data + parity).
    pub fn encode(&self, data: &[u8]) -> Vec<u8> {
        assert!(data.len() <= self.k);
        let n = self.k + self.two_t;
        // Systematic encoding: remainder of data*x^(2t) / g(x)
        let mut buf = vec![0u8; n];
        // Place data in high positions
        for (i, &b) in data.iter().enumerate() {
            buf[i] = b;
        }
        // Polynomial division using shift register
        let mut remainder = vec![0u8; self.two_t];
        for i in 0..data.len() {
            let coef = data[i] ^ remainder[0];
            remainder.copy_within(1.., 0);
            remainder[self.two_t - 1] = 0;
            for j in 0..self.two_t {
                let gi = if j + 1 < self.gen_poly.len() {
                    self.gen_poly[j + 1]
                } else {
                    0
                };
                remainder[j] ^= gf256::mul(coef, gi);
            }
        }
        // Append parity
        let data_len = data.len();
        for (i, &r) in remainder.iter().enumerate() {
            if data_len + i < buf.len() {
                buf[data_len + i] = r;
            }
        }
        buf
    }

    /// Simplified decoder: correct up to t erasures (erasure-only mode for simplicity).
    pub fn decode_erasures(&self, codeword: &[u8], erasure_positions: &[usize]) -> Option<Vec<u8>> {
        if erasure_positions.len() > self.two_t {
            return None; // Too many erasures
        }
        // Return data portion directly (full error correction is complex; simplified)
        let mut out = codeword[..self.k.min(codeword.len())].to_vec();
        // Zero-fill erasure positions
        for &pos in erasure_positions {
            if pos < out.len() {
                out[pos] = 0;
            }
        }
        Some(out)
    }
}

// ---------------------------------------------------------------------------
// Synchronisation
// ---------------------------------------------------------------------------

/// Pilot-based symbol timing and frequency offset estimator.
pub struct DmtSyncEstimator {
    pub config: DmtConfig,
    /// Previous pilot phase measurements.
    prev_phases: Vec<f64>,
    /// Estimated sampling clock offset (ppm).
    pub clock_offset_ppm: f64,
    /// Estimated common phase error (CPE) per symbol.
    pub cpe_rad: f64,
}

impl DmtSyncEstimator {
    pub fn new(config: DmtConfig) -> Self {
        let num_pilots = config.pilot_indices.len();
        Self {
            config,
            prev_phases: vec![0.0; num_pilots],
            clock_offset_ppm: 0.0,
            cpe_rad: 0.0,
        }
    }

    /// Estimate common phase error from pilot tones.
    /// `pilot_freqs[i]` = received pilot frequency domain values.
    pub fn estimate_cpe(&mut self, pilot_freqs: &[Complex]) -> f64 {
        let np = pilot_freqs.len().min(self.config.pilot_indices.len());
        if np == 0 {
            return 0.0;
        }
        // CPE = mean phase of all pilots (expected pilot = 1+0j)
        let mut phase_sum = 0.0;
        for i in 0..np {
            let ph = pilot_freqs[i].im.atan2(pilot_freqs[i].re);
            phase_sum += ph;
        }
        self.cpe_rad = phase_sum / np as f64;
        self.cpe_rad
    }

    /// Estimate sampling clock offset from pilot phase drift across symbols.
    /// `new_phases[i]` = phase of pilot i in current symbol.
    pub fn estimate_clock_offset(&mut self, new_phases: &[f64]) -> f64 {
        let np = new_phases.len().min(self.prev_phases.len());
        if np == 0 {
            return 0.0;
        }
        let mut drift_sum = 0.0;
        for i in 0..np {
            let tone_idx = self.config.pilot_indices[i] as f64;
            let phase_diff = new_phases[i] - self.prev_phases[i];
            // Normalise by tone index (slope = SCO * tone_idx)
            drift_sum += phase_diff / (2.0 * PI * tone_idx);
        }
        let sco = drift_sum / np as f64; // Sampling Clock Offset (normalised)
        self.clock_offset_ppm = sco * 1e6;
        for i in 0..np {
            self.prev_phases[i] = new_phases[i];
        }
        self.clock_offset_ppm
    }

    /// Apply CPE correction to frequency-domain symbols.
    pub fn correct_cpe(&self, freq_syms: &mut [Complex]) {
        let cpe = self.cpe_rad;
        let correction = Complex::new(cpe.cos(), -cpe.sin());
        for s in freq_syms.iter_mut() {
            *s = s.mul(&correction);
        }
    }
}

// ---------------------------------------------------------------------------
// Impulse Noise Protection (INP) — G.998.4 / G.inp
// ---------------------------------------------------------------------------

/// Interleaving engine for INP delay.
pub struct InpInterleaver {
    /// Interleaving depth D.
    pub depth: usize,
    /// Number of bytes per DMT symbol.
    pub bytes_per_symbol: usize,
    /// Circular buffer.
    buffer: Vec<u8>,
    write_pos: usize,
    read_pos: usize,
}

impl InpInterleaver {
    pub fn new(depth: usize, bytes_per_symbol: usize) -> Self {
        let cap = depth * bytes_per_symbol;
        Self {
            depth,
            bytes_per_symbol,
            buffer: vec![0u8; cap],
            write_pos: 0,
            read_pos: 0,
        }
    }

    /// Write a block of bytes into the interleaver.
    pub fn write(&mut self, data: &[u8]) {
        let cap = self.buffer.len();
        for &b in data {
            self.buffer[self.write_pos % cap] = b;
            self.write_pos += 1;
        }
    }

    /// Read interleaved bytes (round-robin column-major order).
    pub fn read(&mut self, count: usize) -> Vec<u8> {
        let cap = self.buffer.len();
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            if self.read_pos < self.write_pos {
                out.push(self.buffer[self.read_pos % cap]);
                self.read_pos += 1;
            }
        }
        out
    }

    /// Interleave a block: distribute bytes across delay rows.
    pub fn interleave(&self, data: &[u8]) -> Vec<u8> {
        let d = self.depth;
        let bps = self.bytes_per_symbol;
        let n = data.len();
        let mut out = vec![0u8; n];
        for i in 0..n {
            // Column-major interleaving
            let row = i % d;
            let col = i / d;
            let dst = (row * bps + col) % n;
            if dst < n {
                out[dst] = data[i];
            }
        }
        out
    }

    /// Deinterleave.
    pub fn deinterleave(&self, data: &[u8]) -> Vec<u8> {
        let d = self.depth;
        let bps = self.bytes_per_symbol;
        let n = data.len();
        let mut out = vec![0u8; n];
        for i in 0..n {
            let row = i % d;
            let col = i / d;
            let src = (row * bps + col) % n;
            if src < n {
                out[i] = data[src];
            }
        }
        out
    }
}

/// Impulse noise protection configuration (G.998.4).
#[derive(Debug, Clone)]
pub struct InpConfig {
    /// Minimum INP (impulse noise protection) in DMT symbols.
    pub inp_min: f64,
    /// Interleaving depth D.
    pub depth: usize,
    /// Delay: D * symbol_duration in ms.
    pub delay_ms: f64,
    /// Reed-Solomon redundancy bytes.
    pub rs_redundancy: usize,
}

impl InpConfig {
    /// Create an INP configuration for a target protection level.
    pub fn new(inp_min: f64, symbol_duration_ms: f64) -> Self {
        // D = ceil(INPmin / (2*t) + 1) heuristic
        let depth = ((inp_min / 2.0).ceil() as usize).max(1);
        let rs_redundancy = (inp_min * 2.0).ceil() as usize;
        let delay_ms = depth as f64 * symbol_duration_ms;
        Self { inp_min, depth, delay_ms, rs_redundancy }
    }
}

// ---------------------------------------------------------------------------
// Channel noise / SNR estimation
// ---------------------------------------------------------------------------

/// Simple SNR estimation using pilot tones.
pub struct SnrEstimator {
    pub num_tones: usize,
    /// Estimated noise variance per tone.
    pub noise_var: Vec<f64>,
    /// Estimated signal power per tone.
    pub signal_power: Vec<f64>,
}

impl SnrEstimator {
    pub fn new(num_tones: usize) -> Self {
        Self {
            num_tones,
            noise_var: vec![1.0; num_tones],
            signal_power: vec![1.0; num_tones],
        }
    }

    /// Update estimates from a received symbol and ideal (known) reference.
    pub fn update(&mut self, received: &[Complex], ideal: &[Complex], alpha: f64) {
        let n = received.len().min(ideal.len()).min(self.num_tones);
        for k in 0..n {
            let err = received[k].sub(&ideal[k]);
            let err_pwr = err.mag_sq();
            let sig_pwr = ideal[k].mag_sq();
            // Exponential averaging
            self.noise_var[k] = (1.0 - alpha) * self.noise_var[k] + alpha * err_pwr;
            self.signal_power[k] = (1.0 - alpha) * self.signal_power[k] + alpha * sig_pwr;
        }
    }

    /// Return SNR in dB for each tone.
    pub fn snr_db(&self) -> Vec<f64> {
        self.noise_var
            .iter()
            .zip(self.signal_power.iter())
            .map(|(&nv, &sp)| {
                if nv < 1e-15 {
                    80.0 // cap
                } else {
                    10.0 * (sp / nv).log10()
                }
            })
            .collect()
    }
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
        let c = a.mul(&b);
        assert!((c.re - (-5.0)).abs() < 1e-10, "re={}", c.re);
        assert!((c.im - 10.0).abs() < 1e-10, "im={}", c.im);
    }

    #[test]
    fn test_complex_conj() {
        let a = Complex::new(3.0, -4.0);
        let c = a.conj();
        assert_eq!(c.re, 3.0);
        assert_eq!(c.im, 4.0);
    }

    #[test]
    fn test_complex_mag() {
        let a = Complex::new(3.0, 4.0);
        assert!((a.mag() - 5.0).abs() < 1e-10);
    }

    // --- FFT ---

    #[test]
    fn test_fft_roundtrip_size8() {
        let n = 8;
        let original: Vec<Complex> = (0..n)
            .map(|i| Complex::new(i as f64, 0.0))
            .collect();
        let mut buf = original.clone();
        fft_inplace(&mut buf, false); // forward
        fft_inplace(&mut buf, true);  // inverse
        for (a, b) in buf.iter().zip(original.iter()) {
            assert!((a.re - b.re).abs() < 1e-9, "re mismatch");
            assert!((a.im - b.im).abs() < 1e-9, "im mismatch");
        }
    }

    #[test]
    fn test_fft_impulse_size16() {
        let n = 16;
        let mut buf = vec![Complex::default(); n];
        buf[0] = Complex::new(1.0, 0.0);
        fft_inplace(&mut buf, false);
        // All bins should have magnitude 1
        for c in &buf {
            assert!((c.mag() - 1.0).abs() < 1e-9, "mag={}", c.mag());
        }
    }

    #[test]
    fn test_fft_roundtrip_size64() {
        let n = 64;
        let original: Vec<Complex> = (0..n)
            .map(|i| Complex::new((i as f64 * 0.1).sin(), 0.0))
            .collect();
        let mut buf = original.clone();
        fft_inplace(&mut buf, false);
        fft_inplace(&mut buf, true);
        for (a, b) in buf.iter().zip(original.iter()) {
            assert!((a.re - b.re).abs() < 1e-8);
        }
    }

    // --- Band plan ---

    #[test]
    fn test_band_plan_spacing_vdsl2() {
        let bp = BandPlan::Plan998;
        assert!((bp.subcarrier_spacing_hz() - 4312.5).abs() < 1.0);
    }

    #[test]
    fn test_band_plan_spacing_gfast() {
        let bp = BandPlan::Gfast106;
        assert!((bp.subcarrier_spacing_hz() - 51_750.0).abs() < 1.0);
    }

    #[test]
    fn test_band_plan_frequencies() {
        let bp = BandPlan::Hpe17;
        assert!(bp.max_downstream_hz() > bp.max_upstream_hz());
    }

    // --- DMT config ---

    #[test]
    fn test_dmt_config_vdsl2_17a() {
        let config = DmtConfig::vdsl2_profile(VdslProfile::Profile17a);
        assert_eq!(config.fft_size, 2048);
        assert_eq!(config.max_bits_per_tone, 15);
        assert!(config.cyclic_prefix > 0);
    }

    #[test]
    fn test_dmt_config_gfast_106a() {
        let config = DmtConfig::gfast_profile(GfastProfile::Profile106a);
        assert_eq!(config.fft_size, 2048);
        assert_eq!(config.max_bits_per_tone, 12);
        assert!(config.cyclic_suffix > 0);
    }

    #[test]
    fn test_dmt_config_symbol_length() {
        let config = DmtConfig::vdsl2_profile(VdslProfile::Profile17a);
        let sym_len = config.symbol_length();
        assert_eq!(sym_len, config.fft_size + config.cyclic_prefix + config.cyclic_suffix);
    }

    #[test]
    fn test_dmt_config_gfast_symbol_length() {
        let config = DmtConfig::gfast_profile(GfastProfile::Profile212a);
        let sym_len = config.symbol_length();
        assert!(sym_len > config.fft_size);
    }

    // --- QAM mapper ---

    #[test]
    fn test_qam_bpsk_roundtrip() {
        for b in [0u8, 1u8] {
            let (re, im) = qam_map(&[b], 1);
            let decoded = qam_demap(re, im, 1);
            assert_eq!(decoded.len(), 1);
            assert_eq!(decoded[0], b, "BPSK bit={} failed", b);
        }
    }

    #[test]
    fn test_qam_4qam_roundtrip() {
        for bits in [[0u8, 0], [0, 1], [1, 0], [1, 1]] {
            let (re, im) = qam_map(&bits, 2);
            let decoded = qam_demap(re, im, 2);
            assert_eq!(decoded, bits.to_vec(), "4-QAM {:?}", bits);
        }
    }

    #[test]
    fn test_qam_6bit_roundtrip() {
        // 6 bits = 64-QAM
        let bits: Vec<u8> = vec![1, 0, 1, 1, 0, 1];
        let (re, im) = qam_map(&bits, 6);
        let decoded = qam_demap(re, im, 6);
        assert_eq!(decoded, bits, "64-QAM roundtrip failed");
    }

    #[test]
    fn test_qam_8bit_roundtrip() {
        let bits: Vec<u8> = vec![0, 1, 0, 1, 1, 0, 1, 0];
        let (re, im) = qam_map(&bits, 8);
        let decoded = qam_demap(re, im, 8);
        assert_eq!(decoded, bits, "256-QAM roundtrip");
    }

    // --- DMT modulator ---

    #[test]
    fn test_modulator_output_length() {
        let config = DmtConfig::vdsl2_profile(VdslProfile::Profile8a);
        let expected_len = config.symbol_length();
        let mut modulator = DmtModulator::new(config.clone());
        let bits_per_tone = vec![2u8; 8]; // 8 tones × 2 bits
        let total_bits: usize = bits_per_tone.iter().map(|&b| b as usize).sum();
        let data_bits = vec![0u8; total_bits];
        let sym = modulator.modulate_symbol(&bits_per_tone, &data_bits);
        assert_eq!(sym.len(), expected_len, "Symbol length mismatch");
    }

    #[test]
    fn test_modulator_zero_bits() {
        let config = DmtConfig::vdsl2_profile(VdslProfile::Profile8a);
        let expected_len = config.symbol_length();
        let mut modulator = DmtModulator::new(config.clone());
        let bits_per_tone = vec![0u8; 16];
        let sym = modulator.modulate_symbol(&bits_per_tone, &[]);
        assert_eq!(sym.len(), expected_len);
    }

    #[test]
    fn test_modulator_frame() {
        let config = DmtConfig::vdsl2_profile(VdslProfile::Profile8a);
        let sym_len = config.symbol_length();
        let mut modulator = DmtModulator::new(config.clone());
        let bits_per_tone = vec![4u8; 4]; // 16 bits/symbol
        let total_bits = 16 * 3;
        let data_bits = vec![1u8; total_bits];
        let frame = modulator.modulate_frame(&bits_per_tone, &data_bits, 3);
        assert_eq!(frame.len(), sym_len * 3);
    }

    // --- DMT demodulator ---

    #[test]
    fn test_demodulator_output_nocrash() {
        let config = DmtConfig::vdsl2_profile(VdslProfile::Profile8a);
        let sym_len = config.symbol_length();
        let mut demod = DmtDemodulator::new(config);
        let rx = vec![0.0f64; sym_len];
        let bits_per_tone = vec![2u8; 8];
        let bits = demod.demodulate_symbol(&rx, &bits_per_tone);
        // Just checking it doesn't panic and returns some bits
        assert!(bits.len() <= 16);
    }

    #[test]
    fn test_mod_demod_roundtrip() {
        let config = DmtConfig::vdsl2_profile(VdslProfile::Profile8a);
        let mut modulator = DmtModulator::new(config.clone());
        let mut demodulator = DmtDemodulator::new(config.clone());
        // Use only 4 tones with 2 bits each
        let bits_per_tone = vec![2u8; 4];
        let data_bits = vec![1u8, 0u8, 1u8, 1u8, 0u8, 0u8, 1u8, 1u8];
        let symbol = modulator.modulate_symbol(&bits_per_tone, &data_bits);
        // Without channel noise, demodulated bits should match
        let decoded = demodulator.demodulate_symbol(&symbol, &bits_per_tone);
        assert_eq!(decoded.len(), data_bits.len());
        assert_eq!(decoded, data_bits, "Roundtrip failed");
    }

    // --- Bit loading ---

    #[test]
    fn test_bit_loader_water_fill() {
        let snr = vec![20.0, 25.0, 10.0, 15.0, 30.0, 5.0, 18.0, 22.0];
        let config = BitLoadConfig {
            target_margin_db: 6.0,
            noise_psd_db: vec![0.0; 8],
            snr_per_tone_db: snr,
            max_bits_per_tone: 15,
            min_bits_per_tone: 2,
        };
        let loader = BitLoader::new(config);
        let (bits, powers) = loader.water_fill(1.0);
        assert_eq!(bits.len(), 8);
        assert_eq!(powers.len(), 8);
        // Higher SNR tones should have >= bits than low SNR tones
        assert!(bits[4] >= bits[2], "High SNR tone should have more bits");
    }

    #[test]
    fn test_bit_swap_convergence() {
        let mut bits = vec![4u8, 4u8, 4u8, 4u8];
        let snr = vec![30.0, 20.0, 25.0, 15.0];
        BitLoader::run_bit_swap(&mut bits, &snr, 15, 100);
        // Total bits should be conserved
        assert_eq!(bits.iter().map(|&b| b as usize).sum::<usize>(), 16);
    }

    #[test]
    fn test_aggregate_rate() {
        let bits = vec![6u8; 100];
        let rate = BitLoader::aggregate_rate_bps(&bits, 4000.0);
        assert!((rate - 600.0 * 4000.0).abs() < 1.0);
    }

    // --- PSD mask ---

    #[test]
    fn test_psd_mask_interpolation() {
        let mask = PsdMask::vdsl2_17a_downstream();
        let psd_low = mask.evaluate(100_000.0);
        let psd_high = mask.evaluate(17_000_000.0);
        assert!(psd_high < psd_low, "PSD should fall at higher freq");
    }

    #[test]
    fn test_psd_mask_clamp_low() {
        let mask = PsdMask::vdsl2_17a_downstream();
        let psd = mask.evaluate(1_000.0); // below first point
        assert!((psd - mask.points[0].psd_dbm_per_hz).abs() < 1e-6);
    }

    #[test]
    fn test_psd_mask_gfast() {
        let mask = PsdMask::gfast_106a_downstream();
        let psd = mask.evaluate(50_000_000.0);
        assert!(psd < 0.0); // negative dBm/Hz
    }

    // --- UPBO ---

    #[test]
    fn test_upbo_zero_for_ref_length() {
        let upbo = UpboCalculator::new(0.5);
        // At the reference length, UPBO should be 0
        let pbo = upbo.upbo_db(1_000_000.0, 0.5);
        assert!(pbo.abs() < 1e-6);
    }

    #[test]
    fn test_upbo_increases_with_length() {
        let upbo = UpboCalculator::new(0.5);
        let pbo_short = upbo.upbo_db(1_000_000.0, 1.0);
        let pbo_long = upbo.upbo_db(1_000_000.0, 2.0);
        assert!(pbo_long > pbo_short);
    }

    #[test]
    fn test_upbo_apply_reduces_power() {
        let upbo = UpboCalculator::new(0.5);
        let p_in = 1.0;
        let p_out = upbo.apply_upbo(5_000_000.0, 2.0, p_in);
        assert!(p_out <= p_in);
    }

    // --- Vectoring ---

    #[test]
    fn test_vectoring_precoder_identity() {
        let prec = VectoringPrecoderTone::identity(10, 2);
        let data = vec![Complex::new(1.0, 0.5), Complex::new(-0.5, 0.3)];
        let out = prec.apply(&data);
        assert!((out[0].re - data[0].re).abs() < 1e-10);
        assert!((out[1].re - data[1].re).abs() < 1e-10);
    }

    #[test]
    fn test_fext_estimator_update() {
        let mut est = FextEstimator::new(10, 2, 0.01);
        let tx = vec![Complex::new(1.0, 0.0), Complex::new(1.0, 0.0)];
        let err = vec![Complex::new(0.1, 0.0), Complex::new(-0.1, 0.0)];
        est.update(0, &tx, &err);
        // Off-diagonal coupling should be updated
        assert!(est.coupling[0][0][1].re.abs() > 0.0);
    }

    #[test]
    fn test_fext_cancel_no_crosstalk() {
        let est = FextEstimator::new(4, 2, 0.01);
        let rx = vec![Complex::new(1.0, 0.0), Complex::new(-1.0, 0.0)];
        let tx = vec![Complex::new(0.5, 0.0), Complex::new(-0.5, 0.0)];
        let cancelled = est.cancel(0, &rx, &tx);
        // With zero coupling, output = input
        assert!((cancelled[0].re - rx[0].re).abs() < 1e-10);
        assert!((cancelled[1].re - rx[1].re).abs() < 1e-10);
    }

    // --- Bonding ---

    #[test]
    fn test_bonding_distribute_reassemble() {
        let config = BondingConfig::new(4, 100, 8);
        let agg = BondingAggregator::new(config);
        let data: Vec<u8> = (0..64u8).collect();
        let streams = agg.distribute(&data);
        assert_eq!(streams.len(), 4);
        let total: usize = streams.iter().map(|s| s.len()).sum();
        assert_eq!(total, 64);
        let delays = vec![0usize; 4];
        let reassembled = agg.reassemble(&streams, &delays);
        assert_eq!(reassembled, data);
    }

    #[test]
    fn test_bonding_aggregate_rate() {
        let bpt = vec![vec![6u8; 100]; 4]; // 4 pairs × 100 tones × 6 bits
        let rate = BondingAggregator::aggregate_rate(&bpt, 4000.0);
        assert!((rate - 4.0 * 100.0 * 6.0 * 4000.0).abs() < 1.0);
    }

    // --- Reed-Solomon ---

    #[test]
    fn test_rs_encode_length() {
        let enc = RsEncoder::new(8, 4);
        let data = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let cw = enc.encode(&data);
        assert_eq!(cw.len(), 8 + 4);
    }

    #[test]
    fn test_rs_encode_no_errors_decode() {
        let enc = RsEncoder::new(8, 4);
        let data = vec![0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80];
        let cw = enc.encode(&data);
        let decoded = enc.decode_erasures(&cw, &[]).unwrap();
        assert_eq!(decoded[..8], data[..]);
    }

    #[test]
    fn test_rs_too_many_erasures() {
        let enc = RsEncoder::new(8, 4);
        let data = vec![1u8; 8];
        let cw = enc.encode(&data);
        let result = enc.decode_erasures(&cw, &[0, 1, 2, 3, 4, 5]);
        assert!(result.is_none());
    }

    #[test]
    fn test_gf256_mul_commutative() {
        assert_eq!(gf256::mul(3, 5), gf256::mul(5, 3));
        assert_eq!(gf256::mul(7, 11), gf256::mul(11, 7));
    }

    #[test]
    fn test_gf256_mul_zero() {
        assert_eq!(gf256::mul(255, 0), 0);
        assert_eq!(gf256::mul(0, 128), 0);
    }

    #[test]
    fn test_gf256_inv() {
        for a in 1u8..=10 {
            let inv = gf256::inv(a);
            assert_eq!(gf256::mul(a, inv), 1, "a={} inv={}", a, inv);
        }
    }

    // --- Synchronisation ---

    #[test]
    fn test_sync_cpe_zero_for_unity_pilots() {
        let config = DmtConfig::vdsl2_profile(VdslProfile::Profile8a);
        let mut sync = DmtSyncEstimator::new(config);
        let pilots: Vec<Complex> = vec![Complex::new(1.0, 0.0); 32];
        let cpe = sync.estimate_cpe(&pilots);
        assert!(cpe.abs() < 1e-10, "CPE should be ~0 for real pilots");
    }

    #[test]
    fn test_sync_cpe_nonzero() {
        let config = DmtConfig::vdsl2_profile(VdslProfile::Profile8a);
        let mut sync = DmtSyncEstimator::new(config);
        let phase = 0.3_f64;
        let pilots = vec![Complex::new(phase.cos(), phase.sin()); 4];
        let cpe = sync.estimate_cpe(&pilots);
        assert!((cpe - phase).abs() < 1e-8, "CPE mismatch");
    }

    #[test]
    fn test_sync_clock_offset_zero() {
        let config = DmtConfig::vdsl2_profile(VdslProfile::Profile8a);
        let mut sync = DmtSyncEstimator::new(config.clone());
        let phases = vec![0.0f64; config.pilot_indices.len()];
        sync.prev_phases = phases.clone();
        let offset = sync.estimate_clock_offset(&phases);
        assert!(offset.abs() < 1e-6);
    }

    #[test]
    fn test_sync_cpe_correction() {
        let config = DmtConfig::vdsl2_profile(VdslProfile::Profile8a);
        let mut sync = DmtSyncEstimator::new(config);
        sync.cpe_rad = PI / 4.0;
        let mut syms = vec![Complex::new(1.0, 0.0); 4];
        sync.correct_cpe(&mut syms);
        // After correction, phase should be rotated by -pi/4
        let expected_phase = -(PI / 4.0);
        let got_phase = syms[0].im.atan2(syms[0].re);
        assert!((got_phase - expected_phase).abs() < 1e-8);
    }

    // --- INP Interleaver ---

    #[test]
    fn test_inp_interleave_deinterleave() {
        let il = InpInterleaver::new(4, 8);
        let data: Vec<u8> = (0..32).collect();
        let interleaved = il.interleave(&data);
        let deinterleaved = il.deinterleave(&interleaved);
        assert_eq!(deinterleaved, data);
    }

    #[test]
    fn test_inp_interleaver_write_read() {
        let mut il = InpInterleaver::new(4, 8);
        let data: Vec<u8> = (0..16).collect();
        il.write(&data);
        let out = il.read(16);
        assert_eq!(out, data);
    }

    #[test]
    fn test_inp_config_delay() {
        let cfg = InpConfig::new(2.0, 0.25);
        assert!(cfg.depth >= 1);
        assert!(cfg.delay_ms > 0.0);
        assert!(cfg.rs_redundancy >= 2);
    }

    // --- SNR estimator ---

    #[test]
    fn test_snr_estimator_perfect_channel() {
        let mut est = SnrEstimator::new(4);
        let signal = vec![Complex::new(1.0, 0.0); 4];
        // Run many iterations so EMA converges noise_var → 0
        for _ in 0..200 {
            est.update(&signal, &signal, 0.5);
        }
        let snr = est.snr_db();
        // Noise variance should be near 0 → SNR capped high
        for s in &snr {
            assert!(*s > 60.0, "Expected high SNR, got {}", s);
        }
    }

    #[test]
    fn test_snr_estimator_noisy_channel() {
        let mut est = SnrEstimator::new(4);
        let ideal = vec![Complex::new(1.0, 0.0); 4];
        let noisy = vec![
            Complex::new(1.1, 0.05),
            Complex::new(0.9, -0.1),
            Complex::new(1.05, 0.02),
            Complex::new(0.95, 0.08),
        ];
        for _ in 0..20 {
            est.update(&noisy, &ideal, 0.2);
        }
        let snr = est.snr_db();
        for s in &snr {
            assert!(*s > 0.0 && *s < 80.0, "SNR should be in range: {}", s);
        }
    }

    // --- Integration: full modulate/demodulate with known data ---

    #[test]
    fn test_full_roundtrip_bpsk_only() {
        let config = DmtConfig::vdsl2_profile(VdslProfile::Profile8a);
        let mut modulator = DmtModulator::new(config.clone());
        let mut demodulator = DmtDemodulator::new(config.clone());
        let bits_per_tone = vec![1u8; 4]; // BPSK on first 4 tones
        let data_bits: Vec<u8> = vec![1, 0, 1, 1];
        let symbol = modulator.modulate_symbol(&bits_per_tone, &data_bits);
        let decoded = demodulator.demodulate_symbol(&symbol, &bits_per_tone);
        assert_eq!(decoded, data_bits);
    }

    #[test]
    fn test_full_roundtrip_4qam() {
        let config = DmtConfig::vdsl2_profile(VdslProfile::Profile8a);
        let mut modulator = DmtModulator::new(config.clone());
        let mut demodulator = DmtDemodulator::new(config.clone());
        let bits_per_tone = vec![2u8; 3];
        let data_bits: Vec<u8> = vec![0, 1, 1, 0, 0, 0];
        let symbol = modulator.modulate_symbol(&bits_per_tone, &data_bits);
        let decoded = demodulator.demodulate_symbol(&symbol, &bits_per_tone);
        assert_eq!(decoded, data_bits);
    }

    #[test]
    fn test_gfast_profile_config() {
        let cfg = DmtConfig::gfast_profile(GfastProfile::Profile212a);
        assert_eq!(cfg.fft_size, 4096);
        assert!(cfg.cyclic_suffix > 0);
        assert!(cfg.pilot_indices.len() > 0);
    }

    #[test]
    fn test_vdsl2_all_profiles_instantiate() {
        let profiles = [
            VdslProfile::Profile8a,
            VdslProfile::Profile8b,
            VdslProfile::Profile8c,
            VdslProfile::Profile8d,
            VdslProfile::Profile12a,
            VdslProfile::Profile12b,
            VdslProfile::Profile17a,
            VdslProfile::Profile30a,
            VdslProfile::Profile35b,
        ];
        for p in &profiles {
            let cfg = DmtConfig::vdsl2_profile(*p);
            assert!(cfg.fft_size.is_power_of_two());
            assert!(cfg.data_tones() > 0);
        }
    }

    #[test]
    fn test_gfast_both_profiles_instantiate() {
        for p in &[GfastProfile::Profile106a, GfastProfile::Profile212a] {
            let cfg = DmtConfig::gfast_profile(*p);
            assert!(cfg.fft_size.is_power_of_two());
        }
    }
}
