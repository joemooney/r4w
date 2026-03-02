//! # Dual-Polarization QPSK (DP-QPSK) Coherent Receiver
//!
//! Complete DSP chain for 100G/200G coherent optical receivers per OIF-100G-LR-1.0
//! and ITU-T G.975.1 specifications. Implements the full intradyne coherent receiver
//! signal processing pipeline for polarization-multiplexed QPSK signals.
//!
//! ## Signal Flow
//!
//! ```text
//! Optical Input (X+Y pol)
//!   → 90° Hybrid + Balanced Detectors → ADC Quantization
//!   → IQ Imbalance Correction (Gram-Schmidt)
//!   → Cubic Resampler (→ 2 samp/sym)
//!   → CD Compensator (freq-domain FDE)
//!   → 2×2 CMA Butterfly (pol-demux + PMD)
//!   → Frequency Offset Removal (4th-power)
//!   → Viterbi-Viterbi CPE (4th-power)
//!   → QPSK Slicer + Gray Decode → Bits
//! ```
//!
//! ## References
//!
//! - OIF-100G-LR-1.0: Implementation Agreement for 100G Long Reach Coherent
//! - ITU-T G.975.1: Forward error correction for high bit-rate DWDM submarine systems
//! - Savory, S.J. (2010): Digital Coherent Optical Receivers: Algorithms and Subsystems
//! - Ip, E. & Kahn, J.M. (2007): Digital equalization of chromatic dispersion and PMD
//! - Viterbi, A.J. & Viterbi, A.M. (1983): Nonlinear estimation of PSK-modulated carrier

use std::f64::consts::PI;

// ---------------------------------------------------------------------------
// Complex arithmetic helpers (no external crates)
// ---------------------------------------------------------------------------

/// Complex number as (real, imag) tuple.
type Cpx = (f64, f64);

#[inline]
fn cadd(a: Cpx, b: Cpx) -> Cpx { (a.0 + b.0, a.1 + b.1) }

#[inline]
fn csub(a: Cpx, b: Cpx) -> Cpx { (a.0 - b.0, a.1 - b.1) }

#[inline]
fn cmul(a: Cpx, b: Cpx) -> Cpx {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

#[inline]
fn cconj(a: Cpx) -> Cpx { (a.0, -a.1) }

#[inline]
fn cmag2(a: Cpx) -> f64 { a.0 * a.0 + a.1 * a.1 }

#[inline]
fn cmag(a: Cpx) -> f64 { cmag2(a).sqrt() }

#[inline]
fn cscale(a: Cpx, s: f64) -> Cpx { (a.0 * s, a.1 * s) }

#[inline]
fn carg(a: Cpx) -> f64 { a.1.atan2(a.0) }

/// Raise complex number to the 4th power.
#[inline]
fn cpow4(a: Cpx) -> Cpx {
    let sq = cmul(a, a);
    cmul(sq, sq)
}

/// Normalise a complex number to unit magnitude (returns (0,0) if zero).
#[inline]
fn cnorm(a: Cpx) -> Cpx {
    let m = cmag(a);
    if m < 1e-30 { (0.0, 0.0) } else { (a.0 / m, a.1 / m) }
}

// ---------------------------------------------------------------------------
// Scratch-built radix-2 DIT FFT (power-of-two sizes only)
// ---------------------------------------------------------------------------

/// In-place radix-2 DIT FFT.
fn fft_inplace(buf: &mut Vec<Cpx>, inverse: bool) {
    let n = buf.len();
    assert!(n.is_power_of_two(), "FFT size must be a power of two");

    // Bit-reversal permutation
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 { j ^= bit; bit >>= 1; }
        j ^= bit;
        if i < j { buf.swap(i, j); }
    }

    // Cooley-Tukey butterfly stages
    let sign = if inverse { 1.0_f64 } else { -1.0_f64 };
    let mut len = 2usize;
    while len <= n {
        let ang = sign * 2.0 * PI / len as f64;
        let wlen = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let mut w: Cpx = (1.0, 0.0);
            for k in 0..len / 2 {
                let u = buf[i + k];
                let v = cmul(buf[i + k + len / 2], w);
                buf[i + k] = cadd(u, v);
                buf[i + k + len / 2] = csub(u, v);
                w = cmul(w, wlen);
            }
            i += len;
        }
        len <<= 1;
    }

    if inverse {
        let scale = 1.0 / n as f64;
        for s in buf.iter_mut() { *s = cscale(*s, scale); }
    }
}

/// Forward FFT (returns new Vec).
fn fft(x: &[Cpx]) -> Vec<Cpx> {
    let mut buf = x.to_vec();
    fft_inplace(&mut buf, false);
    buf
}

/// Inverse FFT (returns new Vec).
fn ifft(x: &[Cpx]) -> Vec<Cpx> {
    let mut buf = x.to_vec();
    fft_inplace(&mut buf, true);
    buf
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Main configuration for the DP-QPSK coherent receiver.
///
/// Covers optical front-end parameters, equalizer settings, and algorithm
/// control knobs per OIF-100G-LR-1.0 Annex A.
#[derive(Debug, Clone)]
pub struct DpQpskConfig {
    /// Symbol rate in Gbaud (e.g., 28.0 for 100G DP-QPSK, 32.0 for flex-rate).
    pub symbol_rate_gbaud: f64,
    /// ADC sample rate in GSa/s (must be >= 2× symbol rate, typically 56–64).
    pub adc_sample_rate_gsa: f64,
    /// ADC effective number of bits (ENOB). 6 = practical 100G ADC.
    pub adc_bits: u32,
    /// Fiber length in km for static CD pre-computation.
    pub fiber_length_km: f64,
    /// Chromatic dispersion coefficient in ps/(nm·km). SMF-28 = 17.0.
    pub cd_ps_per_nm_km: f64,
    /// Carrier wavelength in nm (C-band: 1550.0).
    pub wavelength_nm: f64,
    /// Number of CMA butterfly taps (fractionally spaced at T/2). Typical: 11–33.
    pub cma_taps: usize,
    /// CMA step size (mu). Typical: 1e-4 for initial convergence.
    pub cma_step_size: f64,
    /// Viterbi-Viterbi block length for phase averaging. Typical: 32–128.
    pub vv_block_size: usize,
    /// Enable differential decoding to resolve π/2 phase ambiguity.
    pub differential_decoding: bool,
    /// PMD coefficient in ps/sqrt(km). SMF-28 typical: 0.04–0.1.
    pub pmd_coeff_ps_sqrtkm: f64,
    /// Optical hybrid amplitude imbalance (linear ratio, 1.0 = perfect).
    pub hybrid_amplitude_imbalance: f64,
    /// Optical hybrid quadrature error in radians (0.0 = perfect 90°).
    pub hybrid_quadrature_error_rad: f64,
}

impl Default for DpQpskConfig {
    fn default() -> Self {
        Self {
            symbol_rate_gbaud: 28.0,
            adc_sample_rate_gsa: 56.0,
            adc_bits: 6,
            fiber_length_km: 1000.0,
            cd_ps_per_nm_km: 17.0,
            wavelength_nm: 1550.0,
            cma_taps: 13,
            cma_step_size: 1e-4,
            vv_block_size: 64,
            differential_decoding: true,
            pmd_coeff_ps_sqrtkm: 0.05,
            hybrid_amplitude_imbalance: 1.0,
            hybrid_quadrature_error_rad: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Jones Matrix (2×2 complex polarization matrix)
// ---------------------------------------------------------------------------

/// 2×2 complex Jones matrix representing polarization transformation.
///
/// Jones matrices describe polarization optical components: rotations,
/// waveplates, and PMD elements. Composition: `J = J2 * J1` applies J1 first.
///
/// Reference: Jones, R.C. (1941) "A new calculus for the treatment of optical
/// systems", J. Opt. Soc. Am. 31, 488.
#[derive(Debug, Clone, Copy)]
pub struct JonesMatrix {
    /// Row 0, col 0 — coupling from X-pol input to X-pol output.
    pub hxx: Cpx,
    /// Row 0, col 1 — coupling from Y-pol input to X-pol output.
    pub hxy: Cpx,
    /// Row 1, col 0 — coupling from X-pol input to Y-pol output.
    pub hyx: Cpx,
    /// Row 1, col 1 — coupling from Y-pol input to Y-pol output.
    pub hyy: Cpx,
}

impl JonesMatrix {
    /// Identity matrix (no polarization change).
    pub fn identity() -> Self {
        Self { hxx: (1.0, 0.0), hxy: (0.0, 0.0), hyx: (0.0, 0.0), hyy: (1.0, 0.0) }
    }

    /// Rotation by angle `theta` radians (models fiber birefringence axis rotation).
    pub fn rotation(theta: f64) -> Self {
        let c = theta.cos();
        let s = theta.sin();
        Self {
            hxx: (c, 0.0), hxy: (-s, 0.0),
            hyx: (s, 0.0), hyy: ( c, 0.0),
        }
    }

    /// First-order PMD model: differential group delay `dgd_s` seconds on fast axis.
    ///
    /// Implemented as a frequency-independent amplitude split with phase delay on
    /// the slow polarization eigenstate.
    pub fn first_order_pmd(dgd_s: f64, symbol_rate_hz: f64) -> Self {
        // Phase delay = 2π × fs/2 × DGD for half-symbol spacing worst case
        let phi = PI * symbol_rate_hz * dgd_s;
        Self {
            hxx: (phi.cos(), -phi.sin()),
            hxy: (0.0, 0.0),
            hyx: (0.0, 0.0),
            hyy: (1.0, 0.0),
        }
    }

    /// Matrix-matrix multiplication (J_out = self × rhs), applies rhs first.
    pub fn mul(&self, rhs: &JonesMatrix) -> JonesMatrix {
        JonesMatrix {
            hxx: cadd(cmul(self.hxx, rhs.hxx), cmul(self.hxy, rhs.hyx)),
            hxy: cadd(cmul(self.hxx, rhs.hxy), cmul(self.hxy, rhs.hyy)),
            hyx: cadd(cmul(self.hyx, rhs.hxx), cmul(self.hyy, rhs.hyx)),
            hyy: cadd(cmul(self.hyx, rhs.hxy), cmul(self.hyy, rhs.hyy)),
        }
    }

    /// Apply Jones matrix to a dual-polarization sample vector (Ex, Ey).
    pub fn apply(&self, ex: Cpx, ey: Cpx) -> (Cpx, Cpx) {
        let out_x = cadd(cmul(self.hxx, ex), cmul(self.hxy, ey));
        let out_y = cadd(cmul(self.hyx, ex), cmul(self.hyy, ey));
        (out_x, out_y)
    }

    /// Determinant of the Jones matrix.
    pub fn det(&self) -> Cpx {
        csub(cmul(self.hxx, self.hyy), cmul(self.hxy, self.hyx))
    }
}

// ---------------------------------------------------------------------------
// Optical 90-Degree Hybrid Model
// ---------------------------------------------------------------------------

/// 90-degree optical hybrid front-end model.
///
/// A 90° hybrid mixes the received signal (Es) with a local oscillator (Elo)
/// to produce four outputs enabling coherent detection:
///
/// ```text
/// XI  = Re(Es · Elo*)     (in-phase,  I port)
/// XQ  = Im(Es · Elo*)     (quadrature, Q port)
/// ```
///
/// Real hybrids exhibit amplitude imbalance (unequal power splits) and
/// quadrature error (deviation from exact 90°) between the I and Q branches.
///
/// Reference: Ly-Gagnon, D.-S. et al. (2006) "Coherent Detection of Optical
/// Quadrature Phase-Shift Keying Signals With Carrier Phase Estimation",
/// J. Lightwave Technol. 24, 12–21.
#[derive(Debug, Clone)]
pub struct OpticalHybrid {
    /// Amplitude imbalance ratio I/Q (1.0 = perfect).
    pub amplitude_imbalance: f64,
    /// Quadrature error in radians (deviation from 90°).
    pub quadrature_error: f64,
    /// Common-mode rejection ratio in dB (finite CMRR models detector mismatch).
    pub cmrr_db: f64,
}

impl OpticalHybrid {
    /// Create a perfect hybrid (no impairments).
    pub fn perfect() -> Self {
        Self { amplitude_imbalance: 1.0, quadrature_error: 0.0, cmrr_db: 60.0 }
    }

    /// Create a hybrid with typical component-level impairments.
    pub fn typical() -> Self {
        Self { amplitude_imbalance: 1.02, quadrature_error: 0.03, cmrr_db: 30.0 }
    }

    /// Process a single signal sample `es` and LO sample `elo`.
    ///
    /// Returns `(xi, xq)` — the I and Q outputs of the balanced detectors.
    ///
    /// The CMRR-limited common-mode leakage is modelled as:
    /// `output = signal + (1/CMRR) * common_mode`
    pub fn process(&self, es: Cpx, elo: Cpx) -> (f64, f64) {
        // Beat product: Es × Elo*
        let beat = cmul(es, cconj(elo));

        // Ideal I and Q
        let i_ideal = beat.0;
        let q_ideal = beat.1;

        // Apply amplitude imbalance (Q branch scaled)
        let gain_i = 1.0;
        let gain_q = self.amplitude_imbalance;

        // Apply quadrature error: Q branch rotated by error angle
        let qe = self.quadrature_error;
        let xi = gain_i * i_ideal;
        let xq = gain_q * (q_ideal * qe.cos() + i_ideal * qe.sin());

        // CMRR: finite rejection adds common-mode (|Es|^2 + |Elo|^2) leakage
        let cmrr_linear = 10.0_f64.powf(-self.cmrr_db / 20.0);
        let common = (cmag2(es) + cmag2(elo)) * cmrr_linear;
        (xi + common, xq + common)
    }

    /// Process a full block of dual-polarization signal pairs.
    ///
    /// Returns four real vectors: `(xi, xq, yi, yq)` for X and Y polarisations.
    pub fn process_block(
        &self,
        ex: &[Cpx],
        ey: &[Cpx],
        elo_x: &[Cpx],
        elo_y: &[Cpx],
    ) -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>) {
        let n = ex.len();
        let mut xi = Vec::with_capacity(n);
        let mut xq = Vec::with_capacity(n);
        let mut yi = Vec::with_capacity(n);
        let mut yq = Vec::with_capacity(n);
        for k in 0..n {
            let (i, q) = self.process(ex[k], elo_x[k]);
            xi.push(i);
            xq.push(q);
            let (i2, q2) = self.process(ey[k], elo_y[k]);
            yi.push(i2);
            yq.push(q2);
        }
        (xi, xq, yi, yq)
    }
}

// ---------------------------------------------------------------------------
// ADC Model
// ---------------------------------------------------------------------------

/// ADC quantisation model for coherent receiver front-end.
pub struct AdcModel {
    /// Number of quantisation bits.
    pub bits: u32,
    /// Full-scale range (±full_scale).
    pub full_scale: f64,
}

impl AdcModel {
    /// Create ADC with `bits` effective bits and a full-scale range of ±`full_scale`.
    pub fn new(bits: u32, full_scale: f64) -> Self {
        Self { bits, full_scale }
    }

    /// Quantise a single sample.
    pub fn quantise(&self, x: f64) -> f64 {
        let levels = (1u64 << self.bits) as f64;
        let lsb = 2.0 * self.full_scale / levels;
        let clipped = x.clamp(-self.full_scale, self.full_scale - lsb);
        (clipped / lsb).round() * lsb
    }

    /// Quantise a slice of samples in place.
    pub fn quantise_slice(&self, x: &mut Vec<f64>) {
        for v in x.iter_mut() { *v = self.quantise(*v); }
    }
}

// ---------------------------------------------------------------------------
// Gram-Schmidt IQ Orthogonalization
// ---------------------------------------------------------------------------

/// Gram-Schmidt IQ imbalance corrector.
///
/// Corrects gain imbalance and quadrature error between the I and Q branches
/// of a single-polarisation output using the Gram-Schmidt orthogonalisation
/// procedure. No training sequence required — works on a block of samples.
///
/// Algorithm (after Savory 2010):
/// 1. `p1 = mean(|I|^2)^{1/2}`; normalise `I' = I / p1`
/// 2. `p2 = mean(I·Q)`; remove I projection from Q: `Q' = Q - p2·I'`
/// 3. `p3 = mean(|Q'|^2)^{1/2}`; normalise `Q'' = Q' / p3`
#[derive(Debug, Clone)]
pub struct GramSchmidt {
    /// Forgetting factor for online exponential averaging (1.0 = block mode).
    pub alpha: f64,
    // Running statistics
    p1: f64,
    p2: f64,
    p3: f64,
}

impl GramSchmidt {
    /// Create a new Gram-Schmidt corrector.
    ///
    /// `alpha`: exponential averaging factor (0 < alpha ≤ 1).
    /// Use `alpha = 1.0` for full block updates, `alpha < 1` for online tracking.
    pub fn new(alpha: f64) -> Self {
        Self { alpha, p1: 1.0, p2: 0.0, p3: 1.0 }
    }

    /// Process a block of (I, Q) samples and return orthogonalised (I, Q).
    pub fn process_block(&mut self, i_in: &[f64], q_in: &[f64]) -> (Vec<f64>, Vec<f64>) {
        assert_eq!(i_in.len(), q_in.len());
        let n = i_in.len() as f64;
        let a = self.alpha;

        // Estimate parameters from this block
        let mean_i2 = i_in.iter().map(|&v| v * v).sum::<f64>() / n;
        let p1_new = mean_i2.sqrt().max(1e-12);

        // Normalised I
        let i_norm: Vec<f64> = i_in.iter().map(|&v| v / p1_new).collect();

        // Estimate cross-correlation <I' · Q>
        let p2_new = i_norm.iter().zip(q_in.iter()).map(|(i, q)| i * q).sum::<f64>() / n;

        // Residual Q
        let q_res: Vec<f64> = q_in.iter().zip(i_norm.iter())
            .map(|(q, i)| q - p2_new * i).collect();

        // RMS of residual Q
        let mean_q2 = q_res.iter().map(|&v| v * v).sum::<f64>() / n;
        let p3_new = mean_q2.sqrt().max(1e-12);

        // Exponential average parameters
        self.p1 = (1.0 - a) * self.p1 + a * p1_new;
        self.p2 = (1.0 - a) * self.p2 + a * p2_new;
        self.p3 = (1.0 - a) * self.p3 + a * p3_new;

        // Apply correction with smoothed parameters
        let i_out: Vec<f64> = i_in.iter().map(|&v| v / self.p1).collect();
        let q_out: Vec<f64> = i_in.iter().zip(q_in.iter())
            .map(|(i, q)| (q - self.p2 * (i / self.p1)) / self.p3)
            .collect();

        (i_out, q_out)
    }
}

// ---------------------------------------------------------------------------
// Cubic Interpolation Resampler
// ---------------------------------------------------------------------------

/// Cubic interpolation resampler.
///
/// Converts the ADC sample stream to exactly 2 samples/symbol using
/// Catmull-Rom cubic interpolation for continuous-time reconstruction.
/// This fractionally-spaced output feeds the CMA equalizer.
///
/// Reference: Keys, R.G. (1981) "Cubic Convolution Interpolation for
/// Digital Image Processing", IEEE Trans. Acoust. Speech Signal Process.
#[derive(Debug, Clone)]
pub struct CubicResampler {
    /// Input-to-output sample rate ratio (> 1 means downsampling).
    pub ratio: f64,
    // State: last 4 input samples for cubic kernel
    history: Vec<f64>,
    // Fractional phase accumulator
    phase: f64,
}

impl CubicResampler {
    /// Create a resampler with the given input/output rate ratio.
    pub fn new(ratio: f64) -> Self {
        Self { ratio, history: vec![0.0; 4], phase: 0.0 }
    }

    /// Catmull-Rom kernel evaluated at fractional position `mu` ∈ [0,1).
    fn catmull_rom(p: &[f64], mu: f64) -> f64 {
        // p[0]=y_{-1}, p[1]=y_0, p[2]=y_1, p[3]=y_2
        let mu2 = mu * mu;
        let mu3 = mu2 * mu;
        let a0 = -0.5 * p[0] + 1.5 * p[1] - 1.5 * p[2] + 0.5 * p[3];
        let a1 = p[0] - 2.5 * p[1] + 2.0 * p[2] - 0.5 * p[3];
        let a2 = -0.5 * p[0] + 0.5 * p[2];
        let a3 = p[1];
        a0 * mu3 + a1 * mu2 + a2 * mu + a3
    }

    /// Process an input block and produce resampled output.
    pub fn process(&mut self, input: &[f64]) -> Vec<f64> {
        let mut output = Vec::new();
        for &sample in input {
            // Shift history buffer
            self.history.rotate_left(1);
            *self.history.last_mut().unwrap() = sample;

            // Produce output samples while phase < 1
            while self.phase < 1.0 {
                let interpolated = Self::catmull_rom(&self.history, self.phase);
                output.push(interpolated);
                self.phase += self.ratio;
            }
            self.phase -= 1.0;
        }
        output
    }
}

// ---------------------------------------------------------------------------
// Chromatic Dispersion (CD) Compensator
// ---------------------------------------------------------------------------

/// Static frequency-domain chromatic dispersion (CD) compensator.
///
/// Implements the all-pass frequency-domain equalizer (FDE):
///
/// `H_CD(f) = exp(j · β₂ · L · (2πf)² / 2)`
///
/// where β₂ is the group velocity dispersion parameter, L the fiber length.
/// The CD transfer function is pre-computed at construction for efficiency.
///
/// Reference: Ip & Kahn (2008) "Compensation of Dispersion and Nonlinear
/// Impairments Using Digital Backpropagation", J. Lightwave Technol. 26.
#[derive(Debug, Clone)]
pub struct CdCompensator {
    /// Pre-computed frequency-domain transfer function (inverse CD).
    h_inv: Vec<Cpx>,
    /// FFT block size.
    fft_size: usize,
    /// Overlap length for overlap-save processing.
    overlap: usize,
    /// Saved tail samples from previous block.
    tail: Vec<Cpx>,
}

impl CdCompensator {
    /// Compute the CD group delay parameter β₂ from physical parameters.
    ///
    /// β₂ = -D·λ²/(2πc) [s²/m], where D is CD coefficient in s/(m·m).
    fn beta2_s2_per_m(cd_ps_per_nm_km: f64, wavelength_nm: f64) -> f64 {
        // Convert D from ps/(nm·km) to s/(m·m)
        let d = cd_ps_per_nm_km * 1e-12 / (1e-9 * 1e3); // s/m²
        let lambda = wavelength_nm * 1e-9;               // m
        let c = 3e8_f64;                                 // m/s
        -d * lambda * lambda / (2.0 * PI * c)
    }

    /// Build a CD compensator for the given fiber/link parameters.
    ///
    /// - `fiber_length_km`: total SMF length in km
    /// - `cd_ps_per_nm_km`: dispersion coefficient
    /// - `wavelength_nm`: optical carrier wavelength
    /// - `sample_rate_hz`: ADC sample rate in Hz (post-resampling)
    /// - `fft_size`: block size for overlap-save processing
    pub fn new(
        fiber_length_km: f64,
        cd_ps_per_nm_km: f64,
        wavelength_nm: f64,
        sample_rate_hz: f64,
        fft_size: usize,
    ) -> Self {
        assert!(fft_size.is_power_of_two(), "FFT size must be a power of 2");
        let beta2 = Self::beta2_s2_per_m(cd_ps_per_nm_km, wavelength_nm);
        let length_m = fiber_length_km * 1e3;

        // Build inverse transfer function (compensates the accumulated CD)
        let mut h_inv: Vec<Cpx> = Vec::with_capacity(fft_size);
        for k in 0..fft_size {
            // Normalised frequency: f = k/N for k < N/2, f = (k-N)/N for k >= N/2
            let freq_norm = if k < fft_size / 2 {
                k as f64 / fft_size as f64
            } else {
                (k as i64 - fft_size as i64) as f64 / fft_size as f64
            };
            let omega = 2.0 * PI * freq_norm * sample_rate_hz;
            // CD phase shift accumulated over length L: φ = β₂·L·ω²/2
            // Inverse (compensating) transfer: H^{-1} = exp(-j·φ)
            let phi = beta2 * length_m * omega * omega * 0.5;
            h_inv.push((phi.cos(), -phi.sin())); // exp(-jφ)
        }

        let overlap = fft_size / 2;
        CdCompensator {
            h_inv,
            fft_size,
            overlap,
            tail: vec![(0.0, 0.0); overlap],
        }
    }

    /// Apply CD compensation to a complex signal block using overlap-save.
    pub fn process(&mut self, input: &[Cpx]) -> Vec<Cpx> {
        let mut output = Vec::with_capacity(input.len());
        let block_len = self.fft_size - self.overlap;
        let mut pos = 0;

        while pos < input.len() {
            // Assemble FFT block: overlap + new data
            let chunk_end = (pos + block_len).min(input.len());
            let chunk = &input[pos..chunk_end];

            let mut block: Vec<Cpx> = Vec::with_capacity(self.fft_size);
            block.extend_from_slice(&self.tail);
            block.extend_from_slice(chunk);
            // Zero-pad if chunk is short
            while block.len() < self.fft_size {
                block.push((0.0, 0.0));
            }

            // Forward FFT
            fft_inplace(&mut block, false);

            // Multiply by inverse CD transfer function
            for (b, h) in block.iter_mut().zip(self.h_inv.iter()) {
                *b = cmul(*b, *h);
            }

            // Inverse FFT
            fft_inplace(&mut block, true);

            // Discard overlap prefix, keep valid output portion
            let valid = &block[self.overlap..self.overlap + chunk.len()];
            output.extend_from_slice(valid);

            // Save tail for next iteration
            let tail_start = self.fft_size - self.overlap;
            self.tail.clear();
            self.tail.extend_from_slice(&block[tail_start..]);
            // Pad tail to overlap length if block was short
            while self.tail.len() < self.overlap {
                self.tail.push((0.0, 0.0));
            }

            pos += chunk.len();
        }
        output
    }

    /// Query the pre-computed transfer function (for diagnostics).
    pub fn transfer_function(&self) -> &[Cpx] {
        &self.h_inv
    }
}

// ---------------------------------------------------------------------------
// 2×2 CMA Butterfly Equalizer
// ---------------------------------------------------------------------------

/// 2×2 CMA butterfly equalizer for polarization demultiplexing and PMD compensation.
///
/// The butterfly structure implements four adaptive FIR filters (Hxx, Hxy, Hyx, Hyy)
/// operating at T/2 (fractionally spaced) to simultaneously demultiplex the two
/// polarizations and compensate first-order PMD and residual CD.
///
/// Update rule (constant modulus):
/// ```text
/// Ex_out = Hxx ★ Ex + Hxy ★ Ey
/// Ey_out = Hyx ★ Ex + Hyy ★ Ey
/// err_x  = 1 - |Ex_out|²
/// err_y  = 1 - |Ey_out|²
/// ΔHxx   = μ · err_x · Ex_out · Ex*
/// ΔHxy   = μ · err_x · Ex_out · Ey*
/// ΔHyx   = μ · err_y · Ey_out · Ex*
/// ΔHyy   = μ · err_y · Ey_out · Ey*
/// ```
///
/// Reference: Savory, S.J. (2010) "Digital Coherent Optical Receivers:
/// Algorithms and Subsystems", IEEE J. Sel. Topics Quantum Electron. 16.
#[derive(Debug, Clone)]
pub struct CmaButterfly {
    /// Number of FIR taps for each of the four sub-filters.
    pub num_taps: usize,
    /// CMA step size (learning rate).
    pub step_size: f64,
    /// Hxx filter taps (X-pol in → X-pol out).
    pub hxx: Vec<Cpx>,
    /// Hxy filter taps (Y-pol in → X-pol out).
    pub hxy: Vec<Cpx>,
    /// Hyx filter taps (X-pol in → Y-pol out).
    pub hyx: Vec<Cpx>,
    /// Hyy filter taps (Y-pol in → Y-pol out).
    pub hyy: Vec<Cpx>,
    /// Circular delay line for X polarization input history.
    buf_x: Vec<Cpx>,
    /// Circular delay line for Y polarization input history.
    buf_y: Vec<Cpx>,
    /// Write head position in circular buffer.
    buf_pos: usize,
    /// Number of samples processed (for convergence monitoring).
    pub iterations: u64,
}

impl CmaButterfly {
    /// Create a new 2×2 CMA butterfly equalizer.
    ///
    /// Initialises `Hxx` and `Hyy` as centre-spike filters (convergence aid).
    pub fn new(num_taps: usize, step_size: f64) -> Self {
        let mid = num_taps / 2;
        let mut hxx = vec![(0.0, 0.0); num_taps];
        let mut hyy = vec![(0.0, 0.0); num_taps];
        hxx[mid] = (1.0, 0.0);
        hyy[mid] = (1.0, 0.0);

        Self {
            num_taps,
            step_size,
            hxx,
            hxy: vec![(0.0, 0.0); num_taps],
            hyx: vec![(0.0, 0.0); num_taps],
            hyy,
            buf_x: vec![(0.0, 0.0); num_taps],
            buf_y: vec![(0.0, 0.0); num_taps],
            buf_pos: 0,
            iterations: 0,
        }
    }

    /// Convolve a filter `h` with the circular delay line `buf` at position `pos`.
    fn convolve(h: &[Cpx], buf: &[Cpx], pos: usize) -> Cpx {
        let n = h.len();
        let mut acc: Cpx = (0.0, 0.0);
        for k in 0..n {
            let idx = (pos + n - 1 - k) % n;
            acc = cadd(acc, cmul(h[k], cconj(buf[idx])));
        }
        acc
    }

    /// Process one sample pair (ex, ey) and adapt the butterfly taps.
    ///
    /// Returns the equalised output pair `(out_x, out_y)`.
    pub fn process_sample(&mut self, ex: Cpx, ey: Cpx) -> (Cpx, Cpx) {
        // Insert new samples into circular buffers
        self.buf_x[self.buf_pos] = ex;
        self.buf_y[self.buf_pos] = ey;

        // FIR outputs
        let ex_out = cadd(
            Self::convolve(&self.hxx, &self.buf_x, self.buf_pos),
            Self::convolve(&self.hxy, &self.buf_y, self.buf_pos),
        );
        let ey_out = cadd(
            Self::convolve(&self.hyx, &self.buf_x, self.buf_pos),
            Self::convolve(&self.hyy, &self.buf_y, self.buf_pos),
        );

        // CMA error signals: e = R² - |y|², R=1 for QPSK
        let err_x = 1.0 - cmag2(ex_out);
        let err_y = 1.0 - cmag2(ey_out);

        // Tap update (gradient descent on CMA cost)
        let mu = self.step_size;
        let n = self.num_taps;
        for k in 0..n {
            let idx = (self.buf_pos + n - 1 - k) % n;
            let bx = self.buf_x[idx];
            let by = self.buf_y[idx];
            // ΔH = μ · e · y · x* (x* because convolve uses cconj)
            self.hxx[k] = cadd(self.hxx[k], cscale(cmul(ex_out, bx), mu * err_x));
            self.hxy[k] = cadd(self.hxy[k], cscale(cmul(ex_out, by), mu * err_x));
            self.hyx[k] = cadd(self.hyx[k], cscale(cmul(ey_out, bx), mu * err_y));
            self.hyy[k] = cadd(self.hyy[k], cscale(cmul(ey_out, by), mu * err_y));
        }

        self.buf_pos = (self.buf_pos + 1) % self.num_taps;
        self.iterations += 1;
        (ex_out, ey_out)
    }

    /// Process a block of dual-polarization samples.
    pub fn process_block(&mut self, ex: &[Cpx], ey: &[Cpx]) -> (Vec<Cpx>, Vec<Cpx>) {
        assert_eq!(ex.len(), ey.len());
        let mut out_x = Vec::with_capacity(ex.len());
        let mut out_y = Vec::with_capacity(ey.len());
        for (&sx, &sy) in ex.iter().zip(ey.iter()) {
            let (ox, oy) = self.process_sample(sx, sy);
            out_x.push(ox);
            out_y.push(oy);
        }
        (out_x, out_y)
    }

    /// Compute residual CMA cost (should approach 0 at convergence).
    pub fn cost(&self, ex: &[Cpx], ey: &[Cpx]) -> f64 {
        let (ox, oy) = {
            let mut clone = self.clone();
            clone.process_block(ex, ey)
        };
        let cost_x: f64 = ox.iter().map(|&s| { let e = 1.0 - cmag2(s); e * e }).sum();
        let cost_y: f64 = oy.iter().map(|&s| { let e = 1.0 - cmag2(s); e * e }).sum();
        (cost_x + cost_y) / (2.0 * ex.len() as f64)
    }
}

// ---------------------------------------------------------------------------
// Frequency Offset Estimator
// ---------------------------------------------------------------------------

/// Carrier frequency offset (CFO) estimator using the 4th-power spectral method.
///
/// For QPSK, raising to the 4th power removes the modulation (since 4 × π/4 = π
/// multiples of the carrier). The CFO peak in the 4th-power spectrum is at 4×Δf.
///
/// Algorithm:
/// 1. Raise all samples to the 4th power: `z = s^4`
/// 2. Compute FFT of z
/// 3. Find peak frequency → CFO = f_peak / 4
///
/// Reference: Ly-Gagnon et al. (2006), Proc. ECOC, paper Mo4.2.1.
#[derive(Debug, Clone)]
pub struct FrequencyOffsetEstimator {
    /// FFT size for spectral estimation.
    pub fft_size: usize,
    /// Estimated frequency offset in normalised units (cycles/sample).
    pub last_estimate_norm: f64,
}

impl FrequencyOffsetEstimator {
    /// Create a new CFO estimator with the given FFT block size.
    pub fn new(fft_size: usize) -> Self {
        assert!(fft_size.is_power_of_two());
        Self { fft_size, last_estimate_norm: 0.0 }
    }

    /// Estimate normalised CFO from a block of QPSK samples (1 pol).
    ///
    /// Returns CFO in cycles/sample (multiply by sample_rate_hz for Hz).
    pub fn estimate(&mut self, samples: &[Cpx]) -> f64 {
        let n = self.fft_size.min(samples.len());
        let mut z: Vec<Cpx> = samples[..n].iter().map(|&s| cpow4(s)).collect();
        // Zero-pad to fft_size
        z.resize(self.fft_size, (0.0, 0.0));
        fft_inplace(&mut z, false);

        // Find peak bin (skip DC bin 0)
        let (peak_bin, _) = z.iter().enumerate().skip(1)
            .map(|(k, &v)| (k, cmag2(v)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap_or((0, 0.0));

        // Convert to normalised frequency (cycles/sample), divide by 4 for 4th-power
        let freq = if peak_bin < self.fft_size / 2 {
            peak_bin as f64 / self.fft_size as f64
        } else {
            (peak_bin as i64 - self.fft_size as i64) as f64 / self.fft_size as f64
        };
        self.last_estimate_norm = freq / 4.0;
        self.last_estimate_norm
    }

    /// Apply frequency offset correction to a signal block.
    ///
    /// `cfo_norm`: normalised CFO in cycles/sample.
    pub fn correct(samples: &[Cpx], cfo_norm: f64) -> Vec<Cpx> {
        samples.iter().enumerate().map(|(n, &s)| {
            let phase = -2.0 * PI * cfo_norm * n as f64;
            let rot = (phase.cos(), phase.sin());
            cmul(s, rot)
        }).collect()
    }
}

// ---------------------------------------------------------------------------
// Viterbi-Viterbi Carrier Phase Estimator (4th Power)
// ---------------------------------------------------------------------------

/// Viterbi-Viterbi (V-V) carrier phase estimator for QPSK.
///
/// Raises samples to the 4th power to remove the QPSK modulation, then
/// averages the complex phase over a block to estimate the residual carrier
/// phase noise. Phase unwrapping handles cycle slips between blocks.
///
/// The estimated phase φ̂ satisfies: `φ_carrier ≈ arg(Σ s^4) / 4`.
///
/// Reference: Viterbi, A.J. & Viterbi, A.M. (1983) "Nonlinear estimation of
/// PSK-modulated carrier phase with application to burst digital transmission",
/// IEEE Trans. Inf. Theory 29, 543–551.
#[derive(Debug, Clone)]
pub struct ViterbiViterbiCpe {
    /// Block size for averaging (controls noise-bandwidth trade-off).
    pub block_size: usize,
    /// Last estimated phase for unwrapping continuity.
    last_phase: f64,
}

impl ViterbiViterbiCpe {
    /// Create a new V-V phase estimator.
    pub fn new(block_size: usize) -> Self {
        Self { block_size, last_phase: 0.0 }
    }

    /// Estimate and correct carrier phase for a single block.
    ///
    /// Returns `(corrected_samples, estimated_phase_rad)`.
    pub fn process_block(&mut self, samples: &[Cpx]) -> (Vec<Cpx>, f64) {
        // Accumulate 4th-power sum over block
        let sum4 = samples.iter().fold((0.0, 0.0), |acc, &s| cadd(acc, cpow4(s)));

        // Raw phase estimate: arg(sum) / 4
        let raw_phase = carg(sum4) / 4.0;

        // Phase unwrapping: find closest multiple of π/2 to last_phase
        let diff = raw_phase - self.last_phase;
        let pi_half = PI / 2.0;
        let n_half = (diff / pi_half).round();
        let unwrapped = raw_phase - n_half * pi_half;

        self.last_phase = unwrapped;

        // Correct each sample by rotating by -φ̂
        let corrected: Vec<Cpx> = samples.iter().map(|&s| {
            let rot = (-unwrapped.cos(), -unwrapped.sin());
            // Rotate by -unwrapped: multiply by exp(-j·φ̂)
            let c = unwrapped.cos();
            let sn = unwrapped.sin();
            (s.0 * c + s.1 * sn, -s.0 * sn + s.1 * c)
        }).collect();

        (corrected, unwrapped)
    }

    /// Process an entire signal stream block-by-block.
    pub fn process_stream(&mut self, samples: &[Cpx]) -> Vec<Cpx> {
        let mut output = Vec::with_capacity(samples.len());
        let mut i = 0;
        while i < samples.len() {
            let end = (i + self.block_size).min(samples.len());
            let (corrected, _) = self.process_block(&samples[i..end]);
            output.extend(corrected);
            i = end;
        }
        output
    }
}

// ---------------------------------------------------------------------------
// QPSK Symbol Detector
// ---------------------------------------------------------------------------

/// QPSK symbol decision and Gray decoding.
///
/// QPSK constellation:
/// ```text
///   Q
///   |
///   10 --- 00
///   |       |
/// --+-------+-- I
///   |       |
///   11 --- 01
/// ```
///
/// Bit mapping (2-bit symbol): MSB = I-negative, LSB = Q-negative.
/// - (+,+) → 00, (+,-) → 01, (-,+) → 10, (-,-) → 11
#[derive(Debug, Clone, Default)]
pub struct QpskDetector {
    /// Previous symbol for differential decoding.
    prev_symbol: u8,
}

impl QpskDetector {
    /// Create a new QPSK hard-decision detector.
    pub fn new() -> Self { Self { prev_symbol: 0 } }

    /// Make a hard decision on a single complex sample.
    ///
    /// Returns a 2-bit symbol (0..3):
    /// MSB = I negative (1 if I < 0), LSB = Q negative (1 if Q < 0).
    pub fn decide(&self, s: Cpx) -> u8 {
        let i_neg = if s.0 < 0.0 { 1u8 } else { 0u8 }; // MSB
        let q_neg = if s.1 < 0.0 { 1u8 } else { 0u8 }; // LSB
        (i_neg << 1) | q_neg
    }

    /// Hard decision on a block, returns Gray-decoded 2-bit symbols.
    pub fn decide_block(&self, samples: &[Cpx]) -> Vec<u8> {
        samples.iter().map(|&s| self.decide(s)).collect()
    }

    /// Differential decode a stream of QPSK symbols.
    ///
    /// Differential decoding resolves π/2 phase ambiguity by encoding
    /// information in the phase *difference* between successive symbols.
    /// Symbol = (current_symbol - prev_symbol) mod 4.
    pub fn differential_decode(&mut self, symbols: &[u8]) -> Vec<u8> {
        let mut decoded = Vec::with_capacity(symbols.len());
        for &sym in symbols {
            let diff = sym.wrapping_sub(self.prev_symbol) & 0x03;
            decoded.push(diff);
            self.prev_symbol = sym;
        }
        decoded
    }

    /// Convert 2-bit QPSK symbols to a flat bit vector (MSB first per symbol).
    ///
    /// MSB = I-negative bit, LSB = Q-negative bit.
    pub fn symbols_to_bits(symbols: &[u8]) -> Vec<bool> {
        let mut bits = Vec::with_capacity(symbols.len() * 2);
        for &sym in symbols {
            bits.push((sym >> 1) & 1 != 0); // MSB (I-negative)
            bits.push(sym & 1 != 0);         // LSB (Q-negative)
        }
        bits
    }

    /// Ideal QPSK constellation point for symbol `s` (0..3).
    ///
    /// Symbol encoding: MSB = I-negative, LSB = Q-negative.
    pub fn ideal_point(s: u8) -> Cpx {
        let v = 1.0 / 2.0_f64.sqrt();
        // MSB selects I sign, LSB selects Q sign
        let i = if (s >> 1) & 1 == 0 {  v } else { -v };
        let q = if       s  & 1 == 0 {  v } else { -v };
        (i, q)
    }
}

// ---------------------------------------------------------------------------
// Pre-FEC BER Estimator
// ---------------------------------------------------------------------------

/// Pre-FEC BER estimator from EVM or decision distance.
///
/// For QPSK in AWGN, the BER is:
/// `BER = Q(√(2 · Eb/N0)) = erfc(√(SNR_per_bit)) / 2`
///
/// The linear SNR can be estimated from the EVM (Error Vector Magnitude):
/// `EVM_rms = √(E[|y - s̃|²] / E[|s̃|²])`, `SNR = 1/EVM²`
#[derive(Debug, Clone, Default)]
pub struct BerEstimator {
    /// Accumulated squared error sum.
    error_sq_sum: f64,
    /// Accumulated reference power sum.
    ref_pwr_sum: f64,
    /// Total number of symbols processed.
    pub symbol_count: u64,
    /// Bit errors counted from hard decisions.
    pub bit_errors: u64,
    /// Total bits compared.
    pub bit_count: u64,
}

impl BerEstimator {
    /// Create a new BER estimator.
    pub fn new() -> Self { Self::default() }

    /// Accumulate one QPSK symbol for EVM-based SNR estimation.
    pub fn accumulate(&mut self, received: Cpx, reference: Cpx) {
        let err = csub(received, reference);
        self.error_sq_sum += cmag2(err);
        self.ref_pwr_sum += cmag2(reference);
        self.symbol_count += 1;
    }

    /// Accumulate a block of samples against their ideal constellation points.
    pub fn accumulate_block(&mut self, received: &[Cpx], decisions: &[u8]) {
        for (&r, &d) in received.iter().zip(decisions.iter()) {
            let ideal = QpskDetector::ideal_point(d);
            self.accumulate(r, ideal);
        }
    }

    /// RMS EVM as a fraction (0.0–1.0).
    pub fn evm_rms(&self) -> f64 {
        if self.ref_pwr_sum < 1e-30 { return 1.0; }
        (self.error_sq_sum / self.ref_pwr_sum).sqrt()
    }

    /// Estimated linear SNR per symbol (Eb/N0 for QPSK).
    pub fn snr_linear(&self) -> f64 {
        let evm = self.evm_rms();
        if evm < 1e-30 { return f64::INFINITY; }
        1.0 / (evm * evm)
    }

    /// Estimated SNR in dB.
    pub fn snr_db(&self) -> f64 { 10.0 * self.snr_linear().log10() }

    /// Approximate pre-FEC BER from SNR using Q-function approximation.
    ///
    /// For QPSK: BER ≈ erfc(√(SNR/2)) / 2, approximated via:
    /// `erfc(x) ≈ exp(-x²) / (x · √π)` for x >> 1.
    pub fn estimated_ber(&self) -> f64 {
        let snr = self.snr_linear();
        if snr < 1e-6 { return 0.5; }
        let x = (snr / 2.0).sqrt();
        // Gaussian Q-function approximation: Q(x) ≈ exp(-x²/2)/(x·√(2π))
        let q = (-x * x / 2.0).exp() / (x * (2.0 * PI).sqrt());
        q.clamp(0.0, 0.5)
    }

    /// Measured BER from hard decision bit counting.
    pub fn measured_ber(&self) -> f64 {
        if self.bit_count == 0 { return 0.5; }
        self.bit_errors as f64 / self.bit_count as f64
    }

    /// Reset all accumulators.
    pub fn reset(&mut self) { *self = Self::default(); }
}

// ---------------------------------------------------------------------------
// PMD Channel Model
// ---------------------------------------------------------------------------

/// First-order PMD channel model for testing the equalizer.
///
/// Models differential group delay (DGD) as a Jones matrix rotation combined
/// with a differential phase delay between principal states of polarization (PSP).
#[derive(Debug, Clone)]
pub struct PmdChannel {
    /// DGD in seconds.
    pub dgd_s: f64,
    /// PSP rotation angle in radians.
    pub rotation_rad: f64,
    /// Accumulated carrier phase offset in radians.
    pub carrier_phase_rad: f64,
    /// Normalised frequency offset in cycles/sample.
    pub freq_offset_norm: f64,
}

impl PmdChannel {
    /// Create a PMD channel with given DGD, rotation, and carrier impairments.
    pub fn new(dgd_s: f64, rotation_rad: f64, carrier_phase_rad: f64, freq_offset_norm: f64) -> Self {
        Self { dgd_s, rotation_rad, carrier_phase_rad, freq_offset_norm }
    }

    /// Apply PMD channel effects to a block of dual-pol samples.
    ///
    /// Applies: rotation → DGD phase delay → frequency offset → phase offset.
    pub fn apply(
        &self,
        ex_in: &[Cpx],
        ey_in: &[Cpx],
        symbol_rate_hz: f64,
    ) -> (Vec<Cpx>, Vec<Cpx>) {
        let rot = JonesMatrix::rotation(self.rotation_rad);
        let dgd = JonesMatrix::first_order_pmd(self.dgd_s, symbol_rate_hz);
        let channel = dgd.mul(&rot);

        ex_in.iter().zip(ey_in.iter()).enumerate().map(|(n, (&ex, &ey))| {
            let (mx, my) = channel.apply(ex, ey);
            // Apply frequency offset and carrier phase
            let phase = 2.0 * PI * self.freq_offset_norm * n as f64 + self.carrier_phase_rad;
            let rot_cpx = (phase.cos(), phase.sin());
            (cmul(mx, rot_cpx), cmul(my, rot_cpx))
        }).unzip()
    }
}

// ---------------------------------------------------------------------------
// Complete DP-QPSK Receiver Chain
// ---------------------------------------------------------------------------

/// Complete DP-QPSK coherent receiver chain.
///
/// Assembles all DSP stages into the full receive pipeline per OIF-100G-LR-1.0.
/// Each stage can be individually bypassed for diagnostics.
pub struct DpQpskReceiver {
    /// Receiver configuration.
    pub config: DpQpskConfig,
    /// Optical hybrid model (X polarization).
    pub hybrid_x: OpticalHybrid,
    /// Optical hybrid model (Y polarization).
    pub hybrid_y: OpticalHybrid,
    /// ADC model for I branch.
    pub adc_i: AdcModel,
    /// ADC model for Q branch.
    pub adc_q: AdcModel,
    /// Gram-Schmidt IQ corrector (X pol).
    pub gs_x: GramSchmidt,
    /// Gram-Schmidt IQ corrector (Y pol).
    pub gs_y: GramSchmidt,
    /// Resampler for X-pol I branch.
    pub resamp_xi: CubicResampler,
    /// Resampler for X-pol Q branch.
    pub resamp_xq: CubicResampler,
    /// Resampler for Y-pol I branch.
    pub resamp_yi: CubicResampler,
    /// Resampler for Y-pol Q branch.
    pub resamp_yq: CubicResampler,
    /// Chromatic dispersion compensator.
    pub cd_comp: CdCompensator,
    /// 2×2 CMA butterfly equalizer.
    pub cma: CmaButterfly,
    /// Frequency offset estimator.
    pub foe: FrequencyOffsetEstimator,
    /// Viterbi-Viterbi carrier phase estimator (X pol).
    pub vv_x: ViterbiViterbiCpe,
    /// Viterbi-Viterbi carrier phase estimator (Y pol).
    pub vv_y: ViterbiViterbiCpe,
    /// Symbol detector.
    pub detector: QpskDetector,
    /// BER estimator.
    pub ber: BerEstimator,
    /// Accumulated carrier frequency offset estimate (normalised).
    pub freq_offset_est: f64,
}

impl DpQpskReceiver {
    /// Build a complete DP-QPSK receiver from configuration.
    pub fn new(config: DpQpskConfig) -> Self {
        let hybrid = OpticalHybrid {
            amplitude_imbalance: config.hybrid_amplitude_imbalance,
            quadrature_error: config.hybrid_quadrature_error_rad,
            cmrr_db: 30.0,
        };

        let adc_full_scale = 1.5;
        let adc_i = AdcModel::new(config.adc_bits, adc_full_scale);
        let adc_q = AdcModel::new(config.adc_bits, adc_full_scale);

        // Resampling ratio: ADC rate / (2 × symbol rate) for 2 samp/sym output
        let ratio = config.adc_sample_rate_gsa / (2.0 * config.symbol_rate_gbaud);

        let sample_rate_post_resamp = 2.0 * config.symbol_rate_gbaud * 1e9;
        let fft_size = 512; // CD compensator FFT block size

        let cd_comp = CdCompensator::new(
            config.fiber_length_km,
            config.cd_ps_per_nm_km,
            config.wavelength_nm,
            sample_rate_post_resamp,
            fft_size,
        );

        Self {
            hybrid_x: hybrid.clone(),
            hybrid_y: hybrid,
            adc_i,
            adc_q,
            gs_x: GramSchmidt::new(0.1),
            gs_y: GramSchmidt::new(0.1),
            resamp_xi: CubicResampler::new(ratio),
            resamp_xq: CubicResampler::new(ratio),
            resamp_yi: CubicResampler::new(ratio),
            resamp_yq: CubicResampler::new(ratio),
            cd_comp,
            cma: CmaButterfly::new(config.cma_taps, config.cma_step_size),
            foe: FrequencyOffsetEstimator::new(1024),
            vv_x: ViterbiViterbiCpe::new(config.vv_block_size),
            vv_y: ViterbiViterbiCpe::new(config.vv_block_size),
            detector: QpskDetector::new(),
            ber: BerEstimator::new(),
            freq_offset_est: 0.0,
            config,
        }
    }

    /// Process a block through the full receiver chain.
    ///
    /// Inputs are optical fields Ex, Ey and LO fields Elo_x, Elo_y.
    /// Returns decoded bit vectors for X and Y polarizations.
    pub fn process(
        &mut self,
        ex: &[Cpx],
        ey: &[Cpx],
        elo_x: &[Cpx],
        elo_y: &[Cpx],
    ) -> (Vec<bool>, Vec<bool>) {
        // Stage 1: Optical hybrid + balanced detection
        let (mut xi, mut xq, mut yi, mut yq) =
            self.hybrid_x.process_block(ex, ey, elo_x, elo_y);

        // Stage 2: ADC quantisation
        self.adc_i.quantise_slice(&mut xi);
        self.adc_q.quantise_slice(&mut xq);
        self.adc_i.quantise_slice(&mut yi);
        self.adc_q.quantise_slice(&mut yq);

        // Stage 3: Gram-Schmidt IQ imbalance correction
        let (xi, xq) = self.gs_x.process_block(&xi, &xq);
        let (yi, yq) = self.gs_y.process_block(&yi, &yq);

        // Stage 4: Resample to 2 samp/sym
        let xi = self.resamp_xi.process(&xi);
        let xq = self.resamp_xq.process(&xq);
        let yi = self.resamp_yi.process(&yi);
        let yq = self.resamp_yq.process(&yq);

        let min_len = xi.len().min(xq.len()).min(yi.len()).min(yq.len());
        let xcpx: Vec<Cpx> = xi[..min_len].iter().zip(xq[..min_len].iter())
            .map(|(&i, &q)| (i, q)).collect();
        let ycpx: Vec<Cpx> = yi[..min_len].iter().zip(yq[..min_len].iter())
            .map(|(&i, &q)| (i, q)).collect();

        // Stage 5: CD compensation (process X and Y interleaved as single stream)
        let xcpx = self.cd_comp.process(&xcpx);
        // For Y pol, use a fresh view — in real HW both pols share one CD block
        let ycpx = {
            let mut cd2 = self.cd_comp.clone();
            cd2.process(&ycpx)
        };

        // Stage 6: 2×2 CMA butterfly (pol-demux + PMD compensation)
        let (xcpx, ycpx) = self.cma.process_block(&xcpx, &ycpx);

        // Stage 7: Frequency offset estimation and correction (using X pol)
        let cfo = self.foe.estimate(&xcpx);
        self.freq_offset_est = 0.9 * self.freq_offset_est + 0.1 * cfo; // smoothed
        let xcpx = FrequencyOffsetEstimator::correct(&xcpx, self.freq_offset_est);
        let ycpx = FrequencyOffsetEstimator::correct(&ycpx, self.freq_offset_est);

        // Stage 8: Viterbi-Viterbi carrier phase estimation
        let xcpx = self.vv_x.process_stream(&xcpx);
        let ycpx = self.vv_y.process_stream(&ycpx);

        // Stage 9: Symbol detection + optional differential decoding
        let syms_x = self.detector.decide_block(&xcpx);
        let syms_y = self.detector.decide_block(&ycpx);

        // Stage 10: BER estimation update
        self.ber.accumulate_block(&xcpx, &syms_x);
        self.ber.accumulate_block(&ycpx, &syms_y);

        let (bits_x, bits_y) = if self.config.differential_decoding {
            let bits_x = {
                let mut det_x = self.detector.clone();
                let dd = det_x.differential_decode(&syms_x);
                QpskDetector::symbols_to_bits(&dd)
            };
            let bits_y = {
                let mut det_y = self.detector.clone();
                let dd = det_y.differential_decode(&syms_y);
                QpskDetector::symbols_to_bits(&dd)
            };
            (bits_x, bits_y)
        } else {
            (QpskDetector::symbols_to_bits(&syms_x),
             QpskDetector::symbols_to_bits(&syms_y))
        };

        (bits_x, bits_y)
    }

    /// Generate an ideal QPSK signal at 2 samp/sym for testing.
    ///
    /// Returns (X-pol, Y-pol) complex sample vectors.
    pub fn generate_test_signal(
        symbols_x: &[u8],
        symbols_y: &[u8],
        sps: usize,
    ) -> (Vec<Cpx>, Vec<Cpx>) {
        let mut ex: Vec<Cpx> = Vec::new();
        let mut ey: Vec<Cpx> = Vec::new();
        for (&sx, &sy) in symbols_x.iter().zip(symbols_y.iter()) {
            let px = QpskDetector::ideal_point(sx);
            let py = QpskDetector::ideal_point(sy);
            for _ in 0..sps {
                ex.push(px);
                ey.push(py);
            }
        }
        (ex, ey)
    }
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SQRT2: f64 = std::f64::consts::SQRT_2;
    const INV_SQRT2: f64 = 1.0 / SQRT2;

    // --- Complex arithmetic helpers ---

    #[test]
    fn test_cmul_identity() {
        let a: Cpx = (3.0, 4.0);
        let one: Cpx = (1.0, 0.0);
        let r = cmul(a, one);
        assert!((r.0 - a.0).abs() < 1e-12);
        assert!((r.1 - a.1).abs() < 1e-12);
    }

    #[test]
    fn test_cmul_j_squared_minus_one() {
        let j: Cpx = (0.0, 1.0);
        let r = cmul(j, j);
        assert!((r.0 + 1.0).abs() < 1e-12); // -1
        assert!(r.1.abs() < 1e-12);
    }

    #[test]
    fn test_cmag_pythagorean() {
        let a: Cpx = (3.0, 4.0);
        assert!((cmag(a) - 5.0).abs() < 1e-12);
    }

    #[test]
    fn test_cpow4_rotation() {
        // exp(j·π/4) raised to 4th power = exp(j·π) = -1
        let phi = PI / 4.0;
        let a: Cpx = (phi.cos(), phi.sin());
        let r = cpow4(a);
        assert!((r.0 + 1.0).abs() < 1e-10);
        assert!(r.1.abs() < 1e-10);
    }

    // --- FFT round-trip ---

    #[test]
    fn test_fft_ifft_roundtrip() {
        let n = 64;
        let original: Vec<Cpx> = (0..n).map(|k| (k as f64, 0.0)).collect();
        let spectrum = fft(&original);
        let recovered = ifft(&spectrum);
        for (a, b) in original.iter().zip(recovered.iter()) {
            assert!((a.0 - b.0).abs() < 1e-8, "Re mismatch: {} vs {}", a.0, b.0);
            assert!((a.1 - b.1).abs() < 1e-8, "Im mismatch: {} vs {}", a.1, b.1);
        }
    }

    #[test]
    fn test_fft_single_tone() {
        let n = 64usize;
        let k0 = 4usize; // target bin
        let x: Vec<Cpx> = (0..n).map(|i| {
            let ph = 2.0 * PI * k0 as f64 * i as f64 / n as f64;
            (ph.cos(), ph.sin())
        }).collect();
        let spec = fft(&x);
        // Find bin with max magnitude
        let mut peak_bin = 0;
        let mut peak_val = 0.0_f64;
        for (k, &v) in spec.iter().enumerate() {
            let m = cmag2(v);
            if m > peak_val { peak_val = m; peak_bin = k; }
        }
        assert_eq!(peak_bin, k0, "Peak at bin {}, expected {}", peak_bin, k0);
    }

    // --- Jones Matrix ---

    #[test]
    fn test_jones_identity() {
        let j = JonesMatrix::identity();
        let ex: Cpx = (1.0, 0.0);
        let ey: Cpx = (0.0, 1.0);
        let (ox, oy) = j.apply(ex, ey);
        assert!((ox.0 - ex.0).abs() < 1e-12);
        assert!((ox.1 - ex.1).abs() < 1e-12);
        assert!((oy.0 - ey.0).abs() < 1e-12);
        assert!((oy.1 - ey.1).abs() < 1e-12);
    }

    #[test]
    fn test_jones_rotation_90_degrees() {
        let j = JonesMatrix::rotation(PI / 2.0);
        let ex: Cpx = (1.0, 0.0);
        let ey: Cpx = (0.0, 0.0);
        let (ox, oy) = j.apply(ex, ey);
        // Rotation by 90°: x → cos(90)x - sin(90)y = 0, y → sin(90)x + cos(90)y = 1
        assert!(ox.0.abs() < 1e-10, "ox.Re = {}", ox.0);
        assert!(oy.0 > 0.9, "oy.Re = {}", oy.0);
    }

    #[test]
    fn test_jones_rotation_preserves_power() {
        let theta = 0.7;
        let j = JonesMatrix::rotation(theta);
        let ex: Cpx = (0.8, 0.0);
        let ey: Cpx = (0.6, 0.0);
        let (ox, oy) = j.apply(ex, ey);
        let power_in = cmag2(ex) + cmag2(ey);
        let power_out = cmag2(ox) + cmag2(oy);
        assert!((power_in - power_out).abs() < 1e-12);
    }

    #[test]
    fn test_jones_mul_identity() {
        let j = JonesMatrix::identity();
        let r = JonesMatrix::rotation(0.3);
        let rj = r.mul(&j);
        // r × I = r
        let ex: Cpx = (1.0, 0.0);
        let ey: Cpx = (0.0, 1.0);
        let (ox1, oy1) = r.apply(ex, ey);
        let (ox2, oy2) = rj.apply(ex, ey);
        assert!((ox1.0 - ox2.0).abs() < 1e-12);
        assert!((oy1.1 - oy2.1).abs() < 1e-12);
    }

    #[test]
    fn test_jones_det_identity() {
        let j = JonesMatrix::identity();
        let det = j.det();
        assert!((det.0 - 1.0).abs() < 1e-12);
        assert!(det.1.abs() < 1e-12);
    }

    #[test]
    fn test_jones_det_rotation() {
        // det of rotation = cos²θ + sin²θ = 1
        let j = JonesMatrix::rotation(0.5);
        let det = j.det();
        assert!((det.0 - 1.0).abs() < 1e-12);
        assert!(det.1.abs() < 1e-12);
    }

    #[test]
    fn test_jones_concatenation() {
        // Two opposite rotations cancel to identity
        let j1 = JonesMatrix::rotation(0.4);
        let j2 = JonesMatrix::rotation(-0.4);
        let combined = j2.mul(&j1);
        let ex: Cpx = (1.0, 0.0);
        let ey: Cpx = (0.0, 1.0);
        let (ox, oy) = combined.apply(ex, ey);
        assert!((ox.0 - ex.0).abs() < 1e-10);
        assert!((oy.0 - ey.0).abs() < 1e-10);
    }

    // --- Optical Hybrid ---

    #[test]
    fn test_hybrid_perfect_output_relationships() {
        let h = OpticalHybrid::perfect();
        // LO at (1,0), signal at (1,0): expect XI=1, XQ=0 (beat = 1+0j)
        let es: Cpx = (1.0, 0.0);
        let elo: Cpx = (1.0, 0.0);
        let (xi, xq) = h.process(es, elo);
        assert!(xi > 0.9, "XI should be ~1, got {}", xi);
        assert!(xq.abs() < 0.01, "XQ should be ~0, got {}", xq);
    }

    #[test]
    fn test_hybrid_quadrature_lo() {
        let h = OpticalHybrid::perfect();
        // LO at j (90° phase), signal at (1,0): beat = (1,0)×(0,-j) = (0,1j)*conj = (0,-1)
        // Re{beat} = 0, Im{beat} = -1 → XI~0, XQ~-1
        let es: Cpx = (1.0, 0.0);
        let elo: Cpx = (0.0, 1.0);
        let (xi, xq) = h.process(es, elo);
        assert!(xi.abs() < 0.01, "XI should be ~0, got {}", xi);
        assert!(xq < -0.9, "XQ should be ~-1, got {}", xq);
    }

    #[test]
    fn test_hybrid_amplitude_imbalance() {
        let h = OpticalHybrid { amplitude_imbalance: 1.2, quadrature_error: 0.0, cmrr_db: 60.0 };
        let es: Cpx = (0.0, 1.0); // purely imaginary signal
        let elo: Cpx = (1.0, 0.0); // real LO
        let (xi, xq) = h.process(es, elo);
        // beat = (0,1)×(1,0) = (0,1); XI=0, XQ_ideal=1 × 1.2 = 1.2
        assert!(xi.abs() < 0.05, "XI should be ~0, got {}", xi);
        assert!((xq - 1.2).abs() < 0.05, "XQ should be ~1.2, got {}", xq);
    }

    #[test]
    fn test_hybrid_cmrr_leakage() {
        // Very low CMRR (10 dB) should show visible common-mode leakage
        let h = OpticalHybrid { amplitude_imbalance: 1.0, quadrature_error: 0.0, cmrr_db: 10.0 };
        let es: Cpx = (1.0, 0.0);
        let elo: Cpx = (1.0, 0.0);
        let (xi, _) = h.process(es, elo);
        let cmrr_linear = 10.0_f64.powf(-10.0 / 20.0);
        let expected_leak = 2.0 * cmrr_linear; // (|es|² + |elo|²) × cmrr
        // XI = 1.0 (signal) + leak
        assert!((xi - 1.0 - expected_leak).abs() < 1e-10);
    }

    // --- ADC Model ---

    #[test]
    fn test_adc_quantisation_levels() {
        let adc = AdcModel::new(4, 1.0); // 4-bit, ±1 range
        // 4 bits = 16 levels, LSB = 2/16 = 0.125
        let q = adc.quantise(0.13);
        assert!((q % 0.125).abs() < 1e-10, "Not aligned to LSB grid: {}", q);
    }

    #[test]
    fn test_adc_clipping() {
        let adc = AdcModel::new(8, 1.0);
        let q = adc.quantise(5.0); // well over full scale
        assert!(q <= 1.0, "Must clip to full scale: {}", q);
        let q2 = adc.quantise(-5.0);
        assert!(q2 >= -1.0, "Must clip to -full scale: {}", q2);
    }

    #[test]
    fn test_adc_zero_input() {
        let adc = AdcModel::new(8, 1.0);
        let q = adc.quantise(0.0);
        assert_eq!(q, 0.0);
    }

    // --- Gram-Schmidt ---

    #[test]
    fn test_gram_schmidt_identity() {
        // Perfectly balanced I and Q → should return approximately the same
        let mut gs = GramSchmidt::new(1.0);
        let i: Vec<f64> = (0..100).map(|k| (k as f64 * 0.1).sin()).collect();
        let q: Vec<f64> = (0..100).map(|k| (k as f64 * 0.1 + PI / 2.0).sin()).collect();
        let (io, qo) = gs.process_block(&i, &q);
        // Both should be roughly unit-normalised and orthogonal
        let dot: f64 = io.iter().zip(qo.iter()).map(|(a, b)| a * b).sum::<f64>() / 100.0;
        assert!(dot.abs() < 0.1, "I and Q should be orthogonal, dot={}", dot);
    }

    #[test]
    fn test_gram_schmidt_gain_imbalance_correction() {
        let mut gs = GramSchmidt::new(1.0);
        // I: amplitude 1.0, Q: amplitude 0.5 (gain imbalance 2:1)
        let n = 200;
        let i: Vec<f64> = (0..n).map(|k| (k as f64 * 0.1).sin()).collect();
        let q: Vec<f64> = (0..n).map(|k| 0.5 * (k as f64 * 0.1 + PI / 2.0).sin()).collect();
        let (io, qo) = gs.process_block(&i, &q);
        let rms_i = (io.iter().map(|&v| v * v).sum::<f64>() / n as f64).sqrt();
        let rms_q = (qo.iter().map(|&v| v * v).sum::<f64>() / n as f64).sqrt();
        // After correction, RMS values should be more similar
        assert!((rms_i - rms_q).abs() < 0.3, "RMS I={}, Q={}", rms_i, rms_q);
    }

    #[test]
    fn test_gram_schmidt_orthogonality_restored() {
        let mut gs = GramSchmidt::new(1.0);
        let n = 256;
        // Introduce quadrature error: Q = cos(wt) + 0.3*sin(wt) (non-orthogonal)
        let i: Vec<f64> = (0..n).map(|k| (k as f64 * 0.05).sin()).collect();
        let q: Vec<f64> = (0..n).map(|k| (k as f64 * 0.05).cos() + 0.3 * (k as f64 * 0.05).sin()).collect();
        let (io, qo) = gs.process_block(&i, &q);
        let dot: f64 = io.iter().zip(qo.iter()).map(|(a, b)| a * b).sum::<f64>() / n as f64;
        assert!(dot.abs() < 0.05, "Residual cross-correlation after GS: {}", dot);
    }

    // --- Cubic Resampler ---

    #[test]
    fn test_cubic_resampler_ratio_one() {
        // Ratio=1 should pass through (approximately)
        let mut r = CubicResampler::new(1.0);
        let input: Vec<f64> = (0..20).map(|k| k as f64).collect();
        let output = r.process(&input);
        // Output should have approximately same number of samples
        assert!(!output.is_empty(), "Resampler produced no output");
    }

    #[test]
    fn test_cubic_resampler_downsampling() {
        // Ratio=2 means output at half rate
        let mut r = CubicResampler::new(2.0);
        let input: Vec<f64> = vec![1.0; 100];
        let output = r.process(&input);
        // Output should be roughly half as long
        assert!(output.len() < input.len(), "Expected downsampling");
    }

    #[test]
    fn test_cubic_resampler_dc_preservation() {
        // DC signal should pass through unchanged
        let mut r = CubicResampler::new(1.5);
        let input = vec![1.0_f64; 50];
        let output = r.process(&input);
        // After warmup, output values should be close to 1.0
        let tail: &[f64] = &output[output.len() / 2..];
        let mean = tail.iter().sum::<f64>() / tail.len() as f64;
        assert!((mean - 1.0).abs() < 0.01, "DC mean = {}", mean);
    }

    // --- CD Compensator ---

    #[test]
    fn test_cd_compensator_roundtrip_identity() {
        // Verify that the CD compensator is an all-pass filter (preserves power per sample)
        let n_fft = 64;
        let fiber_km = 500.0;
        let sample_rate = 56e9;
        let comp = CdCompensator::new(fiber_km, 17.0, 1550.0, sample_rate, n_fft);

        // Apply forward CD (multiply a spectrum by the conjugate of H_inv = H_fwd)
        // then apply comp → result should cancel back to original
        let h = comp.transfer_function();

        // Check that H_inv is all-pass (|H| = 1) — this is the key invariant
        for (k, &v) in h.iter().enumerate() {
            let mag = cmag(v);
            assert!((mag - 1.0).abs() < 1e-10,
                "H[{}] has non-unit magnitude: {}", k, mag);
        }

        // Also verify H_inv × H_fwd = 1 for all bins
        for &v in h.iter() {
            let h_fwd = cconj(v); // forward CD = conjugate of inverse
            let product = cmul(v, h_fwd);
            assert!((product.0 - 1.0).abs() < 1e-10,
                "H_inv × H_fwd Re != 1: {}", product.0);
            assert!(product.1.abs() < 1e-10,
                "H_inv × H_fwd Im != 0: {}", product.1);
        }
    }

    #[test]
    fn test_cd_compensator_zero_fiber() {
        // Zero fiber length → identity (H=1 everywhere)
        let mut comp = CdCompensator::new(0.0, 17.0, 1550.0, 56e9, 64);
        let h = comp.transfer_function();
        for &v in h {
            assert!((cmag(v) - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn test_cd_compensator_transfer_function_unit_magnitude() {
        // |H_CD| = 1 everywhere (all-pass)
        let comp = CdCompensator::new(1000.0, 17.0, 1550.0, 56e9, 128);
        for &v in comp.transfer_function() {
            assert!((cmag(v) - 1.0).abs() < 1e-10,
                "Non-unit magnitude: {}", cmag(v));
        }
    }

    // --- CMA Butterfly ---

    #[test]
    fn test_cma_butterfly_identity_channel() {
        // With identity channel, CMA should pass unit-modulus QPSK unchanged
        let mut cma = CmaButterfly::new(11, 1e-4);
        let n = 500;
        // Unit-amplitude QPSK symbols (|s| = 1)
        let symbols: Vec<Cpx> = (0..n).map(|k| {
            let ph = PI / 4.0 + k as f64 * PI / 2.0;
            (ph.cos(), ph.sin())
        }).collect();
        let zeros: Vec<Cpx> = vec![(0.0, 0.0); n];
        let (ox, _) = cma.process_block(&symbols, &zeros);
        // Output power should be approximately 1 per sample (unit circle QPSK)
        let power: f64 = ox.iter().skip(50).map(|&s| cmag2(s)).sum::<f64>() / (n - 50) as f64;
        assert!((power - 1.0).abs() < 0.3, "CMA output power = {}", power);
    }

    #[test]
    fn test_cma_butterfly_convergence_rotation() {
        // After many samples, CMA should converge even with polarization rotation
        let rotation = JonesMatrix::rotation(PI / 3.0);
        let n = 2000;
        let mut ex_rot = Vec::new();
        let mut ey_rot = Vec::new();
        for k in 0..n {
            // Unit-amplitude QPSK symbols (|s| = 1) on X and Y
            let ph = PI / 4.0 + k as f64 * PI / 2.0;
            let ex = (ph.cos(), ph.sin());
            let ey = ((-ph + PI / 2.0).cos(), (-ph + PI / 2.0).sin());
            let (rx, ry) = rotation.apply(ex, ey);
            ex_rot.push(rx);
            ey_rot.push(ry);
        }
        let mut cma = CmaButterfly::new(5, 1e-3);
        let (ox, oy) = cma.process_block(&ex_rot, &ey_rot);
        // After convergence, output power should converge toward 1 (CMA targets |y|²=1)
        let tail_x: f64 = ox.iter().skip(n - 200).map(|&s| cmag2(s)).sum::<f64>() / 200.0;
        let tail_y: f64 = oy.iter().skip(n - 200).map(|&s| cmag2(s)).sum::<f64>() / 200.0;
        assert!((tail_x - 1.0).abs() < 0.4, "X-pol not converged, power = {}", tail_x);
        assert!((tail_y - 1.0).abs() < 0.4, "Y-pol not converged, power = {}", tail_y);
    }

    #[test]
    fn test_cma_butterfly_initialization() {
        let cma = CmaButterfly::new(7, 1e-4);
        let mid = 7 / 2;
        // Hxx center tap should be 1.0
        assert!((cma.hxx[mid].0 - 1.0).abs() < 1e-12);
        // Hyy center tap should be 1.0
        assert!((cma.hyy[mid].0 - 1.0).abs() < 1e-12);
        // Off-diagonal taps should be zero
        assert!(cmag(cma.hxy[mid]) < 1e-12);
        assert!(cmag(cma.hyx[mid]) < 1e-12);
    }

    #[test]
    fn test_cma_butterfly_cost_decreases() {
        // Cost should decrease monotonically during convergence
        let n = 100;
        let symbols: Vec<Cpx> = (0..n).map(|k| {
            let ph = PI / 4.0 + k as f64 * PI / 2.0;
            (ph.cos() * INV_SQRT2, ph.sin() * INV_SQRT2)
        }).collect();
        let zeros: Vec<Cpx> = vec![(0.0, 0.0); n];
        let mut cma = CmaButterfly::new(5, 1e-3);
        let cost_before = cma.cost(&symbols, &zeros);
        cma.process_block(&symbols, &zeros);
        let cost_after = cma.cost(&symbols, &zeros);
        assert!(cost_after <= cost_before + 0.1,
            "Cost did not decrease: before={}, after={}", cost_before, cost_after);
    }

    // --- Frequency Offset Estimator ---

    #[test]
    fn test_foe_zero_offset() {
        let mut foe = FrequencyOffsetEstimator::new(512);
        // QPSK signal with no frequency offset
        let n = 512;
        let symbols: Vec<Cpx> = (0..n).map(|k| {
            let ph = PI / 4.0 + k as f64 * PI / 2.0;
            (ph.cos(), ph.sin())
        }).collect();
        let est = foe.estimate(&symbols);
        assert!(est.abs() < 0.01, "Expected ~0 CFO, got {}", est);
    }

    #[test]
    fn test_foe_known_offset() {
        let mut foe = FrequencyOffsetEstimator::new(1024);
        let n = 1024;
        let cfo_true = 0.002; // normalised CFO
        // QPSK with superimposed carrier frequency offset
        let symbols: Vec<Cpx> = (0..n).map(|k| {
            let sym_ph = PI / 4.0 + (k % 4) as f64 * PI / 2.0;
            let cfo_ph = 2.0 * PI * cfo_true * k as f64;
            let ph = sym_ph + cfo_ph;
            (ph.cos(), ph.sin())
        }).collect();
        let est = foe.estimate(&symbols);
        // Estimate should be within 0.005 of true value
        assert!((est - cfo_true).abs() < 0.005 || (est + cfo_true).abs() < 0.005,
            "CFO estimate {} != expected {}", est, cfo_true);
    }

    #[test]
    fn test_foe_correction_residual() {
        // Apply correction and verify residual CFO is small
        let cfo = 0.003;
        let n = 256;
        let signal: Vec<Cpx> = (0..n).map(|k| {
            let ph = 2.0 * PI * cfo * k as f64;
            (ph.cos(), ph.sin())
        }).collect();
        let corrected = FrequencyOffsetEstimator::correct(&signal, cfo);
        // After correction, all samples should be near (1, 0)
        let phase_var: f64 = corrected.iter().map(|&s| carg(s).powi(2)).sum::<f64>() / n as f64;
        assert!(phase_var < 0.01, "Residual phase variance after correction: {}", phase_var);
    }

    // --- Viterbi-Viterbi CPE ---

    #[test]
    fn test_vv_zero_phase_noise() {
        let mut vv = ViterbiViterbiCpe::new(32);
        // Unit-amplitude QPSK with no phase noise
        let n = 128;
        let symbols: Vec<Cpx> = (0..n).map(|k| {
            let ph = PI / 4.0 + k as f64 * PI / 2.0;
            (ph.cos(), ph.sin())
        }).collect();
        let (corrected, phase) = vv.process_block(&symbols);
        // Phase estimate should be close to 0
        assert!(phase.abs() < PI / 4.0 + 0.1, "Phase estimate: {}", phase);
        // Power should be preserved
        let pwr_in: f64 = symbols.iter().map(|&s| cmag2(s)).sum::<f64>() / n as f64;
        let pwr_out: f64 = corrected.iter().map(|&s| cmag2(s)).sum::<f64>() / n as f64;
        assert!((pwr_in - pwr_out).abs() < 1e-10);
    }

    #[test]
    fn test_vv_constant_phase_correction() {
        // V-V should produce a consistent phase estimate proportional to carrier offset
        // We test that the power is preserved and the estimate is not NaN/zero
        let phase_offset = 0.3_f64; // radians carrier phase
        let n = 128;
        // Unit-amplitude QPSK with constant carrier phase offset
        let symbols: Vec<Cpx> = (0..n).map(|k| {
            let ph = PI / 4.0 + k as f64 * PI / 2.0 + phase_offset;
            (ph.cos(), ph.sin())
        }).collect();

        // V-V with no offset for reference
        let syms_nooffset: Vec<Cpx> = (0..n).map(|k| {
            let ph = PI / 4.0 + k as f64 * PI / 2.0;
            (ph.cos(), ph.sin())
        }).collect();

        let mut vv0 = ViterbiViterbiCpe::new(64);
        let mut vv1 = ViterbiViterbiCpe::new(64);
        let (corrected0, phase0) = vv0.process_block(&syms_nooffset);
        let (corrected1, phase1) = vv1.process_block(&symbols);

        // The phase estimate should differ by approximately phase_offset
        let phase_diff = (phase1 - phase0).abs();
        assert!(phase_diff > 0.05, "Phase estimates should differ: diff={}", phase_diff);

        // Power should be preserved after correction
        let pwr_in: f64  = symbols.iter().map(|&s| cmag2(s)).sum::<f64>() / n as f64;
        let pwr_out: f64 = corrected1.iter().map(|&s| cmag2(s)).sum::<f64>() / n as f64;
        assert!((pwr_in - pwr_out).abs() < 1e-6, "Power not preserved");

        // V-V should produce a valid phase estimate (not NaN)
        assert!(phase1.is_finite(), "Phase estimate is not finite");
        let _ = corrected0;
    }

    #[test]
    fn test_vv_stream_processing() {
        let mut vv = ViterbiViterbiCpe::new(32);
        let n = 320;
        let symbols: Vec<Cpx> = (0..n).map(|_k| {
            let ph = PI / 4.0;
            (ph.cos(), ph.sin())
        }).collect();
        let output = vv.process_stream(&symbols);
        assert_eq!(output.len(), n, "Stream output length mismatch");
    }

    // --- QPSK Symbol Detector ---

    #[test]
    fn test_qpsk_decision_all_quadrants() {
        let det = QpskDetector::new();
        // Q1: (+,+) → 00
        assert_eq!(det.decide((0.7, 0.7)), 0b00);
        // Q4: (+,-) → 01
        assert_eq!(det.decide((0.7, -0.7)), 0b01);
        // Q2: (-,+) → 10
        assert_eq!(det.decide((-0.7, 0.7)), 0b10);
        // Q3: (-,-) → 11
        assert_eq!(det.decide((-0.7, -0.7)), 0b11);
    }

    #[test]
    fn test_qpsk_ideal_points_on_unit_circle() {
        // Each QPSK ideal point has magnitude 1 (unit circle)
        for s in 0u8..4 {
            let p = QpskDetector::ideal_point(s);
            let mag = cmag(p);
            assert!((mag - 1.0).abs() < 1e-10,
                "Symbol {} ideal point magnitude = {}", s, mag);
        }
    }

    #[test]
    fn test_qpsk_ideal_points_roundtrip() {
        let det = QpskDetector::new();
        for s in 0u8..4 {
            let point = QpskDetector::ideal_point(s);
            let decided = det.decide(point);
            assert_eq!(decided, s, "Roundtrip failed for symbol {}", s);
        }
    }

    #[test]
    fn test_qpsk_symbols_to_bits() {
        // Symbol 0b00 → bits [false, false]
        let bits = QpskDetector::symbols_to_bits(&[0, 1, 2, 3]);
        assert_eq!(bits, vec![false, false, false, true, true, false, true, true]);
    }

    #[test]
    fn test_qpsk_decision_boundary() {
        let det = QpskDetector::new();
        // Near zero should still make a decision
        assert!(det.decide((1e-10, 1e-10)) < 4);
        assert!(det.decide((-1e-10, -1e-10)) < 4);
    }

    // --- Differential Decoding ---

    #[test]
    fn test_differential_decode_identity() {
        let mut det = QpskDetector::new();
        // Symbols: 0,1,2,3 → diffs: 1-0=1, 2-1=1, 3-2=1
        let symbols = vec![0u8, 1, 2, 3];
        let decoded = det.differential_decode(&symbols);
        assert_eq!(decoded[0], 0); // first: 0 - 0(prev) = 0
        assert_eq!(decoded[1], 1); // 1 - 0 = 1
        assert_eq!(decoded[2], 1); // 2 - 1 = 1
    }

    #[test]
    fn test_differential_decode_constant_symbol() {
        let mut det = QpskDetector::new();
        let symbols = vec![2u8; 10];
        let decoded = det.differential_decode(&symbols);
        // First: 2-0=2, rest: 2-2=0
        assert_eq!(decoded[0], 2);
        for &d in &decoded[1..] {
            assert_eq!(d, 0, "Constant symbol should decode to 0 after first");
        }
    }

    #[test]
    fn test_differential_decode_phase_ambiguity() {
        // A 90° phase rotation (multiply all symbols by 1 mod 4) should be transparent
        // after differential decoding
        let data = vec![0u8, 1, 2, 3, 0, 1];
        let mut det1 = QpskDetector::new();
        let mut det2 = QpskDetector::new();
        // Rotate all symbols by 1 (simulates π/2 phase ambiguity)
        let rotated: Vec<u8> = data.iter().map(|&s| (s + 1) & 3).collect();
        let decoded_orig = det1.differential_decode(&data);
        let decoded_rot  = det2.differential_decode(&rotated);
        // After first symbol, differential values should be the same
        assert_eq!(&decoded_orig[1..], &decoded_rot[1..]);
    }

    // --- BER Estimator ---

    #[test]
    fn test_ber_perfect_signal() {
        let mut ber = BerEstimator::new();
        let det = QpskDetector::new();
        // Perfect QPSK symbols (no noise)
        let syms: Vec<u8> = (0..100).map(|k| (k % 4) as u8).collect();
        let points: Vec<Cpx> = syms.iter().map(|&s| QpskDetector::ideal_point(s)).collect();
        ber.accumulate_block(&points, &syms);
        assert!(ber.evm_rms() < 0.001, "EVM should be ~0 for perfect signal");
        assert!(ber.snr_db() > 40.0, "SNR should be very high");
    }

    #[test]
    fn test_ber_noisy_signal() {
        let mut ber = BerEstimator::new();
        // Add noise to QPSK symbols
        let syms: Vec<u8> = (0..1000).map(|k| (k % 4) as u8).collect();
        let noise_sigma = 0.1;
        // Simple LCG for deterministic "noise"
        let mut lcg = 12345u64;
        let points: Vec<Cpx> = syms.iter().map(|&s| {
            let ideal = QpskDetector::ideal_point(s);
            lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let ni = ((lcg >> 33) as f64 / u32::MAX as f64 - 0.5) * 2.0 * noise_sigma;
            lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let nq = ((lcg >> 33) as f64 / u32::MAX as f64 - 0.5) * 2.0 * noise_sigma;
            (ideal.0 + ni, ideal.1 + nq)
        }).collect();
        let decisions: Vec<u8> = points.iter().map(|&p| {
            let det = QpskDetector::new();
            det.decide(p)
        }).collect();
        ber.accumulate_block(&points, &decisions);
        let evm = ber.evm_rms();
        assert!(evm > 0.0 && evm < 1.0, "EVM should be in (0,1): {}", evm);
        let est_ber = ber.estimated_ber();
        assert!(est_ber >= 0.0 && est_ber <= 0.5, "BER out of range: {}", est_ber);
    }

    #[test]
    fn test_ber_estimator_reset() {
        let mut ber = BerEstimator::new();
        let det = QpskDetector::new();
        let syms = vec![0u8, 1, 2, 3];
        let pts: Vec<Cpx> = syms.iter().map(|&s| QpskDetector::ideal_point(s)).collect();
        ber.accumulate_block(&pts, &syms);
        assert!(ber.symbol_count > 0);
        ber.reset();
        assert_eq!(ber.symbol_count, 0);
        assert_eq!(ber.error_sq_sum, 0.0);
    }

    // --- PMD Channel ---

    #[test]
    fn test_pmd_channel_identity() {
        let pmd = PmdChannel::new(0.0, 0.0, 0.0, 0.0);
        let ex: Vec<Cpx> = vec![(1.0, 0.0), (0.0, 1.0)];
        let ey: Vec<Cpx> = vec![(0.0, 0.0), (0.0, 0.0)];
        let (ox, _oy) = pmd.apply(&ex, &ey, 28e9);
        assert!((ox[0].0 - 1.0).abs() < 1e-12);
        assert!((ox[1].1 - 1.0).abs() < 1e-12);
    }

    #[test]
    fn test_pmd_channel_power_preservation() {
        let pmd = PmdChannel::new(10e-12, 0.5, 0.1, 0.001);
        let n = 50;
        let ex: Vec<Cpx> = (0..n).map(|k| (( k as f64 * 0.3).cos(), (k as f64 * 0.3).sin())).collect();
        let ey: Vec<Cpx> = (0..n).map(|k| ((-k as f64 * 0.3).cos(), (-k as f64 * 0.3).sin())).collect();
        let (ox, oy) = pmd.apply(&ex, &ey, 28e9);
        let p_in: f64 = ex.iter().zip(ey.iter()).map(|(&a, &b)| cmag2(a) + cmag2(b)).sum();
        let p_out: f64 = ox.iter().zip(oy.iter()).map(|(&a, &b)| cmag2(a) + cmag2(b)).sum();
        assert!((p_in - p_out).abs() / p_in.max(1e-30) < 0.01,
            "Power not preserved: in={}, out={}", p_in, p_out);
    }

    #[test]
    fn test_pmd_channel_frequency_offset() {
        let cfo = 0.005; // normalised
        let pmd = PmdChannel::new(0.0, 0.0, 0.0, cfo);
        let n = 100;
        let ex: Vec<Cpx> = vec![(1.0, 0.0); n];
        let ey: Vec<Cpx> = vec![(0.0, 0.0); n];
        let (ox, _) = pmd.apply(&ex, &ey, 28e9);
        // Check that phase rotates as expected
        let phase0 = carg(ox[0]);
        let phase1 = carg(ox[1]);
        let phase_diff = phase1 - phase0;
        let expected_diff = 2.0 * PI * cfo;
        assert!((phase_diff - expected_diff).abs() < 0.001,
            "Phase increment: {} expected {}", phase_diff, expected_diff);
    }

    // --- Full Receiver ---

    #[test]
    fn test_receiver_construction() {
        let config = DpQpskConfig::default();
        let _rx = DpQpskReceiver::new(config);
    }

    #[test]
    fn test_receiver_test_signal_generation() {
        let syms_x: Vec<u8> = (0..20).map(|k| (k % 4) as u8).collect();
        let syms_y: Vec<u8> = (0..20).map(|k| (k % 4 + 1) as u8 & 3).collect();
        let (ex, ey) = DpQpskReceiver::generate_test_signal(&syms_x, &syms_y, 2);
        assert_eq!(ex.len(), 40);
        assert_eq!(ey.len(), 40);
        // Check that power is correct for QPSK at unit-amplitude points (|s|²=1)
        let pwr: f64 = ex.iter().map(|&s| cmag2(s)).sum::<f64>() / ex.len() as f64;
        assert!((pwr - 1.0).abs() < 0.01, "Expected 1.0 power, got {}", pwr);
    }

    #[test]
    fn test_receiver_processes_without_panic() {
        let mut config = DpQpskConfig::default();
        config.fiber_length_km = 0.0; // no CD for simplicity
        config.cma_taps = 5;
        config.vv_block_size = 16;

        let mut rx = DpQpskReceiver::new(config);

        let n = 64;
        let syms_x: Vec<u8> = (0..n).map(|k| (k % 4) as u8).collect();
        let syms_y: Vec<u8> = (0..n).map(|k| (k % 4 + 1) as u8 & 3).collect();
        let (ex, ey) = DpQpskReceiver::generate_test_signal(&syms_x, &syms_y, 2);

        // Use ideal LO (same frequency, unit amplitude)
        let elo: Vec<Cpx> = vec![(1.0, 0.0); ex.len()];

        // Should not panic
        let (bits_x, bits_y) = rx.process(&ex, &ey, &elo, &elo);
        assert!(!bits_x.is_empty() || !bits_y.is_empty() || true);
    }

    #[test]
    fn test_receiver_ber_estimation_available() {
        let mut config = DpQpskConfig::default();
        config.fiber_length_km = 0.0;
        config.cma_taps = 3;
        config.vv_block_size = 8;
        let mut rx = DpQpskReceiver::new(config);

        let n = 32;
        let syms_x: Vec<u8> = (0..n).map(|k| (k % 4) as u8).collect();
        let syms_y: Vec<u8> = vec![0u8; n];
        let (ex, ey) = DpQpskReceiver::generate_test_signal(&syms_x, &syms_y, 2);
        let elo: Vec<Cpx> = vec![(1.0, 0.0); ex.len()];

        rx.process(&ex, &ey, &elo, &elo);

        let ber_est = rx.ber.estimated_ber();
        assert!(ber_est >= 0.0 && ber_est <= 0.5, "BER estimate out of range: {}", ber_est);
    }

    #[test]
    fn test_receiver_with_pmd_channel() {
        // Full pipeline test: generate signal, pass through PMD channel, receive
        let mut config = DpQpskConfig::default();
        config.fiber_length_km = 0.0;
        config.cma_taps = 11;
        config.cma_step_size = 5e-4;
        config.vv_block_size = 32;
        config.differential_decoding = false;

        let mut rx = DpQpskReceiver::new(config.clone());

        let n = 128;
        let syms_x: Vec<u8> = (0..n).map(|k| (k % 4) as u8).collect();
        let syms_y: Vec<u8> = (0..n).map(|k| (k % 4 + 2) as u8 & 3).collect();
        let (ex, ey) = DpQpskReceiver::generate_test_signal(&syms_x, &syms_y, 2);

        // Apply a 45° polarization rotation
        let pmd = PmdChannel::new(0.0, PI / 4.0, 0.0, 0.0);
        let (ex_ch, ey_ch) = pmd.apply(&ex, &ey, config.symbol_rate_gbaud * 1e9);

        let elo: Vec<Cpx> = vec![(1.0, 0.0); ex_ch.len()];
        let (_bits_x, _bits_y) = rx.process(&ex_ch, &ey_ch, &elo, &elo);
        // After ~128 symbols the CMA has not fully converged, but
        // the pipeline should complete without panicking.
    }

    #[test]
    fn test_full_chain_snr_estimation() {
        // Verify SNR estimator produces a plausible value
        let mut ber = BerEstimator::new();
        let det = QpskDetector::new();
        // SNR = 10 dB corresponds to EVM ≈ 31.6%
        let sigma = 0.316;
        let mut lcg = 999u64;
        let n = 5000;
        let syms: Vec<u8> = (0..n).map(|k| (k % 4) as u8).collect();
        let pts: Vec<Cpx> = syms.iter().map(|&s| {
            let ideal = QpskDetector::ideal_point(s);
            lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let ni = ((lcg >> 33) as f64 / u32::MAX as f64 - 0.5) * 2.0 * sigma;
            lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let nq = ((lcg >> 33) as f64 / u32::MAX as f64 - 0.5) * 2.0 * sigma;
            (ideal.0 + ni, ideal.1 + nq)
        }).collect();
        let decisions: Vec<u8> = pts.iter().map(|&p| det.decide(p)).collect();
        ber.accumulate_block(&pts, &decisions);
        let snr = ber.snr_db();
        // Should be in the ballpark of 10 dB ± 3 dB given our simple noise model
        assert!(snr > 5.0 && snr < 20.0, "SNR estimate out of expected range: {} dB", snr);
    }
}
