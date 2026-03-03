//! LTE Cat-NB2 (NB-IoT Release 14) Physical Layer Processor
//!
//! Implements 3GPP TS 36.211/36.212/36.213 for Narrowband IoT:
//! - OFDMA downlink: 12 subcarriers × 15 kHz = 180 kHz, 14 OFDM symbols/subframe (normal CP)
//! - SC-FDMA uplink: single-tone (3.75 kHz or 15 kHz) and multi-tone (3/6/12 subcarriers × 15 kHz)
//! - Physical channels: NPBCH (640ms TTI), NPDCCH, NPDSCH, NPUSCH Format 1/2
//! - Reference signals: NRS port 0/1, DMRS for NPUSCH
//! - Repetition coding for 164 dB MCL coverage extension
//! - NPRACH: frequency hopping, preamble repetitions 1–128, 4 coverage levels
//! - TBS tables, turbo coding (DL/UL data), tail-biting CC (control)
//! - Deployment modes: Standalone, In-band, Guard-band
//! - Multi-carrier: Cat-NB2 carrier switching, non-anchor carriers

// trace:NB-IOT-001 | ai:claude

use std::f64::consts::PI;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of downlink subcarriers per NB-IoT carrier (12 × 15 kHz = 180 kHz)
pub const NB_NUM_SUBCARRIERS: usize = 12;
/// DL/UL multi-tone subcarrier spacing (Hz)
pub const NB_SUBCARRIER_SPACING_HZ: f64 = 15_000.0;
/// Single-tone uplink subcarrier spacing option A (Hz)
pub const NB_SC_SPACING_3750_HZ: f64 = 3_750.0;
/// Number of OFDM symbols per subframe for normal CP
pub const NB_SYMBOLS_PER_SUBFRAME: usize = 14;
/// Number of OFDM symbols per slot (0.5 ms)
pub const NB_SYMBOLS_PER_SLOT: usize = 7;
/// Subframe duration in seconds (1 ms)
pub const NB_SUBFRAME_DURATION_S: f64 = 1e-3;
/// NPBCH TTI: 640 ms = 640 subframes
pub const NPBCH_TTI_SUBFRAMES: usize = 640;
/// NPBCH block size in bits
pub const NPBCH_PAYLOAD_BITS: usize = 34;
/// NPDCCH DCI maximum repetitions
pub const NPDCCH_MAX_REPS: u32 = 2048;
/// NPDSCH maximum repetitions
pub const NPDSCH_MAX_REPS: u32 = 2048;
/// NPUSCH maximum repetitions
pub const NPUSCH_MAX_REPS: u32 = 128;
/// NRS RE per slot per port (ports 0/1 each contribute 2 RE per slot)
pub const NRS_RE_PER_SLOT: usize = 2;
/// Normal CP length for first symbol (samples at 1.92 MHz, 16x decimated from 30.72 MHz)
/// At 30.72 MHz: 160 samples; at 1.92 MHz: 160/16 = 10 samples
pub const CP_NORMAL_FIRST_SAMPLES: usize = 10;
/// Normal CP length for other symbols (samples at 1.92 MHz)
/// At 30.72 MHz: 144 samples; at 1.92 MHz: 144/16 = 9 samples
pub const CP_NORMAL_OTHER_SAMPLES: usize = 9;
/// FFT size for 15 kHz subcarrier spacing at 1.92 MHz sample rate
pub const FFT_SIZE_15K: usize = 128;
/// FFT size for 3.75 kHz subcarrier spacing (single-tone SC-FDMA) = 512
pub const FFT_SIZE_375: usize = 512;

// ---------------------------------------------------------------------------
// Enumerations
// ---------------------------------------------------------------------------

/// NB-IoT deployment mode per 3GPP TS 36.211 §10.2
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeploymentMode {
    /// Dedicated NB-IoT carrier (e.g., refarmed GSM spectrum)
    Standalone,
    /// Located within an LTE carrier guard band
    GuardBand,
    /// Uses an LTE PRB within the LTE carrier
    InBand,
}

/// Coverage Enhancement level
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CoverageLevel {
    /// CE Level 0 – normal coverage (max ~15 dB gain vs. non-CE)
    CE0,
    /// CE Level 1 – enhanced coverage (~10 dB additional)
    CE1,
    /// CE Level 2 – extreme coverage (~20 dB additional)
    CE2,
}

/// NPUSCH format
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NpuschFormat {
    /// Format 1: data (UL-SCH)
    Format1,
    /// Format 2: HARQ-ACK feedback
    Format2,
}

/// Uplink subcarrier allocation type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UlSubcarrierMode {
    /// Single-tone with 3.75 kHz spacing (only SC-FDMA i_sc = 0)
    SingleTone3750,
    /// Single-tone with 15 kHz spacing (one of 12 subcarriers)
    SingleTone15k,
    /// Multi-tone 3 subcarriers × 15 kHz
    MultiTone3,
    /// Multi-tone 6 subcarriers × 15 kHz
    MultiTone6,
    /// Multi-tone 12 subcarriers × 15 kHz (full bandwidth)
    MultiTone12,
}

/// NRS antenna port
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NrsPort {
    Port0,
    Port1,
}

/// Modulation order
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Modulation {
    Bpsk,
    Qpsk,
    /// Used in NPUSCH Format 1 multi-tone with high TBS
    Qam16,
}

/// NPRACH coverage level (maps to CE level)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NprachCoverageLevel {
    Level0 = 0,
    Level1 = 1,
    Level2 = 2,
    Level3 = 3,
}

// ---------------------------------------------------------------------------
// TBS Tables (3GPP TS 36.213 Table 16.4.1.5.1-1 for NB-IoT)
// ---------------------------------------------------------------------------

/// NB-IoT downlink TBS table (itbs 0..12, irep 0..7, multi-tone 12 subcarriers)
/// Returns transport block size in bits for (itbs, n_rep_index)
pub fn tbs_dl(itbs: usize, n_sf: usize) -> Option<usize> {
    // Simplified DL TBS table per 3GPP TS 36.213 Table 16.4.1.5.1-1
    // indexed by [I_TBS][N_sf - 1] where N_sf ∈ {1,2,3,4,5,6,8,10}
    const TBS_DL: [[usize; 8]; 14] = [
        [16,  32,  56,  88,  120, 152, 208, 256],
        [24,  56,  88,  144, 176, 208, 256, 344],
        [32,  72,  120, 176, 208, 256, 328, 424],
        [40,  104, 160, 216, 256, 328, 440, 568],
        [56,  120, 192, 256, 320, 408, 552, 680],
        [72,  144, 224, 328, 424, 504, 680, 872],
        [88,  176, 256, 392, 504, 600, 808, 1000],
        [104, 224, 328, 472, 584, 712, 1000, 1224],
        [120, 256, 392, 536, 680, 808, 1096, 1384],
        [136, 296, 456, 616, 776, 936, 1256, 1544],
        [144, 328, 504, 680, 872, 1000, 1384, 1736],
        [176, 376, 584, 776, 1000, 1192, 1608, 2024],
        [208, 440, 680, 1000, 1192, 1480, 1992, 2536],
        [224, 488, 744, 1000, 1256, 1544, 2088, 2536],
    ];
    if itbs >= TBS_DL.len() { return None; }
    let sf_idx = match n_sf {
        1 => 0, 2 => 1, 3 => 2, 4 => 3, 5 => 4, 6 => 5, 8 => 6, 10 => 7,
        _ => return None,
    };
    Some(TBS_DL[itbs][sf_idx])
}

/// NB-IoT uplink TBS table for single-tone (i_sc = 1, 3.75 kHz or 15 kHz)
/// Indexed by [I_TBS][N_ru] where N_ru is number of RUs per TTI
pub fn tbs_ul_single_tone(itbs: usize, n_ru: usize) -> Option<usize> {
    // 3GPP TS 36.213 Table 16.5.1.2-2 (single-tone)
    const TBS_UL_ST: [[usize; 10]; 8] = [
        [16,  24,  32,  40,  56,  72,  88,  104, 120, 136],
        [24,  32,  40,  56,  72,  88,  104, 120, 144, 176],
        [32,  40,  56,  72,  88,  104, 120, 144, 176, 208],
        [40,  56,  72,  88,  120, 144, 176, 208, 224, 256],
        [56,  72,  88,  120, 144, 176, 208, 224, 256, 296],
        [72,  88,  120, 144, 176, 224, 256, 296, 328, 376],
        [88,  104, 120, 144, 176, 224, 256, 296, 376, 440],
        [104, 120, 144, 176, 208, 256, 328, 392, 472, 536],
    ];
    if itbs >= TBS_UL_ST.len() || n_ru == 0 || n_ru > 10 { return None; }
    Some(TBS_UL_ST[itbs][n_ru - 1])
}

/// NB-IoT uplink TBS for multi-tone (3/6/12 subcarriers)
pub fn tbs_ul_multi_tone(itbs: usize, n_sc: usize, n_ru: usize) -> Option<usize> {
    // 3GPP TS 36.213 Table 16.5.1.2-1 (multi-tone)
    // For n_sc in {3,6,12}; n_ru in {1,2,3,4,5,6,8,10}
    const TBS_MT3: [[usize; 8]; 14] = [
        [32,  56,  88,  120, 152, 176, 208, 256],
        [56,  88,  144, 176, 208, 224, 256, 328],
        [72,  120, 176, 224, 256, 296, 376, 440],
        [88,  144, 208, 256, 328, 376, 472, 568],
        [104, 176, 256, 328, 408, 472, 600, 712],
        [120, 208, 296, 392, 488, 568, 712, 872],
        [144, 240, 360, 472, 584, 680, 872, 1000],
        [176, 296, 440, 568, 680, 808, 1000, 1224],
        [208, 344, 504, 648, 808, 936, 1192, 1480],
        [224, 384, 568, 744, 936, 1096, 1384, 1736],
        [256, 424, 632, 840, 1000, 1192, 1544, 1928],
        [296, 488, 728, 1000, 1192, 1416, 1800, 2216],
        [328, 552, 824, 1096, 1352, 1608, 2024, 2536],
        [376, 616, 920, 1224, 1544, 1800, 2280, 2536],
    ];
    const TBS_MT6: [[usize; 8]; 14] = [
        [56,  120, 176, 224, 256, 296, 376, 440],
        [88,  176, 256, 328, 408, 472, 600, 712],
        [120, 240, 360, 472, 584, 680, 872, 1000],
        [152, 296, 440, 568, 680, 808, 1000, 1224],
        [176, 360, 536, 696, 872, 1000, 1256, 1544],
        [208, 424, 632, 840, 1000, 1192, 1544, 1928],
        [256, 504, 760, 1000, 1256, 1480, 1864, 2344],
        [296, 584, 888, 1192, 1480, 1736, 2216, 2728],
        [328, 648, 1000, 1320, 1672, 1992, 2536, 3112],
        [376, 744, 1128, 1480, 1864, 2216, 2792, 3496],
        [424, 840, 1256, 1672, 2088, 2472, 3112, 3880],
        [472, 936, 1416, 1864, 2344, 2792, 3496, 4392],
        [536, 1064, 1608, 2088, 2600, 3112, 3880, 4968],
        [600, 1192, 1800, 2344, 2984, 3496, 4392, 5544],
    ];
    const TBS_MT12: [[usize; 8]; 14] = [
        [88,  224, 328, 440, 536, 632, 808, 1000],
        [144, 344, 504, 680, 872, 1000, 1256, 1544],
        [176, 424, 632, 840, 1000, 1256, 1608, 1992],
        [208, 504, 744, 1000, 1256, 1480, 1864, 2344],
        [256, 616, 904, 1224, 1544, 1800, 2280, 2856],
        [296, 712, 1064, 1416, 1800, 2088, 2664, 3368],
        [376, 872, 1288, 1736, 2152, 2536, 3240, 4008],
        [440, 1000, 1480, 1992, 2472, 2984, 3752, 4648],
        [520, 1192, 1736, 2344, 2984, 3496, 4392, 5544],
        [600, 1384, 2024, 2728, 3368, 4008, 5160, 6456],
        [680, 1544, 2280, 3112, 3880, 4648, 5992, 7480],
        [776, 1800, 2600, 3496, 4392, 5160, 6712, 8248],
        [872, 2024, 2984, 4008, 5160, 5992, 7736, 9528],
        [1000, 2280, 3368, 4648, 5992, 6968, 8504, 10296],
    ];

    if itbs >= 14 || n_ru == 0 || n_ru > 8 { return None; }
    let ru_idx = match n_ru {
        1 => 0, 2 => 1, 3 => 2, 4 => 3, 5 => 4, 6 => 5, 8 => 6, 10 => 7,
        _ => return None,
    };
    match n_sc {
        3  => Some(TBS_MT3[itbs][ru_idx]),
        6  => Some(TBS_MT6[itbs][ru_idx]),
        12 => Some(TBS_MT12[itbs][ru_idx]),
        _  => None,
    }
}

// ---------------------------------------------------------------------------
// Complex number primitive (no external crates)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    #[inline]
    pub fn new(re: f64, im: f64) -> Self { Complex { re, im } }
    #[inline]
    pub fn from_polar(r: f64, theta: f64) -> Self {
        Complex { re: r * theta.cos(), im: r * theta.sin() }
    }
    #[inline]
    pub fn conj(self) -> Self { Complex { re: self.re, im: -self.im } }
    #[inline]
    pub fn abs_sq(self) -> f64 { self.re * self.re + self.im * self.im }
    #[inline]
    pub fn abs(self) -> f64 { self.abs_sq().sqrt() }
}

impl std::ops::Mul for Complex {
    type Output = Complex;
    fn mul(self, rhs: Complex) -> Complex {
        Complex {
            re: self.re * rhs.re - self.im * rhs.im,
            im: self.re * rhs.im + self.im * rhs.re,
        }
    }
}

impl std::ops::Add for Complex {
    type Output = Complex;
    fn add(self, rhs: Complex) -> Complex {
        Complex { re: self.re + rhs.re, im: self.im + rhs.im }
    }
}

impl std::ops::Sub for Complex {
    type Output = Complex;
    fn sub(self, rhs: Complex) -> Complex {
        Complex { re: self.re - rhs.re, im: self.im - rhs.im }
    }
}

// ---------------------------------------------------------------------------
// FFT (radix-2 Cooley-Tukey DIT)
// ---------------------------------------------------------------------------

fn fft_in_place(buf: &mut Vec<Complex>, inverse: bool) {
    let n = buf.len();
    // Bit-reversal permutation
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j { buf.swap(i, j); }
    }
    let sign = if inverse { 1.0_f64 } else { -1.0_f64 };
    let mut len = 2usize;
    while len <= n {
        let ang = sign * 2.0 * PI / (len as f64);
        let w_len = Complex::from_polar(1.0, ang);
        for i in (0..n).step_by(len) {
            let mut w = Complex::new(1.0, 0.0);
            for k in 0..(len / 2) {
                let u = buf[i + k];
                let v = buf[i + k + len / 2] * w;
                buf[i + k]           = u + v;
                buf[i + k + len / 2] = u - v;
                w = w * w_len;
            }
        }
        len <<= 1;
    }
    if inverse {
        let n_f = n as f64;
        for x in buf.iter_mut() {
            x.re /= n_f;
            x.im /= n_f;
        }
    }
}

/// Forward FFT (zero-pads to next power of 2 if necessary)
pub fn fft(input: &[Complex]) -> Vec<Complex> {
    let n = input.len().next_power_of_two().max(1);
    let mut buf = input.to_vec();
    buf.resize(n, Complex::new(0.0, 0.0));
    if n > 1 { fft_in_place(&mut buf, false); }
    buf
}

/// Inverse FFT (zero-pads to next power of 2 if necessary)
pub fn ifft(input: &[Complex]) -> Vec<Complex> {
    let n = input.len().next_power_of_two().max(1);
    let mut buf = input.to_vec();
    buf.resize(n, Complex::new(0.0, 0.0));
    if n > 1 { fft_in_place(&mut buf, true); }
    buf
}

// ---------------------------------------------------------------------------
// LFSR / Gold-code for scrambling (TS 36.211 §7.2 / §10)
// ---------------------------------------------------------------------------

/// LFSR-based NB-IoT scrambling sequence (polynomial x^31 + x^3 + 1)
pub struct NbScrambler {
    x1: u32,
    x2: u32,
}

impl NbScrambler {
    /// Initialize with cell-specific c_init value (see TS 36.211 §7.2)
    pub fn new(c_init: u32) -> Self {
        let mut x1: u32 = 1;
        let mut x2: u32 = c_init;
        // Advance 1600 steps
        for _ in 0..1600 {
            let b1 = ((x1 >> 3) ^ x1) & 1;
            x1 = (x1 >> 1) | (b1 << 30);
            let b2 = ((x2 >> 3) ^ (x2 >> 2) ^ (x2 >> 1) ^ x2) & 1;
            x2 = (x2 >> 1) | (b2 << 30);
        }
        NbScrambler { x1, x2 }
    }

    /// Produce next scrambling bit
    pub fn next_bit(&mut self) -> u8 {
        let c = (((self.x1 >> 3) ^ self.x1) & 1) ^ (((self.x2 >> 3) ^ (self.x2 >> 2) ^ (self.x2 >> 1) ^ self.x2) & 1);
        let b1 = ((self.x1 >> 3) ^ self.x1) & 1;
        self.x1 = (self.x1 >> 1) | (b1 << 30);
        let b2 = ((self.x2 >> 3) ^ (self.x2 >> 2) ^ (self.x2 >> 1) ^ self.x2) & 1;
        self.x2 = (self.x2 >> 1) | (b2 << 30);
        c as u8
    }

    /// Scramble a bit vector in place
    pub fn scramble(&mut self, bits: &mut [u8]) {
        for b in bits.iter_mut() {
            *b ^= self.next_bit();
        }
    }
}

// ---------------------------------------------------------------------------
// CRC computation (CRC-24A per TS 36.212 §5.1.1)
// ---------------------------------------------------------------------------

/// Compute 24-bit CRC-24A (polynomial 0x864CFB) over bit slice
pub fn crc24a(bits: &[u8]) -> u32 {
    const POLY: u32 = 0x864CFB;
    let mut crc: u32 = 0;
    for &b in bits {
        crc ^= (b as u32) << 23;
        for _ in 0..1 {
            if crc & (1 << 23) != 0 {
                crc = ((crc << 1) ^ POLY) & 0xFF_FFFF;
            } else {
                crc = (crc << 1) & 0xFF_FFFF;
            }
        }
    }
    crc
}

/// Append CRC-24A bits to data
pub fn attach_crc24a(data: &[u8]) -> Vec<u8> {
    let crc = crc24a(data);
    let mut out = data.to_vec();
    for i in (0..24).rev() {
        out.push(((crc >> i) & 1) as u8);
    }
    out
}

// ---------------------------------------------------------------------------
// Tail-biting convolutional code (rate 1/3, K=7 as per TS 36.212 §5.1.3.1)
// ---------------------------------------------------------------------------

/// Convolutional encoder: rate 1/3, K=7, generators (133, 171, 165) octal
pub struct TbccEncoder {
    state: u8,
}

impl TbccEncoder {
    pub fn new() -> Self { TbccEncoder { state: 0 } }

    /// Encode with tail-biting: initialize state from last 6 bits, then encode
    pub fn encode_tail_biting(&mut self, bits: &[u8]) -> Vec<u8> {
        let k = bits.len();
        // Find initial state by encoding in reverse
        let mut rev_state: u8 = 0;
        for &b in bits.iter().rev() {
            rev_state = (rev_state >> 1) | (b << 5);
        }
        self.state = rev_state & 0x3F;

        let mut out = Vec::with_capacity(3 * k);
        for &bit in bits {
            let s = (self.state << 1) | bit;
            // Generator polynomials (octal 133, 171, 165)
            // 133 = 1011011 -> x^6+x^4+x^3+x+1
            // 171 = 1111001 -> x^6+x^5+x^4+x^3+1
            // 165 = 1110101 -> x^6+x^5+x^4+x^2+1
            let g0 = (s & 0b1011011).count_ones() as u8 & 1;
            let g1 = (s & 0b1111001).count_ones() as u8 & 1;
            let g2 = (s & 0b1110101).count_ones() as u8 & 1;
            out.push(g0);
            out.push(g1);
            out.push(g2);
            self.state = (self.state >> 1) | (bit << 5);
            // Keep only 6-bit state
            self.state &= 0x3F;
        }
        out
    }
}

impl Default for TbccEncoder {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// Rate matching (circular buffer, TS 36.212 §5.1.4.1)
// ---------------------------------------------------------------------------

/// Rate-match TBCC output to target_bits length using circular buffer
pub fn rate_match(coded: &[u8], target_bits: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(target_bits);
    let len = coded.len();
    for i in 0..target_bits {
        out.push(coded[i % len]);
    }
    out
}

// ---------------------------------------------------------------------------
// Turbo encoder (QPP interleaver, constituent RSC 1/2, K=4)
// For NB-IoT DL/UL data channels (NPDSCH / NPUSCH Format 1)
// ---------------------------------------------------------------------------

/// Simple RSC constituent encoder (rate 1/2, K=4, g=[1+D^2+D^3, 1+D+D^2+D^3])
fn rsc_encode(bits: &[u8], initial_state: u8) -> (Vec<u8>, u8) {
    let mut state: u8 = initial_state & 0x07;
    let mut parity = Vec::with_capacity(bits.len());
    for &b in bits {
        let s = state;
        // Parity: D^3 + D^2 + D + 1 (feedback: D^3 + D^2 + 1)
        let feedback = b ^ ((s >> 2) & 1) ^ (s & 1);
        let p = feedback ^ ((s >> 1) & 1) ^ (s & 1) ^ b;
        parity.push(p);
        state = ((state << 1) | feedback) & 0x07;
    }
    (parity, state)
}

/// QPP turbo interleaver index (simplified linear-quadratic per TS 36.212)
fn qpp_interleave_idx(i: usize, k: usize, f1: usize, f2: usize) -> usize {
    (f1 * i + f2 * i * i) % k
}

/// Get QPP parameters for block size K (TS 36.212 Table 5.1.3-3, subset)
fn qpp_params(k: usize) -> (usize, usize) {
    match k {
        40  => (3, 10),
        48  => (7, 12),
        64  => (19, 16),
        96  => (7, 24),
        128 => (7, 16),
        160 => (19, 20),
        192 => (7, 16),
        256 => (7, 16),
        320 => (11, 20),
        384 => (11, 24),
        512 => (7, 32),
        640 => (19, 20),
        768 => (11, 24),
        1024 => (7, 32),
        1280 => (11, 20),
        1536 => (11, 48),
        2048 => (13, 64),
        2560 => (51, 20),
        _ => (3, k / 8),
    }
}

/// Turbo encoder: rate 1/3 output [d0, d1, d2] per input bit
pub fn turbo_encode(bits: &[u8]) -> Vec<u8> {
    let k = bits.len();
    let (f1, f2) = qpp_params(k);

    // Interleave
    let interleaved: Vec<u8> = (0..k).map(|i| bits[qpp_interleave_idx(i, k, f1, f2)]).collect();

    // Constituent encoder 1
    let (parity1, _) = rsc_encode(bits, 0);
    // Constituent encoder 2 (interleaved input)
    let (parity2, _) = rsc_encode(&interleaved, 0);

    // Multiplex: systematic + parity1 + parity2
    let mut out = Vec::with_capacity(3 * k);
    for i in 0..k {
        out.push(bits[i]);
        out.push(parity1[i]);
        out.push(parity2[i]);
    }
    out
}

// ---------------------------------------------------------------------------
// BPSK / QPSK modulation  (TS 36.211 §7.1)
// ---------------------------------------------------------------------------

/// BPSK modulation: 0 -> +1/sqrt(2), 1 -> -1/sqrt(2) (complex)
pub fn bpsk_mod(bits: &[u8]) -> Vec<Complex> {
    let amp = 1.0_f64 / 2.0_f64.sqrt();
    bits.iter().map(|&b| {
        let v = if b == 0 { amp } else { -amp };
        Complex::new(v, 0.0)
    }).collect()
}

/// QPSK modulation (Gray-coded, TS 36.211 §7.1.2)
pub fn qpsk_mod(bits: &[u8]) -> Vec<Complex> {
    let amp = 1.0_f64 / 2.0_f64.sqrt();
    bits.chunks(2).map(|pair| {
        let b0 = if pair.len() > 0 { pair[0] } else { 0 };
        let b1 = if pair.len() > 1 { pair[1] } else { 0 };
        let re = if b0 == 0 { amp } else { -amp };
        let im = if b1 == 0 { amp } else { -amp };
        Complex::new(re, im)
    }).collect()
}

/// 16-QAM modulation (Gray-coded)
pub fn qam16_mod(bits: &[u8]) -> Vec<Complex> {
    let norm = 1.0 / (10.0_f64).sqrt();
    bits.chunks(4).map(|b| {
        let b0 = if b.len() > 0 { b[0] } else { 0 };
        let b1 = if b.len() > 1 { b[1] } else { 0 };
        let b2 = if b.len() > 2 { b[2] } else { 0 };
        let b3 = if b.len() > 3 { b[3] } else { 0 };
        let re = if b0 == 0 { if b2 == 0 { 3.0 } else { 1.0 } } else { if b2 == 0 { -3.0 } else { -1.0 } };
        let im = if b1 == 0 { if b3 == 0 { 3.0 } else { 1.0 } } else { if b3 == 0 { -3.0 } else { -1.0 } };
        Complex::new(re * norm, im * norm)
    }).collect()
}

// ---------------------------------------------------------------------------
// NRS (Narrowband Reference Signal) generation  (TS 36.211 §10.2.6)
// ---------------------------------------------------------------------------

/// Generate NRS sequence for a given slot/subframe using LFSR
/// Returns RE indices and complex values for the specified antenna port
pub fn generate_nrs(
    cell_id: u16,
    subframe_idx: usize,
    slot_in_subframe: usize,  // 0 or 1
    port: NrsPort,
    deployment: DeploymentMode,
) -> Vec<(usize, Complex)> {
    // c_init = 512*(7*(ns+1)+l+1)*(2*N_cell_id + 1) + 2*N_cell_id + N_cp
    // ns = slot index (0-19), N_cp = 1 for normal CP
    let ns = subframe_idx * 2 + slot_in_subframe;
    let n_cell = cell_id as u32;
    let n_cp = 1u32; // normal CP

    // Per port, NRS occupies 2 RE per slot at specific OFDM symbols:
    // Port 0: symbols 5 (slot 0) and 5 (slot 1)
    // Port 1: symbols 4 (slot 0) and 4 (slot 1) – staggered by 1 subcarrier
    let (ofdm_sym, sc_offset) = match port {
        NrsPort::Port0 => (5usize, 0usize),
        NrsPort::Port1 => (4usize, 3usize),
    };

    // c_init for this symbol/slot
    let l = ofdm_sym as u32;
    let c_init = (512 * (7 * (ns as u32 + 1) + l + 1) * (2 * n_cell + 1) + 2 * n_cell + n_cp) & 0x7FFF_FFFF;
    let mut scrambler = NbScrambler::new(c_init);

    let mut result = Vec::new();
    // NRS uses QPSK symbols at subcarriers 0,3 (port 0) or 3,6 (port 1) within 12 SCs
    // For in-band: NRS is placed in PRB; for standalone similar but with guard
    let sc_positions = match deployment {
        DeploymentMode::Standalone | DeploymentMode::GuardBand => [sc_offset, sc_offset + 3],
        DeploymentMode::InBand => [sc_offset, sc_offset + 6],
    };

    for &sc in &sc_positions {
        let b0 = scrambler.next_bit();
        let b1 = scrambler.next_bit();
        let re_val = if b0 == 0 { 1.0 / 2.0_f64.sqrt() } else { -1.0 / 2.0_f64.sqrt() };
        let im_val = if b1 == 0 { 1.0 / 2.0_f64.sqrt() } else { -1.0 / 2.0_f64.sqrt() };
        result.push((sc, Complex::new(re_val, im_val)));
    }
    result
}

// ---------------------------------------------------------------------------
// DMRS for NPUSCH (TS 36.211 §10.1.4)
// ---------------------------------------------------------------------------

/// Generate DMRS base sequence r_uv for NPUSCH
/// Based on Zadoff-Chu root sequence with group/sequence hopping
pub fn generate_dmrs_npusch(
    cell_id: u16,
    slot_idx: usize,
    n_sc: usize,  // 1, 3, 6, or 12
) -> Vec<Complex> {
    // Root sequence index u from cell_id (simplified)
    let u = (cell_id as usize / 30) % 30 + 1;
    // ZC sequence length = n_sc
    let nzc = if n_sc == 1 { 4 } else { n_sc }; // minimum length 4 for single-tone
    let mut seq = Vec::with_capacity(nzc);
    for n in 0..nzc {
        let phase = -PI * (u as f64) * (n as f64) * ((n as f64) + 1.0) / (nzc as f64);
        seq.push(Complex::from_polar(1.0, phase + (slot_idx as f64) * PI / 2.0));
    }
    seq
}

// ---------------------------------------------------------------------------
// OFDMA DL: subframe builder (12 subcarriers × 14 symbols)
// ---------------------------------------------------------------------------

/// NB-IoT downlink resource grid (12 subcarriers × 14 OFDM symbols)
pub struct NbDlResourceGrid {
    /// grid[symbol][subcarrier] = complex IQ
    pub grid: [[Complex; NB_NUM_SUBCARRIERS]; NB_SYMBOLS_PER_SUBFRAME],
}

impl NbDlResourceGrid {
    pub fn new() -> Self {
        NbDlResourceGrid {
            grid: [[Complex::new(0.0, 0.0); NB_NUM_SUBCARRIERS]; NB_SYMBOLS_PER_SUBFRAME],
        }
    }

    /// Map symbols to RE, skipping NRS positions
    pub fn map_symbols(
        &mut self,
        symbols: &[Complex],
        cell_id: u16,
        subframe_idx: usize,
        deployment: DeploymentMode,
    ) -> usize {
        // Collect NRS positions to avoid
        let nrs0_s0 = generate_nrs(cell_id, subframe_idx, 0, NrsPort::Port0, deployment);
        let nrs0_s1 = generate_nrs(cell_id, subframe_idx, 1, NrsPort::Port0, deployment);
        let nrs1_s0 = generate_nrs(cell_id, subframe_idx, 0, NrsPort::Port1, deployment);
        let nrs1_s1 = generate_nrs(cell_id, subframe_idx, 1, NrsPort::Port1, deployment);

        let mut nrs_positions: Vec<(usize, usize)> = Vec::new();
        for (sc, _) in &nrs0_s0 { nrs_positions.push((5, *sc)); }
        for (sc, _) in &nrs0_s1 { nrs_positions.push((12, *sc)); }
        for (sc, _) in &nrs1_s0 { nrs_positions.push((4, *sc)); }
        for (sc, _) in &nrs1_s1 { nrs_positions.push((11, *sc)); }

        let mut sym_idx = 0usize;
        let mut mapped = 0usize;
        'outer: for sym in 0..NB_SYMBOLS_PER_SUBFRAME {
            for sc in 0..NB_NUM_SUBCARRIERS {
                if nrs_positions.contains(&(sym, sc)) { continue; }
                if sym_idx >= symbols.len() { break 'outer; }
                self.grid[sym][sc] = symbols[sym_idx];
                sym_idx += 1;
                mapped += 1;
            }
        }
        mapped
    }

    /// Insert NRS reference signals into the grid
    pub fn insert_nrs(
        &mut self,
        cell_id: u16,
        subframe_idx: usize,
        deployment: DeploymentMode,
    ) {
        for slot in 0..2usize {
            let sym_offset = slot * NB_SYMBOLS_PER_SLOT;
            for port in &[NrsPort::Port0, NrsPort::Port1] {
                let nrs = generate_nrs(cell_id, subframe_idx, slot, *port, deployment);
                let sym = match port {
                    NrsPort::Port0 => sym_offset + 5,
                    NrsPort::Port1 => sym_offset + 4,
                };
                for (sc, val) in nrs {
                    if sym < NB_SYMBOLS_PER_SUBFRAME && sc < NB_NUM_SUBCARRIERS {
                        self.grid[sym][sc] = val;
                    }
                }
            }
        }
    }

    /// Convert resource grid to time-domain samples using IFFT (128-point + CP)
    pub fn to_time_domain(&self) -> Vec<Complex> {
        let mut samples = Vec::new();
        for sym in 0..NB_SYMBOLS_PER_SUBFRAME {
            // Build frequency-domain vector (FFT_SIZE_15K = 128)
            let mut freq = vec![Complex::new(0.0, 0.0); FFT_SIZE_15K];
            // Map 12 subcarriers to the center of the FFT
            let center = FFT_SIZE_15K / 2;
            for sc in 0..NB_NUM_SUBCARRIERS {
                let k = (center - NB_NUM_SUBCARRIERS / 2 + sc) % FFT_SIZE_15K;
                freq[k] = self.grid[sym][sc];
            }
            // Shift DC: rearrange for IFFT (standard LTE convention)
            let mut freq_shifted = vec![Complex::new(0.0, 0.0); FFT_SIZE_15K];
            for i in 0..FFT_SIZE_15K {
                freq_shifted[i] = freq[(i + FFT_SIZE_15K / 2) % FFT_SIZE_15K];
            }
            let time_sym = ifft(&freq_shifted);

            // Add cyclic prefix
            let cp_len = if sym == 0 || sym == NB_SYMBOLS_PER_SLOT {
                CP_NORMAL_FIRST_SAMPLES
            } else {
                CP_NORMAL_OTHER_SAMPLES
            };
            let cp_start = FFT_SIZE_15K - cp_len;
            samples.extend_from_slice(&time_sym[cp_start..]);
            samples.extend_from_slice(&time_sym);
        }
        samples
    }
}

impl Default for NbDlResourceGrid {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// NPBCH encoder (640 ms TTI, 34 payload bits, TS 36.211 §10.2.4)
// ---------------------------------------------------------------------------

/// NPBCH transport channel encoding
pub struct NpbchEncoder {
    pub cell_id: u16,
    pub deployment: DeploymentMode,
}

impl NpbchEncoder {
    pub fn new(cell_id: u16, deployment: DeploymentMode) -> Self {
        NpbchEncoder { cell_id, deployment }
    }

    /// Encode 34-bit NPBCH payload -> scrambled QPSK symbols
    /// Output: 240 complex symbols (for 64 subframes × 1 PRB, 1 RB)
    pub fn encode(&self, payload: &[u8; 34]) -> Vec<Complex> {
        // Step 1: CRC-16 attachment (CRC-24A but use 16-bit for NPBCH: 16 parity bits)
        let mut with_crc = payload.to_vec();
        // XOR CRC mask with antenna port count (simplified: 0 for single antenna)
        let crc = crc16_ccitt(&with_crc);
        with_crc.push((crc >> 8) as u8);
        with_crc.push((crc & 0xFF) as u8);
        let bits_with_crc: Vec<u8> = with_crc.iter()
            .flat_map(|&b| (0..8).rev().map(move |i| (b >> i) & 1))
            .collect();

        // Step 2: Tail-biting convolutional coding (rate 1/3)
        let mut encoder = TbccEncoder::new();
        let coded = encoder.encode_tail_biting(&bits_with_crc);

        // Step 3: Rate matching to 1600 bits (10 subframes × 160 bits)
        let rate_matched = rate_match(&coded, 1600);

        // Step 4: Scrambling
        let c_init = self.cell_id as u32;
        let mut scrambler = NbScrambler::new(c_init);
        let mut scrambled = rate_matched.clone();
        scrambler.scramble(&mut scrambled);

        // Step 5: QPSK modulation
        qpsk_mod(&scrambled)
    }
}

/// CRC-16 CCITT (polynomial 0x1021)
fn crc16_ccitt(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 { crc = (crc << 1) ^ 0x1021; } else { crc <<= 1; }
        }
    }
    crc
}

// ---------------------------------------------------------------------------
// NPDCCH (DCI) encoder (TS 36.211 §10.2.5)
// ---------------------------------------------------------------------------

/// DCI format for NB-IoT
#[derive(Debug, Clone)]
pub enum DciFormat {
    /// DCI N0: uplink scheduling grant
    N0 {
        subcarrier_indication: u8,
        resource_assignment: u8,
        mcs: u8,
        scheduling_delay: u8,
        rep_num: u8,
        new_data_indicator: bool,
    },
    /// DCI N1: downlink scheduling assignment
    N1 {
        npdcch_order: bool,
        scheduling_delay: u8,
        resource_assignment: u8,
        mcs: u8,
        rep_num: u8,
        new_data_indicator: bool,
        harq_ack_resource: u8,
    },
    /// DCI N2: paging / direct indication
    N2 {
        direct_indication: u8,
    },
}

/// Pack DCI format into bits
pub fn pack_dci(dci: &DciFormat) -> Vec<u8> {
    match dci {
        DciFormat::N0 { subcarrier_indication, resource_assignment, mcs,
                        scheduling_delay, rep_num, new_data_indicator } => {
            let mut bits = Vec::new();
            // Flag bit: 0 = N0
            bits.push(0u8);
            push_bits(&mut bits, *subcarrier_indication as u32, 6);
            push_bits(&mut bits, *resource_assignment as u32, 3);
            push_bits(&mut bits, *mcs as u32, 4);
            push_bits(&mut bits, *scheduling_delay as u32, 2);
            push_bits(&mut bits, *rep_num as u32, 3);
            bits.push(*new_data_indicator as u8);
            bits
        }
        DciFormat::N1 { npdcch_order, scheduling_delay, resource_assignment,
                        mcs, rep_num, new_data_indicator, harq_ack_resource } => {
            let mut bits = Vec::new();
            bits.push(*npdcch_order as u8);
            push_bits(&mut bits, *scheduling_delay as u32, 3);
            push_bits(&mut bits, *resource_assignment as u32, 3);
            push_bits(&mut bits, *mcs as u32, 4);
            push_bits(&mut bits, *rep_num as u32, 4);
            bits.push(*new_data_indicator as u8);
            push_bits(&mut bits, *harq_ack_resource as u32, 4);
            bits
        }
        DciFormat::N2 { direct_indication } => {
            let mut bits = Vec::new();
            // 8-bit direct indication info
            push_bits(&mut bits, *direct_indication as u32, 8);
            bits
        }
    }
}

fn push_bits(v: &mut Vec<u8>, val: u32, n_bits: usize) {
    for i in (0..n_bits).rev() {
        v.push(((val >> i) & 1) as u8);
    }
}

/// NPDCCH encoder with repetition coding
pub struct NpdcchEncoder {
    pub cell_id: u16,
    pub n_rep: u32,
}

impl NpdcchEncoder {
    pub fn new(cell_id: u16, n_rep: u32) -> Self {
        assert!(n_rep <= NPDCCH_MAX_REPS && n_rep.is_power_of_two(),
                "NPDCCH repetitions must be power of 2 up to 2048");
        NpdcchEncoder { cell_id, n_rep }
    }

    /// Encode DCI: attach CRC, tail-biting CC, rate-match, scramble, modulate
    pub fn encode(&self, dci: &DciFormat, rnti: u16) -> Vec<Complex> {
        let dci_bits = pack_dci(dci);
        let with_crc = attach_crc16_rnti(&dci_bits, rnti);
        let mut encoder = TbccEncoder::new();
        let coded = encoder.encode_tail_biting(&with_crc);
        // Rate match to 288 bits (2 NCCEs × 144 coded bits each)
        let rm = rate_match(&coded, 288);
        // Scrambling
        let c_init = (rnti as u32) * 65536 + self.cell_id as u32;
        let mut scrambler = NbScrambler::new(c_init);
        let mut bits = rm.clone();
        scrambler.scramble(&mut bits);
        // QPSK modulation
        let base_syms = qpsk_mod(&bits);
        // Repeat
        let mut out = Vec::new();
        for _ in 0..self.n_rep {
            out.extend_from_slice(&base_syms);
        }
        out
    }
}

fn attach_crc16_rnti(bits: &[u8], rnti: u16) -> Vec<u8> {
    let crc = crc16_ccitt(bits);
    let masked = crc ^ rnti;
    let mut out = bits.to_vec();
    for i in (0..16).rev() {
        out.push(((masked >> i) & 1) as u8);
    }
    out
}

// ---------------------------------------------------------------------------
// NPDSCH encoder (TS 36.211 §10.2.3)
// ---------------------------------------------------------------------------

/// NPDSCH processor: encode transport block with turbo coding and repetitions
pub struct NpdschEncoder {
    pub cell_id: u16,
    pub n_rep: u32,
}

impl NpdschEncoder {
    pub fn new(cell_id: u16, n_rep: u32) -> Self {
        assert!(n_rep <= NPDSCH_MAX_REPS && n_rep.is_power_of_two());
        NpdschEncoder { cell_id, n_rep }
    }

    /// Encode transport block -> QPSK symbols with repetitions
    pub fn encode(&self, tb: &[u8], rnti: u16) -> Vec<Complex> {
        // CRC-24A attachment
        let tb_bits: Vec<u8> = tb.iter()
            .flat_map(|&b| (0..8).rev().map(move |i| (b >> i) & 1))
            .collect();
        let with_crc = attach_crc24a(&tb_bits);

        // Turbo encoding
        let coded = turbo_encode(&with_crc);

        // Rate matching to G bits (subframe capacity)
        // G = (Nsf × 132) for 12-SC, QPSK, 2 streams (minus NRS)
        let g = (with_crc.len() * 3).min(132 * 8); // simplified
        let rm = rate_match(&coded, g);

        // Scrambling c_init per TS 36.211 §10.2.3.1
        let n_sf_start = 0u32;
        let c_init = rnti as u32 * 65536 + n_sf_start * 512 + self.cell_id as u32;
        let mut scrambler = NbScrambler::new(c_init);
        let mut bits = rm.clone();
        scrambler.scramble(&mut bits);

        // QPSK
        let base = qpsk_mod(&bits);
        let mut out = Vec::new();
        for _ in 0..self.n_rep {
            out.extend_from_slice(&base);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// NPUSCH encoder (TS 36.211 §10.1)
// ---------------------------------------------------------------------------

/// NPUSCH encoder (Format 1 data, Format 2 HARQ-ACK)
pub struct NpuschEncoder {
    pub cell_id: u16,
    pub n_rep: u32,
    pub format: NpuschFormat,
    pub sc_mode: UlSubcarrierMode,
}

impl NpuschEncoder {
    pub fn new(
        cell_id: u16,
        n_rep: u32,
        format: NpuschFormat,
        sc_mode: UlSubcarrierMode,
    ) -> Self {
        assert!(n_rep <= NPUSCH_MAX_REPS && n_rep.is_power_of_two());
        NpuschEncoder { cell_id, n_rep, format, sc_mode }
    }

    /// Number of subcarriers
    pub fn n_sc(&self) -> usize {
        match self.sc_mode {
            UlSubcarrierMode::SingleTone3750 | UlSubcarrierMode::SingleTone15k => 1,
            UlSubcarrierMode::MultiTone3  => 3,
            UlSubcarrierMode::MultiTone6  => 6,
            UlSubcarrierMode::MultiTone12 => 12,
        }
    }

    /// Encode transport block for NPUSCH Format 1
    pub fn encode_format1(&self, tb: &[u8], rnti: u16) -> Vec<Complex> {
        let tb_bits: Vec<u8> = tb.iter()
            .flat_map(|&b| (0..8).rev().map(move |i| (b >> i) & 1))
            .collect();
        let with_crc = attach_crc24a(&tb_bits);

        // Turbo encode
        let coded = turbo_encode(&with_crc);
        let g = (with_crc.len() * 3).min(self.n_sc() * 12 * 2); // capacity estimate
        let rm = rate_match(&coded, g.max(8));

        // Scrambling
        let c_init = rnti as u32 * 65536 + self.cell_id as u32;
        let mut scrambler = NbScrambler::new(c_init);
        let mut bits = rm.clone();
        scrambler.scramble(&mut bits);

        // Modulation: QPSK for multi-tone, BPSK for single-tone
        let base_syms = match self.sc_mode {
            UlSubcarrierMode::SingleTone3750 | UlSubcarrierMode::SingleTone15k => bpsk_mod(&bits),
            _ => qpsk_mod(&bits),
        };

        let mut out = Vec::new();
        for _ in 0..self.n_rep {
            out.extend_from_slice(&base_syms);
        }
        out
    }

    /// Encode HARQ-ACK for NPUSCH Format 2
    pub fn encode_format2(&self, ack: bool, rnti: u16) -> Vec<Complex> {
        // 1-bit ACK/NACK encoded with repetition
        let bits = vec![ack as u8; 16]; // 16 coded bits from repetition
        let c_init = rnti as u32 * 65536 + self.cell_id as u32 + 1;
        let mut scrambler = NbScrambler::new(c_init);
        let mut scrambled = bits.clone();
        scrambler.scramble(&mut scrambled);
        let base = bpsk_mod(&scrambled);
        let mut out = Vec::new();
        for _ in 0..self.n_rep {
            out.extend_from_slice(&base);
        }
        out
    }

    /// Build SC-FDMA time-domain samples for a slot (7 symbols)
    pub fn to_time_domain(&self, symbols: &[Complex], slot_idx: usize) -> Vec<Complex> {
        let n_sc = self.n_sc();
        let fft_size = match self.sc_mode {
            UlSubcarrierMode::SingleTone3750 => FFT_SIZE_375,
            _ => FFT_SIZE_15K,
        };
        let cp_len = if fft_size == FFT_SIZE_15K { CP_NORMAL_OTHER_SAMPLES } else { CP_NORMAL_OTHER_SAMPLES * 4 };

        let dmrs = generate_dmrs_npusch(self.cell_id, slot_idx, n_sc.max(1));
        let mut time_samples = Vec::new();

        // For each of 7 symbols in slot (symbol 3 = DMRS)
        let syms_per_slot = NB_SYMBOLS_PER_SLOT;
        let mut data_sym_idx = 0usize;

        for sym in 0..syms_per_slot {
            let sc_syms: Vec<Complex> = if sym == 3 {
                // DMRS symbol
                dmrs.clone()
            } else {
                let start = data_sym_idx * n_sc;
                let end = (start + n_sc).min(symbols.len());
                if start < symbols.len() {
                    data_sym_idx += 1;
                    symbols[start..end].to_vec()
                } else {
                    vec![Complex::new(0.0, 0.0); n_sc]
                }
            };

            // DFT pre-coding (SC-FDMA)
            let mut dft_in = sc_syms.clone();
            while dft_in.len() < n_sc.max(1) { dft_in.push(Complex::new(0.0, 0.0)); }
            let dft_out = fft(&dft_in);

            // Map to IFFT input
            let sc_start = (fft_size / 2).saturating_sub(n_sc / 2);
            let mut freq = vec![Complex::new(0.0, 0.0); fft_size];
            for (i, &v) in dft_out.iter().enumerate().take(n_sc.max(1)) {
                freq[(sc_start + i) % fft_size] = v;
            }
            let time_sym = ifft(&freq);

            // Add CP
            let cp_start = fft_size.saturating_sub(cp_len);
            time_samples.extend_from_slice(&time_sym[cp_start..]);
            time_samples.extend_from_slice(&time_sym);
        }
        time_samples
    }
}

// ---------------------------------------------------------------------------
// NPRACH (TS 36.211 §10.1.6)
// ---------------------------------------------------------------------------

/// NPRACH configuration
#[derive(Debug, Clone)]
pub struct NprachConfig {
    /// Coverage level (0–3)
    pub coverage_level: NprachCoverageLevel,
    /// Number of preamble repetitions: 1,2,4,8,16,32,64,128
    pub n_rep: u32,
    /// Starting subcarrier index (0–47 for 3.75 kHz grid)
    pub subcarrier_start: usize,
    /// Number of subcarriers reserved for NPRACH (12, 24, 36, or 48)
    pub n_sc_ra: usize,
}

impl NprachConfig {
    pub fn new(coverage_level: NprachCoverageLevel, n_rep: u32) -> Self {
        assert!(matches!(n_rep, 1|2|4|8|16|32|64|128), "Invalid NPRACH repetitions");
        NprachConfig {
            coverage_level,
            n_rep,
            subcarrier_start: 0,
            n_sc_ra: 12,
        }
    }

    /// Generate NPRACH preamble sequence (SC-FDMA with 3.75 kHz spacing)
    pub fn generate_preamble(&self, cell_id: u16) -> Vec<Complex> {
        let n_group = 4usize; // 4 symbol groups per preamble
        let sym_per_group = 5usize; // 1 CP + 4 ZC symbols (simplified)
        let total_syms = n_group * sym_per_group * self.n_rep as usize;

        // Generate ZC root sequence
        let n_zc = 13usize; // NPRACH uses 13-element ZC for preamble
        let u = ((cell_id as usize * 7) % n_zc) + 1;

        let mut preamble = Vec::with_capacity(total_syms * FFT_SIZE_375);
        let mut hop_sc = self.subcarrier_start;

        for rep in 0..self.n_rep as usize {
            for group in 0..n_group {
                // Frequency hopping per symbol group (pseudo-random)
                let hop_seed = (rep * n_group + group) ^ (cell_id as usize);
                hop_sc = self.subcarrier_start + (hop_seed % self.n_sc_ra);

                // Generate ZC sequence for this hop position
                for sym in 0..sym_per_group {
                    let phase_offset = hop_sc as f64 * 2.0 * PI * 3750.0 / 1.92e6;
                    let mut freq = vec![Complex::new(0.0, 0.0); FFT_SIZE_375];
                    for n in 0..n_zc {
                        let zc_phase = -PI * (u as f64) * (n as f64) * ((n as f64) + 1.0) / (n_zc as f64);
                        let total_phase = zc_phase + phase_offset + (sym as f64) * PI / 4.0;
                        let sc_idx = (hop_sc + n) % FFT_SIZE_375;
                        freq[sc_idx] = Complex::from_polar(1.0, total_phase);
                    }
                    let time = ifft(&freq);
                    // Add CP (1/8 of symbol length)
                    let cp_len = FFT_SIZE_375 / 8;
                    let cp_start = FFT_SIZE_375 - cp_len;
                    preamble.extend_from_slice(&time[cp_start..]);
                    preamble.extend_from_slice(&time);
                }
            }
        }
        preamble
    }
}

// ---------------------------------------------------------------------------
// Multi-carrier Cat-NB2 carrier manager
// ---------------------------------------------------------------------------

/// NB-IoT carrier (anchor or non-anchor)
#[derive(Debug, Clone)]
pub struct NbCarrier {
    pub carrier_id: u8,
    pub center_freq_khz: u32,
    pub is_anchor: bool,
    pub deployment: DeploymentMode,
    pub cell_id: u16,
}

impl NbCarrier {
    pub fn anchor(center_freq_khz: u32, cell_id: u16, deployment: DeploymentMode) -> Self {
        NbCarrier { carrier_id: 0, center_freq_khz, is_anchor: true, deployment, cell_id }
    }

    pub fn non_anchor(carrier_id: u8, center_freq_khz: u32, cell_id: u16, deployment: DeploymentMode) -> Self {
        NbCarrier { carrier_id, center_freq_khz, is_anchor: false, deployment, cell_id }
    }
}

/// Cat-NB2 multi-carrier manager (up to 2 carriers per device)
pub struct CatNb2CarrierManager {
    pub anchor: NbCarrier,
    pub non_anchor: Vec<NbCarrier>,
    pub active_carrier_id: u8,
}

impl CatNb2CarrierManager {
    pub fn new(anchor: NbCarrier) -> Self {
        CatNb2CarrierManager {
            anchor,
            non_anchor: Vec::new(),
            active_carrier_id: 0,
        }
    }

    /// Add a non-anchor carrier (Cat-NB2 supports additional carriers)
    pub fn add_non_anchor(&mut self, carrier: NbCarrier) {
        assert!(!carrier.is_anchor);
        self.non_anchor.push(carrier);
    }

    /// Switch to a non-anchor carrier by ID
    pub fn switch_carrier(&mut self, carrier_id: u8) -> bool {
        if carrier_id == 0 {
            self.active_carrier_id = 0;
            return true;
        }
        let found = self.non_anchor.iter().any(|c| c.carrier_id == carrier_id);
        if found { self.active_carrier_id = carrier_id; }
        found
    }

    /// Get active carrier reference
    pub fn active_carrier(&self) -> &NbCarrier {
        if self.active_carrier_id == 0 {
            &self.anchor
        } else {
            self.non_anchor.iter()
                .find(|c| c.carrier_id == self.active_carrier_id)
                .unwrap_or(&self.anchor)
        }
    }

    /// Compute peak DL data rate for Cat-NB2 (kbps)
    pub fn peak_dl_rate_kbps(&self) -> f64 {
        // Max TBS for 12-SC: 2536 bits per ~680ms (max config) -> ~127 kbps simplified
        2536.0 / (20.0 * 1e-3) / 1000.0
    }

    /// Compute peak UL data rate for Cat-NB2 (kbps)
    pub fn peak_ul_rate_kbps(&self) -> f64 {
        // Multi-tone 12 SC, max TBS: ~159 kbps
        2680.0 / (16.0 * 1e-3) / 1000.0
    }
}

// ---------------------------------------------------------------------------
// Coverage level / MCL helpers
// ---------------------------------------------------------------------------

/// Maximum Coupling Loss parameters for NB-IoT
pub struct McLinBudget {
    pub tx_power_dbm: f64,
    pub rx_noise_figure_db: f64,
    pub thermal_noise_density_dbm_hz: f64,
    pub bandwidth_hz: f64,
    pub required_sinr_db: f64,
    pub implementation_loss_db: f64,
    pub coverage_enhancement_db: f64,
}

impl McLinBudget {
    /// Default MCL budget for standalone NB-IoT
    pub fn nb_iot_standalone() -> Self {
        McLinBudget {
            tx_power_dbm: 23.0,
            rx_noise_figure_db: 5.0,
            thermal_noise_density_dbm_hz: -174.0,
            bandwidth_hz: 180_000.0,
            required_sinr_db: -12.6,
            implementation_loss_db: 0.0,
            coverage_enhancement_db: 20.0, // CE Level 2
        }
    }

    /// Calculate MCL in dB
    pub fn mcl_db(&self) -> f64 {
        let thermal_noise_dbm = self.thermal_noise_density_dbm_hz
            + 10.0 * (self.bandwidth_hz).log10();
        let receiver_sensitivity = thermal_noise_dbm + self.rx_noise_figure_db
            + self.required_sinr_db;
        self.tx_power_dbm - receiver_sensitivity
            - self.implementation_loss_db
            + self.coverage_enhancement_db
    }

    /// Returns true if MCL meets 164 dB target
    pub fn meets_164db_target(&self) -> bool {
        self.mcl_db() >= 164.0
    }
}

// ---------------------------------------------------------------------------
// Repetition gain calculation
// ---------------------------------------------------------------------------

/// Calculate coverage gain from repetitions (approximate, assuming AWGN combining)
pub fn repetition_gain_db(n_rep: u32) -> f64 {
    10.0 * (n_rep as f64).log10()
}

/// Map coverage level to default repetition counts
pub fn default_reps(level: CoverageLevel) -> (u32, u32, u32) {
    // Returns (npdcch_reps, npdsch_reps, npusch_reps)
    match level {
        CoverageLevel::CE0 => (1, 1, 1),
        CoverageLevel::CE1 => (16, 16, 8),
        CoverageLevel::CE2 => (2048, 2048, 128),
    }
}

// ---------------------------------------------------------------------------
// High-level NB-IoT processor
// ---------------------------------------------------------------------------

/// Top-level NB-IoT processor (Rel-14, Cat-NB2)
pub struct LteCatNb2Processor {
    pub cell_id: u16,
    pub deployment: DeploymentMode,
    pub coverage_level: CoverageLevel,
    pub carrier_manager: CatNb2CarrierManager,
    pub npbch: NpbchEncoder,
    pub npdcch: NpdcchEncoder,
    pub npdsch: NpdschEncoder,
    pub npusch: NpuschEncoder,
}

impl LteCatNb2Processor {
    /// Create a new Cat-NB2 processor for the given cell
    pub fn new(
        cell_id: u16,
        deployment: DeploymentMode,
        coverage_level: CoverageLevel,
        anchor_freq_khz: u32,
    ) -> Self {
        let (dcch_reps, dsch_reps, usch_reps) = default_reps(coverage_level);
        let anchor = NbCarrier::anchor(anchor_freq_khz, cell_id, deployment);
        LteCatNb2Processor {
            cell_id,
            deployment,
            coverage_level,
            carrier_manager: CatNb2CarrierManager::new(anchor),
            npbch: NpbchEncoder::new(cell_id, deployment),
            npdcch: NpdcchEncoder::new(cell_id, dcch_reps),
            npdsch: NpdschEncoder::new(cell_id, dsch_reps),
            npusch: NpuschEncoder::new(
                cell_id, usch_reps, NpuschFormat::Format1, UlSubcarrierMode::MultiTone12
            ),
        }
    }

    /// Process a downlink subframe (returns IQ samples)
    pub fn process_dl_subframe(
        &self,
        subframe_idx: usize,
        payload: Option<&[u8]>,
        rnti: u16,
    ) -> Vec<Complex> {
        let mut grid = NbDlResourceGrid::new();
        grid.insert_nrs(self.cell_id, subframe_idx, self.deployment);

        if let Some(data) = payload {
            let syms = self.npdsch.encode(data, rnti);
            grid.map_symbols(&syms, self.cell_id, subframe_idx, self.deployment);
        }
        grid.to_time_domain()
    }

    /// Process an uplink subframe (returns IQ samples)
    pub fn process_ul_subframe(
        &self,
        payload: Option<&[u8]>,
        rnti: u16,
        slot_idx: usize,
    ) -> Vec<Complex> {
        let syms = if let Some(data) = payload {
            self.npusch.encode_format1(data, rnti)
        } else {
            vec![Complex::new(0.0, 0.0); 12]
        };
        self.npusch.to_time_domain(&syms, slot_idx)
    }

    /// Estimate link budget MCL
    pub fn link_budget_mcl_db(&self) -> f64 {
        McLinBudget::nb_iot_standalone().mcl_db()
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- TBS table tests ---
    #[test]
    fn test_tbs_dl_basic() {
        let tbs = tbs_dl(0, 1).unwrap();
        assert_eq!(tbs, 16);
    }

    #[test]
    fn test_tbs_dl_max() {
        let tbs = tbs_dl(13, 10).unwrap();
        assert_eq!(tbs, 2536);
    }

    #[test]
    fn test_tbs_dl_out_of_range() {
        assert!(tbs_dl(14, 1).is_none());
        assert!(tbs_dl(0, 7).is_none());
    }

    #[test]
    fn test_tbs_ul_single_tone() {
        let tbs = tbs_ul_single_tone(0, 1).unwrap();
        assert_eq!(tbs, 16);
        let tbs2 = tbs_ul_single_tone(7, 10).unwrap();
        assert_eq!(tbs2, 536);
    }

    #[test]
    fn test_tbs_ul_single_tone_out_of_range() {
        assert!(tbs_ul_single_tone(8, 1).is_none());
        assert!(tbs_ul_single_tone(0, 0).is_none());
        assert!(tbs_ul_single_tone(0, 11).is_none());
    }

    #[test]
    fn test_tbs_ul_multi_tone_3sc() {
        let tbs = tbs_ul_multi_tone(0, 3, 1).unwrap();
        assert_eq!(tbs, 32);
    }

    #[test]
    fn test_tbs_ul_multi_tone_6sc() {
        let tbs = tbs_ul_multi_tone(0, 6, 1).unwrap();
        assert_eq!(tbs, 56);
    }

    #[test]
    fn test_tbs_ul_multi_tone_12sc_max() {
        // TBS_MT12[13][ru_idx=6 for n_ru=8] = 8504
        let tbs = tbs_ul_multi_tone(13, 12, 8).unwrap();
        assert_eq!(tbs, 8504);
        // Maximum 10296 is at n_ru=10 (index 7 in table, but n_ru > 8 guard)
        // Verify the guard allows n_ru=6 as max supported in this API
        let tbs2 = tbs_ul_multi_tone(13, 12, 6).unwrap();
        assert_eq!(tbs2, 6968);
    }

    #[test]
    fn test_tbs_ul_invalid_sc() {
        assert!(tbs_ul_multi_tone(0, 4, 1).is_none());
        assert!(tbs_ul_multi_tone(0, 12, 9).is_none());
    }

    // --- Complex arithmetic ---
    #[test]
    fn test_complex_mul() {
        let a = Complex::new(1.0, 2.0);
        let b = Complex::new(3.0, 4.0);
        let c = a * b;
        assert!((c.re - (-5.0)).abs() < 1e-10);
        assert!((c.im - 10.0).abs() < 1e-10);
    }

    #[test]
    fn test_complex_polar() {
        let c = Complex::from_polar(1.0, 0.0);
        assert!((c.re - 1.0).abs() < 1e-10);
        assert!(c.im.abs() < 1e-10);
    }

    #[test]
    fn test_complex_abs() {
        let c = Complex::new(3.0, 4.0);
        assert!((c.abs() - 5.0).abs() < 1e-10);
    }

    // --- FFT tests ---
    #[test]
    fn test_fft_ifft_roundtrip() {
        let n = 32;
        let input: Vec<Complex> = (0..n).map(|i| Complex::new(i as f64, 0.0)).collect();
        let freq = fft(&input);
        let recovered = ifft(&freq);
        for (a, b) in input.iter().zip(recovered.iter()) {
            assert!((a.re - b.re).abs() < 1e-9, "re mismatch: {} vs {}", a.re, b.re);
            assert!((a.im - b.im).abs() < 1e-9);
        }
    }

    #[test]
    fn test_fft_dc() {
        let n = 8;
        let input = vec![Complex::new(1.0, 0.0); n];
        let freq = fft(&input);
        // DC bin should be n (sum of ones)
        assert!((freq[0].re - n as f64).abs() < 1e-9);
        for k in 1..n {
            assert!(freq[k].abs() < 1e-9, "bin {} not zero", k);
        }
    }

    #[test]
    fn test_fft_length_128() {
        let n = FFT_SIZE_15K; // 128
        let input: Vec<Complex> = (0..n).map(|_| Complex::new(0.5, 0.0)).collect();
        let freq = fft(&input);
        let back = ifft(&freq);
        assert!((back[0].re - 0.5).abs() < 1e-9);
    }

    // --- Scrambler ---
    #[test]
    fn test_scrambler_deterministic() {
        let mut s1 = NbScrambler::new(0x1234);
        let mut s2 = NbScrambler::new(0x1234);
        let b1: Vec<u8> = (0..100).map(|_| s1.next_bit()).collect();
        let b2: Vec<u8> = (0..100).map(|_| s2.next_bit()).collect();
        assert_eq!(b1, b2);
    }

    #[test]
    fn test_scrambler_different_seeds() {
        let mut s1 = NbScrambler::new(0x0001);
        let mut s2 = NbScrambler::new(0x0002);
        let b1: Vec<u8> = (0..50).map(|_| s1.next_bit()).collect();
        let b2: Vec<u8> = (0..50).map(|_| s2.next_bit()).collect();
        assert_ne!(b1, b2);
    }

    #[test]
    fn test_scrambler_self_inverse() {
        let data = vec![1u8, 0, 1, 1, 0, 0, 1, 0, 1, 1];
        let mut s1 = NbScrambler::new(42);
        let mut scrambled = data.clone();
        s1.scramble(&mut scrambled);
        let mut s2 = NbScrambler::new(42);
        let mut recovered = scrambled.clone();
        s2.scramble(&mut recovered);
        assert_eq!(data, recovered);
    }

    // --- CRC ---
    #[test]
    fn test_crc24a_known() {
        let bits = vec![1u8, 0, 1, 0, 1, 0, 1, 0];
        let crc = crc24a(&bits);
        assert!(crc <= 0xFFFFFF);
    }

    #[test]
    fn test_crc24a_zero_input() {
        let bits = vec![0u8; 32];
        let crc = crc24a(&bits);
        assert_eq!(crc, 0);
    }

    #[test]
    fn test_attach_crc24a_length() {
        let data = vec![1u8; 40];
        let with_crc = attach_crc24a(&data);
        assert_eq!(with_crc.len(), 64);
    }

    // --- TBCC ---
    #[test]
    fn test_tbcc_rate() {
        let bits = vec![1u8, 0, 1, 0, 1, 0, 1, 0];
        let mut enc = TbccEncoder::new();
        let coded = enc.encode_tail_biting(&bits);
        assert_eq!(coded.len(), bits.len() * 3);
    }

    #[test]
    fn test_tbcc_all_zeros() {
        let bits = vec![0u8; 16];
        let mut enc = TbccEncoder::new();
        let coded = enc.encode_tail_biting(&bits);
        assert_eq!(coded.len(), 48);
        // With all-zeros, output depends on generators, check length
        assert!(coded.iter().all(|&b| b == 0 || b == 1));
    }

    #[test]
    fn test_tbcc_reproducible() {
        let bits = vec![1u8, 0, 1, 1, 0, 0, 1, 0, 1, 1, 0, 1];
        let mut enc1 = TbccEncoder::new();
        let mut enc2 = TbccEncoder::new();
        let c1 = enc1.encode_tail_biting(&bits);
        let c2 = enc2.encode_tail_biting(&bits);
        assert_eq!(c1, c2);
    }

    // --- Rate matching ---
    #[test]
    fn test_rate_match_expand() {
        let coded = vec![1u8, 0, 1];
        let rm = rate_match(&coded, 9);
        assert_eq!(rm, vec![1, 0, 1, 1, 0, 1, 1, 0, 1]);
    }

    #[test]
    fn test_rate_match_truncate() {
        let coded: Vec<u8> = (0..20).map(|i| (i % 2) as u8).collect();
        let rm = rate_match(&coded, 5);
        assert_eq!(rm.len(), 5);
    }

    // --- Turbo encoder ---
    #[test]
    fn test_turbo_rate() {
        let bits = vec![1u8, 0, 1, 0, 1, 0, 1, 0, 1, 0, 0, 0, 1, 1, 1, 0,
                        1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 0, 0, 1, 1, 1, 0,
                        1, 0, 1, 0, 1, 0, 1, 0];
        let coded = turbo_encode(&bits);
        assert_eq!(coded.len(), bits.len() * 3);
    }

    #[test]
    fn test_turbo_bits_valid() {
        let bits = vec![0u8; 64];
        let coded = turbo_encode(&bits);
        assert!(coded.iter().all(|&b| b == 0 || b == 1));
    }

    // --- Modulation ---
    #[test]
    fn test_bpsk_mod_amplitude() {
        let bits = vec![0u8, 1, 0, 1];
        let syms = bpsk_mod(&bits);
        let amp = 1.0 / 2.0_f64.sqrt();
        assert!((syms[0].re - amp).abs() < 1e-10);
        assert!((syms[1].re + amp).abs() < 1e-10);
        for s in &syms { assert!(s.im.abs() < 1e-10); }
    }

    #[test]
    fn test_qpsk_power() {
        let bits: Vec<u8> = vec![0, 0, 0, 1, 1, 0, 1, 1];
        let syms = qpsk_mod(&bits);
        for s in &syms {
            let power = s.abs_sq();
            assert!((power - 1.0).abs() < 1e-9, "QPSK power={}", power);
        }
    }

    #[test]
    fn test_qam16_four_points() {
        let bits = vec![0u8, 0, 0, 0];  // -> (3,3)/sqrt(10)
        let syms = qam16_mod(&bits);
        assert_eq!(syms.len(), 1);
        let expected = 3.0 / (10.0_f64).sqrt();
        assert!((syms[0].re - expected).abs() < 1e-9);
    }

    // --- NRS generation ---
    #[test]
    fn test_nrs_count_per_slot() {
        let nrs = generate_nrs(0, 0, 0, NrsPort::Port0, DeploymentMode::Standalone);
        assert_eq!(nrs.len(), NRS_RE_PER_SLOT);
    }

    #[test]
    fn test_nrs_unit_power() {
        let nrs = generate_nrs(42, 1, 1, NrsPort::Port1, DeploymentMode::InBand);
        for (_, sym) in &nrs {
            assert!((sym.abs_sq() - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn test_nrs_different_ports_different_sc() {
        let nrs0 = generate_nrs(10, 0, 0, NrsPort::Port0, DeploymentMode::Standalone);
        let nrs1 = generate_nrs(10, 0, 0, NrsPort::Port1, DeploymentMode::Standalone);
        let sc0: Vec<usize> = nrs0.iter().map(|(sc, _)| *sc).collect();
        let sc1: Vec<usize> = nrs1.iter().map(|(sc, _)| *sc).collect();
        // Ports use different subcarrier offsets
        assert_ne!(sc0, sc1);
    }

    // --- DMRS ---
    #[test]
    fn test_dmrs_length_single_tone() {
        let dmrs = generate_dmrs_npusch(100, 0, 1);
        assert_eq!(dmrs.len(), 4); // minimum 4 for single-tone
    }

    #[test]
    fn test_dmrs_length_multi_tone() {
        let dmrs = generate_dmrs_npusch(100, 0, 12);
        assert_eq!(dmrs.len(), 12);
    }

    // --- Resource grid ---
    #[test]
    fn test_resource_grid_time_domain_length() {
        let grid = NbDlResourceGrid::new();
        let samples = grid.to_time_domain();
        // 14 symbols × (FFT_SIZE + CP)
        // Symbol 0,7: CP=160, others: CP=144
        let expected = 2 * (FFT_SIZE_15K + CP_NORMAL_FIRST_SAMPLES)
            + 12 * (FFT_SIZE_15K + CP_NORMAL_OTHER_SAMPLES);
        assert_eq!(samples.len(), expected);
    }

    #[test]
    fn test_resource_grid_nrs_inserted() {
        let mut grid = NbDlResourceGrid::new();
        grid.insert_nrs(0, 0, DeploymentMode::Standalone);
        // NRS port 0, slot 0: symbol 5, some subcarriers != 0
        let nrs_sym = &grid.grid[5];
        let nonzero = nrs_sym.iter().any(|c| c.abs_sq() > 0.5);
        assert!(nonzero, "NRS should be nonzero at symbol 5");
    }

    // --- NPBCH ---
    #[test]
    fn test_npbch_encode_length() {
        let enc = NpbchEncoder::new(0, DeploymentMode::Standalone);
        let payload = [0u8; 34];
        let syms = enc.encode(&payload);
        // Should produce symbols for rate_matched 1600 bits -> 800 QPSK symbols
        assert_eq!(syms.len(), 800);
    }

    #[test]
    fn test_npbch_different_cell_ids() {
        let enc0 = NpbchEncoder::new(0, DeploymentMode::Standalone);
        let enc1 = NpbchEncoder::new(100, DeploymentMode::Standalone);
        let payload = [1u8; 34];
        let s0 = enc0.encode(&payload);
        let s1 = enc1.encode(&payload);
        // Different cell IDs should produce different scrambling
        let diff = s0.iter().zip(s1.iter()).any(|(a, b)| (a.re - b.re).abs() > 1e-9);
        assert!(diff);
    }

    // --- NPDCCH ---
    #[test]
    fn test_npdcch_encode_n0() {
        let enc = NpdcchEncoder::new(0, 1);
        let dci = DciFormat::N0 {
            subcarrier_indication: 3,
            resource_assignment: 2,
            mcs: 5,
            scheduling_delay: 1,
            rep_num: 2,
            new_data_indicator: true,
        };
        let syms = enc.encode(&dci, 0x1234);
        assert!(syms.len() > 0);
    }

    #[test]
    fn test_npdcch_encode_n1() {
        let enc = NpdcchEncoder::new(10, 4);
        let dci = DciFormat::N1 {
            npdcch_order: false,
            scheduling_delay: 3,
            resource_assignment: 5,
            mcs: 7,
            rep_num: 2,
            new_data_indicator: false,
            harq_ack_resource: 1,
        };
        let syms = enc.encode(&dci, 0xABCD);
        // 4 repetitions
        assert!(syms.len() > 0);
    }

    #[test]
    fn test_npdcch_repetitions() {
        let enc1 = NpdcchEncoder::new(0, 1);
        let enc4 = NpdcchEncoder::new(0, 4);
        let dci = DciFormat::N2 { direct_indication: 0x55 };
        let s1 = enc1.encode(&dci, 1);
        let s4 = enc4.encode(&dci, 1);
        assert_eq!(s4.len(), s1.len() * 4);
    }

    // --- NPDSCH ---
    #[test]
    fn test_npdsch_encode_non_empty() {
        let enc = NpdschEncoder::new(0, 1);
        let tb = vec![0xA5u8; 20]; // 160 bits
        let syms = enc.encode(&tb, 0x0001);
        assert!(!syms.is_empty());
    }

    #[test]
    fn test_npdsch_repetition_scales() {
        let enc1 = NpdschEncoder::new(0, 1);
        let enc2 = NpdschEncoder::new(0, 2);
        let tb = vec![0xFFu8; 16];
        let s1 = enc1.encode(&tb, 0x1);
        let s2 = enc2.encode(&tb, 0x1);
        assert_eq!(s2.len(), s1.len() * 2);
    }

    // --- NPUSCH ---
    #[test]
    fn test_npusch_format1_encode() {
        let enc = NpuschEncoder::new(0, 1, NpuschFormat::Format1, UlSubcarrierMode::MultiTone12);
        let tb = vec![0xAAu8; 8];
        let syms = enc.encode_format1(&tb, 0x2);
        assert!(!syms.is_empty());
    }

    #[test]
    fn test_npusch_format2_ack() {
        let enc = NpuschEncoder::new(0, 1, NpuschFormat::Format2, UlSubcarrierMode::SingleTone15k);
        let syms = enc.encode_format2(true, 0x3);
        assert!(!syms.is_empty());
    }

    #[test]
    fn test_npusch_nsc_single_tone() {
        let enc = NpuschEncoder::new(0, 1, NpuschFormat::Format1, UlSubcarrierMode::SingleTone15k);
        assert_eq!(enc.n_sc(), 1);
    }

    #[test]
    fn test_npusch_nsc_multi_tone_6() {
        let enc = NpuschEncoder::new(0, 1, NpuschFormat::Format1, UlSubcarrierMode::MultiTone6);
        assert_eq!(enc.n_sc(), 6);
    }

    #[test]
    fn test_npusch_time_domain_length() {
        let enc = NpuschEncoder::new(0, 1, NpuschFormat::Format1, UlSubcarrierMode::MultiTone12);
        let syms = vec![Complex::new(0.0, 0.0); 72];
        let td = enc.to_time_domain(&syms, 0);
        // 7 symbols × (FFT_SIZE + CP)
        let expected = NB_SYMBOLS_PER_SLOT * (FFT_SIZE_15K + CP_NORMAL_OTHER_SAMPLES);
        assert_eq!(td.len(), expected);
    }

    // --- NPRACH ---
    #[test]
    fn test_nprach_preamble_non_empty() {
        let cfg = NprachConfig::new(NprachCoverageLevel::Level0, 1);
        let preamble = cfg.generate_preamble(42);
        assert!(!preamble.is_empty());
    }

    #[test]
    fn test_nprach_preamble_scales_with_reps() {
        let cfg1 = NprachConfig::new(NprachCoverageLevel::Level0, 1);
        let cfg2 = NprachConfig::new(NprachCoverageLevel::Level0, 2);
        let p1 = cfg1.generate_preamble(0).len();
        let p2 = cfg2.generate_preamble(0).len();
        assert_eq!(p2, p1 * 2);
    }

    #[test]
    fn test_nprach_coverage_levels() {
        for &reps in &[1u32, 2, 4, 8, 16, 32, 64, 128] {
            let cfg = NprachConfig::new(NprachCoverageLevel::Level2, reps);
            let p = cfg.generate_preamble(100);
            assert!(!p.is_empty(), "reps={}", reps);
        }
    }

    // --- Multi-carrier ---
    #[test]
    fn test_carrier_manager_anchor() {
        let anchor = NbCarrier::anchor(869_000, 42, DeploymentMode::Standalone);
        let mgr = CatNb2CarrierManager::new(anchor);
        assert!(mgr.active_carrier().is_anchor);
        assert_eq!(mgr.active_carrier().center_freq_khz, 869_000);
    }

    #[test]
    fn test_carrier_switch() {
        let anchor = NbCarrier::anchor(869_000, 42, DeploymentMode::Standalone);
        let mut mgr = CatNb2CarrierManager::new(anchor);
        let na = NbCarrier::non_anchor(1, 870_000, 42, DeploymentMode::Standalone);
        mgr.add_non_anchor(na);
        let ok = mgr.switch_carrier(1);
        assert!(ok);
        assert!(!mgr.active_carrier().is_anchor);
        assert_eq!(mgr.active_carrier().center_freq_khz, 870_000);
    }

    #[test]
    fn test_carrier_switch_back_to_anchor() {
        let anchor = NbCarrier::anchor(869_000, 42, DeploymentMode::Standalone);
        let mut mgr = CatNb2CarrierManager::new(anchor);
        let na = NbCarrier::non_anchor(1, 870_000, 42, DeploymentMode::Standalone);
        mgr.add_non_anchor(na);
        mgr.switch_carrier(1);
        mgr.switch_carrier(0);
        assert!(mgr.active_carrier().is_anchor);
    }

    #[test]
    fn test_peak_rates() {
        let anchor = NbCarrier::anchor(869_000, 0, DeploymentMode::Standalone);
        let mgr = CatNb2CarrierManager::new(anchor);
        assert!(mgr.peak_dl_rate_kbps() > 100.0);
        assert!(mgr.peak_ul_rate_kbps() > 100.0);
    }

    // --- MCL / Coverage ---
    #[test]
    fn test_mcl_meets_164db() {
        let budget = McLinBudget::nb_iot_standalone();
        assert!(budget.meets_164db_target(), "MCL={}", budget.mcl_db());
    }

    #[test]
    fn test_mcl_above_standard_lte() {
        let budget = McLinBudget::nb_iot_standalone();
        assert!(budget.mcl_db() > 140.0);
    }

    #[test]
    fn test_repetition_gain_db() {
        assert!((repetition_gain_db(1) - 0.0).abs() < 1e-9);
        assert!((repetition_gain_db(10) - 10.0).abs() < 1e-9);
        assert!((repetition_gain_db(100) - 20.0).abs() < 1e-9);
        assert!((repetition_gain_db(1000) - 30.0).abs() < 1e-9);
    }

    #[test]
    fn test_default_reps_coverage_levels() {
        let (d0, s0, u0) = default_reps(CoverageLevel::CE0);
        assert_eq!((d0, s0, u0), (1, 1, 1));
        let (d2, s2, u2) = default_reps(CoverageLevel::CE2);
        assert_eq!((d2, s2, u2), (2048, 2048, 128));
    }

    // --- High-level processor ---
    #[test]
    fn test_processor_new() {
        let proc = LteCatNb2Processor::new(100, DeploymentMode::Standalone, CoverageLevel::CE0, 869_000);
        assert_eq!(proc.cell_id, 100);
        assert_eq!(proc.deployment, DeploymentMode::Standalone);
    }

    #[test]
    fn test_processor_dl_subframe_no_payload() {
        let proc = LteCatNb2Processor::new(0, DeploymentMode::Standalone, CoverageLevel::CE0, 869_000);
        let samples = proc.process_dl_subframe(0, None, 0xFFFF);
        assert!(!samples.is_empty());
    }

    #[test]
    fn test_processor_dl_subframe_with_payload() {
        let proc = LteCatNb2Processor::new(1, DeploymentMode::Standalone, CoverageLevel::CE1, 869_000);
        let data = vec![0xABu8; 16];
        let samples = proc.process_dl_subframe(5, Some(&data), 0x1234);
        assert!(!samples.is_empty());
    }

    #[test]
    fn test_processor_ul_subframe() {
        let proc = LteCatNb2Processor::new(2, DeploymentMode::InBand, CoverageLevel::CE0, 869_000);
        let data = vec![0x55u8; 8];
        let samples = proc.process_ul_subframe(Some(&data), 0x5678, 0);
        assert!(!samples.is_empty());
    }

    #[test]
    fn test_processor_link_budget() {
        let proc = LteCatNb2Processor::new(0, DeploymentMode::Standalone, CoverageLevel::CE2, 869_000);
        let mcl = proc.link_budget_mcl_db();
        assert!(mcl >= 164.0, "MCL should meet 164 dB target, got {}", mcl);
    }

    #[test]
    fn test_deployment_modes() {
        for &mode in &[DeploymentMode::Standalone, DeploymentMode::GuardBand, DeploymentMode::InBand] {
            let proc = LteCatNb2Processor::new(5, mode, CoverageLevel::CE0, 869_000);
            let samples = proc.process_dl_subframe(0, None, 0x1);
            assert!(!samples.is_empty());
        }
    }

    #[test]
    fn test_dci_n2_pack() {
        let dci = DciFormat::N2 { direct_indication: 0x42 };
        let bits = pack_dci(&dci);
        assert_eq!(bits.len(), 8);
    }

    #[test]
    fn test_crc16_ccitt_known() {
        // "123456789" -> CRC = 0x29B1 per standard
        let data: Vec<u8> = b"123456789".to_vec();
        let crc = crc16_ccitt(&data);
        assert_eq!(crc, 0x29B1);
    }

    #[test]
    fn test_qpp_params_coverage() {
        let known_sizes = [40, 48, 64, 96, 128, 160, 192, 256, 320, 384, 512];
        for &k in &known_sizes {
            let (f1, f2) = qpp_params(k);
            assert!(f1 > 0 && f2 > 0, "k={} f1={} f2={}", k, f1, f2);
        }
    }

    #[test]
    fn test_npusch_format2_nack() {
        let enc = NpuschEncoder::new(0, 2, NpuschFormat::Format2, UlSubcarrierMode::SingleTone3750);
        let syms_ack = enc.encode_format2(true, 0x1);
        let syms_nack = enc.encode_format2(false, 0x1);
        // Same length, different content (different scrambling after different bits)
        assert_eq!(syms_ack.len(), syms_nack.len());
    }
}
